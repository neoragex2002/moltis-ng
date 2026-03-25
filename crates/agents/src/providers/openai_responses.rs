use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

use async_trait::async_trait;
use futures::StreamExt;
use secrecy::ExposeSecret;
use tokio_stream::Stream;
use tracing::{debug, trace, warn};

use moltis_config::schema::{
    BuiltinWebSearchConfig, OpenAiResponsesGenerationConfig, OpenAiResponsesPromptCacheConfig,
    PromptCacheBucketHashConfig, ReasoningEffort, TextVerbosity, WebSearchContextSize,
};

use crate::as_sent_summary::{DEFAULT_MAX_LIST_ITEMS, sha256_hex, text_preview_value};
use crate::model::{
    ChatMessage, CompletionResponse, ContentPart, LlmProvider, LlmRequestContext, StreamEvent,
    ToolCall, Usage, UserContent,
};

use super::openai_compat::to_responses_api_tools;

const OPENAI_RESPONSES_ENDPOINT_PATH: &str = "/responses";

fn responses_endpoint(base_url: &str) -> String {
    format!(
        "{}{OPENAI_RESPONSES_ENDPOINT_PATH}",
        base_url.trim_end_matches('/')
    )
}

fn messages_to_responses_input(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .flat_map(|msg| match msg {
            ChatMessage::System { content } => vec![serde_json::json!({
                "type": "message",
                "role": "developer",
                "content": [{"type": "input_text", "text": content}],
            })],
            ChatMessage::User { content } => {
                let content_blocks = match content {
                    UserContent::Text(text) => {
                        vec![serde_json::json!({"type": "input_text", "text": text})]
                    },
                    UserContent::Multimodal(parts) => parts
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text(text) => {
                                serde_json::json!({"type": "input_text", "text": text})
                            },
                            ContentPart::Image { media_type, data } => {
                                let data_uri = format!("data:{media_type};base64,{data}");
                                serde_json::json!({
                                    "type": "input_image",
                                    "image_url": data_uri,
                                })
                            },
                        })
                        .collect(),
                };
                vec![serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": content_blocks,
                })]
            },
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                if !tool_calls.is_empty() {
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    for tc in tool_calls {
                        items.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": tc.arguments.to_string(),
                        }));
                    }
                    if let Some(text) = content
                        && !text.is_empty()
                    {
                        items.insert(
                            0,
                            serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}]
                            }),
                        );
                    }
                    items
                } else {
                    let text = content.as_deref().unwrap_or("");
                    vec![serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}]
                    })]
                }
            },
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => vec![serde_json::json!({
                "type": "function_call_output",
                "call_id": tool_call_id,
                "output": content,
            })],
        })
        .collect()
}

fn parse_responses_output(resp: &serde_json::Value) -> (Option<String>, Vec<ToolCall>, Usage) {
    let usage = Usage {
        input_tokens: resp["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: resp["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        cache_read_tokens: resp["usage"]["input_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0) as u32,
        ..Default::default()
    };

    let mut text_buf = String::new();
    let mut tool_calls = Vec::new();

    let Some(items) = resp["output"].as_array() else {
        return (None, tool_calls, usage);
    };

    for item in items {
        match item["type"].as_str().unwrap_or("") {
            "message" => {
                if item["role"].as_str() != Some("assistant") {
                    continue;
                }
                if let Some(content) = item["content"].as_array() {
                    for block in content {
                        if block["type"].as_str() == Some("output_text") {
                            if let Some(text) = block["text"].as_str() {
                                text_buf.push_str(text);
                            }
                        }
                    }
                }
            },
            "function_call" => {
                let id = item["call_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .unwrap_or("")
                    .to_string();
                let name = item["name"].as_str().unwrap_or("").to_string();
                let arguments = if let Some(args_str) = item["arguments"].as_str() {
                    serde_json::from_str(args_str).unwrap_or(serde_json::json!({}))
                } else if item["arguments"].is_object() {
                    item["arguments"].clone()
                } else {
                    serde_json::json!({})
                };
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            },
            _ => {},
        }
    }

    let text = if text_buf.is_empty() {
        None
    } else {
        Some(text_buf)
    };
    (text, tool_calls, usage)
}

fn base_url_is_openai_platform(base_url: &str) -> bool {
    match reqwest::Url::parse(base_url) {
        Ok(parsed_url) => parsed_url.host_str() == Some("api.openai.com"),
        Err(_) => base_url.contains("api.openai.com"),
    }
}

#[derive(Default)]
struct ResponsesSseCollector {
    text: String,
    tool_calls_by_index: HashMap<usize, (String, String)>,
    tool_args_by_index: HashMap<usize, String>,
    usage: Usage,
}

impl ResponsesSseCollector {
    fn ingest_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Delta(delta) => self.text.push_str(&delta),
            StreamEvent::ToolCallStart { id, name, index } => {
                self.tool_calls_by_index.insert(index, (id, name));
            },
            StreamEvent::ToolCallArgumentsDelta { index, delta } => {
                self.tool_args_by_index
                    .entry(index)
                    .or_default()
                    .push_str(&delta);
            },
            StreamEvent::Done(usage) => self.usage = usage,
            StreamEvent::Error(_) | StreamEvent::ToolCallComplete { .. } => {},
        }
    }

    fn into_completion(self) -> CompletionResponse {
        let mut indices: Vec<usize> = self.tool_calls_by_index.keys().copied().collect();
        indices.sort_unstable();

        let mut tool_calls = Vec::new();
        for index in indices {
            let Some((id, name)) = self.tool_calls_by_index.get(&index) else {
                continue;
            };
            let args_str = self
                .tool_args_by_index
                .get(&index)
                .map(String::as_str)
                .unwrap_or("{}");
            let arguments =
                serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
            tool_calls.push(ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments,
            });
        }

        CompletionResponse {
            text: (!self.text.is_empty()).then_some(self.text),
            tool_calls,
            usage: self.usage,
        }
    }
}

#[derive(Debug, Default)]
struct ResponsesSseParser {
    buf: Vec<u8>,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    tool_calls_by_index: HashMap<usize, (String, String)>,
    tool_index_by_call_id: HashMap<String, usize>,
    tool_index_by_item_key: HashMap<(u64, String), usize>,
    tool_args_started: HashSet<usize>,
    current_tool_index: usize,
    done: bool,
}

impl ResponsesSseParser {
    fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn take_next_frame(&mut self) -> Option<Vec<u8>> {
        let lf = Self::find_subsequence(&self.buf, b"\n\n");
        let crlf = Self::find_subsequence(&self.buf, b"\r\n\r\n");
        let (pos, delim_len) = match (lf, crlf) {
            (Some(a), Some(b)) => {
                if a <= b {
                    (a, 2)
                } else {
                    (b, 4)
                }
            },
            (Some(a), None) => (a, 2),
            (None, Some(b)) => (b, 4),
            (None, None) => return None,
        };

        let remainder = self.buf.split_off(pos + delim_len);
        let frame_with_delim = std::mem::replace(&mut self.buf, remainder);
        Some(frame_with_delim[..pos].to_vec())
    }

    fn assemble_data_payload(frame: &[u8]) -> Option<Vec<u8>> {
        let mut data_lines: Vec<&[u8]> = Vec::new();
        for raw_line in frame.split(|b| *b == b'\n') {
            let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            if line.is_empty() {
                continue;
            }
            if line.starts_with(b":") {
                continue;
            }
            let Some(rest) = line.strip_prefix(b"data:") else {
                continue;
            };
            let value = rest.strip_prefix(b" ").unwrap_or(rest);
            data_lines.push(value);
        }

        if data_lines.is_empty() {
            return None;
        }

        let mut out = Vec::new();
        for (idx, line) in data_lines.into_iter().enumerate() {
            if idx > 0 {
                out.push(b'\n');
            }
            out.extend_from_slice(line);
        }
        Some(out)
    }

    fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
        let mut start = 0;
        while start < bytes.len() && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        &bytes[start..end]
    }

    fn resolve_args_index(&self, evt: &serde_json::Value) -> Option<usize> {
        evt.get("output_index")
            .and_then(|v| v.as_u64())
            .and_then(|output_index| {
                evt.get("item_id")
                    .and_then(|v| v.as_str())
                    .and_then(|item_id| {
                        self.tool_index_by_item_key
                            .get(&(output_index, item_id.to_string()))
                            .copied()
                    })
            })
            .or_else(|| {
                evt.get("call_id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| self.tool_index_by_call_id.get(id).copied())
            })
            .or_else(|| {
                if self.current_tool_index > 0 {
                    Some(self.current_tool_index - 1)
                } else {
                    None
                }
            })
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Vec<StreamEvent> {
        if self.done {
            return Vec::new();
        }

        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();

        while let Some(frame) = self.take_next_frame() {
            let Some(payload_bytes) = Self::assemble_data_payload(&frame) else {
                continue;
            };

            if Self::trim_ascii_whitespace(&payload_bytes) == b"[DONE]" {
                let mut indices: Vec<usize> = self.tool_calls_by_index.keys().copied().collect();
                indices.sort_unstable();
                for index in indices {
                    out.push(StreamEvent::ToolCallComplete { index });
                }
                out.push(StreamEvent::Done(Usage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    cache_read_tokens: self.cache_read_tokens,
                    ..Default::default()
                }));
                self.done = true;
                break;
            }

            let Ok(payload_str) = std::str::from_utf8(&payload_bytes) else {
                continue;
            };
            let Ok(evt) = serde_json::from_str::<serde_json::Value>(payload_str) else {
                continue;
            };
            let evt_type = evt["type"].as_str().unwrap_or("");
            trace!(evt_type = %evt_type, evt = %evt, "openai-responses stream event");

            match evt_type {
                "response.output_text.delta" => {
                    if let Some(delta) = evt["delta"].as_str()
                        && !delta.is_empty()
                    {
                        out.push(StreamEvent::Delta(delta.to_string()));
                    }
                },
                "response.output_item.added" => {
                    let output_index = evt.get("output_index").and_then(|v| v.as_u64());
                    let item_id = evt
                        .get("item")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    if evt["item"]["type"].as_str() == Some("function_call") {
                        let id = evt["item"]["call_id"]
                            .as_str()
                            .map(|v| v.to_string())
                            .or_else(|| item_id.clone())
                            .unwrap_or_default();
                        let name = evt["item"]["name"].as_str().unwrap_or("").to_string();
                        let index = self.current_tool_index;
                        self.current_tool_index = self.current_tool_index.saturating_add(1);
                        self.tool_calls_by_index
                            .insert(index, (id.clone(), name.clone()));
                        self.tool_index_by_call_id.insert(id.clone(), index);
                        if let (Some(output_index), Some(item_id)) = (output_index, item_id) {
                            self.tool_index_by_item_key
                                .insert((output_index, item_id), index);
                        }
                        out.push(StreamEvent::ToolCallStart { id, name, index });
                    }
                },
                "response.function_call_arguments.delta" => {
                    if let Some(delta) = evt["delta"].as_str()
                        && !delta.is_empty()
                    {
                        let Some(index) = self.resolve_args_index(&evt) else {
                            continue;
                        };
                        self.tool_args_started.insert(index);
                        out.push(StreamEvent::ToolCallArgumentsDelta {
                            index,
                            delta: delta.to_string(),
                        });
                    }
                },
                "response.function_call_arguments.done" => {
                    let Some(arguments) = evt.get("arguments").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if arguments.is_empty() {
                        continue;
                    }
                    let Some(index) = self.resolve_args_index(&evt) else {
                        continue;
                    };
                    if !self.tool_args_started.contains(&index) {
                        self.tool_args_started.insert(index);
                        out.push(StreamEvent::ToolCallArgumentsDelta {
                            index,
                            delta: arguments.to_string(),
                        });
                    }
                },
                "response.completed" => {
                    if let Some(u) = evt["response"]["usage"].as_object() {
                        self.input_tokens =
                            u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        self.output_tokens =
                            u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                        self.cache_read_tokens = u
                            .get("input_tokens_details")
                            .and_then(|v| v.get("cached_tokens"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                    }

                    let mut indices: Vec<usize> =
                        self.tool_calls_by_index.keys().copied().collect();
                    indices.sort_unstable();
                    for index in indices {
                        out.push(StreamEvent::ToolCallComplete { index });
                    }
                    out.push(StreamEvent::Done(Usage {
                        input_tokens: self.input_tokens,
                        output_tokens: self.output_tokens,
                        cache_read_tokens: self.cache_read_tokens,
                        ..Default::default()
                    }));
                    self.done = true;
                    break;
                },
                "error" | "response.failed" => {
                    let msg = evt["error"]["message"]
                        .as_str()
                        .or_else(|| evt["message"].as_str())
                        .unwrap_or("unknown error");
                    out.push(StreamEvent::Error(msg.to_string()));
                    self.done = true;
                    break;
                },
                _ => {},
            }
        }

        out
    }
}

pub struct OpenAiResponsesProvider {
    api_key: secrecy::Secret<String>,
    model: String,
    base_url: String,
    provider_name: String,
    client: reqwest::Client,
    builtin_web_search: Option<BuiltinWebSearchConfig>,
    generation: Option<OpenAiResponsesGenerationConfig>,
    prompt_cache: Option<OpenAiResponsesPromptCacheConfig>,
}

impl OpenAiResponsesProvider {
    pub fn new(api_key: secrecy::Secret<String>, model: String, base_url: String) -> Self {
        Self::new_with_name(
            api_key,
            model,
            base_url,
            "openai-responses".into(),
            None,
            None,
            None,
        )
    }

    pub fn new_with_name(
        api_key: secrecy::Secret<String>,
        model: String,
        base_url: String,
        provider_name: String,
        builtin_web_search: Option<BuiltinWebSearchConfig>,
        generation: Option<OpenAiResponsesGenerationConfig>,
        prompt_cache: Option<OpenAiResponsesPromptCacheConfig>,
    ) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_name,
            client: reqwest::Client::new(),
            builtin_web_search,
            generation,
            prompt_cache,
        }
    }

    fn is_builtin_web_search_enabled(&self) -> bool {
        self.builtin_web_search
            .as_ref()
            .map(|cfg| cfg.enabled)
            .unwrap_or(false)
    }

    fn is_prompt_cache_enabled(&self) -> bool {
        self.prompt_cache
            .as_ref()
            .map(|cfg| cfg.enabled)
            .unwrap_or(false)
    }

    fn prompt_cache_key_for_request(&self, ctx: Option<&LlmRequestContext>) -> Option<String> {
        if !self.is_prompt_cache_enabled() {
            return None;
        }

        let prompt_cache_key = ctx
            .and_then(|c| c.prompt_cache_key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if let Some(prompt_cache_key) = prompt_cache_key {
            return Some(self.prompt_cache_bucket_id(prompt_cache_key));
        }

        debug!(
            event = "provider.prompt_cache_key.omitted",
            provider = %self.provider_name,
            model = %self.model,
            reason_code = "prompt_cache_key_missing",
            decision = "omit",
            "prompt_cache enabled but caller omitted prompt_cache_key; omitting southbound prompt cache"
        );
        None
    }

    fn prompt_cache_bucket_id(&self, bucket_key: &str) -> String {
        let Some(ref cfg) = self.prompt_cache else {
            return bucket_key.to_string();
        };

        let should_hash = match cfg.bucket_hash {
            PromptCacheBucketHashConfig::Bool(force) => force,
            PromptCacheBucketHashConfig::Mode(_) => bucket_key.as_bytes().len() > 64,
        };

        if should_hash {
            blake3::hash(bucket_key.as_bytes()).to_hex().to_string()
        } else {
            bucket_key.to_string()
        }
    }

    fn apply_generation_options(&self, body: &mut serde_json::Value) {
        let Some(ref cfg) = self.generation else {
            return;
        };

        let resolved_limits = super::resolved_openai_limits(&self.model);
        let mut max_output_tokens = cfg.max_output_tokens.unwrap_or(resolved_limits.output);
        if max_output_tokens > resolved_limits.output {
            warn!(
                provider = %self.provider_name,
                model = %self.model,
                configured = max_output_tokens,
                limit = resolved_limits.output,
                "clamping max_output_tokens to model limit"
            );
            max_output_tokens = resolved_limits.output;
        }
        body["max_output_tokens"] = serde_json::json!(max_output_tokens);

        if let Some(reasoning_effort) = cfg.reasoning_effort {
            let effort = match reasoning_effort {
                ReasoningEffort::None => "none",
                ReasoningEffort::Minimal => "minimal",
                ReasoningEffort::Low => "low",
                ReasoningEffort::Medium => "medium",
                ReasoningEffort::High => "high",
                ReasoningEffort::Xhigh => "xhigh",
            };
            body["reasoning"] = serde_json::json!({"effort": effort});
        }

        if let Some(text_verbosity) = cfg.text_verbosity {
            let verbosity = match text_verbosity {
                TextVerbosity::Low => "low",
                TextVerbosity::Medium => "medium",
                TextVerbosity::High => "high",
            };
            body["text"] = serde_json::json!({"verbosity": verbosity});
        }

        if let Some(temperature) = cfg.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }
    }

    fn build_responses_body_with_context(
        &self,
        ctx: Option<&LlmRequestContext>,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        stream: bool,
    ) -> serde_json::Value {
        let mut body = self.build_responses_body(messages, tools, stream);

        self.apply_generation_options(&mut body);

        if let Some(prompt_cache_key) = self.prompt_cache_key_for_request(ctx) {
            body["prompt_cache_key"] = serde_json::json!(prompt_cache_key);
        }

        body
    }

    fn build_builtin_web_search_tool(cfg: &BuiltinWebSearchConfig) -> serde_json::Value {
        let mut tool = serde_json::json!({
            "type": "web_search",
        });

        if let Some(ref domains) = cfg.allowed_domains {
            let allowed_domains: Vec<&str> = domains
                .iter()
                .map(|d| d.trim())
                .filter(|d| !d.is_empty())
                .collect();
            if !allowed_domains.is_empty() {
                tool["filters"] = serde_json::json!({
                    "allowed_domains": allowed_domains,
                });
            }
        }

        if let Some(size) = cfg.search_context_size {
            let size = match size {
                WebSearchContextSize::Low => "low",
                WebSearchContextSize::Medium => "medium",
                WebSearchContextSize::High => "high",
            };
            tool["search_context_size"] = serde_json::json!(size);
        }

        if let Some(ref loc) = cfg.user_location {
            let mut user_location = serde_json::json!({
                "type": "approximate",
            });
            if let Some(ref city) = loc.city {
                if !city.trim().is_empty() {
                    user_location["city"] = serde_json::json!(city);
                }
            }
            if let Some(ref country) = loc.country {
                if !country.trim().is_empty() {
                    user_location["country"] = serde_json::json!(country);
                }
            }
            if let Some(ref region) = loc.region {
                if !region.trim().is_empty() {
                    user_location["region"] = serde_json::json!(region);
                }
            }
            if let Some(ref timezone) = loc.timezone {
                if !timezone.trim().is_empty() {
                    user_location["timezone"] = serde_json::json!(timezone);
                }
            }
            tool["user_location"] = user_location;
        }

        tool
    }

    async fn post_responses(&self, body: &serde_json::Value) -> anyhow::Result<reqwest::Response> {
        let mut req = self
            .client
            .post(responses_endpoint(&self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json");
        if base_url_is_openai_platform(&self.base_url) {
            req = req.header("OpenAI-Beta", "responses=experimental");
        }
        if body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            req = req.header("Accept", "text/event-stream");
        } else {
            req = req.header("Accept", "application/json");
        }
        let req = req.json(body);
        Ok(req.send().await?)
    }

    fn build_responses_body(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        stream: bool,
    ) -> serde_json::Value {
        let input = messages_to_responses_input(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "input": input,
            "store": false,
        });

        if stream {
            body["stream"] = serde_json::Value::Bool(true);
        }

        let builtin_search_enabled = self.is_builtin_web_search_enabled();
        let function_tools: Vec<serde_json::Value> = if builtin_search_enabled {
            let filtered: Vec<serde_json::Value> = tools
                .iter()
                .filter(|t| t.get("name").and_then(|v| v.as_str()) != Some("web_search"))
                .cloned()
                .collect();
            let dropped = tools.len().saturating_sub(filtered.len());
            if dropped > 0 {
                debug!(
                    provider = %self.provider_name,
                    model = %self.model,
                    dropped,
                    "filtered local function tool 'web_search' because builtin web_search is enabled"
                );
            }
            filtered
        } else {
            tools.to_vec()
        };

        let mut out_tools: Vec<serde_json::Value> = Vec::new();
        if !function_tools.is_empty() {
            out_tools.extend(to_responses_api_tools(&function_tools));
        }

        if let Some(ref cfg) = self.builtin_web_search {
            if cfg.enabled {
                out_tools.push(Self::build_builtin_web_search_tool(cfg));
                if cfg.include_sources {
                    let include = body.get_mut("include");
                    match include {
                        Some(serde_json::Value::Array(arr)) => {
                            if !arr
                                .iter()
                                .any(|v| v.as_str() == Some("web_search_call.action.sources"))
                            {
                                arr.push(serde_json::json!("web_search_call.action.sources"));
                            }
                        },
                        _ => {
                            body["include"] = serde_json::json!(["web_search_call.action.sources"]);
                        },
                    }
                }
            }
        }

        if !out_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(out_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        body
    }

    async fn complete_using_body(
        &self,
        body: serde_json::Value,
        tools_count: usize,
    ) -> anyhow::Result<CompletionResponse> {
        debug!(
            provider = %self.provider_name,
            model = %self.model,
            input_items_count = body["input"].as_array().map(|a| a.len()).unwrap_or(0),
            tools_count,
            "openai-responses complete request"
        );
        trace!(
            body = %serde_json::to_string(&body).unwrap_or_default(),
            "openai-responses request body"
        );

        let http_resp = self.post_responses(&body).await?;
        let status = http_resp.status();
        if !status.is_success() {
            let body_text = http_resp.text().await.unwrap_or_default();
            warn!(
                status = %status,
                provider = %self.provider_name,
                model = %self.model,
                body = %body_text,
                "openai-responses API error"
            );
            anyhow::bail!("OpenAI Responses API error HTTP {status}: {body_text}");
        }

        let content_type = http_resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if content_type.contains("application/json") {
            let resp = http_resp.json::<serde_json::Value>().await?;
            trace!(response = %resp, "openai-responses raw response");
            let (text, tool_calls, usage) = parse_responses_output(&resp);
            return Ok(CompletionResponse {
                text,
                tool_calls,
                usage,
            });
        }

        let mut byte_stream = http_resp.bytes_stream();
        let mut parser = ResponsesSseParser::default();
        let mut collector = ResponsesSseCollector::default();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk?;
            for event in parser.push_bytes(&chunk) {
                match event {
                    StreamEvent::Error(msg) => {
                        anyhow::bail!("OpenAI Responses API stream error: {msg}");
                    },
                    StreamEvent::Done(_) => {
                        collector.ingest_event(event);
                        return Ok(collector.into_completion());
                    },
                    other => collector.ingest_event(other),
                }
            }
        }

        anyhow::bail!("OpenAI Responses API stream ended unexpectedly");
    }

    fn stream_with_tools_impl<'a>(
        &'a self,
        ctx: Option<LlmRequestContext>,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            let body = self.build_responses_body_with_context(ctx.as_ref(), &messages, &tools, true);

            debug!(
                provider = %self.provider_name,
                model = %self.model,
                input_items_count = body["input"].as_array().map(|a| a.len()).unwrap_or(0),
                tools_count = tools.len(),
                "openai-responses stream_with_tools request"
            );
            trace!(
                body = %serde_json::to_string(&body).unwrap_or_default(),
                "openai-responses stream request body"
            );

            let http_resp = match self.post_responses(&body).await {
                Ok(r) => r,
                Err(err) => {
                    yield StreamEvent::Error(err.to_string());
                    return;
                }
            };

            if let Err(err) = http_resp.error_for_status_ref() {
                let status = err.status().map(|s| s.as_u16()).unwrap_or(0);
                let body_text = http_resp.text().await.unwrap_or_default();
                yield StreamEvent::Error(format!(
                    "OpenAI Responses API error HTTP {status}: {body_text}"
                ));
                return;
            }

            let content_type = http_resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            if content_type.contains("application/json") {
                let resp = match http_resp.json::<serde_json::Value>().await {
                    Ok(v) => v,
                    Err(err) => {
                        yield StreamEvent::Error(err.to_string());
                        return;
                    }
                };
                let (text, tool_calls, usage) = parse_responses_output(&resp);
                if let Some(text) = text
                    && !text.is_empty()
                {
                    yield StreamEvent::Delta(text);
                }
                for (index, tc) in tool_calls.into_iter().enumerate() {
                    yield StreamEvent::ToolCallStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        index,
                    };
                    let args = tc.arguments.to_string();
                    if !args.is_empty() {
                        yield StreamEvent::ToolCallArgumentsDelta { index, delta: args };
                    }
                    yield StreamEvent::ToolCallComplete { index };
                }
                yield StreamEvent::Done(usage);
                return;
            }

            let mut parser = ResponsesSseParser::default();
            let mut byte_stream = http_resp.bytes_stream();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(err) => {
                        yield StreamEvent::Error(err.to_string());
                        return;
                    }
                };

                for event in parser.push_bytes(&chunk) {
                    let done = matches!(event, StreamEvent::Done(_) | StreamEvent::Error(_));
                    yield event;
                    if done {
                        return;
                    }
                }
            }

            yield StreamEvent::Error("OpenAI Responses API stream ended unexpectedly".into());
        })
    }

    #[cfg(test)]
    fn parse_streaming_completion_from_sse(payload: &str) -> anyhow::Result<CompletionResponse> {
        let mut parser = ResponsesSseParser::default();
        let mut collector = ResponsesSseCollector::default();

        for event in parser.push_bytes(payload.as_bytes()) {
            match event {
                StreamEvent::Error(msg) => {
                    anyhow::bail!("OpenAI Responses API stream error: {msg}");
                },
                StreamEvent::Done(_) => {
                    collector.ingest_event(event);
                    break;
                },
                other => collector.ingest_event(other),
            }
        }

        Ok(collector.into_completion())
    }
}

#[async_trait]
impl LlmProvider for OpenAiResponsesProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn id(&self) -> &str {
        &self.model
    }

    fn supports_tools(&self) -> bool {
        super::supports_tools_for_model(&self.model)
    }

    fn context_window(&self) -> u32 {
        super::resolved_openai_limits(&self.model).context
    }

    fn input_limit(&self) -> Option<u32> {
        super::cached_openai_model_limits(&moltis_config::data_dir(), &self.model)
            .and_then(|l| l.input)
    }

    fn output_limit(&self) -> Option<u32> {
        Some(super::resolved_openai_limits(&self.model).output)
    }

    fn supports_vision(&self) -> bool {
        super::supports_vision_for_model(&self.model)
    }

    fn debug_as_sent_summary(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> Option<serde_json::Value> {
        let developer_preamble = messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::System { content } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
            .take(DEFAULT_MAX_LIST_ITEMS)
            .collect();

        let developer_message_count = messages
            .iter()
            .filter(|m| matches!(m, ChatMessage::System { .. }))
            .count();
        let user_message_count = messages
            .iter()
            .filter(|m| matches!(m, ChatMessage::User { .. }))
            .count();
        let function_call_output_count = messages
            .iter()
            .filter(|m| matches!(m, ChatMessage::Tool { .. }))
            .count();

        let mut assistant_message_item_count: usize = 0;
        let mut function_call_count: usize = 0;

        let mut roles_preview: Vec<&'static str> = Vec::new();
        let mut tool_call_names_preview: Vec<&str> = Vec::new();
        let has_multimodal_images = messages.iter().any(|m| match m {
            ChatMessage::User {
                content: UserContent::Multimodal(parts),
            } => parts.iter().any(|p| matches!(p, ContentPart::Image { .. })),
            _ => false,
        });

        let mut input_types_preview: Vec<&'static str> = Vec::new();

        for msg in messages.iter().take(DEFAULT_MAX_LIST_ITEMS) {
            match msg {
                ChatMessage::System { .. } => {
                    roles_preview.push("developer");
                    if input_types_preview.len() < DEFAULT_MAX_LIST_ITEMS {
                        input_types_preview.push("message:developer");
                    }
                },
                ChatMessage::User { .. } => roles_preview.push("user"),
                ChatMessage::Assistant { tool_calls, .. } => {
                    roles_preview.push("assistant");
                    if tool_calls.is_empty() {
                        if input_types_preview.len() < DEFAULT_MAX_LIST_ITEMS {
                            input_types_preview.push("message:assistant");
                        }
                    } else {
                        let has_text = matches!(msg, ChatMessage::Assistant { content: Some(t), .. } if !t.is_empty());
                        if has_text {
                            if input_types_preview.len() < DEFAULT_MAX_LIST_ITEMS {
                                input_types_preview.push("message:assistant");
                            }
                        }
                        for _ in 0..tool_calls.len() {
                            if input_types_preview.len() >= DEFAULT_MAX_LIST_ITEMS {
                                break;
                            }
                            input_types_preview.push("function_call");
                        }
                    }
                    for tc in tool_calls.iter().take(DEFAULT_MAX_LIST_ITEMS) {
                        if tool_call_names_preview.len() >= DEFAULT_MAX_LIST_ITEMS {
                            break;
                        }
                        tool_call_names_preview.push(tc.name.as_str());
                    }
                },
                ChatMessage::Tool { .. } => {
                    roles_preview.push("function_call_output");
                    if input_types_preview.len() < DEFAULT_MAX_LIST_ITEMS {
                        input_types_preview.push("function_call_output");
                    }
                },
            }

            if matches!(msg, ChatMessage::User { .. })
                && input_types_preview.len() < DEFAULT_MAX_LIST_ITEMS
            {
                input_types_preview.push("message:user");
            }
        }

        // Count all tool calls and assistant message items (not just preview).
        for msg in messages {
            if let ChatMessage::Assistant { tool_calls, .. } = msg {
                if tool_calls.is_empty() {
                    assistant_message_item_count = assistant_message_item_count.saturating_add(1);
                    continue;
                }

                function_call_count = function_call_count.saturating_add(tool_calls.len());
                if matches!(msg, ChatMessage::Assistant { content: Some(t), .. } if !t.is_empty()) {
                    assistant_message_item_count = assistant_message_item_count.saturating_add(1);
                }
            }
        }

        let input_count = developer_message_count
            .saturating_add(user_message_count)
            .saturating_add(assistant_message_item_count)
            .saturating_add(function_call_count)
            .saturating_add(function_call_output_count);

        let hash_seed = serde_json::json!({
            "provider": self.provider_name,
            "model": self.model,
            "developerPreambleSha256": sha256_hex(&developer_preamble),
            "rolesPreview": roles_preview,
            "inputTypesPreview": input_types_preview,
            "toolCallNamesPreview": tool_call_names_preview,
            "toolsCount": tools.len(),
            "toolNamesPreview": tool_names,
            "messagesCount": messages.len(),
            "inputCount": input_count,
        });

        Some(serde_json::json!({
            "method": "as-sent-summary",
            "provider": self.provider_name,
            "kind": "openai_responses_v1",
            "model": self.model,
            "hash": format!("sha256:{}", sha256_hex(&serde_json::to_string(&hash_seed).unwrap_or_default())),
            "developerPreamble": text_preview_value(&developer_preamble),
            "inputCount": input_count,
            "inputCounts": {
                "messageDeveloper": developer_message_count,
                "messageUser": user_message_count,
                "messageAssistant": assistant_message_item_count,
                "functionCall": function_call_count,
                "functionCallOutput": function_call_output_count,
            },
            "inputTypesPreview": input_types_preview,
            "rolesPreview": roles_preview,
            "toolCallsTotal": function_call_count,
            "toolCallNamesPreview": tool_call_names_preview,
            "omitsImages": has_multimodal_images,
            "omitsToolSchemas": true,
            "tools": {
                "count": tools.len(),
                "namesPreview": tool_names,
            }
        }))
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        // Some Codex-style Responses gateways only support streaming mode.
        // Use `stream: true` and collect the full result into a single response.
        let body = self.build_responses_body_with_context(None, messages, tools, true);
        self.complete_using_body(body, tools.len()).await
    }

    async fn complete_with_context(
        &self,
        ctx: &LlmRequestContext,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        let body = self.build_responses_body_with_context(Some(ctx), messages, tools, true);
        self.complete_using_body(body, tools.len()).await
    }

    fn stream(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        self.stream_with_tools(messages, vec![])
    }

    fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        self.stream_with_tools_impl(None, messages, tools)
    }

    fn debug_request_overrides(&self, ctx: Option<&LlmRequestContext>) -> serde_json::Value {
        let mut root = serde_json::Map::new();

        if self.is_prompt_cache_enabled() {
            let session_key = ctx
                .and_then(|c| c.session_key.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let session_id = ctx
                .and_then(|c| c.session_id.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let prompt_cache_key = ctx
                .and_then(|c| c.prompt_cache_key.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let source = if prompt_cache_key.is_some() && prompt_cache_key == session_id {
                "session_id"
            } else if prompt_cache_key.is_some() && prompt_cache_key == session_key {
                "session_key"
            } else if prompt_cache_key.is_some() {
                "explicit"
            } else {
                "omitted"
            };
            let hashed = match self.prompt_cache.as_ref().map(|c| c.bucket_hash) {
                Some(PromptCacheBucketHashConfig::Bool(force)) => force,
                Some(PromptCacheBucketHashConfig::Mode(_)) => prompt_cache_key
                    .is_some_and(|value| value.as_bytes().len() > 64),
                None => false,
            };
            root.insert(
                "prompt_cache".to_string(),
                serde_json::json!({
                    "enabled": true,
                    "source": source,
                    "hashed": hashed,
                }),
            );
        }

        if let Some(prompt_cache_key) = self.prompt_cache_key_for_request(ctx) {
            root.insert(
                "prompt_cache_key".to_string(),
                serde_json::json!(prompt_cache_key),
            );
        }

        let mut generation = serde_json::Map::new();
        if let Some(ref cfg) = self.generation {
            let resolved_limits = super::resolved_openai_limits(&self.model);
            let default_max = resolved_limits.output;
            let configured = cfg.max_output_tokens.unwrap_or(default_max);
            let mut effective = configured;
            let mut clamped = false;
            if effective > resolved_limits.output {
                effective = resolved_limits.output;
                clamped = true;
            }
            generation.insert(
                "max_output_tokens".to_string(),
                serde_json::json!({
                    "configured": configured,
                    "effective": effective,
                    "limit": resolved_limits.output,
                    "clamped": clamped,
                }),
            );

            if let Some(reasoning_effort) = cfg.reasoning_effort {
                let effort = match reasoning_effort {
                    ReasoningEffort::None => "none",
                    ReasoningEffort::Minimal => "minimal",
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::Xhigh => "xhigh",
                };
                generation.insert("reasoning_effort".to_string(), serde_json::json!(effort));
            }

            if let Some(text_verbosity) = cfg.text_verbosity {
                let verbosity = match text_verbosity {
                    TextVerbosity::Low => "low",
                    TextVerbosity::Medium => "medium",
                    TextVerbosity::High => "high",
                };
                generation.insert("text_verbosity".to_string(), serde_json::json!(verbosity));
            }

            if let Some(temperature) = cfg.temperature {
                generation.insert("temperature".to_string(), serde_json::json!(temperature));
            }
        }
        if !generation.is_empty() {
            root.insert(
                "generation".to_string(),
                serde_json::Value::Object(generation),
            );
        }

        serde_json::Value::Object(root)
    }

    fn stream_with_tools_with_context(
        &self,
        ctx: &LlmRequestContext,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        self.stream_with_tools_impl(Some(ctx.clone()), messages, tools)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use secrecy::Secret;

    use super::*;

    fn test_provider(base_url: &str) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider::new(
            Secret::new("test-key".to_string()),
            "gpt-4o".to_string(),
            base_url.to_string(),
        )
    }

    fn test_provider_with_builtin_web_search(
        base_url: &str,
        builtin_web_search: BuiltinWebSearchConfig,
    ) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider::new_with_name(
            Secret::new("test-key".to_string()),
            "gpt-4o".to_string(),
            base_url.to_string(),
            "openai-responses".into(),
            Some(builtin_web_search),
            None,
            None,
        )
    }

    fn test_provider_with_generation(
        base_url: &str,
        generation: OpenAiResponsesGenerationConfig,
    ) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider::new_with_name(
            Secret::new("test-key".to_string()),
            "gpt-4o".to_string(),
            base_url.to_string(),
            "openai-responses".into(),
            None,
            Some(generation),
            None,
        )
    }

    fn test_provider_with_prompt_cache(
        base_url: &str,
        prompt_cache: OpenAiResponsesPromptCacheConfig,
    ) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider::new_with_name(
            Secret::new("test-key".to_string()),
            "gpt-4o".to_string(),
            base_url.to_string(),
            "openai-responses".into(),
            None,
            None,
            Some(prompt_cache),
        )
    }

    #[test]
    fn build_responses_body_omits_instructions_field() {
        let provider = test_provider("https://api.example.com/v1");
        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &[], false);
        assert_eq!(body["model"].as_str(), Some("gpt-4o"));
        assert!(
            body.get("instructions").is_none(),
            "instructions must be omitted (absent), not set"
        );
        assert!(body.get("input").is_some());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn build_responses_body_sends_system_messages_as_developer_input_items() {
        let provider = test_provider("https://api.example.com/v1");
        let body = provider.build_responses_body(
            &[ChatMessage::system("sys-a"), ChatMessage::user("ping")],
            &[],
            false,
        );

        assert!(
            body.get("instructions").is_none(),
            "instructions must be omitted even when system messages exist"
        );

        let input = body
            .get("input")
            .and_then(|v| v.as_array())
            .expect("expected input array");
        assert!(input.len() >= 2, "expected at least developer + user items");

        assert_eq!(
            input[0].get("type").and_then(|v| v.as_str()),
            Some("message")
        );
        assert_eq!(
            input[0].get("role").and_then(|v| v.as_str()),
            Some("developer")
        );
        assert_eq!(
            input[0]
                .get("content")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str()),
            Some("input_text")
        );
        assert_eq!(
            input[0]
                .get("content")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str()),
            Some("sys-a")
        );

        assert_eq!(
            input[1].get("type").and_then(|v| v.as_str()),
            Some("message")
        );
        assert_eq!(input[1].get("role").and_then(|v| v.as_str()), Some("user"));
    }

    #[test]
    fn build_responses_body_preserves_system_message_order_as_multiple_developer_items() {
        let provider = test_provider("https://api.example.com/v1");
        let body = provider.build_responses_body(
            &[
                ChatMessage::system("sys-1"),
                ChatMessage::system("sys-2"),
                ChatMessage::user("ping"),
            ],
            &[],
            false,
        );
        let input = body["input"].as_array().expect("expected input array");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"].as_str(), Some("developer"));
        assert_eq!(input[0]["content"][0]["text"].as_str(), Some("sys-1"));
        assert_eq!(input[1]["role"].as_str(), Some("developer"));
        assert_eq!(input[1]["content"][0]["text"].as_str(), Some("sys-2"));
        assert_eq!(input[2]["role"].as_str(), Some("user"));
    }

    #[test]
    fn build_responses_body_includes_prompt_cache_key_when_enabled_and_session_key_provided() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Bool(false);

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let ctx = LlmRequestContext {
            session_key: Some("agent:zhuzhu:main".to_string()),
            session_id: Some("main".to_string()),
            prompt_cache_key: Some("main".to_string()),
            run_id: None,
        };

        let body = provider.build_responses_body_with_context(
            Some(&ctx),
            &[ChatMessage::user("ping")],
            &[],
            false,
        );
        assert_eq!(body["prompt_cache_key"].as_str(), Some("main"));
    }

    #[test]
    fn prompt_cache_key_differs_for_different_session_ids() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Bool(false);

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let ctx_a = LlmRequestContext {
            session_key: Some("agent:zhuzhu:session-a".to_string()),
            session_id: Some("session:a".to_string()),
            prompt_cache_key: Some("session:a".to_string()),
            run_id: None,
        };
        let ctx_b = LlmRequestContext {
            session_key: Some("agent:zhuzhu:session-b".to_string()),
            session_id: Some("session:b".to_string()),
            prompt_cache_key: Some("session:b".to_string()),
            run_id: None,
        };

        let body_a = provider.build_responses_body_with_context(
            Some(&ctx_a),
            &[ChatMessage::user("ping")],
            &[],
            false,
        );
        let body_b = provider.build_responses_body_with_context(
            Some(&ctx_b),
            &[ChatMessage::user("ping")],
            &[],
            false,
        );

        assert_eq!(body_a["prompt_cache_key"].as_str(), Some("session:a"));
        assert_eq!(body_b["prompt_cache_key"].as_str(), Some("session:b"));
        assert_ne!(body_a["prompt_cache_key"], body_b["prompt_cache_key"]);
    }

    #[test]
    fn build_responses_body_hashes_prompt_cache_key_when_auto_and_long() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Mode(
            moltis_config::schema::PromptCacheBucketHashMode::Auto,
        );

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let session_key = "a".repeat(65);
        let ctx = LlmRequestContext {
            session_key: Some("agent:zhuzhu:main".to_string()),
            session_id: Some(session_key.clone()),
            prompt_cache_key: Some(session_key.clone()),
            run_id: None,
        };

        let body = provider.build_responses_body_with_context(
            Some(&ctx),
            &[ChatMessage::user("ping")],
            &[],
            false,
        );

        let expected = blake3::hash(session_key.as_bytes()).to_hex().to_string();
        assert_eq!(body["prompt_cache_key"].as_str(), Some(expected.as_str()));
        assert_eq!(expected.len(), 64);
    }

    #[test]
    fn build_responses_body_omits_prompt_cache_key_when_context_missing() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Bool(false);

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body_with_context(
            None,
            &[ChatMessage::user("ping")],
            &[],
            false,
        );

        assert!(
            body.get("prompt_cache_key").is_none(),
            "prompt_cache_key must be omitted when caller did not provide a canonical cache bucket"
        );
    }

    #[test]
    fn build_responses_body_applies_generation_options_when_configured() {
        let mut cfg = OpenAiResponsesGenerationConfig::default();
        cfg.max_output_tokens = Some(1024);
        cfg.reasoning_effort = Some(ReasoningEffort::None);
        cfg.text_verbosity = Some(TextVerbosity::High);
        cfg.temperature = Some(0.2);

        let provider = test_provider_with_generation("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body_with_context(
            None,
            &[ChatMessage::user("ping")],
            &[],
            false,
        );
        assert_eq!(body["max_output_tokens"].as_u64(), Some(1024));
        assert_eq!(body["reasoning"]["effort"].as_str(), Some("none"));
        assert_eq!(body["text"]["verbosity"].as_str(), Some("high"));
        let temp = body["temperature"].as_f64().expect("expected temperature");
        assert!((temp - 0.2).abs() < 1e-6);
    }

    #[test]
    fn build_responses_body_clamps_max_output_tokens_to_model_limit() {
        let limit = super::super::resolved_openai_limits("gpt-4o").output;

        let mut cfg = OpenAiResponsesGenerationConfig::default();
        cfg.max_output_tokens = Some(limit.saturating_add(1));

        let provider = test_provider_with_generation("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body_with_context(
            None,
            &[ChatMessage::user("ping")],
            &[],
            false,
        );
        assert_eq!(body["max_output_tokens"].as_u64(), Some(limit as u64));
    }

    #[test]
    fn debug_request_overrides_reports_generation_options() {
        let mut cfg = OpenAiResponsesGenerationConfig::default();
        cfg.max_output_tokens = Some(1024);
        cfg.reasoning_effort = Some(ReasoningEffort::None);
        cfg.text_verbosity = Some(TextVerbosity::High);
        cfg.temperature = Some(0.2);

        let provider = test_provider_with_generation("https://api.example.com/v1", cfg);
        let dbg = provider.debug_request_overrides(None);
        let generation = dbg["generation"]
            .as_object()
            .expect("expected generation object");
        let max = generation["max_output_tokens"]
            .as_object()
            .expect("expected max_output_tokens object");
        assert_eq!(max["configured"].as_u64(), Some(1024));
        assert_eq!(max["effective"].as_u64(), Some(1024));
        assert_eq!(max["clamped"].as_bool(), Some(false));
        assert_eq!(generation["reasoning_effort"].as_str(), Some("none"));
        assert_eq!(generation["text_verbosity"].as_str(), Some("high"));
        let temp = generation["temperature"]
            .as_f64()
            .expect("expected temperature");
        assert!((temp - 0.2).abs() < 1e-6);
    }

    #[test]
    fn debug_request_overrides_reports_prompt_cache_key() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Bool(false);

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let ctx = LlmRequestContext {
            session_key: Some("agent:zhuzhu:telegram".to_string()),
            session_id: Some("telegram:bot:123".to_string()),
            prompt_cache_key: Some("telegram:bot:123".to_string()),
            run_id: None,
        };
        let dbg = provider.debug_request_overrides(Some(&ctx));
        assert_eq!(dbg["prompt_cache"]["enabled"].as_bool(), Some(true));
        assert_eq!(dbg["prompt_cache"]["source"].as_str(), Some("session_id"));
        assert_eq!(dbg["prompt_cache"]["hashed"].as_bool(), Some(false));
        assert_eq!(dbg["prompt_cache_key"].as_str(), Some("telegram:bot:123"));
    }

    #[test]
    fn debug_request_overrides_reports_prompt_cache_omission_when_context_missing() {
        let mut cfg = OpenAiResponsesPromptCacheConfig::default();
        cfg.enabled = true;
        cfg.bucket_hash = PromptCacheBucketHashConfig::Bool(false);

        let provider = test_provider_with_prompt_cache("https://api.example.com/v1", cfg);
        let dbg = provider.debug_request_overrides(None);

        assert_eq!(dbg["prompt_cache"]["enabled"].as_bool(), Some(true));
        assert_eq!(dbg["prompt_cache"]["source"].as_str(), Some("omitted"));
        assert_eq!(dbg["prompt_cache"]["hashed"].as_bool(), Some(false));
        assert!(
            dbg.get("prompt_cache_key").is_none(),
            "debug payload must not invent fallback prompt_cache_key"
        );
    }

    #[test]
    fn build_responses_body_injects_builtin_web_search_without_function_tools() {
        let mut cfg = BuiltinWebSearchConfig::default();
        cfg.enabled = true;

        let provider = test_provider_with_builtin_web_search("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &[], false);

        let tools = body["tools"].as_array().expect("expected tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"].as_str(), Some("web_search"));
        assert_eq!(body["tool_choice"].as_str(), Some("auto"));
    }

    #[test]
    fn build_responses_body_filters_local_web_search_function_tool_when_builtin_enabled() {
        let mut cfg = BuiltinWebSearchConfig::default();
        cfg.enabled = true;

        let provider = test_provider_with_builtin_web_search("https://api.example.com/v1", cfg);
        let tools = vec![
            serde_json::json!({
                "name": "web_search",
                "description": "local web search tool",
                "parameters": {
                    "type": "object",
                    "properties": {},
                }
            }),
            serde_json::json!({
                "name": "exec",
                "description": "exec tool",
                "parameters": {
                    "type": "object",
                    "properties": {},
                }
            }),
        ];

        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &tools, false);
        let out_tools = body["tools"].as_array().expect("expected tools array");

        assert!(
            out_tools
                .iter()
                .any(|t| t["type"].as_str() == Some("web_search"))
        );
        assert!(out_tools.iter().any(|t| {
            t["type"].as_str() == Some("function") && t["name"].as_str() == Some("exec")
        }));
        assert!(!out_tools.iter().any(|t| {
            t["type"].as_str() == Some("function") && t["name"].as_str() == Some("web_search")
        }));
    }

    #[test]
    fn build_responses_body_include_sources_sets_include_param() {
        let mut cfg = BuiltinWebSearchConfig::default();
        cfg.enabled = true;
        cfg.include_sources = true;

        let provider = test_provider_with_builtin_web_search("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &[], false);

        let include = body["include"].as_array().expect("expected include array");
        assert!(
            include
                .iter()
                .any(|v| v.as_str() == Some("web_search_call.action.sources"))
        );
    }

    #[test]
    fn build_responses_body_includes_user_location_when_configured() {
        let mut cfg = BuiltinWebSearchConfig::default();
        cfg.enabled = true;
        cfg.user_location = Some(moltis_config::schema::WebSearchUserLocation::default());

        let provider = test_provider_with_builtin_web_search("https://api.example.com/v1", cfg);
        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &[], false);

        let tools = body["tools"].as_array().expect("expected tools array");
        let web_search = tools
            .iter()
            .find(|t| t["type"].as_str() == Some("web_search"))
            .expect("expected web_search tool");
        assert_eq!(
            web_search["user_location"]["type"].as_str(),
            Some("approximate")
        );
    }

    #[test]
    fn parse_responses_output_extracts_text_and_usage() {
        let resp = serde_json::json!({
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type":"output_text","text":"hello"}]
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 4,
                "input_tokens_details": {"cached_tokens": 0}
            }
        });

        let (text, tool_calls, usage) = parse_responses_output(&resp);
        assert_eq!(text.as_deref(), Some("hello"));
        assert!(tool_calls.is_empty());
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn responses_sse_parser_emits_text_and_tool_events() {
        let payload = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"He\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"llo\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"item1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"do_thing\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"{\\\"x\\\":1\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":6,\"input_tokens_details\":{\"cached_tokens\":12}}}}\n\n"
        );

        let mut parser = ResponsesSseParser::default();
        let events = parser.push_bytes(payload.as_bytes());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::Delta(d) if d == "He"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::Delta(d) if d == "llo"))
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallStart { id, name, index } if id == "c1" && name == "do_thing" && *index == 0)));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallArgumentsDelta { index, delta } if *index == 0 && delta.contains("\"x\""))));
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::Done(u) if u.input_tokens == 5 && u.output_tokens == 6 && u.cache_read_tokens == 12)));
    }

    #[test]
    fn streaming_completion_collector_joins_text_and_tool_args() {
        let payload = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"item1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"do_thing\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"{\\\"x\\\":1\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":4,\"input_tokens_details\":{\"cached_tokens\":3}}}}\n\n"
        );

        let resp = OpenAiResponsesProvider::parse_streaming_completion_from_sse(payload).unwrap();
        assert_eq!(resp.text.as_deref(), Some("hi"));
        assert_eq!(resp.usage.input_tokens, 7);
        assert_eq!(resp.usage.output_tokens, 4);
        assert_eq!(resp.usage.cache_read_tokens, 3);
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "c1");
        assert_eq!(resp.tool_calls[0].name, "do_thing");
        assert_eq!(resp.tool_calls[0].arguments, serde_json::json!({"x": 1}));
    }

    #[test]
    fn responses_sse_parser_accepts_data_without_space_and_multiline_frames() {
        let payload = concat!(
            "data:{\n",
            "data:  \"type\": \"response.output_text.delta\",\n",
            "data:  \"delta\": \"hello\"\n",
            "data:}\n",
            "\n",
            "data:[DONE]\n\n"
        );

        let mut parser = ResponsesSseParser::default();
        let events = parser.push_bytes(payload.as_bytes());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::Delta(d) if d == "hello"))
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Done(_))));
    }

    #[test]
    fn parse_streaming_completion_from_sse_returns_error_on_error_event() {
        let payload = "data: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n";
        let err = OpenAiResponsesProvider::parse_streaming_completion_from_sse(payload)
            .expect_err("expected error");
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn streaming_parser_ignores_web_search_call_events() {
        let payload = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.web_search_call.searching\",\"output_index\":0,\"item_id\":\"ws1\",\"sequence_number\":1}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":4}}}\n\n"
        );
        let resp = OpenAiResponsesProvider::parse_streaming_completion_from_sse(payload).unwrap();
        assert_eq!(resp.text.as_deref(), Some("hi"));
        assert_eq!(resp.usage.input_tokens, 7);
        assert_eq!(resp.usage.output_tokens, 4);
    }
}

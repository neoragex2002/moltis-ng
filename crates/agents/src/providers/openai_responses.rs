use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

use async_trait::async_trait;
use futures::StreamExt;
use secrecy::ExposeSecret;
use tokio_stream::Stream;
use tracing::{debug, trace, warn};

use crate::model::{ChatMessage, CompletionResponse, ContentPart, LlmProvider, StreamEvent, ToolCall, Usage, UserContent};

use super::openai_compat::to_responses_api_tools;

const OPENAI_RESPONSES_ENDPOINT_PATH: &str = "/responses";

fn responses_endpoint(base_url: &str) -> String {
    format!(
        "{}{OPENAI_RESPONSES_ENDPOINT_PATH}",
        base_url.trim_end_matches('/')
    )
}

fn responses_instructions(messages: &[ChatMessage]) -> Option<String> {
    let chunks: Vec<&str> = messages
        .iter()
        .filter_map(|m| match m {
            ChatMessage::System { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    if chunks.is_empty() {
        return None;
    }
    Some(chunks.join("\n\n"))
}

fn messages_to_responses_input(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .flat_map(|msg| match msg {
            ChatMessage::System { .. } => vec![],
            ChatMessage::User { content } => {
                let content_blocks = match content {
                    UserContent::Text(text) => vec![serde_json::json!({"type": "input_text", "text": text})],
                    UserContent::Multimodal(parts) => parts
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text(text) => {
                                serde_json::json!({"type": "input_text", "text": text})
                            }
                            ContentPart::Image { media_type, data } => {
                                let data_uri = format!("data:{media_type};base64,{data}");
                                serde_json::json!({
                                    "type": "input_image",
                                    "image_url": data_uri,
                                })
                            }
                        })
                        .collect(),
                };
                vec![serde_json::json!({
                    "role": "user",
                    "content": content_blocks,
                })]
            }
            ChatMessage::Assistant { content, tool_calls } => {
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
            }
            ChatMessage::Tool { tool_call_id, content } => vec![serde_json::json!({
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
            }
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
                tool_calls.push(ToolCall { id, name, arguments });
            }
            _ => {}
        }
    }

    let text = if text_buf.is_empty() { None } else { Some(text_buf) };
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
            }
            StreamEvent::ToolCallArgumentsDelta { index, delta } => {
                self.tool_args_by_index.entry(index).or_default().push_str(&delta);
            }
            StreamEvent::Done(usage) => self.usage = usage,
            StreamEvent::Error(_) | StreamEvent::ToolCallComplete { .. } => {}
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
            let args_str = self.tool_args_by_index.get(&index).map(String::as_str).unwrap_or("{}");
            let arguments = serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
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
            }
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
                }
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
                }
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
                }
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
                }
                "response.completed" => {
                    if let Some(u) = evt["response"]["usage"].as_object() {
                        self.input_tokens = u
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        self.output_tokens = u
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                    }

                    let mut indices: Vec<usize> = self.tool_calls_by_index.keys().copied().collect();
                    indices.sort_unstable();
                    for index in indices {
                        out.push(StreamEvent::ToolCallComplete { index });
                    }
                    out.push(StreamEvent::Done(Usage {
                        input_tokens: self.input_tokens,
                        output_tokens: self.output_tokens,
                        ..Default::default()
                    }));
                    self.done = true;
                    break;
                }
                "error" | "response.failed" => {
                    let msg = evt["error"]["message"]
                        .as_str()
                        .or_else(|| evt["message"].as_str())
                        .unwrap_or("unknown error");
                    out.push(StreamEvent::Error(msg.to_string()));
                    self.done = true;
                    break;
                }
                _ => {}
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
}

impl OpenAiResponsesProvider {
    pub fn new(api_key: secrecy::Secret<String>, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_name: "openai-responses".into(),
            client: reqwest::Client::new(),
        }
    }

    pub fn new_with_name(
        api_key: secrecy::Secret<String>,
        model: String,
        base_url: String,
        provider_name: String,
    ) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_name,
            client: reqwest::Client::new(),
        }
    }

    async fn post_responses(
        &self,
        body: &serde_json::Value,
    ) -> anyhow::Result<reqwest::Response> {
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
        if body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false) {
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
        let instructions = responses_instructions(messages)
            .unwrap_or_else(|| "You are a helpful assistant.".to_string());
        let input = messages_to_responses_input(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "input": input,
            "instructions": instructions,
            "store": false,
        });

        if stream {
            body["stream"] = serde_json::Value::Bool(true);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(to_responses_api_tools(tools));
            body["tool_choice"] = serde_json::json!("auto");
        }

        body
    }

    #[cfg(test)]
    fn parse_streaming_completion_from_sse(payload: &str) -> anyhow::Result<CompletionResponse> {
        let mut parser = ResponsesSseParser::default();
        let mut collector = ResponsesSseCollector::default();

        for event in parser.push_bytes(payload.as_bytes()) {
            match event {
                StreamEvent::Error(msg) => {
                    anyhow::bail!("OpenAI Responses API stream error: {msg}");
                }
                StreamEvent::Done(_) => {
                    collector.ingest_event(event);
                    break;
                }
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
        super::context_window_for_model(&self.model)
    }

    fn supports_vision(&self) -> bool {
        super::supports_vision_for_model(&self.model)
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        // Some Codex-style Responses gateways only support streaming mode.
        // Use `stream: true` and collect the full result into a single response.
        let body = self.build_responses_body(messages, tools, true);

        debug!(
            provider = %self.provider_name,
            model = %self.model,
            input_items_count = body["input"].as_array().map(|a| a.len()).unwrap_or(0),
            tools_count = tools.len(),
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
                    }
                    StreamEvent::Done(_) => {
                        collector.ingest_event(event);
                        return Ok(collector.into_completion());
                    }
                    other => collector.ingest_event(other),
                }
            }
        }

        anyhow::bail!("OpenAI Responses API stream ended unexpectedly");
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
        Box::pin(async_stream::stream! {
            let body = self.build_responses_body(&messages, &tools, true);

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

    #[test]
    fn build_responses_body_requires_instructions_and_input() {
        let provider = test_provider("https://api.example.com/v1");
        let body = provider.build_responses_body(&[ChatMessage::user("ping")], &[], false);
        assert_eq!(body["model"].as_str(), Some("gpt-4o"));
        assert!(body.get("instructions").is_some());
        assert!(body.get("input").is_some());
        assert!(body.get("stream").is_none());
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
    }

    #[test]
    fn responses_sse_parser_emits_text_and_tool_events() {
        let payload = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"He\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"llo\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"item1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"do_thing\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"{\\\"x\\\":1\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":6}}}\n\n"
        );

        let mut parser = ResponsesSseParser::default();
        let events = parser.push_bytes(payload.as_bytes());
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Delta(d) if d == "He")));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Delta(d) if d == "llo")));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallStart { id, name, index } if id == "c1" && name == "do_thing" && *index == 0)));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallArgumentsDelta { index, delta } if *index == 0 && delta.contains("\"x\""))));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Done(u) if u.input_tokens == 5 && u.output_tokens == 6)));
    }

    #[test]
    fn streaming_completion_collector_joins_text_and_tool_args() {
        let payload = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"item1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"do_thing\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"{\\\"x\\\":1\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"item1\",\"delta\":\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":4}}}\n\n"
        );

        let resp = OpenAiResponsesProvider::parse_streaming_completion_from_sse(payload).unwrap();
        assert_eq!(resp.text.as_deref(), Some("hi"));
        assert_eq!(resp.usage.input_tokens, 7);
        assert_eq!(resp.usage.output_tokens, 4);
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
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Delta(d) if d == "hello")));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Done(_))));
    }

    #[test]
    fn parse_streaming_completion_from_sse_returns_error_on_error_event() {
        let payload = "data: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n";
        let err = OpenAiResponsesProvider::parse_streaming_completion_from_sse(payload)
            .expect_err("expected error");
        assert!(err.to_string().contains("boom"));
    }
}

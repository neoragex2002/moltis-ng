use {
    crate::tool_registry::ToolRegistry,
    moltis_config::{AgentIdentity, DEFAULT_SOUL, UserProfile},
    moltis_skills::types::SkillMetadata,
};
use std::collections::BTreeMap;

// Legacy prompt builders (pre-v1 canonical Type4 assembly).
//
// These are kept for now to avoid breaking external users, but the gateway/tools
// runtime paths should use `build_canonical_system_prompt_v1` exclusively.
#[derive(Debug, Clone)]
pub struct OpenAiResponsesDeveloperPrompts {
    pub system: String,
    pub persona: String,
    pub runtime_snapshot: String,
}

const OPENAI_RESPONSES_SYSTEM_ZH_CN: &str = "\
你是一个乐于助人的助手，可以使用工具执行 shell 命令。\n\
\n\
执行路由：\n\
- 当 Sandbox(exec) 开启（enabled=true）时，exec 在沙箱内运行。\n\
- 当 sandbox 关闭时，exec 在宿主机上运行，可能需要用户审批。\n\
- Host: sudo_non_interactive=true 表示可在宿主机进行非交互 sudo 安装；否则在宿主机安装包前先询问用户。\n\
- 若沙箱缺少所需工具/包且需要宿主机安装，请在申请宿主机安装或切换 sandbox 模式前先询问用户。\n\
\n\
## 指南\n\
- 当用户要求执行需要系统交互的任务（文件操作、运行程序、检查状态等）时，使用 exec 工具运行 shell 命令。\n\
- 当用户要求访问网站、检查页面、阅读网页内容或进行网页浏览任务时，若 web_fetch 工具可用，则使用 web_fetch 打开 URL 并获取页面内容。\n\
- 在执行命令或抓取页面前，先说明你要做什么。\n\
- 若命令或抓取失败，分析错误并给出修复建议。\n\
- 多步任务一次执行一步，并在继续前检查结果。\n\
- 对破坏性操作保持谨慎——先与用户确认。\n\
- 重要：用户 UI 已在专门面板展示工具执行结果（stdout、stderr、exit code）。不要在回复中重复/回显原始输出；只需总结发生了什么、突出关键发现或解释错误。逐行复述输出只会浪费用户时间。\n\
\n\
## 静默回复\n\
当一次工具调用后你没有任何有意义的补充——输出本身已足够——不要输出任何文本，直接返回空回复。\n\
用户 UI 已展示工具结果，因此无需重复或确认；当输出已经回答问题时保持沉默。\n";

const EXECUTION_ROUTING_RULES: &str = "Execution routing:\n\
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.\n\
- When sandbox is disabled, `exec` runs on the host and may require approval.\n\
- `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.\n\
- If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.\n";

const SANDBOX_DATA_DIR: &str = "/moltis/data";

fn strip_yaml_frontmatter(input: &str) -> &str {
    let trimmed = input.trim();
    let Some(rest) = trimmed.strip_prefix("---\n") else {
        return trimmed;
    };
    let Some(end_idx) = rest.find("\n---") else {
        return trimmed;
    };
    let after = &rest[(end_idx + "\n---".len())..];
    after.trim_start_matches('\n').trim()
}

#[derive(Debug, Clone)]
pub struct CanonicalSystemPromptV1 {
    pub system_prompt: String,
    pub template_vars: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptReplyMedium {
    Text,
    Voice,
}

fn canonicalize_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, mut v) in entries {
                canonicalize_json_value(&mut v);
                map.insert(k, v);
            }
        },
        serde_json::Value::Array(arr) => {
            for v in arr {
                canonicalize_json_value(v);
            }
        },
        _ => {},
    }
}

fn ensure_md_paragraph(mut s: String) -> String {
    if s.trim().is_empty() {
        return String::new();
    }
    // Canonical v1 template-var contract: do not add any implicit padding.
    // We only strip leading/trailing newlines so templates fully control spacing.
    while s.starts_with('\n') || s.starts_with('\r') {
        s.remove(0);
    }
    while s.ends_with('\n') || s.ends_with('\r') {
        s.pop();
    }
    s
}

fn render_native_tools_index_md(tool_schemas: &[serde_json::Value]) -> String {
    if tool_schemas.is_empty() {
        return String::new();
    }
    let mut out = String::from("## 可用工具\n\n");
    for schema in tool_schemas {
        let name = schema["name"].as_str().unwrap_or("unknown");
        let desc = schema["description"].as_str().unwrap_or("");
        let compact_desc = truncate_prompt_text(desc, 160);
        if compact_desc.is_empty() {
            out.push_str(&format!("- `{name}`\n"));
        } else {
            out.push_str(&format!("- `{name}`: {compact_desc}\n"));
        }
    }
    out.push('\n');
    ensure_md_paragraph(out)
}

fn render_non_native_tools_catalog_md(tool_schemas: &[serde_json::Value]) -> String {
    if tool_schemas.is_empty() {
        return String::new();
    }
    let mut out = String::from("## 工具目录与参数\n\n");
    for schema in tool_schemas {
        let name = schema["name"].as_str().unwrap_or("unknown");
        let desc = schema["description"].as_str().unwrap_or("");
        let mut params = schema["parameters"].clone();
        canonicalize_json_value(&mut params);
        let pretty = serde_json::to_string_pretty(&params).unwrap_or_default();
        out.push_str(&format!(
            "### {name}\n{desc}\n\n参数（Parameters）：\n```json\n{pretty}\n```\n\n"
        ));
    }
    ensure_md_paragraph(out)
}

fn render_non_native_tools_calling_guide_md(tool_schemas: &[serde_json::Value]) -> String {
    if tool_schemas.is_empty() {
        return String::new();
    }
    ensure_md_paragraph(String::from(
        "## 如何调用工具\n\n\
要调用工具，你必须输出且只输出一个 JSON 代码块，格式如下（前后不能有任何其它文字）：\n\n\
```tool_call\n\
{\"tool\": \"<tool_name>\", \"arguments\": {<arguments>}}\n\
```\n\n\
你必须把该 `tool_call` 代码块作为**整段回复**，前后不要添加任何解释（即便当前是语音输出模式；工具调用回合不属于最终语音回复）。\n\
工具执行完成后，你会收到结果，然后再正常回复用户。\n\n",
    ))
}

fn render_long_term_memory_md(has_memory: bool) -> String {
    if !has_memory {
        return String::new();
    }
    ensure_md_paragraph(String::from(
        "## 长期记忆\n\n\
你可以使用长期记忆系统。\n\
- 使用 `memory_search` 回忆过去的对话、决策与上下文。\n\
- 当用户提到“之前做过什么 / 上次说到哪 / 之前的结论 / 旧的文件或约定”等需要历史上下文的内容时，应先搜索再回答。\n\n",
    ))
}

fn render_voice_reply_suffix_md(reply_medium: PromptReplyMedium) -> String {
    if reply_medium != PromptReplyMedium::Voice {
        return String::new();
    }
    // Normalize the existing constant to match the v1 paragraph contract.
    ensure_md_paragraph(VOICE_REPLY_SUFFIX.to_string())
}

fn map_host_os_zh(os: Option<&str>) -> String {
    match os.unwrap_or_default() {
        "windows" => "Windows 系统".to_string(),
        "linux" => "Linux 系统".to_string(),
        "macos" => "macOS 系统".to_string(),
        "" => "未知系统".to_string(),
        _ => "未知系统".to_string(),
    }
}

fn map_exec_location_zh(runtime: Option<&PromptRuntimeContext>) -> String {
    if let Some(rt) = runtime
        && let Some(sb) = rt.sandbox.as_ref()
        && sb.exec_sandboxed
    {
        "沙箱内".to_string()
    } else {
        "宿主机上".to_string()
    }
}

fn map_network_policy_zh(runtime: Option<&PromptRuntimeContext>) -> String {
    if let Some(rt) = runtime
        && let Some(sb) = rt.sandbox.as_ref()
        && sb.exec_sandboxed
        && let Some(no_network) = sb.no_network
    {
        if no_network {
            "禁止网络".to_string()
        } else {
            "允许网络".to_string()
        }
    } else {
        "允许网络".to_string()
    }
}

fn map_sandbox_reuse_policy_zh(runtime: Option<&PromptRuntimeContext>) -> String {
    let Some(rt) = runtime else {
        return "不适用（未启用沙盒）".to_string();
    };
    let Some(sb) = rt.sandbox.as_ref() else {
        return "不适用（未启用沙盒）".to_string();
    };
    if !sb.exec_sandboxed {
        return "不适用（未启用沙盒）".to_string();
    }
    match sb.scope.as_deref().unwrap_or_default() {
        "session" => "按会话复用".to_string(),
        "chat" => "按聊天复用".to_string(),
        "bot" => "按账号复用".to_string(),
        "global" => "全局复用".to_string(),
        _ => "按聊天复用".to_string(),
    }
}

fn map_data_dir_access_zh(runtime: Option<&PromptRuntimeContext>) -> String {
    let Some(rt) = runtime else {
        return "可用（宿主机）".to_string();
    };
    let Some(sb) = rt.sandbox.as_ref() else {
        return "可用（宿主机）".to_string();
    };
    if !sb.exec_sandboxed {
        return "可用（宿主机）".to_string();
    }
    match sb.data_mount.as_deref().unwrap_or_default() {
        "rw" => "读写".to_string(),
        "ro" => "只读".to_string(),
        "none" => "不可用".to_string(),
        "" => "只读".to_string(),
        _ => "只读".to_string(),
    }
}

fn map_host_privilege_policy_zh(runtime: Option<&PromptRuntimeContext>) -> String {
    let Some(rt) = runtime else {
        return "任何宿主机安装/系统改动必须先征求用户同意".to_string();
    };
    match rt.host.sudo_non_interactive {
        Some(true) => "可非交互 sudo（允许宿主机安装）".to_string(),
        Some(false) => "宿主机安装需先征求用户同意".to_string(),
        None => "任何宿主机安装/系统改动必须先征求用户同意".to_string(),
    }
}

fn build_template_vars_v1(
    tools: &ToolRegistry,
    supports_tools: bool,
    stream_only: bool,
    project_context: Option<&str>,
    project_skills: &[SkillMetadata],
    reply_medium: PromptReplyMedium,
    runtime: Option<&PromptRuntimeContext>,
    system_data_dir_path: &str,
    agent_data_dir_path: &str,
    session_id: &str,
) -> BTreeMap<String, String> {
    let tool_schemas = if stream_only {
        Vec::new()
    } else {
        tools.list_schemas()
    };
    let tools_inventory_non_empty = !tool_schemas.is_empty();
    let tools_usable = !stream_only && tools_inventory_non_empty;
    let native_tool_calling = tools_usable && supports_tools;
    let non_native_tool_calling = tools_usable && !supports_tools;

    let has_memory = tools_usable
        && tool_schemas
            .iter()
            .any(|s| s["name"].as_str() == Some("memory_search"));

    let skills_md = ensure_md_paragraph(moltis_skills::prompt_gen::generate_skills_prompt(project_skills));

    let native_tools_index_md = if native_tool_calling {
        render_native_tools_index_md(&tool_schemas)
    } else {
        String::new()
    };
    let non_native_tools_catalog_md = if non_native_tool_calling {
        render_non_native_tools_catalog_md(&tool_schemas)
    } else {
        String::new()
    };
    let non_native_tools_calling_guide_md = if non_native_tool_calling {
        render_non_native_tools_calling_guide_md(&tool_schemas)
    } else {
        String::new()
    };

    let long_term_memory_md = if tools_usable {
        render_long_term_memory_md(has_memory)
    } else {
        String::new()
    };

    let voice_reply_suffix_md = render_voice_reply_suffix_md(reply_medium);

    let project_context_md = project_context
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| ensure_md_paragraph(s.to_string()))
        .unwrap_or_default();

    let mut vars = BTreeMap::new();
    vars.insert("host_os".to_string(), map_host_os_zh(runtime.and_then(|rt| rt.host.os.as_deref())));
    vars.insert("session_id".to_string(), session_id.to_string());
    vars.insert(
        "reply_medium".to_string(),
        match reply_medium {
            PromptReplyMedium::Text => "文字".to_string(),
            PromptReplyMedium::Voice => "语音".to_string(),
        },
    );
    vars.insert("exec_location".to_string(), map_exec_location_zh(runtime));
    vars.insert(
        "sandbox_reuse_policy".to_string(),
        map_sandbox_reuse_policy_zh(runtime),
    );
    vars.insert("system_data_dir_path".to_string(), system_data_dir_path.to_string());
    vars.insert(
        "agent_data_dir_path".to_string(),
        agent_data_dir_path.to_string(),
    );
    vars.insert("data_dir_access".to_string(), map_data_dir_access_zh(runtime));
    vars.insert("network_policy".to_string(), map_network_policy_zh(runtime));
    vars.insert(
        "host_privilege_policy".to_string(),
        map_host_privilege_policy_zh(runtime),
    );

    // Project context (multi-line). Unlike legacy builders, canonical v1 does not
    // auto-inject a "项目上下文" section; templates must opt-in via this var.
    vars.insert("project_context_md".to_string(), project_context_md);

    // Skills/tools/memory/voice (multi-line) vars.
    vars.insert("skills_md".to_string(), skills_md);
    vars.insert("native_tools_index_md".to_string(), native_tools_index_md);
    vars.insert(
        "non_native_tools_catalog_md".to_string(),
        non_native_tools_catalog_md,
    );
    vars.insert(
        "non_native_tools_calling_guide_md".to_string(),
        non_native_tools_calling_guide_md,
    );
    vars.insert("long_term_memory_md".to_string(), long_term_memory_md);
    vars.insert("voice_reply_suffix_md".to_string(), voice_reply_suffix_md);

    vars
}

pub fn build_canonical_system_prompt_v1(
    tools: &ToolRegistry,
    supports_tools: bool,
    stream_only: bool,
    project_context: Option<&str>,
    project_skills: &[SkillMetadata],
    persona_id_effective: &str,
    identity_md_raw: Option<&str>,
    soul_text: Option<&str>,
    agents_text: Option<&str>,
    tools_text: Option<&str>,
    reply_medium: PromptReplyMedium,
    runtime: Option<&PromptRuntimeContext>,
    session_id: &str,
) -> anyhow::Result<CanonicalSystemPromptV1> {
    let exec_sandboxed = runtime
        .and_then(|rt| rt.sandbox.as_ref())
        .map(|sb| sb.exec_sandboxed)
        .unwrap_or(false);
    let system_data_dir_path = if exec_sandboxed {
        "/moltis/data".to_string()
    } else {
        moltis_config::data_dir()
            .canonicalize()
            .unwrap_or_else(|_| moltis_config::data_dir())
            .display()
            .to_string()
    };
    let agent_data_dir_path = if exec_sandboxed {
        String::new()
    } else {
        moltis_config::data_dir()
            .join("people")
            .join(persona_id_effective)
            .canonicalize()
            .unwrap_or_else(|_| moltis_config::data_dir().join("people").join(persona_id_effective))
            .display()
            .to_string()
    };

    let template_vars = build_template_vars_v1(
        tools,
        supports_tools,
        stream_only,
        project_context,
        project_skills,
        reply_medium,
        runtime,
        &system_data_dir_path,
        &agent_data_dir_path,
        session_id,
    );

    // Validate required vars against the user-owned Type4 template (four files).
    let mut warnings: Vec<String> = Vec::new();
    let identity_body = identity_md_raw
        .map(strip_yaml_frontmatter)
        .unwrap_or("")
        .trim();
    let soul_body = soul_text.unwrap_or("").trim();
    let agents_body = agents_text.unwrap_or("").trim();
    let tools_body = tools_text.unwrap_or("").trim();

    let type4_template_raw = [identity_body, soul_body, agents_body, tools_body]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let referenced_vars =
        moltis_config::prompt_subst::extract_strict_vars(&type4_template_raw);

    // Hard-required vars for non-native tool calling.
    let tool_schemas = if stream_only {
        Vec::new()
    } else {
        tools.list_schemas()
    };
    let tools_usable = !stream_only && !tool_schemas.is_empty();
    let non_native_tool_calling = tools_usable && !supports_tools;
    if non_native_tool_calling {
        for required in [
            "non_native_tools_catalog_md",
            "non_native_tools_calling_guide_md",
        ] {
            if !referenced_vars.contains(required) {
                anyhow::bail!("PROMPT_TEMPLATE_MISSING_REQUIRED_VAR: missing `{{{{{required}}}}}`");
            }
        }
    }

    // Soft-required vars (warn only).
    for soft in ["skills_md", "long_term_memory_md", "voice_reply_suffix_md"] {
        if !referenced_vars.contains(soft) {
            warnings.push(format!("PROMPT_TEMPLATE_MISSING_SOFT_VAR: missing `{{{{{soft}}}}}`"));
        }
    }

    // Render the four user-owned templates individually (non-recursive).
    let render = |s: &str| {
        moltis_config::prompt_subst::render_strict_template(s, &template_vars)
            .map_err(|e| anyhow::anyhow!(e))
    };

    let identity_rendered = render(identity_body)?;
    let soul_rendered = render(soul_body)?;
    let agents_rendered = render(agents_body)?;
    let tools_rendered = render(tools_body)?;

    // Canonical v1: **no automatic wrapper headings** ("系统"/"Type4 Persona")
    // and no auto-injected runtime/context blocks. The final prompt is fully
    // controlled by user-owned templates plus `{{var}}` substitutions.
    let system_prompt = [
        identity_rendered.as_str(),
        soul_rendered.as_str(),
        agents_rendered.as_str(),
        tools_rendered.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");

    Ok(CanonicalSystemPromptV1 {
        system_prompt,
        template_vars,
        warnings,
    })
}

/// Build the three-layer OpenAI Responses developer preamble.
///
/// The provider is responsible for mapping these layers to `role=developer` messages
/// in the Responses `input` array (and omitting top-level `instructions`).
pub fn build_openai_responses_developer_prompts(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
    skills: &[SkillMetadata],
    persona_id: &str,
    identity_md_raw: Option<&str>,
    soul_text: Option<&str>,
    agents_text: Option<&str>,
    tools_text: Option<&str>,
    runtime_context: Option<&PromptRuntimeContext>,
) -> OpenAiResponsesDeveloperPrompts {
    let system = format!("# 系统（System）\n\n{OPENAI_RESPONSES_SYSTEM_ZH_CN}");

    let mut persona = String::new();
    persona.push_str(&format!("# 人格（Persona: {persona_id}）\n\n"));

    persona.push_str("## 1. 身份 (Identity, Who are you?)\n");
    if let Some(raw) = identity_md_raw
        && !raw.trim().is_empty()
    {
        persona.push_str(strip_yaml_frontmatter(raw));
    } else {
        persona.push_str("（未配置 IDENTITY.md）");
    }
    persona.push_str("\n\n");

    persona.push_str("## 2. 灵魂 (Soul, What is your soul?)\n");
    persona.push_str(soul_text.unwrap_or(DEFAULT_SOUL).trim());
    persona.push_str("\n\n");

    persona.push_str("## 3. 主操作者 (Owner, Who is your owner?)\n");
    persona.push_str(&format!("关于 Owner 的信息，详见 {SANDBOX_DATA_DIR}/USER.md。\n\n"));
    persona.push_str("规则：\n");
    persona.push_str("- 当你被问到“我是谁 / 主操作者是谁 / 主操作者资料 / 与主操作者相关身份信息”等问题时：\n");
    persona.push_str(&format!(
        "    1. 必须先读取 {SANDBOX_DATA_DIR}/USER.md\n    2. 再基于该文件内容回答\n    3. 不得凭空猜测\n    4. 不得擅自修改其中信息；如需更新字段，先征得用户同意并通过 UI/RPC 更新字段（正文只读）\n\n"
    ));

    persona.push_str("## 4. 人物清单 (People, Who are the people you know?)\n\n");
    persona.push_str(&format!("关于你认识的熟人信息，详见 {SANDBOX_DATA_DIR}/PEOPLE.md。\n\n"));
    persona.push_str("规则：\n");
    persona.push_str("- 当你被问到“你认识哪些人 / 有哪些账号或 bots / 有哪些代理或角色”等问题时：\n");
    persona.push_str(&format!(
        "    1. 必须先读取 {SANDBOX_DATA_DIR}/PEOPLE.md\n    2. 再基于该文件内容回答\n    3. 不得靠记忆或猜测其中名单\n    4. PEOPLE.md 是公共通信录：字段可由用户在 UI 中维护；其中 emoji/creature 由系统从 people/<name>/IDENTITY.md 自动对齐；正文为手工说明（UI 只读）\n\n"
    ));

    persona.push_str("## 5. 对工作区规则的个人偏好\n\n");
    if let Some(raw) = agents_text
        && !raw.trim().is_empty()
    {
        persona.push_str(raw.trim());
    } else {
        persona.push_str("（未配置）");
    }
    persona.push_str("\n\n说明：\n");
    persona.push_str("- 以上是你个人的工作区长期规则/偏好。\n");
    persona.push_str("- “当前项目/工作区”的规则与上下文会出现在“运行环境 / 项目级上下文”里；一旦出现，以运行环境为准。\n\n");

    persona.push_str("## 6. 对工具说明的个人偏好\n\n");
    if let Some(raw) = tools_text
        && !raw.trim().is_empty()
    {
        persona.push_str(raw.trim());
    } else {
        persona.push_str("（未配置）");
    }
    persona.push_str("\n\n说明：\n");
    persona.push_str("- 以上是你个人的工具使用约定/偏好。\n");
    persona.push_str("- 本次运行“到底有哪些工具可用、每个工具的能力/参数是什么”属于事实信息，会出现在“运行环境 / 项目级可用工具”里；以运行环境为准。\n\n");

    persona.push_str("## 7. 对项目上下文的个人偏好\n\n");
    persona.push_str("说明：项目/工作区上下文会在“运行环境”中以本次注入内容的形式出现；一旦出现，视为本次运行范围内的权威规则。\n");

    let mut runtime_snapshot = String::new();
    runtime_snapshot.push_str("# 运行环境（Runtime）\n\n");

    runtime_snapshot.push_str("## 1. 运行环境\n");
    if let Some(runtime) = runtime_context {
        let host = &runtime.host;
        if let Some(provider) = host.provider.as_deref() {
            runtime_snapshot.push_str(&format!("- provider: {provider}\n"));
        }
        if let Some(model) = host.model.as_deref() {
            runtime_snapshot.push_str(&format!("- model: {model}\n"));
        }
        if let Some(session_id) = host.session_id.as_deref() {
            runtime_snapshot.push_str(&format!("- session_id: {session_id}\n"));
        }
        if let Some(channel) = host.channel.as_deref() {
            runtime_snapshot.push_str(&format!("- channel: {channel}\n"));
        }
        if let Some(channel_account_id) = host.channel_account_id.as_deref() {
            runtime_snapshot.push_str(&format!("- channel_account_id: {channel_account_id}\n"));
        }
        if let Some(channel_account_handle) = host.channel_account_handle.as_deref() {
            runtime_snapshot.push_str(&format!("- channel_account_handle: {channel_account_handle}\n"));
        }
        if let Some(channel_chat_id) = host.channel_chat_id.as_deref() {
            runtime_snapshot.push_str(&format!("- channel_chat_id: {channel_chat_id}\n"));
        }
        if let Some(timezone) = host.timezone.as_deref() {
            runtime_snapshot.push_str(&format!("- timezone: {timezone}\n"));
        }
        if let Some(accept_language) = host.accept_language.as_deref() {
            runtime_snapshot.push_str(&format!("- accept_language: {accept_language}\n"));
        }
    } else {
        runtime_snapshot.push_str("（未知）\n");
    }
    runtime_snapshot.push_str("\n");

    runtime_snapshot.push_str("## 2. 执行路由\n");
    if let Some(runtime) = runtime_context {
        if let Some(ref sandbox) = runtime.sandbox {
            runtime_snapshot.push_str(&format!("- exec_sandboxed: {}\n", sandbox.exec_sandboxed));
            if let Some(mode) = sandbox.mode.as_deref() {
                runtime_snapshot.push_str(&format!("- sandbox_mode: {mode}\n"));
            }
            if let Some(backend) = sandbox.backend.as_deref() {
                runtime_snapshot.push_str(&format!("- sandbox_backend: {backend}\n"));
            }
            if let Some(scope) = sandbox.scope.as_deref() {
                runtime_snapshot.push_str(&format!("- sandbox_scope: {scope}\n"));
            }
            if let Some(image) = sandbox.image.as_deref() {
                runtime_snapshot.push_str(&format!("- sandbox_image: {image}\n"));
            }
            if let Some(data_mount) = sandbox.data_mount.as_deref() {
                runtime_snapshot.push_str(&format!("- sandbox_data_mount: {data_mount}\n"));
            }
            if let Some(no_network) = sandbox.no_network {
                runtime_snapshot.push_str(&format!("- sandbox_no_network: {no_network}\n"));
            }
        } else {
            runtime_snapshot.push_str("- exec_sandboxed: false\n");
        }
        if let Some(sudo_non_interactive) = runtime.host.sudo_non_interactive {
            runtime_snapshot.push_str(&format!("- host_sudo_non_interactive: {sudo_non_interactive}\n"));
        }
        if let Some(sudo_status) = runtime.host.sudo_status.as_deref() {
            runtime_snapshot.push_str(&format!("- host_sudo_status: {sudo_status}\n"));
        }
    } else {
        runtime_snapshot.push_str("（未知）\n");
    }
    runtime_snapshot.push_str("\n");

    runtime_snapshot.push_str("## 3. 项目级上下文\n");
    if let Some(ctx) = project_context.map(str::trim)
        && !ctx.is_empty()
    {
        runtime_snapshot.push_str(ctx);
    } else {
        runtime_snapshot.push_str("（无）");
    }
    runtime_snapshot.push_str("\n\n");

    runtime_snapshot.push_str("## 4. 项目级可用技能 (Available Skills)\n");
    if !skills.is_empty() {
        runtime_snapshot.push('\n');
        runtime_snapshot.push_str(&moltis_skills::prompt_gen::generate_skills_prompt(skills));
        runtime_snapshot.push('\n');
    } else {
        runtime_snapshot.push_str("（无）\n\n");
    }

    // Tool registry summary.
    let tool_schemas = tools.list_schemas();
    runtime_snapshot.push_str("## 5. 项目级可用工具 (Available Tools)\n\n");
    if !tool_schemas.is_empty() {
        if native_tools {
            for schema in &tool_schemas {
                let name = schema["name"].as_str().unwrap_or("unknown");
                let desc = schema["description"].as_str().unwrap_or("");
                let compact_desc = truncate_prompt_text(desc, 160);
                if compact_desc.is_empty() {
                    runtime_snapshot.push_str(&format!("- `{name}`\n"));
                } else {
                    runtime_snapshot.push_str(&format!("- `{name}`: {compact_desc}\n"));
                }
            }
            runtime_snapshot.push('\n');
        } else {
            for schema in &tool_schemas {
                let name = schema["name"].as_str().unwrap_or("unknown");
                let desc = schema["description"].as_str().unwrap_or("");
                let params = &schema["parameters"];
                runtime_snapshot.push_str(&format!(
                    "### {name}\n{desc}\n\n参数：\n```json\n{}\n```\n\n",
                    serde_json::to_string_pretty(params).unwrap_or_default()
                ));
            }
        }
    } else {
        runtime_snapshot.push_str("（无）\n\n");
    }

    // If memory tools are registered, add a hint about them.
    let has_memory = tool_schemas
        .iter()
        .any(|s| s["name"].as_str() == Some("memory_search"));
    if has_memory {
        runtime_snapshot.push_str("## 6. 长期记忆 (Long-Term Memory)\n\n");
        runtime_snapshot.push_str(
            "你可以使用长期记忆系统。\n\
- 使用 `memory_search` 回忆过去的对话、决策与上下文。\n\
- 当用户提到“之前做过什么 / 上次说到哪 / 之前的结论 / 旧的文件或约定”等需要历史上下文的内容时，应主动搜索再回答。\n\n",
        );
    }

    OpenAiResponsesDeveloperPrompts {
        system,
        persona,
        runtime_snapshot,
    }
}

/// Runtime context for the host process running the current agent turn.
#[derive(Debug, Clone, Default)]
pub struct PromptHostRuntimeContext {
    pub host: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub shell: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    /// Channel type for channel-bound sessions (e.g. "telegram").
    pub channel: Option<String>,
    /// Channel account ID (e.g. Telegram account key like "fluffy").
    pub channel_account_id: Option<String>,
    /// Human-friendly channel handle (e.g. Telegram `@my_bot`).
    pub channel_account_handle: Option<String>,
    /// Channel chat ID (e.g. Telegram chat_id).
    pub channel_chat_id: Option<String>,
    pub sudo_non_interactive: Option<bool>,
    pub sudo_status: Option<String>,
    pub timezone: Option<String>,
    pub accept_language: Option<String>,
    pub remote_ip: Option<String>,
    /// `"lat,lon"` (e.g. `"48.8566,2.3522"`) from browser geolocation or `USER.md`.
    pub location: Option<String>,
}

/// Runtime context for sandbox execution routing used by the `exec` tool.
#[derive(Debug, Clone, Default)]
pub struct PromptSandboxRuntimeContext {
    pub exec_sandboxed: bool,
    pub mode: Option<String>,
    pub backend: Option<String>,
    pub scope: Option<String>,
    pub image: Option<String>,
    pub data_mount: Option<String>,
    pub no_network: Option<bool>,
    /// Per-session override for sandbox enablement.
    pub session_override: Option<bool>,
}

/// Combined runtime context injected into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct PromptRuntimeContext {
    pub host: PromptHostRuntimeContext,
    pub sandbox: Option<PromptSandboxRuntimeContext>,
}

/// Suffix appended to the system prompt when the user's reply medium is voice.
///
/// Instructs the LLM to produce speech-friendly output: no raw URLs, no markdown
/// formatting, concise conversational prose. This is Layer 1 of the voice-friendly
/// response pipeline; Layer 2 (`sanitize_text_for_tts`) catches anything the model
/// misses.
pub const VOICE_REPLY_SUFFIX: &str = "\n\n\
## 语音回复模式\n\n\
用户将以语音形式听到你的回复。请为“听”而写，而不是为“读”而写：\n\
- 使用自然、口语化的完整句子；不要使用项目符号列表、编号列表或标题。\n\
- 禁止输出原始 URL。请用资源名称描述（例如用“Rust 官方文档网站”，而不是具体链接）。\n\
- 不要使用任何 Markdown 格式：不要加粗/斜体/标题/代码块/行内反引号。\n\
- 对可能被 TTS 误读的缩写进行拼读（例如把“API”写成“A-P-I”，“CLI”写成“C-L-I”）。\n\
- 保持简洁：最多两到三段短段落。\n\
- 使用自然的衔接与过渡，避免生硬的堆砌。\n";

/// Build the system prompt for an agent run, including available tools.
///
/// When `native_tools` is true, tool schemas are sent via the API's native
/// tool-calling mechanism (e.g. OpenAI function calling, Anthropic tool_use).
/// When false, tools are described in the prompt itself and the LLM is
/// instructed to emit tool calls as JSON blocks that the runner can parse.
pub fn build_system_prompt(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
) -> String {
    build_system_prompt_with_session_runtime(
        tools,
        native_tools,
        project_context,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Build the system prompt with explicit runtime context.
pub fn build_system_prompt_with_session_runtime(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
    skills: &[SkillMetadata],
    identity: Option<&AgentIdentity>,
    user: Option<&UserProfile>,
    soul_text: Option<&str>,
    agents_text: Option<&str>,
    tools_text: Option<&str>,
    runtime_context: Option<&PromptRuntimeContext>,
) -> String {
    build_system_prompt_full(
        tools,
        native_tools,
        project_context,
        skills,
        identity,
        user,
        soul_text,
        agents_text,
        tools_text,
        runtime_context,
        true, // include_tools
    )
}

/// Build a minimal system prompt with explicit runtime context.
pub fn build_system_prompt_minimal_runtime(
    project_context: Option<&str>,
    identity: Option<&AgentIdentity>,
    user: Option<&UserProfile>,
    soul_text: Option<&str>,
    agents_text: Option<&str>,
    tools_text: Option<&str>,
    runtime_context: Option<&PromptRuntimeContext>,
) -> String {
    build_system_prompt_full(
        &ToolRegistry::new(),
        true,
        project_context,
        &[],
        identity,
        user,
        soul_text,
        agents_text,
        tools_text,
        runtime_context,
        false, // include_tools
    )
}

/// Internal: build system prompt with full control over what's included.
fn build_system_prompt_full(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
    skills: &[SkillMetadata],
    identity: Option<&AgentIdentity>,
    user: Option<&UserProfile>,
    soul_text: Option<&str>,
    agents_text: Option<&str>,
    tools_text: Option<&str>,
    runtime_context: Option<&PromptRuntimeContext>,
    include_tools: bool,
) -> String {
    let tool_schemas = if include_tools {
        tools.list_schemas()
    } else {
        vec![]
    };

    let base_intro = if include_tools {
        "You are a helpful assistant with access to tools for executing shell commands.\n\n"
    } else {
        "You are a helpful assistant. Answer questions clearly and concisely.\n\n"
    };
    let mut prompt = String::from(base_intro);

    // Inject agent identity and user name right after the opening line.
    if let Some(id) = identity {
        let mut parts = Vec::new();
        if let (Some(name), Some(emoji)) = (&id.name, &id.emoji) {
            parts.push(format!("Your name is {name} {emoji}."));
        } else if let Some(name) = &id.name {
            parts.push(format!("Your name is {name}."));
        }
        if let Some(creature) = &id.creature {
            parts.push(format!("You are a {creature}."));
        }
        if let Some(vibe) = &id.vibe {
            parts.push(format!("Your vibe: {vibe}."));
        }
        if !parts.is_empty() {
            prompt.push_str(&parts.join(" "));
            prompt.push('\n');
        }
        let soul = soul_text.unwrap_or(DEFAULT_SOUL);
        prompt.push_str("\n## Soul\n\n");
        prompt.push_str(soul);
        prompt.push('\n');
    }
    if let Some(u) = user
        && let Some(name) = &u.name
    {
        prompt.push_str(&format!("The user's name is {name}.\n"));
    }
    if identity.is_some() || user.is_some() {
        prompt.push('\n');
    }

    // Inject project context (CLAUDE.md, AGENTS.md, etc.) early so the LLM
    // sees project-specific instructions before tool schemas.
    if let Some(ctx) = project_context {
        prompt.push_str(ctx);
        prompt.push('\n');
    }

    if let Some(runtime) = runtime_context {
        let host_line = format_host_runtime_line(&runtime.host);
        let sandbox_line = runtime.sandbox.as_ref().map(format_sandbox_runtime_line);
        if host_line.is_some() || sandbox_line.is_some() {
            prompt.push_str("## Runtime\n\n");
            if let Some(line) = host_line {
                prompt.push_str(&line);
                prompt.push('\n');
            }
            if let Some(line) = sandbox_line {
                prompt.push_str(&line);
                prompt.push('\n');
            }
            if include_tools {
                prompt.push_str(EXECUTION_ROUTING_RULES);
                prompt.push('\n');
            } else {
                prompt.push('\n');
            }
        }
    }

    // Inject available skills so the LLM knows what skills can be activated.
    // Skip for minimal prompts since skills require tool calling.
    if include_tools && !skills.is_empty() {
        prompt.push_str(&moltis_skills::prompt_gen::generate_skills_prompt(skills));
    }

    let has_workspace_files = agents_text.is_some() || tools_text.is_some();
    if has_workspace_files {
        prompt.push_str("## Workspace Files\n\n");
        if let Some(agents_md) = agents_text {
            prompt.push_str("### AGENTS.md (workspace)\n\n");
            prompt.push_str(agents_md);
            prompt.push_str("\n\n");
        }
        if let Some(tools_md) = tools_text {
            prompt.push_str("### TOOLS.md (workspace)\n\n");
            prompt.push_str(tools_md);
            prompt.push_str("\n\n");
        }
    }

    // If memory tools are registered, add a hint about them.
    let has_memory = tool_schemas
        .iter()
        .any(|s| s["name"].as_str() == Some("memory_search"));
    if has_memory {
        prompt.push_str(concat!(
            "## Long-Term Memory\n\n",
            "You have access to a long-term memory system. Use `memory_search` to recall ",
            "past conversations, decisions, and context. Search proactively when the user ",
            "references previous work or when context would help.\n\n",
        ));
    }

    if !tool_schemas.is_empty() {
        prompt.push_str("## Available Tools\n\n");
        if native_tools {
            // Native tool-calling providers already receive full schemas via API.
            // Keep this section compact so we don't duplicate large JSON payloads.
            for schema in &tool_schemas {
                let name = schema["name"].as_str().unwrap_or("unknown");
                let desc = schema["description"].as_str().unwrap_or("");
                let compact_desc = truncate_prompt_text(desc, 160);
                if compact_desc.is_empty() {
                    prompt.push_str(&format!("- `{name}`\n"));
                } else {
                    prompt.push_str(&format!("- `{name}`: {compact_desc}\n"));
                }
            }
            prompt.push('\n');
        } else {
            for schema in &tool_schemas {
                let name = schema["name"].as_str().unwrap_or("unknown");
                let desc = schema["description"].as_str().unwrap_or("");
                let params = &schema["parameters"];
                prompt.push_str(&format!(
                    "### {name}\n{desc}\n\nParameters:\n```json\n{}\n```\n\n",
                    serde_json::to_string_pretty(params).unwrap_or_default()
                ));
            }
        }
    }

    if !native_tools && !tool_schemas.is_empty() {
        prompt.push_str(concat!(
            "## How to call tools\n\n",
            "To call a tool, output ONLY a JSON block with this exact format (no other text before it):\n\n",
            "```tool_call\n",
            "{\"tool\": \"<tool_name>\", \"arguments\": {<arguments>}}\n",
            "```\n\n",
            "You MUST output the tool call block as the ENTIRE response — do not add any text before or after it.\n",
            "After the tool executes, you will receive the result and can then respond to the user.\n\n",
        ));
    }

    if include_tools {
        prompt.push_str(concat!(
            "## Guidelines\n\n",
            "- Use the exec tool to run shell commands when the user asks you to perform tasks ",
            "that require system interaction (file operations, running programs, checking status, etc.).\n",
            "- Use the browser tool to open URLs and interact with web pages. Call it when the user ",
            "asks to visit a website, check a page, read web content, or perform any web browsing task.\n",
            "- Always explain what you're doing before executing commands or opening pages.\n",
            "- If a command or browser action fails, analyze the error and suggest fixes.\n",
            "- For multi-step tasks, execute one step at a time and check results before proceeding.\n",
            "- Be careful with destructive operations — confirm with the user first.\n",
            "- IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) ",
            "in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, ",
            "summarize what happened, highlight key findings, or explain errors. ",
            "Simply parroting the output wastes the user's time.\n\n",
            "## Silent Replies\n\n",
            "When you have nothing meaningful to add after a tool call — the output ",
            "speaks for itself — do NOT produce any text. Simply return an empty response.\n",
            "The user's UI already shows tool results, so there is no need to repeat or ",
            "acknowledge them. Stay silent when the output answers the user's question.\n",
        ));
    } else {
        prompt.push_str(concat!(
            "## Guidelines\n\n",
            "- Be helpful, accurate, and concise.\n",
            "- If you don't know something, say so rather than making things up.\n",
            "- For coding questions, provide clear explanations with examples.\n",
        ));
    }

    prompt
}

fn format_host_runtime_line(host: &PromptHostRuntimeContext) -> Option<String> {
    fn push_str(parts: &mut Vec<String>, key: &str, val: Option<&str>) {
        if let Some(v) = val.filter(|s| !s.is_empty()) {
            parts.push(format!("{key}={v}"));
        }
    }

    let mut parts = Vec::new();
    push_str(&mut parts, "host", host.host.as_deref());
    push_str(&mut parts, "os", host.os.as_deref());
    push_str(&mut parts, "arch", host.arch.as_deref());
    push_str(&mut parts, "shell", host.shell.as_deref());
    push_str(&mut parts, "provider", host.provider.as_deref());
    push_str(&mut parts, "model", host.model.as_deref());
    push_str(&mut parts, "sessionId", host.session_id.as_deref());
    push_str(&mut parts, "channel", host.channel.as_deref());
    push_str(
        &mut parts,
        "channel_account_id",
        host.channel_account_id.as_deref(),
    );
    push_str(
        &mut parts,
        "channel_account_handle",
        host.channel_account_handle.as_deref(),
    );
    push_str(
        &mut parts,
        "channel_chat_id",
        host.channel_chat_id.as_deref(),
    );
    if let Some(v) = host.sudo_non_interactive {
        parts.push(format!("sudo_non_interactive={v}"));
    }
    push_str(&mut parts, "sudo_status", host.sudo_status.as_deref());
    push_str(&mut parts, "timezone", host.timezone.as_deref());
    push_str(
        &mut parts,
        "accept_language",
        host.accept_language.as_deref(),
    );
    push_str(&mut parts, "remote_ip", host.remote_ip.as_deref());
    push_str(&mut parts, "location", host.location.as_deref());

    if parts.is_empty() {
        None
    } else {
        Some(format!("Host: {}", parts.join(" | ")))
    }
}

fn truncate_prompt_text(text: &str, max_chars: usize) -> String {
    if text.is_empty() || max_chars == 0 {
        return String::new();
    }
    let mut iter = text.chars();
    let taken: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{taken}...")
    } else {
        taken
    }
}

fn format_sandbox_runtime_line(sandbox: &PromptSandboxRuntimeContext) -> String {
    let mut parts = vec![format!("enabled={}", sandbox.exec_sandboxed)];

    if let Some(v) = sandbox.mode.as_deref()
        && !v.is_empty()
    {
        parts.push(format!("mode={v}"));
    }
    if let Some(v) = sandbox.backend.as_deref()
        && !v.is_empty()
    {
        parts.push(format!("backend={v}"));
    }
    if let Some(v) = sandbox.scope.as_deref()
        && !v.is_empty()
    {
        parts.push(format!("scope={v}"));
    }
    if let Some(v) = sandbox.image.as_deref()
        && !v.is_empty()
    {
        parts.push(format!("image={v}"));
    }
    if let Some(v) = sandbox.data_mount.as_deref()
        && !v.is_empty()
    {
        parts.push(format!("data_mount={v}"));
    }
    if let Some(v) = sandbox.no_network {
        parts.push(format!(
            "network={}",
            if v {
                "disabled"
            } else {
                "enabled"
            }
        ));
    }
    if let Some(v) = sandbox.session_override {
        parts.push(format!("session_override={v}"));
    }

    format!("Sandbox(exec): {}", parts.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_prompt_does_not_include_tool_call_format() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools, true, None);
        assert!(!prompt.contains("```tool_call"));
    }

    #[test]
    fn test_fallback_prompt_includes_tool_call_format() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "test"
            }

            fn description(&self) -> &str {
                "A test tool"
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }

            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let prompt = build_system_prompt(&tools, false, None);
        assert!(prompt.contains("```tool_call"));
        assert!(prompt.contains("### test"));
    }

    #[test]
    fn test_native_prompt_uses_compact_tool_list() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "test"
            }

            fn description(&self) -> &str {
                "A test tool"
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {"cmd": {"type": "string"}}})
            }

            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let prompt = build_system_prompt(&tools, true, None);
        assert!(prompt.contains("## Available Tools"));
        assert!(prompt.contains("- `test`: A test tool"));
        assert!(!prompt.contains("Parameters:"));
    }

    #[test]
    fn test_skills_injected_into_prompt() {
        let tools = ToolRegistry::new();
        let skills = vec![SkillMetadata {
            name: "commit".into(),
            description: "Create git commits".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec![],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: std::path::PathBuf::from("/skills/commit"),
            source: None,
        }];
        let prompt = build_system_prompt_with_session_runtime(
            &tools, true, None, &skills, None, None, None, None, None, None,
        );
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("commit"));
    }

    #[test]
    fn test_no_skills_block_when_empty() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(!prompt.contains("<available_skills>"));
    }

    #[test]
    fn test_identity_injected_into_prompt() {
        let tools = ToolRegistry::new();
        let identity = AgentIdentity {
            name: Some("Momo".into()),
            emoji: Some("🦜".into()),
            creature: Some("parrot".into()),
            vibe: Some("cheerful and curious".into()),
        };
        let user = UserProfile {
            name: Some("Alice".into()),
            timezone: None,
            location: None,
        };
        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            Some(&identity),
            Some(&user),
            None,
            None,
            None,
            None,
        );
        assert!(prompt.contains("Your name is Momo 🦜."));
        assert!(prompt.contains("You are a parrot."));
        assert!(prompt.contains("Your vibe: cheerful and curious."));
        assert!(prompt.contains("The user's name is Alice."));
        // Default soul should be injected when soul is None.
        assert!(prompt.contains("## Soul"));
        assert!(prompt.contains("Be genuinely helpful"));
    }

    #[test]
    fn test_custom_soul_injected() {
        let tools = ToolRegistry::new();
        let identity = AgentIdentity {
            name: Some("Rex".into()),
            ..Default::default()
        };
        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            Some(&identity),
            None,
            Some("You are a loyal companion who loves fetch."),
            None,
            None,
            None,
        );
        assert!(prompt.contains("## Soul"));
        assert!(prompt.contains("loyal companion who loves fetch"));
        assert!(!prompt.contains("Be genuinely helpful"));
    }

    #[test]
    fn test_no_identity_no_extra_lines() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(!prompt.contains("Your name is"));
        assert!(!prompt.contains("The user's name is"));
        assert!(!prompt.contains("## Soul"));
    }

    #[test]
    fn test_workspace_files_injected_when_provided() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            Some("Follow workspace agent instructions."),
            Some("Prefer read-only tools first."),
            None,
        );
        assert!(prompt.contains("## Workspace Files"));
        assert!(prompt.contains("### AGENTS.md (workspace)"));
        assert!(prompt.contains("Follow workspace agent instructions."));
        assert!(prompt.contains("### TOOLS.md (workspace)"));
        assert!(prompt.contains("Prefer read-only tools first."));
    }

    #[test]
    fn test_runtime_context_injected_when_provided() {
        let tools = ToolRegistry::new();
        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                host: Some("moltis-devbox".into()),
                os: Some("macos".into()),
                arch: Some("aarch64".into()),
                shell: Some("zsh".into()),
                provider: Some("openai".into()),
                model: Some("gpt-5".into()),
                session_id: Some("main".into()),
                channel: None,
                channel_account_id: None,
                channel_account_handle: None,
                channel_chat_id: None,
                sudo_non_interactive: Some(true),
                sudo_status: Some("passwordless".into()),
                timezone: Some("Europe/Paris".into()),
                accept_language: Some("en-US,fr;q=0.9".into()),
                remote_ip: Some("203.0.113.42".into()),
                location: None,
            },
            sandbox: Some(PromptSandboxRuntimeContext {
                exec_sandboxed: true,
                mode: Some("all".into()),
                backend: Some("docker".into()),
                scope: Some("session".into()),
                image: Some("moltis-sandbox:abc123".into()),
                data_mount: Some("ro".into()),
                no_network: Some(true),
                session_override: Some(true),
            }),
        };

        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            Some(&runtime),
        );

        assert!(prompt.contains("## Runtime"));
        assert!(prompt.contains("Host: host=moltis-devbox"));
        assert!(prompt.contains("provider=openai"));
        assert!(prompt.contains("model=gpt-5"));
        assert!(prompt.contains("sudo_non_interactive=true"));
        assert!(prompt.contains("sudo_status=passwordless"));
        assert!(prompt.contains("timezone=Europe/Paris"));
        assert!(prompt.contains("accept_language=en-US,fr;q=0.9"));
        assert!(prompt.contains("remote_ip=203.0.113.42"));
        assert!(prompt.contains("Sandbox(exec): enabled=true"));
        assert!(prompt.contains("backend=docker"));
        assert!(prompt.contains("network=disabled"));
        assert!(prompt.contains("Execution routing:"));
    }

    #[test]
    fn test_runtime_context_includes_location_when_set() {
        let tools = ToolRegistry::new();
        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                host: Some("devbox".into()),
                location: Some("48.8566,2.3522".into()),
                ..Default::default()
            },
            sandbox: None,
        };

        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            Some(&runtime),
        );

        assert!(prompt.contains("location=48.8566,2.3522"));
    }

    #[test]
    fn test_runtime_context_omits_location_when_none() {
        let tools = ToolRegistry::new();
        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                host: Some("devbox".into()),
                location: None,
                ..Default::default()
            },
            sandbox: None,
        };

        let prompt = build_system_prompt_with_session_runtime(
            &tools,
            true,
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            Some(&runtime),
        );

        assert!(!prompt.contains("location="));
    }

    #[test]
    fn test_minimal_prompt_runtime_does_not_add_exec_routing_block() {
        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                host: Some("moltis-devbox".into()),
                ..Default::default()
            },
            sandbox: Some(PromptSandboxRuntimeContext {
                exec_sandboxed: false,
                ..Default::default()
            }),
        };

        let prompt =
            build_system_prompt_minimal_runtime(None, None, None, None, None, None, Some(&runtime));

        assert!(prompt.contains("## Runtime"));
        assert!(prompt.contains("Host: host=moltis-devbox"));
        assert!(prompt.contains("Sandbox(exec): enabled=false"));
        assert!(!prompt.contains("Execution routing:"));
    }

    #[test]
    fn test_silent_replies_section_in_tool_prompt() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools, true, None);
        assert!(prompt.contains("## Silent Replies"));
        assert!(prompt.contains("empty response"));
        assert!(!prompt.contains("__SILENT__"));
    }

    #[test]
    fn test_silent_replies_not_in_minimal_prompt() {
        let prompt = build_system_prompt_minimal_runtime(None, None, None, None, None, None, None);
        assert!(!prompt.contains("## Silent Replies"));
    }

    #[test]
    fn build_openai_responses_developer_prompts_includes_expected_sections_and_redacts_sensitive_runtime()
     {
        let mut tools = ToolRegistry::new();

        struct DummyTool {
            name: &'static str,
            description: &'static str,
        }

        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for DummyTool {
            fn name(&self) -> &str {
                self.name
            }

            fn description(&self) -> &str {
                self.description
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }

            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }

        tools.register(Box::new(DummyTool {
            name: "memory_search",
            description: "Search long-term memory",
        }));
        tools.register(Box::new(DummyTool {
            name: "exec",
            description: "Run a shell command",
        }));

        let skills = vec![SkillMetadata {
            name: "tmux".into(),
            description: "Interact with terminal apps".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec![],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: std::path::PathBuf::from("/skills/tmux"),
            source: None,
        }];

        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                host: Some("DESKTOP".into()),
                os: Some("linux".into()),
                arch: Some("x86_64".into()),
                shell: Some("bash".into()),
                provider: Some("openai-responses".into()),
                model: Some("openai-responses::gpt-5.2".into()),
                session_id: Some("telegram:lovely:8454363355".into()),
                channel: Some("telegram".into()),
                channel_account_id: Some("lovely".into()),
                channel_account_handle: Some("@lovely_apple_bot".into()),
                channel_chat_id: Some("8454363355".into()),
                sudo_non_interactive: Some(false),
                sudo_status: Some("requires_password".into()),
                timezone: Some("Asia/Shanghai".into()),
                accept_language: Some("zh-CN".into()),
                remote_ip: Some("203.0.113.42".into()),
                location: Some("48.8566,2.3522".into()),
            },
            sandbox: Some(PromptSandboxRuntimeContext {
                exec_sandboxed: true,
                mode: Some("all".into()),
                backend: Some("none".into()),
                scope: Some("chat".into()),
                image: Some("ubuntu:25.10".into()),
                data_mount: Some("ro".into()),
                no_network: Some(false),
                session_override: Some(true),
            }),
        };

        let prompts = build_openai_responses_developer_prompts(
            &tools,
            true, // native_tools
            None,
            &skills,
            "default",
            Some("# IDENTITY.md\n\nYou are a robot.\n"),
            Some("# SOUL.md\n\nBe helpful.\n"),
            Some("# AGENTS.md\n\nSome agents.\n"),
            Some("# TOOLS.md\n\nSome tools.\n"),
            Some(&runtime),
        );

        assert!(prompts.system.contains("# 系统（System）"));
        assert!(prompts.system.contains("执行路由："));
        assert!(prompts.system.contains("## 指南"));
        assert!(prompts.system.contains("## 静默回复"));

        assert!(prompts.persona.contains("# 人格（Persona: default）"));
        assert!(prompts.persona.contains("## 1. 身份"));
        assert!(prompts.persona.contains("# IDENTITY.md"));
        assert!(prompts.persona.contains("# SOUL.md"));
        assert!(prompts.persona.contains("## 2. 灵魂"));
        assert!(prompts.persona.contains("## 3. 主操作者"));
        assert!(prompts.persona.contains("/moltis/data/USER.md"));
        assert!(prompts.persona.contains("## 4. 人物清单"));
        assert!(prompts.persona.contains("/moltis/data/PEOPLE.md"));
        assert!(prompts.persona.contains("## 5. 对工作区规则的个人偏好"));
        assert!(prompts.persona.contains("# AGENTS.md"));
        assert!(prompts.persona.contains("## 6. 对工具说明的个人偏好"));
        assert!(prompts.persona.contains("# TOOLS.md"));

        assert!(prompts.runtime_snapshot.contains("# 运行环境（Runtime）"));
        assert!(prompts.runtime_snapshot.contains("## 1. 运行环境"));
        assert!(
            prompts
                .runtime_snapshot
                .contains("provider: openai-responses")
        );
        assert!(
            prompts
                .runtime_snapshot
                .contains("model: openai-responses::gpt-5.2")
        );
        assert!(
            prompts
                .runtime_snapshot
                .contains("channel_account_handle: @lovely_apple_bot")
        );
        assert!(!prompts.runtime_snapshot.contains("remote_ip"));
        assert!(!prompts.runtime_snapshot.contains("location"));
        assert!(prompts.runtime_snapshot.contains("## 5. 项目级可用工具"));
        assert!(prompts.runtime_snapshot.contains("## 6. 长期记忆"));
        assert!(prompts.runtime_snapshot.contains("- `exec`"));
        assert!(prompts.runtime_snapshot.contains("- `memory_search`"));
        assert!(prompts.runtime_snapshot.contains("tmux"));
    }

    #[test]
    fn openai_responses_identity_md_injection_strips_yaml_frontmatter() {
        let tools = ToolRegistry::new();
        let identity_md = "---\nname: Rex\n---\n\n# IDENTITY.md\n\nHello.\n";
        let prompts = build_openai_responses_developer_prompts(
            &tools,
            true,
            None,
            &[],
            "default",
            Some(identity_md),
            Some("# SOUL.md\n\nBe helpful.\n"),
            None,
            None,
            None,
        );
        assert!(prompts.persona.contains("# IDENTITY.md"));
        assert!(prompts.persona.contains("Hello."));
        assert!(!prompts.persona.contains("name: Rex"));
    }

    #[test]
    fn canonical_v1_renders_type4_templates_and_escape_sequences() {
        let tools = ToolRegistry::new();
        let runtime = PromptRuntimeContext {
            host: PromptHostRuntimeContext {
                os: Some("linux".into()),
                ..Default::default()
            },
            sandbox: Some(PromptSandboxRuntimeContext {
                exec_sandboxed: true,
                ..Default::default()
            }),
        };

        let canonical = build_canonical_system_prompt_v1(
            &tools,
            true,
            false,
            Some("Project context here."),
            &[],
            "default",
            Some("# IDENTITY.md\n\n{{project_context_md}}Literal: {{{{foo}}}}\n"),
            Some("# SOUL.md\n\nOS={{host_os}}\n"),
            Some("# AGENTS.md\n\n（空）\n"),
            Some("# TOOLS.md\n\n{{voice_reply_suffix_md}}\n"),
            PromptReplyMedium::Voice,
            Some(&runtime),
            "main",
        )
        .expect("canonical prompt should build");

        assert!(canonical.system_prompt.contains("Project context here."));
        assert!(canonical.system_prompt.contains("Literal: {{foo}}"));
        assert!(canonical.system_prompt.contains("OS=Linux 系统"));
        assert!(canonical.system_prompt.contains("## 语音回复模式"));
    }

    #[test]
    fn canonical_v1_non_native_tools_missing_required_vars_fails_fast() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "tool_x"
            }
            fn description(&self) -> &str {
                "x"
            }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }
            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let err = build_canonical_system_prompt_v1(
            &tools,
            false, // supports_tools=false => non-native tool calling
            false,
            None,
            &[],
            "default",
            Some("identity"),
            Some("soul"),
            Some("agents"),
            Some("tools"),
            PromptReplyMedium::Text,
            None,
            "main",
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("PROMPT_TEMPLATE_MISSING_REQUIRED_VAR"));
    }

    #[test]
    fn canonical_v1_non_native_tools_catalog_canonicalizes_json_key_order() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "sort_test"
            }
            fn description(&self) -> &str {
                "sort"
            }
            fn parameters_schema(&self) -> serde_json::Value {
                // Intentionally put b_key before a_key to test canonicalization.
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "b_key": {"type": "string"},
                        "a_key": {"type": "string"}
                    }
                })
            }
            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let canonical = build_canonical_system_prompt_v1(
            &tools,
            false, // non-native
            false,
            None,
            &[],
            "default",
            Some("identity"),
            Some("soul"),
            Some("agents"),
            Some(
                "{{non_native_tools_catalog_md}}\n{{non_native_tools_calling_guide_md}}\n",
            ),
            PromptReplyMedium::Text,
            None,
            "main",
        )
        .expect("canonical prompt should build");

        let a_idx = canonical
            .system_prompt
            .find("\"a_key\"")
            .expect("a_key should exist");
        let b_idx = canonical
            .system_prompt
            .find("\"b_key\"")
            .expect("b_key should exist");
        assert!(
            a_idx < b_idx,
            "expected canonicalized key order: a_key before b_key"
        );
    }

    #[test]
    fn canonical_v1_native_tools_index_includes_compact_tool_list() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "native_tool"
            }
            fn description(&self) -> &str {
                "A native tool"
            }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }
            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let canonical = build_canonical_system_prompt_v1(
            &tools,
            true,  // native tools
            false, // not stream_only
            None,
            &[],
            "default",
            Some("identity"),
            Some("soul"),
            Some("agents"),
            Some("{{native_tools_index_md}}\n"),
            PromptReplyMedium::Text,
            None,
            "main",
        )
        .expect("canonical prompt should build");

        assert!(canonical.system_prompt.contains("## 可用工具"));
        assert!(canonical.system_prompt.contains("`native_tool`"));
        assert!(canonical.system_prompt.contains("A native tool"));
        assert!(!canonical.system_prompt.contains("```tool_call"));
    }

    #[test]
    fn canonical_v1_warns_when_soft_vars_are_missing() {
        let tools = ToolRegistry::new();
        let canonical = build_canonical_system_prompt_v1(
            &tools,
            true,
            true,
            None,
            &[],
            "default",
            Some("identity"),
            Some("soul"),
            Some("agents"),
            Some("tools"),
            PromptReplyMedium::Text,
            None,
            "main",
        )
        .expect("canonical prompt should build");

        assert!(canonical
            .warnings
            .iter()
            .any(|w| w.contains("PROMPT_TEMPLATE_MISSING_SOFT_VAR")));
    }
}

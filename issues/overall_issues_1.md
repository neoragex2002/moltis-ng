# 总体问题审计（2026-02-16）

本文档汇总了当前工作区（working tree）改动及其相邻集成点中观察到的问题/风险，并给出建议的修复顺序。

## 范围与方法

- 范围：仅覆盖当前 diff 以及与之直接相关的调用点。
- 证据：本仓库内的 `文件:行号` 引用 + 少量上游 SDK/文档链接用于确认协议事实。
- 已做验证：对所有改动的 Rust 文件执行 `lsp_diagnostics`（无诊断问题）。
- 验证说明：初次审计阶段按请求刻意跳过 build/test。随后在修复落地后，已补充运行 targeted 测试：`cargo test -p moltis-gateway markdown_to_html`、`cargo test -p moltis-config`、`cargo test -p moltis-agents`，以及 `node --check crates/gateway/src/assets/js/providers.js`。

### 本次 diff 涉及的改动文件（便于定位）

- `CHANGELOG.md`
- `crates/agents/src/providers/anthropic.rs`
- `crates/agents/src/providers/github_copilot.rs`
- `crates/agents/src/providers/kimi_code.rs`
- `crates/agents/src/providers/mod.rs`
- `crates/agents/src/providers/openai.rs`
- `crates/agents/src/providers/openai_codex.rs`
- `crates/config/src/loader.rs`
- `crates/config/src/template.rs`
- `crates/config/src/validate.rs`
- `crates/gateway/src/assets/js/page-skills.js`
- `crates/gateway/src/assets/js/provider-key-help.js`
- `crates/gateway/src/assets/js/providers.js`
- `crates/gateway/src/auth.rs`
- `crates/gateway/src/lib.rs`
- `crates/gateway/src/provider_setup.rs`
- `crates/gateway/src/server.rs`
- `crates/gateway/src/services.rs`
- `crates/gateway/src/test_support.rs`
- `crates/gateway/src/voice_agent_tools.rs`
- `crates/agents/src/providers/openai_compat.rs`
- `crates/agents/src/providers/openai_responses.rs`
- `crates/agents/src/runner.rs`
- `issues/overall_issues_2.md`（总体问题文档已合并迁移至 `issues/`）

## 当前工作现状（修复后快照）

本节反映的是“当前 working tree”的状态（已按建议顺序把关键问题修复落地）。
下文的“详细问题”章节保留为初次审计（修复前）的历史记录，便于回溯。

### 逐条逐点修复状态（清晰清单）

本节将“当前 working tree 已经修正/完成的工作”按条目明确标注出来。

- [DONE]（P0）`/responses` SSE 解析达到规范完整性（支持 `data:` 无空格、多行 `data:` 拼接、按空行边界组装 frame）。
  - 证据：`crates/agents/src/providers/openai_responses.rs:267`。
- [DONE]（P0）Responses 流中的 tool-call 参数 delta 关联已修正（主键为 `(output_index, item_id)`）。
  - 证据：`crates/agents/src/providers/openai_responses.rs:310`。
- [DONE]（P0）已处理 `response.function_call_arguments.done`，避免丢失最终 arguments。
  - 证据：`crates/agents/src/providers/openai_responses.rs:421`。
- [DONE]（P0）`complete()` 遇到流式错误会 fail-fast（不会“出错但返回成功”）。
  - 证据：`crates/agents/src/providers/openai_responses.rs:668`。

- [DONE]（P1）`openai-responses` 模型发现不再在 `/models` 失败时“消失”（使用 OpenAI 的 fallback catalog 行为）。
  - 证据：`crates/agents/src/providers/mod.rs:1332`（调用 `openai::available_models`），fallback 行为见 `crates/agents/src/providers/openai.rs:315`。
- [DONE]（P1）Skills 搜索接口与 UI 契约已对齐：`/api/skills/search` 返回 `drifted`、`eligible`、`missing_bins`、`install_options` 等字段。
  - 证据：`crates/gateway/src/server.rs:4424`。
- [DONE]（P1）Gemini env var 文档与运行时一致（统一为 `GEMINI_API_KEY`）。
  - 证据：`crates/config/src/template.rs:127`。

- [DONE]（P2）Kimi base_url 已规范化，避免生成 `//chat/completions`。
  - 证据：`crates/agents/src/providers/kimi_code.rs:320`。
- [DONE]（P2）`openai-responses.base_url` 的语义约束（必须以 `/v1` 结尾）已通过 config validation/tests 做到一致。
  - 证据：`crates/config/src/validate.rs:868`（由 `cargo test -p moltis-config` 覆盖）。
- [DONE]（P2）Provider alias 冲突不再静默：重复模型注册会明确告警。
  - 证据：`crates/agents/src/providers/mod.rs:1291`。

- [DONE]（安全）Gateway Markdown 渲染：丢弃 raw HTML，并阻断不安全链接/图片 URL scheme。
  - 证据：`crates/gateway/src/services.rs:207`，测试在 `crates/gateway/src/services.rs:2327`。
- [DONE]（安全）URL scheme 过滤已去除 Unicode 空白，降低混淆 scheme 绕过风险。
  - 证据：`crates/gateway/src/services.rs:164`，回归测试在 `crates/gateway/src/services.rs:2351`。
- [DONE]（安全）providers 页面移除了不安全的动态 `innerHTML` 拼接，改为 DOM 节点 / `textContent`。
  - 证据：`crates/gateway/src/assets/js/providers.js:1048`。

- [DONE]（测试）web-ui 相关测试的全局 config/data dir override 已通过 `TestDirsGuard` 串行化。
  - 证据：`crates/gateway/src/test_support.rs:13`，使用点：`crates/gateway/src/server.rs:5556` 与 `crates/gateway/src/server.rs:6232`。

- [DONE]（JS 健壮性）skills UI 的错误解析不再使用空 catch 块。
  - 证据：`crates/gateway/src/assets/js/page-skills.js:133`。

- [TODO]（P3）重试/退避增强（429、jitter、尊重 `Retry-After`）。
  - 历史上下文：`crates/agents/src/runner.rs:45`。

- [TODO] 仓库整理：当前改动仍未提交且在 `main` 上（需要按原子提交拆分并可选创建 PR）。

### 已修复 / 已落地

- **OpenAI Responses `/responses` 流式正确性**已修复（`crates/agents/src/providers/openai_responses.rs`）：
  - SSE 解析达到规范完整性：同时接受 `data:` 与 `data: `，支持多行 `data:` 拼接，并按空行边界组装 frame（`crates/agents/src/providers/openai_responses.rs:267`）。
  - tool-call 参数 delta 主要使用 `(output_index, item_id)` 做关联（并提供安全回退）（`crates/agents/src/providers/openai_responses.rs:310`）。
  - 处理 `response.function_call_arguments.done`，避免丢掉最终 arguments（`crates/agents/src/providers/openai_responses.rs:421`）。
  - `complete()` 遇到流式 `error` 会 fail-fast（不会“出错但返回成功”）（`crates/agents/src/providers/openai_responses.rs:668`）。

- **Skills 搜索接口契约与 UI 预期已对齐**：
  - `/api/skills/search` 仍走 manifest fast-path，但对返回命中结果进行“按需富化”：补齐 `description`、`drifted`、`eligible`、`missing_bins`、`install_options`（只读取命中项的 `SKILL.md`，避免全量扫描）（`crates/gateway/src/server.rs:4424`）。

- **Gemini env var 文档与运行时一致**（统一为 `GEMINI_API_KEY`）（`crates/config/src/template.rs:127`）。

- **Kimi base_url 规范化**：避免出现 `//chat/completions`（`crates/agents/src/providers/kimi_code.rs:230`）。

- **`openai-responses.base_url` 语义校验（必须以 `/v1` 结尾）**已纳入 config 校验/测试覆盖（`crates/config/src/validate.rs`，并由 `cargo test -p moltis-config` 覆盖）。

- **Provider alias 冲突可观测性增强**：重复模型注册会打出明确 warn（不再“静默跳过”）（`crates/agents/src/providers/mod.rs:1291`）。

- **Gateway Markdown XSS 加固**：
  - `markdown_to_html` 丢弃 raw HTML，并阻断不安全链接/图片 URL scheme（`crates/gateway/src/services.rs:207`）。
  - URL 过滤从仅处理 ASCII 空白升级为处理所有 Unicode 空白，降低混淆 scheme 绕过风险（`crates/gateway/src/services.rs:164`）。

- **Gateway UI XSS sink 移除（providers 页面）**：
  - 去掉动态 HTML 字符串拼接写入，改为 DOM 节点 / `textContent`（保留 `innerHTML = ""` 仅用于清空容器）（`crates/gateway/src/assets/js/providers.js:1070`）。

- **全局目录 override 的测试稳定性**：
  - 新增 `TestDirsGuard` 并在 skills-search 测试中串行化全局 override（`crates/gateway/src/test_support.rs:13`、`crates/gateway/src/server.rs:6232`）。

### 验证证据（targeted）

- `cargo test -p moltis-gateway markdown_to_html`（包含 URL scheme 与 Unicode 空白绕过的回归测试）。
- `cargo test -p moltis-config`。
- `cargo test -p moltis-agents`。
- `node --check crates/gateway/src/assets/js/providers.js`。

### 仍需推进的工作

- **仓库整理**：当前改动还未提交（uncommitted）且在 `main` 上。下一步应按原子提交拆分（本仓库 commit message 以 Conventional 风格为主），并可选创建 PR。
- **可选后续（本轮未覆盖）**：retry/backoff（429 + jitter + `Retry-After`）、SSE parser 的 max-buffer 上限（防御性增强）。

## 执行摘要（优先级总览）

P0（正确性 / 用户可见故障风险）

- OpenAI Responses `/responses` streaming：SSE 解析不完整，并且 tool-call 参数 delta 的关联键使用了错误的 key（并行/交错 tool call 时很可能把参数拼错）。
- OpenAI Responses 的 `complete()` 可能“吞掉”流式错误并返回一个看似成功但内容部分/空的结果。

P1（可靠性 / 易用性）

- `openai-responses` provider：若 `/models` 发现失败且用户未配置 `models`，可能注册出 **0 个模型**。
- Skills `/api/skills/search` 快速路径返回的是“缩水 schema”，但 UI 假设其中存在 `drifted`/`eligible` 等语义字段；导致 badge/依赖提示静默退化。
- 配置模板中 Gemini 的 env var 文档写错（`GOOGLE_API_KEY` 与运行时使用的 `GEMINI_API_KEY` 不一致）。

P2（兼容性 / 一致性）

- Kimi：`base_url` 拼接若以 `/` 结尾，会产生 `//chat/completions`。
- `openai-responses` base_url “必须以 `/v1` 结尾”只在 UI 流程里强制校验，但 `moltis config check` 未覆盖语义校验。
- Provider alias 冲突可能导致跨 provider 的模型注册被静默跳过。

P3（质量 / 可运维性）

- 重试/退避策略很有限（缺少 429 模式；固定延迟；无 jitter）；rate-limit 的 reset 信息没有端到端地传递。
- 有一个新增/更新的测试在修改全局 config/data dir override 时没有使用全局 guard，可能在并行测试下出现 flaky。

## 详细问题

### 1）OpenAI Responses API（`/responses`）provider

相关文件：

- 主实现：`crates/agents/src/providers/openai_responses.rs`
- 共享的 OpenAI-compatible SSE 工具：`crates/agents/src/providers/openai_compat.rs`（对照参考）

#### 1.1 SSE 解析未达到规范完整性（frame 边界、`data:` 变体、多行）

本仓库中观察到的行为：

- 解析器会忽略任何不以 `data: `（需要字面空格）开头的行：`crates/agents/src/providers/openai_responses.rs:239`。
- 解析器按“逐行（`find('\n')`）”消费流，而不是按 SSE 的空行边界拼装一个 message/frame：`crates/agents/src/providers/openai_responses.rs:231`。
- 同一个 SSE event 的多行 `data:` 不会被拼接；JSON 解析会失败或事件被丢弃：`crates/agents/src/providers/openai_responses.rs:256`。

上游证据（协议事实）：

- OpenAI Node SDK 的 SSE decoder 支持 `data:`（有/无前导空格），并把多行 `data:` 用 `\n` 拼接：
  `https://raw.githubusercontent.com/openai/openai-node/fe49a7b4826956bf80445f379eee6039a478d410/src/core/streaming.ts`。

影响：

- 任意符合 SSE 规范的网关/SDK 发送方式（例如 `data:` 不带空格、多行 `data:`、按 event block 组织）都可能导致事件被丢弃，或过早触发“stream ended unexpectedly”。

#### 1.2 tool-call 参数 delta 的关联键用错了

本仓库中观察到的行为：

- tool-call start 事件用 `response.output_item.added` 里的 `call_id` 建索引：`crates/agents/src/providers/openai_responses.rs:270`–`crates/agents/src/providers/openai_responses.rs:279`。
- tool-call arguments delta 用顶层 `call_id` 关联；若没有则回退到猜测的 index（`current_tool_index - 1`，否则 `0`）：`crates/agents/src/providers/openai_responses.rs:282`–`crates/agents/src/providers/openai_responses.rs:301`。
- `response.function_call_arguments.done` 事件被显式忽略：`crates/agents/src/providers/openai_responses.rs:304`。

上游证据（Responses streaming schema）：

- `response.function_call_arguments.delta` 携带的是 `item_id` + `output_index`，而不是 `call_id`：
  `https://raw.githubusercontent.com/openai/openai-python/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_function_call_arguments_delta_event.py`。
- `call_id` 存在于 `function_call` 的 output item 上（不在 delta event 顶层）：
  `https://raw.githubusercontent.com/openai/openai-python/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_function_tool_call.py`。

影响：

- 当并行 tool call 或事件交错时，参数 delta 可能被拼接到错误的 tool call 上，产生无效参数并导致“错误的工具调用”。
- 忽略 `...arguments.done` 会丢掉“最终/合并后的 arguments”（尤其是某些网关主要在 done 事件发送完整 arguments 的情况下）。

#### 1.3 `complete()` 可能吞掉 stream 错误并返回成功

本仓库中观察到的行为：

- collector 会忽略 `StreamEvent::Error`：`crates/agents/src/providers/openai_responses.rs:171`–`crates/agents/src/providers/openai_responses.rs:182`。
- 在 `complete()` 里，stream loop 把 `StreamEvent::Error(_)` 当作终止条件（“done”），然后仍然返回 `Ok(collector.into_completion())`：`crates/agents/src/providers/openai_responses.rs:518`–`crates/agents/src/providers/openai_responses.rs:525`。

影响：

- provider 侧失败可能表现为“看似成功但缺少文本/工具调用不完整”的响应。

#### 1.4 兼容性风险：强制发送 `OpenAI-Beta` header

本仓库中观察到的行为：

- 每次调用 `/responses` 都设置 `OpenAI-Beta: responses=experimental`：`crates/agents/src/providers/openai_responses.rs:155`–`crates/agents/src/providers/openai_responses.rs:160`。

上游证据（当前官方 SDK 不要求）：

- OpenAI Node/Python SDK 默认不会注入该 beta header；使用的仍是标准 auth + content-type。

影响：

- 大多数网关会忽略未知 header，但严格代理或稳定端点可能拒绝/错误处理该 header。

#### 1.5 `ToolCallComplete` 事件顺序不稳定（小问题）

- 在 `[DONE]` 与 `response.completed` 场景下，完成事件直接迭代 `HashMap::keys()` 未排序：
  `crates/agents/src/providers/openai_responses.rs:244`–`crates/agents/src/providers/openai_responses.rs:246`，以及 `crates/agents/src/providers/openai_responses.rs:317`–`crates/agents/src/providers/openai_responses.rs:319`。

影响：

- 如果下游消费者假定稳定顺序，可能出现 run-to-run 的差异。

#### 1.6 注意：测试用例固化了同样（很可能不正确）的 schema 假设

- 单元测试 fixture 让 `response.function_call_arguments.delta` 携带顶层 `call_id`：`crates/agents/src/providers/openai_responses.rs:655`–`crates/agents/src/providers/openai_responses.rs:656`。

影响：

- 未来修正实现时，如果不同时更新测试，会被测试“锁死”在不兼容的事件形状上。

---

### 2）OpenAI `/chat/completions` → `/responses` fallback 行为

文件：`crates/agents/src/providers/openai.rs`

#### 2.1 fallback 触发条件依赖字符串匹配，较脆弱

- “仅支持 responses”判断依赖 body 子串：`crates/agents/src/providers/openai.rs:223`–`crates/agents/src/providers/openai.rs:227`。

影响：

- 网关若返回不同的错误消息结构/措辞，即使 `/responses` 可用，也可能不触发 fallback。

#### 2.2 fallback 被限制在 OpenAI Platform host

- `base_url_is_openai_platform()` 只允许 `api.openai.com`：`crates/agents/src/providers/openai.rs:250`–`crates/agents/src/providers/openai.rs:256`。

影响：

- 对那些“OpenAI-compatible 且支持 `/responses`”的第三方网关，用户可能仍然看到不支持模型的错误，而不是成功 fallback。

---

### 3）Provider registry / discovery / config 与 UI 一致性

相关文件：

- Provider registry：`crates/agents/src/providers/mod.rs`
- 配置模板：`crates/config/src/template.rs`
- 配置校验器：`crates/config/src/validate.rs`
- Provider setup RPC：`crates/gateway/src/provider_setup.rs`
- Provider UI：`crates/gateway/src/assets/js/providers.js`

#### 3.1 `openai-responses` 发现模型失败时可能注册 0 个模型

- `openai` 使用 `openai::available_models()`（包含 fallback catalog）：`crates/agents/src/providers/mod.rs:1273`–`crates/agents/src/providers/mod.rs:1279`。
- `openai-responses` 使用 `openai::live_models()`，失败返回 `Vec::new()`（无 fallback）：`crates/agents/src/providers/mod.rs:1318`–`crates/agents/src/providers/mod.rs:1333`。

影响：

- 如果 discovery 失败且配置里没有 `models = [...]`，`openai-responses` 会在模型列表中“消失”。

#### 3.2 Provider alias 冲突导致模型被静默跳过

- namespace key 为 `{provider_label}::{model_id}`：`crates/agents/src/providers/mod.rs:68`–`crates/agents/src/providers/mod.rs:73`。
- `openai` label 为 `alias.unwrap_or("openai")`：`crates/agents/src/providers/mod.rs:1269`–`crates/agents/src/providers/mod.rs:1271`。
- `openai-responses` label 为 `alias.unwrap_or("openai-responses")`：`crates/agents/src/providers/mod.rs:1315`–`crates/agents/src/providers/mod.rs:1317`。
- 重复的 `(provider_label, model_id)` 会被 `has_provider_model` 静默跳过：
  `crates/agents/src/providers/mod.rs:1283`–`crates/agents/src/providers/mod.rs:1285`，以及 `crates/agents/src/providers/mod.rs:1339`–`crates/agents/src/providers/mod.rs:1341`。

影响：

- 若两个不同 provider 配置了相同 alias 且模型 ID 重叠，其中一方会悄悄不注册。

#### 3.3 Gemini env var 不一致：模板 vs 运行时

- 模板写 Gemini `api_key` 可以来自 `GOOGLE_API_KEY`：`crates/config/src/template.rs:124`–`crates/config/src/template.rs:129`。
- 运行时 provider metadata 使用 `GEMINI_API_KEY`：`crates/gateway/src/provider_setup.rs:507`–`crates/gateway/src/provider_setup.rs:515`。
- registry 的 genai defaults 也使用 `GEMINI_API_KEY`：`crates/agents/src/providers/mod.rs:900`–`crates/agents/src/providers/mod.rs:906`。

影响：

- 用户照模板设置 env var 会失败，Gemini 无法自动配置。

#### 3.4 `openai-responses` base_url “必须以 `/v1` 结尾”只在 UI 强制，`config check` 未覆盖

- UI 有明确提示：`crates/gateway/src/assets/js/providers.js:41`–`crates/gateway/src/assets/js/providers.js:52`，以及 `openai-responses` endpoint 的 label/hint：`crates/gateway/src/assets/js/providers.js:186`–`crates/gateway/src/assets/js/providers.js:201`。
- 模板也文档化了约束：`crates/config/src/template.rs:114`–`crates/config/src/template.rs:123`。
- Provider setup 在 UI save/validate 流程强制该约束（helper）：`crates/gateway/src/provider_setup.rs:773`–`crates/gateway/src/provider_setup.rs:783`。
- 配置校验（`moltis config check`）目前只知道 `base_url` 是一个 leaf key，但没有 provider-specific 的语义校验：`crates/config/src/validate.rs:111`–`crates/config/src/validate.rs:119`。

影响：

- 错误配置可能通过 `config check`，但在运行期才报错。

#### 3.5 Kimi base_url 拼接可能产生双斜杠

- 请求通过 `format!("{}/chat/completions", self.base_url)` 构建（未 trim）：
  `crates/agents/src/providers/kimi_code.rs:226`–`crates/agents/src/providers/kimi_code.rs:230`，以及 `crates/agents/src/providers/kimi_code.rs:315`–`crates/agents/src/providers/kimi_code.rs:319`。

影响：

- `base_url` 以 `/` 结尾时会生成 `...//chat/completions`，部分服务端会拒绝。

#### 3.6 Kimi endpoint 在 provider modal 中不可编辑

- endpoint 输入仅对 `OPENAI_COMPATIBLE_PROVIDERS` 显示，其中不含 `kimi-code`：`crates/gateway/src/assets/js/providers.js:41`–`crates/gateway/src/assets/js/providers.js:52`。
- 但后端 known providers 列表中包含 `kimi-code` 且给了默认 base URL：`crates/gateway/src/provider_setup.rs:624`–`crates/gateway/src/provider_setup.rs:632`。

影响：

- 后端支持“可配置 base_url”的概念，但 UI modal 无法配置。

---

### 4）Skills 搜索接口（`/api/skills/search`）契约 vs UI 预期

相关文件：

- Server handler 与 fast-path search：`crates/gateway/src/server.rs`
- UI consumer 与 badges：`crates/gateway/src/assets/js/page-skills.js`
- 全量（较慢）富信息列表：`crates/gateway/src/services.rs`（`repos_list_full`）

#### 4.1 fast-path search 有意返回缩水字段

- handler 为性能使用 manifest fast-path：`crates/gateway/src/server.rs:4410`–`crates/gateway/src/server.rs:4413`。
- 返回对象无条件包含 `eligible: true` 与 `missing_bins: []`：`crates/gateway/src/server.rs:4464`–`crates/gateway/src/server.rs:4473`。

影响：

- UI autocomplete 的 badge 会静默退化。

#### 4.2 UI 读取了 search 结果未提供的字段

- UI 调用 `/api/skills/search`：`crates/gateway/src/assets/js/page-skills.js:133`–`crates/gateway/src/assets/js/page-skills.js:151`。
- autocomplete 显示 `drifted` 与 `eligible === false` 的 badge：`crates/gateway/src/assets/js/page-skills.js:639`–`crates/gateway/src/assets/js/page-skills.js:640`。
- 缺失依赖 section 依赖 `eligible === false` 与 `missing_bins`：`crates/gateway/src/assets/js/page-skills.js:311`–`crates/gateway/src/assets/js/page-skills.js:316`。

影响：

- 对 search 结果而言，“source changed / blocked”badge 永远不会出现。

#### 4.3 对照：全量 repo 列表包含 drift + eligibility 语义

- `repos_list_full` 包含 `drifted`，并对 SKILL.md repo 计算 eligibility：`crates/gateway/src/services.rs:818`–`crates/gateway/src/services.rs:873`。

权衡：

- fast-path 对延迟友好，也避免扫描大仓库。
- 但目前会丢掉 UI 本来想展示的语义信息。

---

### 5）重试/退避与错误 UX（跨模块）

相关文件：

- agent loop runner 重试模式：`crates/agents/src/runner.rs`
- gateway 错误解析：`crates/gateway/src/chat_error.rs`

#### 5.1 重试策略窄且固定延迟

- 重试 pattern 仅覆盖 5xx 类字符串，没有显式 `http 429` pattern：`crates/agents/src/runner.rs:45`–`crates/agents/src/runner.rs:56`。
- 重试延迟固定为 2 秒：`crates/agents/src/runner.rs:65`–`crates/agents/src/runner.rs:67`。

影响：

- rate-limit 和部分瞬态网络错误可能不重试（或重试过于激进/无 jitter）。

#### 5.2 reset/retry 元数据未端到端保留

- `chat_error` 会从 JSON body 提取 `resets_at`，但不会从 `Retry-After` 等 header 推导：`crates/gateway/src/chat_error.rs:186`–`crates/gateway/src/chat_error.rs:203`。

影响：

- 当 provider 只通过 header 传递重试信息时，UI 很难可靠提示“X 秒后重试”。

---

### 6）全局 config/data dir override 的测试稳定性

相关文件：

- 全局 guard：`crates/gateway/src/test_support.rs`
- skills search 测试：`crates/gateway/src/server.rs`

#### 6.1 guard 已存在，但有一个测试绕过了它

- guard 串行化全局 override，并在 drop 时清理：`crates/gateway/src/test_support.rs:5`–`crates/gateway/src/test_support.rs:43`。
- skills search 测试直接设置全局 data dir（无 guard）：`crates/gateway/src/server.rs:6213`，并在 `crates/gateway/src/server.rs:6252` 清理。

影响：

- 并行测试执行时可能在全局目录状态上产生竞态，导致 flaky。

## 建议的修复顺序（含理由）

### Phase 1（P0）：先把 `/responses` 做到正确且“出错就失败”

目标：避免错误工具调用，并避免“出错但返回成功”的静默失败。

1）修复 `complete()` 的错误传播

- 当 stream yield `StreamEvent::Error(_)` 时，让 `complete()` 返回 error。
- 加一个回归测试：构造 `error` event，断言 `complete()` 返回 `Err`。

证据：`crates/agents/src/providers/openai_responses.rs:518`–`crates/agents/src/providers/openai_responses.rs:525`。

2）实现规范完整的 `/responses` SSE 解析

- 同时接受 `data:` 与 `data: `。
- 按空行边界拼装 frame；多行 `data:` 用 `\n` 拼接。

证据：当前逐行解析在 `crates/agents/src/providers/openai_responses.rs:231`，严格前缀在 `crates/agents/src/providers/openai_responses.rs:239`。

3）修复 tool-call delta 关联方式：使用 `item_id` + `output_index`

- 用 `(output_index, item_id)` 跟踪 output items。
- 收到带 `function_call` item 的 `response.output_item.added` 时，把该 output item 的 `(output_index, item.id 或 call_id)` 绑定到内部 index 映射。
- 对 `response.function_call_arguments.delta/done`，用 `(output_index, item_id)` 做映射并累积 arguments。

上游 schema 证据：`response_function_call_arguments_delta_event.py`（不含 `call_id`，使用 `item_id/output_index`）。

4）更新测试以符合正确的 event schema

- 替换那些把 `call_id` 挂在 delta event 顶层的 fixture。

证据：`crates/agents/src/providers/openai_responses.rs:655`–`crates/agents/src/providers/openai_responses.rs:656`。

### Phase 2（P1）：让 provider discovery 与 skills search 变得可靠

目标：避免“provider 消失”，并恢复/对齐 UI 语义。

5）为 `openai-responses` discovery 增加 fallback catalog

- 对齐 `openai::available_models()` 的行为（带 fallback catalog），或给 responses-only provider 提供一个最小静态 catalog。

证据：`openai` 在 `crates/agents/src/providers/mod.rs:1273`–`crates/agents/src/providers/mod.rs:1279` 使用带 fallback 的 `available_models`；`openai-responses` 在 `crates/agents/src/providers/mod.rs:1318`–`crates/agents/src/providers/mod.rs:1333` 没有。

6）为 `/api/skills/search` 确定并执行一个明确的契约

方案 A（推荐）：只对 top-N 命中返回轻量 `drifted` 与真实 `eligible/missing_bins`。

- 使用 manifest/repo 级 drift 信息 + 仅对返回结果做 requirements 检查。

方案 B：保留缩水契约，但更新 UI，不在 autocomplete 里期待 `drifted/eligible/missing_bins`。

证据：server 把 `eligible/missing_bins` 写死在 `crates/gateway/src/server.rs:4464`–`crates/gateway/src/server.rs:4473`；UI 在 `crates/gateway/src/assets/js/page-skills.js:639`–`crates/gateway/src/assets/js/page-skills.js:640` 使用这些字段。

7）修正 Gemini env var 文档不一致

- 更新 `crates/config/src/template.rs:127`，改为引用 `GEMINI_API_KEY`；或者同时兼容两个 env var 名称（但需要一致的策略）。

证据：`crates/config/src/template.rs:127` vs `crates/gateway/src/provider_setup.rs:511`。

### Phase 3（P2）：兼容性与一致性增强

8）在写入或构建请求时规范化 Kimi base_url

- 在拼接 `/chat/completions` 前 trim 末尾 `/`。

证据：`crates/agents/src/providers/kimi_code.rs:228` 与 `crates/agents/src/providers/kimi_code.rs:317`。

9）为 `openai-responses.base_url` 增加“必须以 `/v1` 结尾”的语义校验

- UI 已经强制；把同样规则加入 config validation，使 `moltis config check` 能捕捉。

证据：UI enforcement helper 在 `crates/gateway/src/provider_setup.rs:773`–`crates/gateway/src/provider_setup.rs:783`，schema 知道 `base_url` key 在 `crates/config/src/validate.rs:111`–`crates/config/src/validate.rs:119`。

10）定义并落地 alias 冲突策略

- 可选策略：跨 provider 拒绝重复 alias；或按 provider 类型做 namespace（例如 `openai-responses:<alias>`）；或把 provider config name 纳入 namespace key。

证据：namespace 构造在 `crates/agents/src/providers/mod.rs:68`–`crates/agents/src/providers/mod.rs:73`，且 openai/openai-responses 都支持 alias。

### Phase 4（P3）：可运维性与测试稳定性

11）改进 retry/backoff

- 考虑对 429 做带 backoff/jitter 的重试，并在可用时尊重 `Retry-After`。

证据：重试 pattern 在 `crates/agents/src/runner.rs:45`–`crates/agents/src/runner.rs:56`，固定延迟在 `crates/agents/src/runner.rs:65`–`crates/agents/src/runner.rs:67`。

12）skills search 测试中使用 `TestDirsGuard`

- 用 guard 包裹 `skills_search_uses_manifest_and_returns_matches`，串行化全局目录 override。

证据：guard 在 `crates/gateway/src/test_support.rs:5`–`crates/gateway/src/test_support.rs:43`；直接 override 在 `crates/gateway/src/server.rs:6213`。

## 每个 Phase 的“完成”标准（建议）

- Phase 1 完成：`/responses` 解析器的单测覆盖 `data:`（无空格）、多行 `data:` frame、以及 item_id/output_index 关联；`complete()` 在 stream error 时返回 `Err`。
- Phase 2 完成：启用 `openai-responses` 时总能注册至少一个模型（即使 discovery 失败）；skills search/autocomplete 语义一致（要么 server 返回字段，要么 UI 不再期待）；Gemini env var 文档与运行时一致。
- Phase 3 完成：Kimi base_url 被规范化；config validation 能捕捉 `openai-responses` 非 `/v1` endpoint；alias 冲突行为被定义并强制执行。
- Phase 4 完成：retry 策略对 rate limit 与瞬态错误有 backoff；涉及全局目录 override 的测试无竞态、无 flaky。

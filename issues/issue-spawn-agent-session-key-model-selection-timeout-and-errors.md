# Issue: `spawn_agent` 子代理异常（session_key 语义 / model 选择与 unknown model / 长程超时与 “stream ended unexpectedly” 错误可观测性）

## 实施现状（Status）【增量更新主入口】
- Status: TODO（待核实与修复）
- Priority: P1（子代理不可控：可能无故失败/超时/错误提示不足）
- Components: tools(spawn_agent) / agents runner / gateway chat timeout / openai-responses provider
- Affected providers/models: 特别影响 `openai-responses::*`（complete 走 streaming collect）

**已实现**
- 存在 `spawn_agent` 工具：`crates/tools/src/spawn_agent.rs`
- 子代理生命周期事件可广播到 Web UI：`crates/gateway/src/server.rs:2365`

**已覆盖测试**
- `spawn_agent` 工具自身有单测（不覆盖长程/超时/错误语义收敛）：`crates/tools/src/spawn_agent.rs`

---

## 背景（Background）
`spawn_agent` 用于让主代理将复杂任务委派给子代理运行一段独立的 agent loop，然后把结果作为 tool output 返回给主代理。

用户反馈（日志证据）显示 `spawn_agent` 在 Telegram session 中出现：
- 子代理长程运行后异常结束，报 `OpenAI Responses API stream ended unexpectedly`
- 主 run 最终触发 gateway 600s 超时（`agent run timed out`）
- `spawn_agent` 有时报 `unknown model:`（model 字段为空）
- 对“子代理是否复用父 session_key”产生疑问：这会影响 prompt cache bucket、debug 可观测性与隔离性
此外，经代码核查，`spawn_agent` 还存在多处“语义不一致/上下文不继承/可观测性不足”的潜在问题：即使修复了空 model，这个工具在实际使用中仍可能出现“用错模型、sandbox 行为漂移、绕过 hooks、长程拖死父 run、错误无法归因”等表现。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **父 session_key**：触发本次主 run 的 session（例如 `telegram:...`）。
- **子代理 session_key**：子代理对上游 LLM 请求使用的 `LlmRequestContext.session_key`（用于 prompt cache key、hooks 归属等）。
- **model（tool 参数）**：`spawn_agent` 的可选参数，用于指定子代理使用的 model id（注册表中的 key）。
- **default_provider（spawn_agent 内部）**：若 tool 参数未指定 model，当前实现使用启动时 registry 的 `first_with_tools()`，并非“父 run 的当前 provider/model”。
- **tool_context（注入参数）**：gateway 在一次 run 中注入到所有 tool 调用参数的运行态字段（例如 `_session_key/_sandbox/_accept_language/_conn_id`）。
- **filtered tool registry（本轮有效工具集）**：gateway 在本次 run 中基于 persona/skills/MCP/禁用开关等计算出的“实际可用工具集”（与 server 启动时注册的静态工具集不同）。
- **hooks**：gateway 为主 run 提供的 BeforeLLMCall/AfterLLMCall/BeforeToolCall/AfterToolCall 等钩子（可用于审计/阻断/观测）；子代理是否继承 hooks 会影响安全与一致性。
- **gateway agent_timeout_secs**：gateway 对整次 `chat.send` 的超时（默认 600s）；超时会 broadcast error，但不会把“更底层的 LLM/tool 失败语义”统一给 Telegram（另见相关 error 收敛 issue）。
- **OpenAI Responses stream ended unexpectedly**：在 openai-responses provider 中，`stream=true` 的响应流在未看到 `Done` 事件就 EOF（通常意味着连接被上游/中间层提前关闭、或协议不完整）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 明确并修正 `spawn_agent` 的 session_key 语义：
  - 是否应继承父 session_key（共享 prompt cache bucket）？
  - 还是应使用派生 key（例如 `parent + ":spawn:<uuid>"`）以隔离缓存与观测？
  - UI/debug 中必须能看出子代理的 effective session_key（至少对开发者可见）。
- [ ] 修复 `unknown model:` 这类“空字符串 model”导致的误报：
  - `model=""` 应视为未提供（fallback 到默认 provider），或提供更明确错误（含可用 model 列表/提示）。
- [ ] 子代理的 provider/model 默认行为必须与文案一致：
  - 当前描述：“未指定则使用父 session 的 current model”
  - 当前实现：使用启动时 `first_with_tools()`（可能与父 run 不一致）
  - 两者必须收敛（要么改实现、要么改描述并显式说明）。
- [ ] 子代理必须继承父 run 的关键 tool_context（至少 `_sandbox/_accept_language/_conn_id/_session_key`），避免同一 session 内“主代理与子代理的工具行为口径不一致”。
- [ ] 子代理必须使用“本轮有效工具集”（filtered registry）或明确声明差异；不得因为使用静态 registry 而出现“父能用、子不能用”或“父禁用、子仍可用”的不一致。
- [ ] hooks 语义必须明确并收敛：
  - 子代理是否继承父 run 的 hooks（默认建议继承，至少继承审计/阻断类 hooks）。
  - 不允许子代理绕过 hooks 导致安全/策略层失效。
- [ ] 子代理长程运行必须具备可控的超时/取消语义：
  - 至少应避免“子代理无限阻塞导致父 run 只能靠 gateway 600s 超时兜底”。
  - 超时后用户/日志必须可解释（是子代理超时还是上游断流）。
- [ ] 错误可观测性增强：
  - `OpenAI Responses API stream ended unexpectedly` 必须补充可定位信息（request id / 发生阶段 / 是否超时/取消），至少在 debug/日志可见。
  - gateway 超时（600s）与 tool/LLM error 的关系要可解释，避免“只看到一个泛化错误”。
  - 子代理生命周期事件必须可归因到具体父 run（至少包含 `run_id`/`session_key`/`tool_call_id` 之一）。

### 非功能目标（Non-functional）
- 不得破坏现有 tool 调用链（保持 `spawn_agent` 仍是普通 tool call）。
- 安全隐私：错误提示不得泄露 Authorization、原始请求 body 等敏感字段；可记录 OpenAI request id（若有）与 run_id。
- 可回滚：优先以增量方式改动（先修空 model、改日志；再做 session_key 语义调整）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
日志样例（用户提供）：
1) `tool execution failed tool=spawn_agent ... error=OpenAI Responses API stream ended unexpectedly`
2) `moltis_gateway::chat: agent run timed out ... timeout_secs=600`
3) `tool execution failed tool=spawn_agent ... error=unknown model:`

### 影响（Impact）
- Telegram 渠道：用户可能长时间无结果或只收到不明确错误；难以判断是超时、上游断流还是 model 配置问题。
- 可靠性：子代理作为“复杂任务卸载”机制，一旦不稳定会反过来放大整体 run 的失败概率。
- 排障成本：缺少 “子代理使用的模型/会话 key/错误阶段” 等关键上下文。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 子代理复用父 session_key（对 LLM context）：
  - `crates/tools/src/spawn_agent.rs:160`：把 tool params 中的 `"_session_key"` 写入子代理 `tool_context`
  - `crates/agents/src/runner.rs:791`：从 `tool_context._session_key` 构建 `LlmRequestContext.session_key`
  - 结论：**子代理对 provider 的 prompt cache bucket 等 context，当前默认继承父 session_key**
- 子代理没有继承父 run 的完整 tool_context（存在行为漂移风险）：
  - 父 run 注入字段：`crates/gateway/src/chat.rs:4301`（`_session_key/_sandbox`）、`crates/gateway/src/chat.rs:4305`（`_accept_language`）、`crates/gateway/src/chat.rs:4308`（`_conn_id`）
  - 子代理只透传 `_session_key`：`crates/tools/src/spawn_agent.rs:156`–`crates/tools/src/spawn_agent.rs:162`
  - `_sandbox` 影响 browser 工具行为（缺失则默认 false）：`crates/tools/src/browser.rs:157`–`crates/tools/src/browser.rs:163`
  - 结论：**子代理调用工具时可能与父会话 sandbox/locale/conn 口径不一致**
- 子代理“默认模型”不是父会话模型：
  - `crates/gateway/src/server.rs:2387`：`SpawnAgentTool::new(... default_provider = registry.first_with_tools())`
  - 结论：**未指定 tool 参数 `model` 时，子代理使用的是启动时选择的 default_provider，而非父 run 当前模型**
- `unknown model:` 空字符串问题：
  - `crates/tools/src/spawn_agent.rs:104`：`let model_id = params["model"].as_str();`（`""` 也会是 Some("")）
  - `crates/tools/src/spawn_agent.rs:124`：`anyhow!("unknown model: {id}")` → 当 `id=""` 时日志呈现为 `unknown model:`
- 文案与实现不一致（会误导用户/LLM）：
  - tool schema 描述：“If not specified, uses the parent's current model.”：`crates/tools/src/spawn_agent.rs:88`–`crates/tools/src/spawn_agent.rs:91`
  - 实现却使用 default_provider：`crates/tools/src/spawn_agent.rs:113`–`crates/tools/src/spawn_agent.rs:120`
- `OpenAI Responses API stream ended unexpectedly` 来源：
  - `crates/agents/src/providers/openai_responses.rs:888`：`complete_using_body()` 读取 SSE/byte stream EOF 后直接 `bail!`
  - 且 `complete_with_context()` **强制 `stream=true` 并“收集流”**：`crates/agents/src/providers/openai_responses.rs:1048`
- openai-responses client 未显式设置 timeout（长程/断流更难归因）：
  - `reqwest::Client::new()`：`crates/agents/src/providers/openai_responses.rs:536`
- 子代理不继承 hooks / 不 forward 事件 / 不带 history（影响一致性与可观测性）：
  - 子代理 loop：`crates/tools/src/spawn_agent.rs:164`–`crates/tools/src/spawn_agent.rs:175`
    - `on_event=None`（子代理内部事件不会进入主 run 的 runner event 流）
    - `history=None`（子代理无对话上下文）
    - `hook_registry=None`（子代理绕过 hooks）
- 子代理使用的是静态工具集快照（与本轮 filtered registry 可能不一致）：
  - 子代理工具集：`crates/tools/src/spawn_agent.rs:138`–`crates/tools/src/spawn_agent.rs:140`（从 SpawnAgentTool 持有的 registry clone）
  - 主 run 工具集：`crates/gateway/src/chat.rs:4314`（传入 `filtered_registry`）
- 子代理生命周期事件缺少与父 run 的关联字段（UI 难归因）：
  - gateway 注册 spawn_tool 时广播子代理 start/end，但 payload 不含 `runId/sessionKey/tool_call_id`：`crates/gateway/src/server.rs:2360`–`crates/gateway/src/server.rs:2385`
- gateway run 超时（600s）：
  - `crates/gateway/src/chat.rs:2596`：`tokio::time::timeout(Duration::from_secs(agent_timeout_secs), agent_fut)`

## 根因分析（Root Cause）
可能是多因素叠加（需进一步定位）：
- A) **模型选择语义不一致**：子代理默认 provider 与父 session 不同，可能触发不兼容（工具支持/限制/网络）或错误归因困难。
- B) **tool 参数空字符串**：LLM 或上游调用者可能把 “未指定 model” 表达为 `""`，触发 `unknown model:` 误报。
- C) **openai-responses complete 采用 streaming collect**：任何中间层断流、超时、或协议不完整都会变成 “stream ended unexpectedly”，错误缺乏上下文。
- D) **超时口径分散**：gateway 的 600s 超时与子代理内部错误没有统一的错误语义出口（见 error taxonomy issue），Telegram 侧更难得到可理解回执。
- E) **tool_context 继承不完整**：子代理只继承 `_session_key`，导致 sandbox/locale/conn 等运行态信息丢失，出现“同一 session 内工具行为漂移”。
- F) **hooks 绕过**：子代理不继承 hooks，可能绕过阻断/审计/策略层，导致安全与一致性风险。
- G) **工具集不一致**：子代理工具集来自静态快照而非本轮 filtered registry，导致“父能用、子不能用/父禁用、子仍可用”的差异。
- H) **长程缺少 tool-level 超时/取消**：`spawn_agent` 作为 tool call 没有独立超时，容易拖死父 run，最终只能靠 gateway 600s 超时兜底。
- I) **可观测性缺口**：子代理生命周期事件缺少与父 run 的关联字段；openai-responses 的 EOF 错误缺少 request/stage 等上下文。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `spawn_agent` 的默认 model 行为与文案一致（“父 session 当前 model”或“全局默认 model”必须二选一并写清）。
  - `model=""` 不得产生 `unknown model:`（应当当作未提供或返回更明确错误）。
  - `stream ended unexpectedly` 需要可定位上下文（至少 provider/model/run_id/stage/是否超时或取消）。
  - 子代理必须继承父 run 的关键 tool_context（至少 `_sandbox/_accept_language/_conn_id/_session_key`），或明确声明“子代理不继承哪些字段”并在 UI/debug 可见。
  - 子代理不得绕过 hooks（至少不得绕过审计/阻断类 hooks）；若出于隔离目的不继承，也必须可配置且默认安全。
  - 子代理的“有效工具集”必须与父 run 一致（或在结果/事件中明确标注差异）。
- 应当：
  - 子代理 session_key 策略可配置：继承父 session_key（共享 cache bucket） vs 派生子 key（隔离）。
  - 为 `spawn_agent` 增加 tool-level timeout（例如默认 120s，可配置），并把“超时”作为结构化错误返回给父代理与渠道用户。

## 方案（Proposed Solution）
### Phase 0（止血，低风险）
- 修复空字符串 `model`：把 `model=""` 视为 None（与 schema“可选”语义一致）。
- 透传关键 tool_context：`_sandbox/_accept_language/_conn_id/_session_key`（至少保证与父 run 的工具行为一致）。
- 改善 `unknown model` 错误信息：
  - 输出 `unknown model: <id>`（对空值明确显示 `<empty>`）
  - 并在 tool result 中附带 “可用 models 提示/如何使用 /model 切换”。
- 增补可观测性（不改变行为）：
  - 子代理 start/end 事件补齐 `runId/sessionKey/tool_call_id`（至少其一），便于 UI 归因。
  - 对 `stream ended unexpectedly` 增加上下文（content-type/stream 模式/已接收事件数/可能的 request id header）。

### Phase 1（语义收敛：默认 provider/model）
两种方向二选一：
1) **按父 session 当前模型**（更符合直觉与文案）
   - `spawn_agent` 需要拿到“本次父 run 实际使用的 provider/model id”，并传入子代理。
   - 可能需要在 tool_context 注入 `_model_id` 或直接扩展 SpawnAgentTool 的 execute params。
2) **保持全局默认 provider**（实现简单，但需修改 tool description + 文档）
   - 明确告诉用户：不指定 `model` 时总用全局默认（registry first_with_tools），与父 session 不一定一致。

建议：优先选 1（按父 session），否则当前行为容易出现“父模型正常但子代理模型不可用/工具不支持”的隐性失败。

### Phase 2（session_key 语义）
提供配置项或默认策略：
- 默认：子代理使用派生 session_key（例如 `${parent}:spawn:${tool_call_id}`），避免 prompt cache bucket 与 debug 归属混淆。
- 可选：显式配置 `inherit_session_key=true` 以共享 prompt cache bucket（如果用户希望复用缓存）。

### Phase 3（工具集/ hooks / timeout 收敛）
- 工具集收敛：
  - 让子代理使用“本轮 filtered registry”或提供一个可明确解释的策略（例如只允许一组子代理安全工具）。
- hooks 收敛：
  - 子代理默认继承父 run hooks（至少审计/阻断类），避免绕过策略层。
- timeout 收敛：
  - 为 `spawn_agent` 引入 tool-level timeout，并将超时作为结构化错误（可被 parse_chat_error 映射为用户可读提示）。

### Phase 4（错误收敛与超时解释）
- 在 `openai-responses` 的 `stream ended unexpectedly` 错误中补充：
  - `content-type`、是否启用 stream、已接收字节数/事件数、以及（若存在）`x-request-id` 之类 header。
- 将 gateway 的 “agent run timed out” 与子代理失败建立关联：
  - 在 run 的 broadcast error payload 中标注 “timed_out=true / timeout_secs / last_tool=spawn_agent（若适用）”
- 长远：纳入 `issues/issue-error-handling-taxonomy-single-egress.md` 的统一失败出口。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] `spawn_agent` 在 `model=""` 时不再报 `unknown model:`，并能正常 fallback 执行。
- [ ] 未指定 model 时，子代理默认模型行为清晰且与实现一致（doc+debug 可见）。
- [ ] 子代理 session_key 策略明确且可观察（inherit vs derived）。
- [ ] 子代理继承关键 tool_context（至少 `_sandbox/_accept_language/_conn_id/_session_key`），并有回归测试覆盖。
- [ ] 子代理不会绕过 hooks（或可配置，且默认安全），并能在 debug/log 中明确说明是否继承。
- [ ] 子代理有效工具集与父 run 一致（或差异可见），避免“父能用、子不能用/父禁用、子仍可用”。
- [ ] `spawn_agent` 有独立可控的 timeout/取消语义，避免长程无限拖死父 run。
- [ ] `stream ended unexpectedly` 错误具备足够上下文，不再是无法定位的单行字符串。
- [ ] 对长程任务，超时行为可解释（用户能知道是 600s 超时而非“莫名 ended”）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `spawn_agent`：`model=""` 视为 None 的单测
- [ ] `spawn_agent`：unknown model 错误信息包含可诊断信息（至少不为空）
- [ ] `spawn_agent`：tool_context 透传（断言 `_sandbox/_accept_language/_conn_id` 在子代理工具调用参数中可见；至少覆盖 `_sandbox`）
- [ ] `spawn_agent`：hooks 继承（若实现为默认继承，新增单测/模拟 hook 断言生效）

### Integration（可选）
- [ ] gateway + spawn_agent：在父 session 选择非默认模型时，子代理默认使用父模型（若选择 Phase 1 方向 1）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-error-handling-taxonomy-single-egress.md`
  - `issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
  - `issues/issue-named-personas-and-per-session-agent-profiles.md`（更广义的 per-session persona/agent 配置能力）
  - `issues/done/issue-chat-debug-panel-llm-session-sandbox-compaction.md`（run/debug 可观测性口径先例）

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（model/session_key/timeout 语义明确）
- [ ] 已补齐自动化测试（覆盖空 model/错误信息）
- [ ] 错误可观测性增强到位（不泄露敏感数据）
- [ ] 文档/描述已更新（避免 “实现与文案不一致”）

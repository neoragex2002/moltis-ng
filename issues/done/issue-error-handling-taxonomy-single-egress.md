# Issue: 收敛 LLM 失败处理（Error Taxonomy + Single Error Egress）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（V1：single egress + taxonomy + 三面板字段贯通）
- Priority: P1（架构收敛；P0 止血见 Telegram 单子）
- Components: `crates/agents` / `crates/gateway` / channels delivery / Web UI debug
- Cross-ref:
  - 渠道止血（Telegram 无回执 + drain targets）：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
  - 术语收敛基线（本单只依赖“最小子集”）：`issues/issue-terminology-and-concept-convergence.md`

**已实现**
- Run failure 统一出口（gateway）：`crates/gateway/src/chat.rs:3939`（`handle_run_failed_event`）
- 失败规范化对象 + taxonomy（gateway）：`crates/gateway/src/run_failure.rs:1`
- 失败出口幂等（单进程 TTL 去重）：`crates/gateway/src/state.rs:580`（`dedupe_check_and_insert`）
- Web UI error card 增补 stage/kind/action/debug：`crates/gateway/src/assets/js/chat-ui.js:106`
- 结构化日志（单行 `event="run.failure"`，含脱敏 raw）：`crates/gateway/src/chat.rs:4059`

**已覆盖测试**
- `normalize_failure` 单测：`crates/gateway/src/run_failure.rs:234`
- gateway 回归（stream error→必回 + drain）：`crates/gateway/src/chat.rs:6999`（`run_streaming_error_sends_channel_error_and_drains_state`）
- gateway 回归（重复失败 egress 仍 drain，且最多发送一次）：`crates/gateway/src/chat.rs:7127`（`run_failed_event_duplicate_still_drains_reply_targets_without_sending`）

**已知差异/后续优化（非阻塞）**
- 跨重启/多实例严格幂等（DB/outbox）仍为 V2+ 增强（见 Q1 补充说明）。
- `request_id` best-effort（当前 V1 仍为空；需要在 provider SDK 层补齐可得性）。
- “完整 Run Supervisor（事件归约器 + action 列表执行器）”未在 V1 上线：当前 V1 以单一入口函数 `handle_run_failed_event` 作为集中出口，后续可在不改字段契约的前提下演进为 supervisor。
- 日志字段口径：`event="run.failure"` 已使用 snake_case 的 `stage/kind/action`，并包含脱敏的 `raw_class/raw_message` 用于快速定位根因。

---

## 人工验收（Manual Verification）
> 本节覆盖：Web UI / 日志 / Telegram 三面板一致性；以及“必回 + 去重 + drain”。

### UI 配置项
- 无新增 UI 配置项（保持现有 provider/channel 配置即可）。
- 说明：Telegram 回执默认 **不**带 debug hint（run_id 等），本单也未新增开关；如需对 Telegram 暴露 debug hint，见 Q2（后续增强）。

### 交互验收步骤（建议按顺序）
1) Web UI：打开 `/chats`，进入任意会话（建议选择一个已绑定 Telegram 的 session）。
2) 触发一个可控失败（二选一）：
   - Auth：临时把 provider 的 API key 改错（Providers 页），发送一条消息后再改回正确 key。
   - 模型不可用：选择一个无权限/不存在的模型（或临时把 model 配错）触发 `unsupported_model`/404/403。
3) Telegram：同一条触发消息必须收到 **恰好 1 条**错误回执（以 `⚠️ ` 开头），且不出现重复错误回执。
4) Web UI：必须出现错误卡片，并能看到：
   - `Provider: <name>`（已有）
   - `stage=... · kind=... · action=...`（新增）
   - debug 摘要行（`message.debug`，新增）
5) 日志：grep `event="run.failure"`，确认单行包含：`run_id/session_key/provider/model/stage/kind/action/dedup_key` 以及 `egress_reply_targets_before/egress_drained_count`。
6) 串线回归：失败后立刻再发送一条正常消息，Telegram 不得“回复到上一次失败消息的 reply_to”（避免 cross-wiring）。

---

## 背景（Background）
当前系统对“LLM 服务出错”的处理分散在多个层面：provider 调用层 / stream 事件层 / agent-run 编排层 / channel 投递层 / UI 展示层，各自做了一部分处理，从而容易出现：
- 同一种错误在不同渠道表现不一致（Web UI 能看到 error，但 Telegram 不回/回得不一样）。
- 错误文案口径漂移：有的偏底层（HTTP/SDK/network），有的偏产品态（“模型不可用/不支持”）。
- “该不该回、回给谁、回几次、是否 drain reply targets”等行为分散，产生边缘问题（漏回、重复回、串线）。

本单目标是把错误处理收敛成两件事：
1) **统一错误语义**（taxonomy + normalized error object）
2) **统一错误出口**（single egress：一次性负责状态/回执/日志/drain/去重）

> 备注：渠道级止血（Telegram 必回 + drain）可作为 Phase 1 先做；本单覆盖更广的 Phase 2/3 收敛。

## 概念与口径（Glossary & Semantics）
- **Normalized Error（规范化错误对象）**：把任意层抛出的 raw error（string/anyhow/HTTP/JSON）映射为少数“语义类别”，并携带 `message.user` / `message.debug` 两套文案与可执行建议。
  - Why：保证跨渠道/跨层输出一致，减少“拼字符串”导致的漂移。
  - Not：不是为了保留底层堆栈/敏感信息；`message.debug` 也必须脱敏。

- **Error Taxonomy（错误语义分类）**：固定少数类别，覆盖绝大多数问题即可，避免无限枚举。

- **Single Error Egress（唯一错误出口）**：run 生命周期中定义一个集中处理点（函数/模块/事件），任何失败最终都走这里。
  - Why：避免“多处触发、多次发送、多处 drain”导致重复回执/漏 drain/串线。
  - Not：不要求一次性把所有层的实现都大迁移；允许分阶段逐步接入。

- **authoritative vs estimate**：
  - authoritative：来自 provider 回包 usage 或明确的协议字段（权威值）。
  - estimate：启发式/推导（用于风险预估，不能当真值）。

### 本单最小术语冻结（Micro-freeze，必须执行）
> 为了避免“等全仓术语收敛”导致 P1 无法推进，本单只冻结与错误处理/可观测性直接相关的最小字段集。
> 这套字段会同时出现在：日志 / Web UI debug payload / run state。

- **run_id**：一次 `chat.send` 的 run 唯一标识（opaque uuid）。用于去重与串联所有事件。
- **session_key**：跨域桥（如 `telegram:<account_id>:<chat_id>[:<thread_id>]`）。用于定位影响范围与复现入口。
- **provider** / **model**：本次失败时的上游提供方与模型 id（as-sent 口径）。
- **stage**：失败发生在链路哪一段（枚举冻结；见下文）。
- **kind**：失败语义类别（taxonomy；见下文）。
- **action**：建议动作枚举（收敛替代 `suggested_action`；见下文）。
- **retryable**：是否建议用户重试（布尔，面向用户/产品口径）。
- **message.user**：Telegram 可直接发送的短句（脱敏、可行动）。
- **message.debug**：Web UI/debug/log 的脱敏摘要（稳定口径，不拼底层堆栈）。
- **request_id?**：上游请求标识（若能拿到，如 OpenAI request-id/response id），用于官方工单定位；不得包含 auth。
- **dedup_key**：失败出口去重键（至少包含 run_id；可加 stage/kind）。
- **egress.sent**：是否已对渠道发送失败回执（用于证明“最多一次”）。
- **egress.reply_targets_before** / **egress.drained_count**：失败出口执行前 pending reply targets 数量与 drain 后数量（用于证明“不会串线”）。
- **details**：分段细节（一个对象；字段由 `stage` 决定；Stage 规范里声明“必须字段”都在 `details` 里，不再散落顶层）。

### Stage 规范（冻结）
> `stage` 不是随便写字符串；必须来自下列枚举之一，并携带该段的最小必要字段。
> **约定：分段必填字段一律放在 `details` 里**（避免顶层字段膨胀、避免各处随意加字段）。

- `gateway_timeout`：gateway 的 `agent_timeout_secs` 超时触发
  - `details` 必须：`timeout_secs`、`elapsed_ms`
- `provider_request`：请求发起/收到非 2xx/解析到明确的上游错误响应
  - `details` 必须：`http_status`；可选：`retry_after_secs`、`provider_error_code`
- `provider_stream`：streaming/SSE 中断、协议不完整（例如 “stream ended unexpectedly”）
  - `details` 必须：`elapsed_ms`；可选：`last_event_type`
- `runner`：agent runner 状态机/编排层错误（非工具/非渠道）
  - `details` 必须：`iteration`；可选：`tool_calls_seen`
- `tool_exec`：工具执行失败（含 tool timeout / sandbox ensure_ready / browser 等）
  - `details` 必须：`tool_name`、`tool_call_id`；可选：`tool_timeout_secs`、`sandbox_id`
- `channel_delivery`：渠道投递失败（Telegram sendMessage/editMessage 失败等）
  - `details` 必须：`channel`、`chat_id`；可选：`reply_to_message_id`、`api_error_code`

#### Stage → details 字段矩阵（V1，必须可验收）
| stage | `details` 必须字段 | `details` 可选字段（建议） |
|---|---|---|
| `gateway_timeout` | `timeout_secs`, `elapsed_ms` |  |
| `provider_request` | `http_status` | `retry_after_secs`, `provider_error_code` |
| `provider_stream` | `elapsed_ms` | `last_event_type` |
| `runner` | `iteration` | `tool_calls_seen` |
| `tool_exec` | `tool_name`, `tool_call_id` | `tool_timeout_secs`, `sandbox_id` |
| `channel_delivery` | `channel`, `chat_id` | `reply_to_message_id`, `api_error_code` |

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 任意一次 run 的失败，在所有渠道呈现一致的“语义类别 + 用户文案口径”（至少 Web UI + Telegram）。
- [x] 明确且唯一：**失败时是否回执、回给谁、回几次**（去重：同一 `run_id` 只回一次；单进程保证）。
- [x] 失败时必须 drain 任意 pending reply targets（避免串线）。
- [x] Web UI debug 能看到：错误类别（kind）、是否可重试（retryable）、建议动作（action）、以及脱敏后的 debug message。

### 非功能目标（Non-functional）
- 安全隐私：`message.user` / `message.debug` / logs **不得**包含 token、完整 HTTP body、路径、堆栈等敏感信息。
- 兼容性：保留现有 UI `state="error"` 的 broadcast 契约（可扩展字段，但不破坏现有渲染）。
- 可观测性（硬要求，必须可验收）：
  - 日志必须携带：`run_id`、`session_key`、`provider`、`model`、`stage`、`kind`、`retryable`、`action`、`dedup_key`、`egress.sent`、`egress.drained_count`。
  - Web UI debug payload 至少展示：`run_id/session_key/provider/model/stage/kind/retryable/action/request_id?/details/egress(drained_count)`。
  - 对任意一次失败，仅靠日志 + Web UI debug，不读源码，也能回答（见验收问答）。

### Non-goals（明确不做）
- 不在本单强制引入“自动重试/自动 fallback”的策略变更（可作为后续独立议题）。
- 不要求一次性重写所有 provider 的错误处理；允许渐进迁移。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
- 相同失败在 Web UI 与 Telegram 的可见性/文案不一致。
- 同一次 run 可能在多个层各自处理 error，导致重复回执或漏回。
- reply targets 不被 drain 时，后续回复存在串线风险。

### 现状核查与证据（As-is / Evidence）
1) **agents 层已有 failover 分类，但语义仅服务于链路切换**（与用户文案/渠道回执不统一）
- `crates/agents/src/provider_chain.rs:24`（`ProviderErrorKind`）
- `crates/agents/src/provider_chain.rs:73`（`classify_error()` 基于字符串 pattern）

2) **gateway run_with_tools：成功会 deliver_channel_replies，但失败只 broadcast error，不负责渠道回执与 drain**
- 成功分支：`crates/gateway/src/chat.rs:4505`（`deliver_channel_replies(...)`）
- 失败分支：`crates/gateway/src/chat.rs:4516`（只 `set_run_error` + `parse_chat_error` + broadcast，然后返回 `None`）

3) **gateway stream-only：stream error 同样只 broadcast error，不负责渠道回执与 drain**
- `crates/gateway/src/chat.rs:5213`（`StreamEvent::Error`：只 broadcast error，然后 `return None`）

4) **channel_events 仅处理 “chat.send 立即失败” 的 error 回执**（看不到 run 内部失败）
- `crates/gateway/src/channel_events.rs:281`（`chat.send(...)` 返回 Err 才回 `⚠️ {e}`）

5) **gateway 侧的 parse_chat_error 是 UI 友好 JSON，但不构成统一 taxonomy，也没有统一 egress**
- `crates/gateway/src/chat_error.rs:10`（`parse_chat_error()`：输出 `{type,title,detail,...}`）

### 根因（Root Cause）
- 错误语义在不同层各自推断（provider_chain / parse_chat_error / raw strings），没有统一“语义对象”。
- 错误发布点分散：broadcast、写 run_error、渠道回执、drain targets 等缺少统一出口与去重。

## 期望行为（Desired Behavior / Spec）
### A) Error Taxonomy（最小集合）
将错误规范化为少数语义类别（建议 v1）：
- Auth
- RateLimit
- ModelNotFoundOrAccessDenied
- QuotaOrBilling
- InvalidRequest（含 context window/参数错误等）
- Network（timeout/DNS/连接中断）
- ProviderUnavailable（5xx/上游故障）
- Cancelled
- Internal

### A1) ErrorKind 字典（V1，必须收敛）
> 本节冻结每个 `kind` 的“人话含义 + 默认动作”。实现允许基于 `stage/details` 覆盖默认值，但必须能解释。

| kind | 人话含义（对用户） | 典型触发（对系统） | 默认 retryable | 默认 action | Telegram `message.user` 模板（示例） |
|---|---|---|---:|---|---|
| Auth | 认证/权限问题（key 不对/没权限） | 401/403、签名失败、token 无效 | false | `check_api_key` | `⚠️ 认证失败：请检查 API Key/权限后再试。` |
| RateLimit | 触发限流（太频繁） | 429、明确 rate-limit code | true | `wait_and_retry` | `⚠️ 请求过于频繁：请稍后再试。` |
| ModelNotFoundOrAccessDenied | 模型不存在/无权限 | 404/403 + “model not found/permission denied” | false | `switch_model` | `⚠️ 当前模型不可用：请更换模型后再试。` |
| QuotaOrBilling | 额度/账单问题 | “insufficient_quota”、billing required | false | `contact_admin` | `⚠️ 额度/账单异常：请检查账户额度或联系管理员。` |
| InvalidRequest | 请求参数不合法/上下文过长 | 400、context window exceeded、invalid param | false | `fix_request` | `⚠️ 请求不合法：请检查参数/缩短上下文后再试。` |
| Network | 网络/连接中断（可重试） | timeout/DNS/reset/stream ended unexpectedly | true | `retry` | `⚠️ 上游连接异常：本次回答中断，可重试一次。` |
| ProviderUnavailable | 上游服务故障（可重试） | 5xx、gateway errors、服务不可用 | true | `wait_and_retry` | `⚠️ 上游服务暂不可用：请稍后重试。` |
| Cancelled | 被取消/超时终止 | gateway timeout、用户取消、上下游取消 | true | `retry` | `⚠️ 本次请求已终止（超时/取消）：你可以重试。` |
| Internal | 系统内部错误（需要排查） | bug/不变量破坏/未知异常 | false | `contact_admin` | `⚠️ 系统内部错误：请联系管理员或稍后再试。` |

### B) Normalized Error 对象（规范字段）
任何 run 的失败最终都应产生一个规范对象（字段名可调整，但语义需稳定）：
- `kind`: taxonomy enum
- `message.user`: 脱敏短文案（可直接发 Telegram）
- `message.debug`: 脱敏摘要（给 UI debug/日志）
- `retryable`: bool
- `action`: `"retry" | "wait_and_retry" | "check_api_key" | "switch_model" | "fix_request" | "contact_admin" | "cancelled"`
- `provider` / `model` / `run_id` / `session_key`
- `stage`: 见 Stage 规范（冻结枚举）
- `request_id?`：上游 request id / response id（如可获取）
- `dedup_key`：失败出口去重键
- `egress.sent`: bool（是否已发送渠道回执）
- `egress.reply_targets_before` / `egress.drained_count`
- `details`: 分段细节（对象；字段由 stage 决定）

### B0.1) Action 字典（V1，必须收敛）
| action | 人话含义 | 备注 |
|---|---|---|
| `retry` | 现在可以重试一次 | 适用于 `Network`、部分 `Cancelled` |
| `wait_and_retry` | 稍后再试 | 适用于 `RateLimit`、`ProviderUnavailable` |
| `check_api_key` | 检查/更新 Key 与权限 | 适用于 `Auth` |
| `switch_model` | 换模型/换 provider | 适用于 `ModelNotFoundOrAccessDenied` |
| `fix_request` | 改请求/缩短上下文 | 适用于 `InvalidRequest` |
| `contact_admin` | 联系管理员/查看账单与额度 | 适用于 `QuotaOrBilling`、`Internal` |
| `cancelled` | 本次请求已取消 | 仅在确实是“取消”而非“失败”时使用 |

### B1) Canonical 失败记录结构（V1，单一真源）
> 为避免“既是错误对象又混入副作用证据”导致概念膨胀，本单冻结：**错误语义（NormalizedError）** 与 **出口证据（egress）** 分离，
> 但三面板展示时仍必须能同时看到两者。

- `NormalizedError`（纯语义，supervisor 归约产物，作为三面板字段真源）
  - 只包含：`stage/kind/retryable/action/message/request_id?/details`
- `RunFailureRecord`（run state 中的失败记录，包含一次失败的完整可观测证据）
  - `run_id`, `session_key`, `provider?`, `model?`, `dedup_key`
  - `error`: `NormalizedError`
  - `raw`（debug-only，必须脱敏）：`{ class, message_redacted }`
  - `egress`: `{ sent, reply_targets_before, drained_count, channel?, chat_id?, reply_to_message_id?, last_error? }`
    - `last_error`（debug-only，可选）：`{ action, class, message_redacted, api_error_code? }`（用于解释“为何未能回执/为何未能 drain/为何未能 broadcast”等二次失败）

字段约定：
- `message.user`：只面向用户；短句；可行动；绝对脱敏。
- `message.debug`：面向 debug/log；稳定摘要；绝对脱敏；允许携带 `raw.class` 等“无敏感”的诊断信息。
- `details`：只放“该 stage 的最小必要字段”，其字段名由 Stage 规范冻结；不得在顶层随意加字段。

### B2) 字段字典（V1，一览表）
> 目的：给实现/测试/验收提供“字段清单”，避免各层各面板再自造字段。

| 字段路径 | 归属 | source/method | 出现在 | 说明 |
|---|---|---|---|---|
| `run_id` | Run | authoritative | Web UI / log / Telegram* | 仅 debug hint 时可出现在 Telegram |
| `session_key` | Run | authoritative | Web UI / log | Telegram 默认不暴露 |
| `provider`, `model` | Run | as-sent | Web UI / log | 以“实际发送给上游”的口径记录 |
| `error.stage` | Error | authoritative | Web UI / log / Telegram（隐式） | 由 FailureStage 冻结枚举 |
| `error.kind` | Error | derived | Web UI / log / Telegram（隐式） | 由 taxonomy 冻结枚举 |
| `error.retryable` | Error | derived | Web UI / log | Telegram 文案应体现“可否重试” |
| `error.action` | Error | derived | Web UI / log / Telegram（可显式） | 见 Action 字典 |
| `error.message.user` | Error | derived | Telegram | **唯一**用户可见主文案 |
| `error.message.debug` | Error | derived | Web UI / log | 脱敏摘要，稳定口径 |
| `error.request_id?` | Error | authoritative | Web UI / log | 若可得则必须填；否则为空 |
| `error.details` | Error | authoritative/derived | Web UI / log | 见 Stage→details 矩阵 |
| `dedup_key` | RunFailure | derived | Web UI / log | 至少包含 `run_id` |
| `raw.class`, `raw.message_redacted` | RunFailure | derived | Web UI / log | debug-only；不得含敏感信息 |
| `egress.sent` | Egress | authoritative | Web UI / log | “是否已回执过”证据 |
| `egress.reply_targets_before` | Egress | authoritative | Web UI / log | 回执/清理前队列长度 |
| `egress.drained_count` | Egress | authoritative | Web UI / log | drain 后清理数量 |
| `egress.channel?`, `egress.chat_id?`, `egress.reply_to_message_id?` | Egress | authoritative | Web UI / log | 仅当与渠道投递相关时填 |
| `egress.last_error?` | Egress | derived | Web UI / log | debug-only；仅当错误出口的某个 action 自己失败时记录 |
| `egress.last_error.action` | Egress | authoritative | Web UI / log | 哪个 action 失败（例如 `DeliverChannelErrorOnce`） |
| `egress.last_error.class` | Egress | derived | Web UI / log | 失败原因分类（脱敏） |
| `egress.last_error.message_redacted` | Egress | derived | Web UI / log | 失败原因摘要（脱敏） |
| `egress.last_error.api_error_code?` | Egress | authoritative | Web UI / log | 若有渠道 API 的错误码可记录（脱敏） |

### C) Single Error Egress（唯一出口 + 去重 + drain）
定义一个唯一失败出口（例如 `handle_run_failure(...)` 或 `RunEvent::Failed { normalized_error, ... }`），负责：
1) 更新 run 状态（供 Web UI / 查询）
2) WebSocket broadcast `state="error"`（保持兼容，可附加 `kind/retryable/action/stage`）
3) **渠道回执**（Telegram 等）：发送 `message.user`，并确保 **同 `run_id` 只发送一次**
4) **drain reply targets**（避免后续串线）
5) 记录日志（带 `run_id/session_key/provider/model/stage/kind`）

### D) 可观测性契约（必须冻结）
> 目的：失败时能“讲清楚发生了什么”，并能在 30 秒内定位到哪一段。

#### D0) 三个输出面板（Surfaces）必须口径一致
本单的“可观测性”必须同时覆盖三类输出面板，并保持字段语义一致：
1) **Web UI debug 面板**（开发者可读、结构化、可复制）
2) **命令行/服务端日志**（结构化单行事件，便于 grep/聚类）
3) **Telegram（或其它 channel）用户回执**（短、脱敏、可行动；且必须与 Web UI 的 kind/stage 口径一致）

原则：
- Web UI / 日志：展示结构化字段（用于定位）
- Telegram：展示 `message.user + action`（用于用户行动）；必要时可附加极短的 debug hint（受控开关，默认关闭）

#### D1) Web UI debug payload（最小字段）
Web UI debug 面板（或 run 详情）必须能看到以下字段（展示名可调整，字段语义不可漂移）：
- `run_id`, `session_key`
- `provider`, `model`
- `stage`, `kind`, `retryable`, `action`
- `request_id?`
- `channel?`, `chat_id?`, `reply_to_message_id?`（如果涉及渠道投递或 reply targets）
- `dedup_key`, `egress.sent`
- `egress.reply_targets_before`, `egress.drained_count`
- `egress.last_error?`（若存在，必须展示）

#### D2) 结构化日志（单行事件，便于 grep）
必须有一条结构化日志（建议 event name：`run.failure`），字段至少包含：
- `run_id`, `session_key`
- `provider`, `model`
- `stage`, `kind`, `retryable`, `action`
- `request_id?`
- `dedup_key`, `egress.sent`
- `egress.reply_targets_before`, `egress.drained_count`
- `egress.last_error?`（若存在，必须记录）

#### D2.1) Telegram 回执（用户可读，最小契约）
Telegram（以及其它 channel 的用户回执）必须满足：
- **必回一次**：同一 `run_id` 最多回执一次（去重）。
- **必脱敏**：不得包含 token、URL query、header、request body、堆栈、主机路径等。
- **必可行动**：至少包含建议动作（例如“重试/检查 key/换模型/稍后再试”）。
- **口径一致**：回执文案必须与 `kind/stage` 对齐（不能出现“UI 显示 RateLimit，但 Telegram 说模型不可用”）。

Telegram 回执建议格式（V1）：
- 主句：`⚠️ <message.user>`（1 行/2 行内，短）
- 末尾动作：`建议：<action 的自然语言化>`（可选）
- （可选 debug hint，默认关闭）：`（run=<run_id_short> stage=<stage> kind=<kind>）`

开关建议（Open Question 中已有）：是否提供 `channels.*.error_debug_hints=true` 用于在 Telegram 回执中附加 debug hint（默认 false）。

#### D3) 失败问答（验收用）
给定任意一次失败，仅凭 Web UI debug + 日志，必须能回答：
1) 失败发生在 `stage` 的哪一段？
2) `kind` 是什么？是否 `retryable`？建议动作为何？
3) 本次 run 的 `provider/model` 是哪个？
4) 对应的 `run_id/session_key` 是哪个（影响范围）？
5) 渠道是否回执过？回给谁（chat_id/reply_to）？回执次数是否被去重为 1？
6) reply targets 是否 drain？`egress.drained_count` 是多少？
7) 如果是上游问题，`request_id` 是什么（便于官方工单）？

## 方案（Proposed Solution）
> 你已确认：采用更“干净”的 **方案 B（Run Supervisor：事件归约 + 副作用集中）**。
> 本节按两条主线拆分：**流程主线（Run Supervisor）** 与 **可观测主线（统一三面板字段契约）**。

### 方案对比（Options）
#### 方案 A（较小改动，止血型）：`handle_run_failure(...)` 直接做副作用
- 思路：在 gateway 里加一个函数，现有失败分支统一调用；函数内做 normalize + 回执 + drain + broadcast + log。
- 优点：落地快。
- 风险：更容易变成“另一个大函数”，并随着后续重试/子代理/会议编排继续膨胀；且各层仍可能继续偷跑副作用。
- 结论：本单不选（可作为过渡/回滚路径记录）。

#### 方案 B（推荐，已选）：Run Supervisor（事件归约 + 副作用集中）
- 核心：各层只 **emit 失败事件**，不做回执/drain/broadcast；Supervisor 统一：
  1) 归约/去重
  2) normalize 成 `NormalizedError`
  3) 执行唯一副作用链（回执/drain/broadcast/log）
- 优点：结构干净，可扩展（重试/backoff、spawn_agent、会议编排都自然归于 supervisor）。
- 风险：需要梳理事件边界与数据携带（但可增量迁移，不必一次性重构）。

### 最终方案（Chosen Approach）：Run Supervisor
#### 两大主线（必须分开推进）
1) **流程主线（Process）**：把失败路径收敛成 “事件 → 归约 → 副作用链” 的唯一通路。
2) **可观测主线（Observability）**：冻结字段契约，并确保 Web UI / 日志 / Telegram 三面板一致输出。

---

## 流程主线（Process）：Run Supervisor 规范
### P0) 事件模型（Event Contracts）
> 所有失败必须被编码成事件；失败事件是唯一入口（single ingress for failures）。

新增（或收敛）事件：
- `RunEvent::Failed { ... }`（最小字段必须覆盖 Micro-freeze）
  - 必须：`run_id`, `session_key`, `stage`, `provider?`, `model?`
  - 必须：`raw.class`, `raw.message_redacted`
  - 可选：`request_id?`, `http_status?`, `retry_after_secs?`, `tool_name?`, `tool_call_id?`, `elapsed_ms?`
  - 可选：`channel`, `chat_id`, `reply_to_message_id`（若已知）

约束：
- 任何层（provider/stream/runner/tool/channel）不得直接发送 Telegram 错误回执；只能 emit Failed 事件。
- 任何层不得直接 drain reply targets；只能由 supervisor 执行。

### P0.1) 关键问答：出错点“不做副作用”那它怎么办？（必须说明）
> 你问得很关键：在方案 B 里，既然只有监督者做副作用，那么错误发生的地方应当如何“善后”？
> 本单冻结如下口径，避免实现时再次发散：

出错点（producer / failure source）在发生错误后只做两类事情：
1) **report（上报失败事件）**：best-effort emit `RunEvent::Failed { ... }`（携带 stage 所需字段）
2) **stop（停止本段执行并返回）**：立即结束当前流程（`return Err` / `break`），并允许做“局部资源清理”（关闭 stream、释放句柄等）

严格禁止（必须/不得）：
- 不得：出错点直接对 Telegram 回执错误（避免重复/口径漂移）
- 不得：出错点直接 drain reply targets（避免漏 drain/重复 drain/串线）
- 不得：出错点直接 broadcast run error（避免 Web UI 与 Telegram 口径不一致）

事件 emit 失败（通道满/已关闭）时的兜底：
- 出错点仍应 **返回 Err** 让 supervisor 通过 join/Err 路径兜底；
- 出错点应写一条 **紧急结构化日志**（最少包含 `run_id/session_key/stage` + `raw.class`），用于在极端情况下仍可定位；
- supervisor 必须在“未收到 Failed 事件但主任务 Err/JoinError”时，把该 Err **包装成 Failed 事件**，走同一套 normalize + 副作用链（保持“副作用唯一执行者”不变）。

### P1) Supervisor 归约器（Reducer）
Supervisor 在收到 `RunEvent::Failed` 后必须：
1) 生成 `dedup_key`（至少包含 `run_id`）
2) 判定是否已处理（`egress.sent` 去重）
3) 执行 normalize（纯函数）得到 `NormalizedError`
4) 产出一组 `EgressAction[]`（副作用清单）

### P2) 副作用链（Egress Actions，顺序冻结）
> 顺序冻结，避免未来又散落到各处。

1) `PersistRunError`：写入 run state（供 UI 查询）
2) `BroadcastRunError`：WebSocket broadcast（保持兼容，附加 debug 字段）
3) `DeliverChannelErrorOnce`：对 Telegram 等渠道回执一次（幂等、去重）
4) `DrainReplyTargets`：清空 pending reply targets（避免串线）
5) `LogRunFailure`：结构化日志 `run.failure`

失败处理要求：
- 任一 action 失败不得中断整个链（best-effort），但必须：
  - 记录 `egress.last_error={action,class,message_redacted,...}`（脱敏）
  - 写结构化日志（至少带 `run_id/session_key/error.stage/error.kind/egress.last_error.*`）
  - 不得覆盖“原始失败”的 `error.stage`（stage 永远指向第一次失败发生的位置）

### P3) 幂等与一致性（Idempotency）
- 去重键：`dedup_key` 至少包含 `run_id`；可附加 `channel/chat_id`。
- 去重状态位置（V1）：run state（内存/DB）中持久化 `egress.sent` 与 `egress.drained_count`，避免重复回执与“看不出是否 drain”。
- 多实例（未来）：若需要跨进程严格幂等，可升级为 outbox，但本单不强制引入。

---

## 可观测主线（Observability）：三面板字段契约
> 本节复用上文 D0/D1/D2/D2.1/D3；实现时必须把字段从 supervisor 的产物一次性贯通。

### O0) “三面板一致”的实现原则（冻结）
- Web UI / 日志：展示结构化字段（定位）
- Telegram：展示 `message.user + action`（行动），可选 debug hint（默认 off）
- 字段来源统一：所有面板字段均来自 `NormalizedError`（或其子集），避免各面板再各自推断 kind/stage。

### O1) `run.failure` 单行事件（建议字段排序）
建议在日志中按稳定顺序输出（便于 grep）：
`run_id, session_key, provider, model, stage, kind, retryable, action, request_id?, dedup_key, egress.sent, egress.reply_targets_before, egress.drained_count`

---

## 实施拆分（Implementation Outline）
### Step 0（不改行为，搭骨架）
- 新增 `RunEvent::Failed`（或等价结构）与 supervisor reducer（空实现也可）
- 新增 `NormalizedError` 与 `normalize_error(...)`（纯函数 + 单测）

### Step 1（接入两条关键失败路径）
- run_with_tools 的失败分支：改为 emit `RunEvent::Failed`
- stream-only 的 error 分支：改为 emit `RunEvent::Failed`
- supervisor 执行副作用链（至少：broadcast + Telegram 回执一次 + drain + log）

### Step 2（扩展接入面 + 回归）
- tool_exec 失败（含 sandbox/tool timeout）：emit failed（携带 tool_name/tool_call_id）
- channel_delivery 失败：emit failed（携带 channel/chat_id/reply_to）
- 增补回归测试与手工验收（见 Test Plan）

### Step 3（清理与禁止“旁路副作用”）
- 代码审查/rg：禁止新的 `broadcast error + return` 旁路路径；统一走 supervisor
- 文档与交叉引用同步

### 失败样例（让输出“可对照验收”）
> 以下样例只给出“字段长什么样”，不是最终文案；但字段必须齐全。

#### 样例 1：OpenAI Responses `stream ended unexpectedly`
- `stage=provider_stream`
- `kind=Network`
- `retryable=true`, `action=retry`
- `request_id`：如果能从响应头/事件中拿到就必须填；拿不到则显式为空并在 `message.debug` 说明来源缺失

Telegram `message.user`（示例）：
- `⚠️ 上游连接中断，已停止本次回答。你可以重试一次。`

Web UI debug 必须出现（示例字段）：
- `run_id`, `session_key`, `provider=openai-responses`, `model=...`
- `stage=provider_stream`, `kind=Network`, `retryable=true`, `action=retry`
- `request_id?`, `elapsed_ms`
- `egress.sent=true`, `egress.drained_count>0`

#### 样例 2：gateway `agent run timed out (timeout_secs=600)`
- `stage=gateway_timeout`
- `kind=Cancelled`（或 `Internal`，但必须全仓一致；推荐 `Cancelled`）
- `retryable=true`, `action=retry`
- `timeout_secs=600`, `elapsed_ms≈600000`

#### 样例 3：tool 失败（`exec` / sandbox ensure_ready）
- `stage=tool_exec`
- `kind=Internal`（或更细如 `Network`，取决于底层错误；taxonomy v1 先不做过细）
- `retryable=true/false`（按错误决定，但必须可解释）
- `tool_name=exec`, `tool_call_id=...`

## 验收标准（Acceptance Criteria）
- [x] 同一种失败（例如 401/429/5xx/网络超时）在 Web UI 与 Telegram 的可见性一致（都能看到错误回执/状态），且 `message.user` 风格统一。
- [x] 同一 `run_id` 的失败只会对 Telegram 回执一次（无重复消息，单进程保证）。
- [x] 失败后 reply targets 被 drain：后续成功回复不会回到旧 message_id（无串线）。
- [x] Web UI `state="error"` 保持兼容；新增字段不会破坏现有渲染。
- [x] 日志不泄露敏感信息，且包含定位字段（run_id/session/provider/model/stage/kind + 脱敏 raw）。
- [x] 满足“失败问答”：不读源码，仅凭日志 + Web UI debug 能回答 D3 的 7 个问题（除 `request_id` 仍为 best-effort）。
- [x] Telegram 回执默认不暴露 `run_id` 等 debug hint（debug hint 开关为后续增强，见 Q2）。

## 测试计划（Test Plan）
### Unit
- [x] `normalize_failure`：覆盖典型错误输入（401/403/context window/timeout/stream ended），断言 kind + retryable + action：`crates/gateway/src/run_failure.rs:234`
- [x] `handle_run_failed_event` 去重：同一 `run_id` 重复触发最多发送一次，且 duplicate 仍 drain：`crates/gateway/src/chat.rs:7127`

### Integration / Gateway
- [x] 模拟 `state.push_channel_reply` 后触发失败出口，断言 `state.peek_channel_replies(session_key)` 为空（已 drain）：`crates/gateway/src/chat.rs:6999`

### Channel / Outbound
- [x] 使用 mock outbound 捕获发送内容：断言 error 回执带正确 `reply_to` 且为短句（脱敏/截断）：`crates/gateway/src/chat.rs:6999`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：Telegram 真机 e2e 难以稳定跑在 CI；但必须提供可执行手工验收步骤。
- 手工验证步骤：
  1) 触发一个可控失败（例如临时将 key 配错触发 Auth，或用不存在的模型触发 ModelNotFound）
  2) 观察 Telegram：必须收到 1 条失败回执（不得无回/不得重复）
  3) 观察 Web UI debug：字段齐全（至少 D1）
  4) 观察日志：存在单行 `run.failure`（字段齐全，至少 D2）
  5) 再发送一条正常消息：不得串线到上一次失败的 reply_to

## 发布与回滚（Rollout & Rollback）
- 发布策略：Phase 1 可先不改 taxonomy，只收敛出口与 drain；Phase 2/3 再逐步切换语义输出。
- 回滚策略：保留旧 broadcast 结构兼容；若渠道回执引发误报，可通过配置开关降级（是否需要开关：见 Open Questions）。

## 实施基础评估（Readiness）
### 已明确且可直接开工的部分（V1 冻结）
- 错误语义：`ErrorKind`（A1）+ `Action`（B0.1）已冻结，且给出默认 `retryable/action/message.user` 口径。
- 失败分段：`FailureStage` + Stage→details 矩阵已冻结（Stage 规范）。
- 单一真源：`RunFailureRecord` 字段字典（B2）可直接作为实现与测试的 check-list。
- 单一出口：Run Supervisor 的“事件→归约→副作用链”顺序已冻结（P0/P1/P2）。
- 三面板一致：Web UI / 日志 / Telegram 的最小字段契约已冻结（D0-D3）。

### 仍可选但不阻塞 V1 的事项（建议默认先关）
- Telegram debug hint：默认 **关闭**，仅在需要时通过配置开关打开（避免对普通用户暴露 `run_id/stage/kind`）。
- 多实例严格幂等（outbox）：默认 **不做**，V1 先保证单进程幂等 + 证据字段齐全。

### 仍需在实现时“查现状/补齐取值”的事项（不需要额外产品决策）
- `request_id` 可得性：需要确认各 provider/SDK 是否能取到（拿不到时必须明确置空，且 `message.debug` 说明缺失原因）。
- `provider/model` 的口径：必须按 “as-sent” 记录（最终发给上游的 provider/model）。

## 交叉引用（Cross References）
  - 渠道止血：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
- 现有 UI 结构化错误：`crates/gateway/src/chat_error.rs:10`
- agents 错误分类：`crates/agents/src/provider_chain.rs:73`
- gateway 错误分支（run_with_tools / stream-only）：`crates/gateway/src/chat.rs:4516` / `crates/gateway/src/chat.rs:5213`
- channel_events 即时错误回执：`crates/gateway/src/channel_events.rs:281`

## 未决问题（Open Questions）
- 默认决策（V1，不再讨论）：
  - 渠道失败回执默认 **开启**（Telegram 必回一次，且去重）。
  - Telegram `message.user` 默认 **不包含** `run_id`；仅在 debug hint 开关开启时附带 `run_id_short/stage/kind`。
  - `NormalizedError` 的权威定义放在 gateway/supervisor 侧（single egress 的唯一真源）；后续如需多入口复用再下沉到 common crate。
  - `request_id` 取值 best-effort：拿得到就填；拿不到就置空（同时 `message.debug` 说明缺失原因）。

- 仍可后续增强（不阻塞 V1）：
  - Q1：去重证据（`egress.sent` 等）是否必须落盘到 DB / outbox，保证 **跨重启/多实例** 仍幂等（V1 先保证单进程幂等 + 证据字段齐全）。
  - Q2：是否需要额外的 config flag 控制“Telegram 错误 debug hint”（例如 `channels.telegram.error_debug_hints=true`，默认 false）。

### Q1 补充说明：什么叫“跨重启/多实例严格幂等”（人话）
> 这不是“没考虑”，而是明确作为 **V2+ 增强**。V1 先把语义/出口/可观测性做收敛，避免范围失控。

V1 的保证范围（单进程幂等）：
- 在同一个进程生命周期内，同一 `run_id` 的失败只会对同一渠道回执一次（依赖内存/本地 run state 的 `egress.sent` 去重）。

V1 **不保证** 的两类情况（会出现“重复回执”）：
1) **进程重启**：如果已经把错误回执发到 Telegram，但进程在持久化/记录 `egress.sent=true` 之前崩溃或重启，重启后可能再次发送同样的错误回执。
2) **多实例**：如果同一 `run_id` 的失败同时被两个实例处理（例如水平扩容），两个实例都可能各自发送一次错误回执。

如果要做到“跨重启/多实例严格幂等”（V2+ 的典型做法）：
- 引入共享持久化的去重/出站记录（例如 DB 表 + 唯一约束的 outbox），以 `dedup_key=(run_id, channel, chat_id, ...)` 作为全局幂等键；
- 只有成功写入 outbox 的实例才允许执行真实发送；其余实例检测到已存在记录则跳过发送。

## Close Checklist（关单清单）
- [x] taxonomy 与字段结构已确定，并写入 Glossary & Spec（概念收敛）
- [x] single egress 生效，且覆盖 run_with_tools + stream-only + timeout 失败路径
- [x] 去重与 drain 回归测试齐全（含 duplicate egress drain）
- [x] 跨渠道行为一致性达标（Web UI + Telegram + logs）
- [x] 文档与交叉引用已同步（无断链）

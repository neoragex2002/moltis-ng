# Issue: 收敛 LLM 失败处理（Error Taxonomy + Single Error Egress）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1（架构收敛；P0 止血见 Telegram 单子）
- Components: `crates/agents` / `crates/gateway` / channels delivery / Web UI debug
- Cross-ref:
  - 渠道止血（Telegram 无回执 + drain targets）：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`

**已实现**
- 暂无（本单为收敛设计与逐步迁移计划）

**已覆盖测试**
- 暂无新增（需补齐：见 Test Plan）

**已知差异/后续优化（非阻塞）**
- 无（本单本身就是“收敛与补齐”的后续）

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
- **Normalized Error（规范化错误对象）**：把任意层抛出的 raw error（string/anyhow/HTTP/JSON）映射为少数“语义类别”，并携带 user/debug 两套文案与可执行建议。
  - Why：保证跨渠道/跨层输出一致，减少“拼字符串”导致的漂移。
  - Not：不是为了保留底层堆栈/敏感信息；debug_message 也必须脱敏。

- **Error Taxonomy（错误语义分类）**：固定少数类别，覆盖绝大多数问题即可，避免无限枚举。

- **Single Error Egress（唯一错误出口）**：run 生命周期中定义一个集中处理点（函数/模块/事件），任何失败最终都走这里。
  - Why：避免“多处触发、多次发送、多处 drain”导致重复回执/漏 drain/串线。
  - Not：不要求一次性把所有层的实现都大迁移；允许分阶段逐步接入。

- **authoritative vs estimate**：
  - authoritative：来自 provider 回包 usage 或明确的协议字段（权威值）。
  - estimate：启发式/推导（用于风险预估，不能当真值）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 任意一次 run 的失败，在所有渠道呈现一致的“语义类别 + 用户文案口径”（至少 Web UI + Telegram）。
- [ ] 明确且唯一：**失败时是否回执、回给谁、回几次**（去重：同一 `run_id` 只回一次）。
- [ ] 失败时必须 drain 任意 pending reply targets（避免串线）。
- [ ] Web UI debug 能看到：错误类别（kind）、是否可重试（retryable）、建议动作（suggested_action）、以及脱敏后的 debug_message。

### 非功能目标（Non-functional）
- 安全隐私：user_message/debug_message/logs **不得**包含 token、完整 HTTP body、路径、堆栈等敏感信息。
- 兼容性：保留现有 UI `state="error"` 的 broadcast 契约（可扩展字段，但不破坏现有渲染）。
- 可观测性：日志必须携带 `run_id/session_key/provider/model/stage/kind`，便于排障与聚合统计。

### Non-goals（明确不做）
- [ ] 不在本单强制引入“自动重试/自动 fallback”的策略变更（可作为后续独立议题）。
- [ ] 不要求一次性重写所有 provider 的错误处理；允许渐进迁移。

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

### B) Normalized Error 对象（规范字段）
任何 run 的失败最终都应产生一个规范对象（字段名可调整，但语义需稳定）：
- `kind`: taxonomy enum
- `user_message`: 脱敏短文案（可直接发 Telegram）
- `debug_message`: 脱敏细节（给 UI debug/日志）
- `retryable`: bool
- `suggested_action`: `"retry" | "wait_and_retry" | "check_api_key" | "switch_model" | "contact_admin" | ...`
- `provider` / `model` / `run_id` / `session_key`
- `stage`: `"provider_call" | "streaming" | "tool_exec" | "runner" | "channel_delivery" | "gateway_internal" | ..."`

### C) Single Error Egress（唯一出口 + 去重 + drain）
定义一个唯一失败出口（例如 `handle_run_failure(...)` 或 `RunEvent::Failed { normalized_error, ... }`），负责：
1) 更新 run 状态（供 Web UI / 查询）
2) WebSocket broadcast `state="error"`（保持兼容，可附加 `kind/retryable/suggested_action/stage`）
3) **渠道回执**（Telegram 等）：发送 `user_message`，并确保 **同 `run_id` 只发送一次**
4) **drain reply targets**（避免后续串线）
5) 记录日志（带 `run_id/session_key/provider/model/stage/kind`）

## 方案（Proposed Solution）
### Phase 0（准备，不改行为）
- 定义 `NormalizedErrorKind` + `NormalizedError` 类型（位置待定：建议在 gateway/agents 边界清晰处）。
- 提供 `normalize_error(raw, ctx) -> NormalizedError`（复用现有 `parse_chat_error` 与 `provider_chain::classify_error`，但输出统一结构）。

### Phase 1（止血：统一出口，先保证“必回一次 + 必 drain”）
- 在 gateway 层引入 `handle_run_failure(...)`，并让现有失败分支（至少 run_with_tools 与 stream-only）统一调用。
- 与 Telegram 单子联动：确保失败也会触发渠道回执（脱敏短文案）与 drain。

### Phase 2（语义收敛：逐步迁移到 taxonomy）
- 将 provider/runner/stream 中的 raw error 映射逐步收敛为 `NormalizedErrorKind`。
- Web UI debug 面板展示 `kind/retryable/suggested_action/stage`（避免只展示 raw 文案）。

### Phase 3（彻底收敛：禁止多处拼错误字符串）
- 各层不再各自决定“回不回/回几次/回给谁”，只产生 normalized error 并交给 single egress。
- 增加针对“重复触发”的回归测试（确保去重稳定）。

## 验收标准（Acceptance Criteria）
- [ ] 同一种失败（例如 401/429/5xx/网络超时）在 Web UI 与 Telegram 的可见性一致（都能看到错误回执/状态），且 user_message 风格统一。
- [ ] 同一 `run_id` 的失败只会对 Telegram 回执一次（无重复消息）。
- [ ] 失败后 reply targets 被 drain：后续成功回复不会回到旧 message_id（无串线）。
- [ ] Web UI `state="error"` 保持兼容；新增字段不会破坏现有渲染。
- [ ] 日志不泄露敏感信息，且包含定位字段（run_id/session/provider/model/stage/kind）。

## 测试计划（Test Plan）
### Unit
- [ ] `normalize_error`：覆盖典型错误输入（401/403/429/5xx/context window/timeout/invalid request），断言 kind + retryable + suggested_action。
- [ ] `handle_run_failure` 去重：同一 `run_id` 重复调用只发送一次渠道回执。

### Integration / Gateway
- [ ] 模拟 `state.push_channel_reply` 后触发失败出口，断言 `state.peek_channel_replies(session_key)` 为空（已 drain）。

### Channel / Outbound
- [ ] 使用 mock outbound 捕获发送内容：断言 error 回执带正确 `reply_to` 且脱敏。

## 发布与回滚（Rollout & Rollback）
- 发布策略：Phase 1 可先不改 taxonomy，只收敛出口与 drain；Phase 2/3 再逐步切换语义输出。
- 回滚策略：保留旧 broadcast 结构兼容；若渠道回执引发误报，可通过配置开关降级（是否需要开关：见 Open Questions）。

## 交叉引用（Cross References）
  - 渠道止血：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
- 现有 UI 结构化错误：`crates/gateway/src/chat_error.rs:10`
- agents 错误分类：`crates/agents/src/provider_chain.rs:73`
- gateway 错误分支（run_with_tools / stream-only）：`crates/gateway/src/chat.rs:4516` / `crates/gateway/src/chat.rs:5213`
- channel_events 即时错误回执：`crates/gateway/src/channel_events.rs:281`

## 未决问题（Open Questions）
- 是否需要一个 config flag 控制“渠道失败回执”（默认开/关）？
- Telegram 的 `user_message` 是否需要包含 `run_id`（通常不建议对普通用户暴露）？
- `NormalizedError` 的权威来源：gateway 统一规范，还是下沉到 agents/common 供多入口复用？

## Close Checklist（关单清单）
- [ ] taxonomy 与字段结构已确定，并写入 Glossary & Spec（概念收敛）
- [ ] single egress 生效，且覆盖 run_with_tools + stream-only 失败路径
- [ ] 去重与 drain 回归测试齐全
- [ ] 跨渠道行为一致性达标（Web UI + Telegram）
- [ ] 文档与交叉引用已同步（无断链）

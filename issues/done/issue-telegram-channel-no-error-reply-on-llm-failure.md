# Issue: LLM 请求失败时 Telegram 会话无错误回执（且 reply targets / logbook 可能残留导致后续串线）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-20）
- Priority: P0（渠道可靠性止血）
- Owners: <TBD>
- Components: gateway/chat / gateway/channel_events / channels delivery / telegram outbound
- Affected providers/models: all（只要是 channel session 都受影响）
- Cross-ref：
  - 全局错误语义收敛（taxonomy + single egress）：`issues/done/issue-error-handling-taxonomy-single-egress.md`（本单不做大收敛，先止血）

**已实现（2026-02-20）**
- run internal failure / stream error：失败时发送错误回执（text）并 drain（targets + logbook）：`crates/gateway/src/chat.rs:4569` / `crates/gateway/src/chat.rs:5271`
- timeout：发送 timeout 回执并 drain（targets + logbook）：`crates/gateway/src/chat.rs:2608`
- silent success：不发空消息但仍 drain（targets + logbook）：`crates/gateway/src/chat.rs:4543` / `crates/gateway/src/chat.rs:5245`
- immediate failure（`chat.send` 立刻 Err）：回执 `⚠️` 且 drain（targets + logbook）：`crates/gateway/src/channel_events.rs:310` / `crates/gateway/src/channel_events.rs:815`
- 修复 logbook 串线：`deliver_channel_replies()` 在任何早退前就 drain status log：`crates/gateway/src/chat.rs:5347`
- V1 队列降级：failed/timeout/**silent** run 后丢弃 queued messages（避免 replay 成功但无回执的黑洞）：`crates/gateway/src/chat.rs:2708`

**已覆盖测试**
- immediate failure drain：`dispatch_to_chat_immediate_failure_drains_reply_targets_and_logbook`：`crates/gateway/src/channel_events.rs:1637`
- run_streaming error 回执 + drain：`run_streaming_error_sends_channel_error_and_drains_state`：`crates/gateway/src/chat.rs:6366`
- run_streaming silent drain（不发送）：`run_streaming_silent_success_drains_state_without_sending`：`crates/gateway/src/chat.rs:6435`

**已知差异/后续优化（非阻塞）**
- 错误回执目前基于 `parse_chat_error()` 的 `title/detail`，并做了“单行化 + 截断”以避免把大段 raw dump 发到 Telegram；更彻底的 taxonomy/去敏策略仍建议按 cross-ref 收敛。
- `message_queue_mode` 与 `channel_reply_queue` 的一条消息↔一条回执绑定目前不完备（见 Root Cause F）；本单给出 **V1 明确降级策略**，避免出现“queued 消息被重放但永远不回执”的隐蔽故障。

---

## 背景（Background）
- 场景：Telegram inbound 作为 channel event 触发 `chat.send()` 异步执行；成功时由 `deliver_channel_replies()` 把最终文本回发 Telegram。
- 约束：Telegram 用户侧必须能看到失败回执（否则体验是“typing → 无任何消息”）；同时必须清理 reply targets/logbook 状态，避免后续串线。
- Out of scope：本单不做全局 error taxonomy；只保证 Telegram（以及 channel 机制整体）在失败/超时/空输出时**行为一致且可解释**。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **reply targets / `channel_reply_queue`**：每个 session 的待回执目标列表（含 `chat_id` + `message_id` threading），由 `GatewayState.push_channel_reply()` 写入、`GatewayState.drain_channel_replies()` 清空。
  - Why：决定“这次 LLM 输出回到哪个 Telegram 消息下”；若残留会导致后续串线。
  - Evidence：`crates/gateway/src/state.rs:531` / `crates/gateway/src/state.rs:542`
- **status log / logbook / `channel_status_log`**：工具执行/模型选择等状态日志缓冲，最终作为 Telegram “Activity log” suffix 发送。
  - Evidence：`crates/gateway/src/state.rs:577` / `crates/gateway/src/chat.rs:5357`
- **immediate failure**：`chat.send(params).await` 直接返回 `Err`（在 channel_events 层可见）
- **run internal failure**：`chat.send()` 返回 `Ok({runId})` 后，后台 agent loop 在 provider/stream/tool 执行中失败（channel_events 层不可见）。
- **terminal state（终止态）**：对“需要回渠道的一次触发”来说，最终只能是：
  - `final(text != "")`
  - `final(text == "")`（silent）
  - `error`（包含 timeout）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 任意 `run internal failure`（provider/stream/tool error）时，Telegram 必须收到一条错误回执（thread 到原消息）。
- [x] 任意终止态（final/error/timeout/silent）必须清理：
  - [x] `channel_reply_queue`（避免串线）
  - [x] `channel_status_log`（避免 logbook 串到后续成功回复）
- [x] `immediate failure`（`chat.send` 立即 Err）也必须清理上述两类状态（当前只回执、不清理）。
- [x] Web UI 的 `state="error"` broadcast 行为保持不变（可扩展字段，但不破坏现有前端）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：错误回执**脱敏 + 截断**（避免把 raw error/body/token/路径直接发到 Telegram）。
  - 不得：失败时遗留 reply targets/status log，导致后续串线或“莫名其妙多出一段 Activity log”。
- 可观测性：
  - 日志应能区分：immediate failure vs run internal failure vs timeout，并打印 `session_key/run_id/target_count` 等最小定位信息。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 侧无错误回执（run 内部失败、stream error、timeout 时尤甚）：
   - 用户看到长时间 typing（如有），然后无任何消息。
2) 串线/错回复风险：
   - 失败后未 drain 的 reply targets 会把后续成功回复 thread 到旧 `message_id`，甚至对多个历史 target 重复回复。
3) logbook 串线风险：
   - status log 未被 drain 时，后续某次成功回复可能会带上上一轮残留的 Activity log。

### 影响（Impact）
- 用户体验：Telegram 端“失败不可见”，误以为机器人宕机/无响应。
- 可靠性：reply targets/status log 残留导致串线、重复回复、logbook 漂移。
- 排障成本：用户只能翻服务器日志；UI/Telegram 不一致更难定位。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - channel_events 在调用 `chat.send` 之前就 push reply target（因此即便后续 `chat.send` 失败/排队，也会残留 target）：
    - `crates/gateway/src/channel_events.rs:126`–`crates/gateway/src/channel_events.rs:130`
    - multimodal 同理：`crates/gateway/src/channel_events.rs:669`–`crates/gateway/src/channel_events.rs:672`
  - immediate failure 仅回 `⚠️ {e}`，但 **不 drain**：
    - `crates/gateway/src/channel_events.rs:290`–`crates/gateway/src/channel_events.rs:308`
  - tools 模式 run internal failure：只 broadcast error，不 deliver / 不 drain：
    - `crates/gateway/src/chat.rs:4526`–`crates/gateway/src/chat.rs:4542`
  - stream-only failure：只 broadcast error，不 deliver / 不 drain：
    - `crates/gateway/src/chat.rs:5223`–`crates/gateway/src/chat.rs:5239`
  - timeout failure：只 broadcast error，不 deliver / 不 drain：
    - `crates/gateway/src/chat.rs:2608`–`crates/gateway/src/chat.rs:2641`
  - silent success：成功分支只有 `if !is_silent` 才 deliver，因此 silent 会遗留 reply targets/status log：
    - `crates/gateway/src/chat.rs:4508`–`crates/gateway/src/chat.rs:4517`
    - `crates/gateway/src/chat.rs:5205`–`crates/gateway/src/chat.rs:5214`
  - `deliver_channel_replies()` 会 drain targets，但 status log 只有在拿到 outbound 且非空文本时才会 drain；早退会导致 logbook 残留：
    - drain targets：`crates/gateway/src/chat.rs:5313`
    - targets empty / text empty 早退：`crates/gateway/src/chat.rs:5315`–`crates/gateway/src/chat.rs:5334`
    - outbound unavailable 早退：`crates/gateway/src/chat.rs:5344`–`crates/gateway/src/chat.rs:5355`
    - drain status log（仅此处）：`crates/gateway/src/chat.rs:5357`–`crates/gateway/src/chat.rs:5359`

## 根因分析（Root Cause）
- A. **channel_events 层错误处理只覆盖 immediate failure**，并且没有清理 reply targets/status log。
  - 结果：即便给用户发了 `⚠️ {e}`，也可能遗留 target → 后续串线。
- B. **gateway run_with_tools / run_streaming 的 error 分支没有“渠道终止态回执”**（只做 UI broadcast）。
  - 结果：Web UI 能看到 error，但 Telegram 无任何消息。
- C. **timeout 也是一种 run internal failure**，目前同样只 broadcast，不回渠道、不清理状态。
- D. **silent success 缺少终止态清理**：`if !is_silent` 才 deliver，导致 reply targets/status log 残留。
- E. **logbook（status log）与 reply targets 的 drain 绑定不完整**：
  - `deliver_channel_replies()` 先 drain targets，再在较后位置 drain status log；targets/text/outbound 任一早退都会让 status log 残留并串到后续成功回复。
- F. **（关键复杂点）`message_queue_mode` 与 reply targets 的耦合不明确**：
  - 当前 reply targets 是“按 session 聚合的 Vec”，并且在 `chat.send()` 可能返回 `queued=true` 时也已被提前 push（见 Evidence）。
  - 如果本单在 error 分支“直接 drain 全部 targets”，可能会把“已排队但尚未 replay 的消息”的回执目标一起清掉，造成后续 replay run “成功但永远不回 Telegram”。
  - 因此本单需要冻结一个 V1 规则：**error/timeout 视为本 session 当前所有 pending 触发都失败（统一回执并清理），并丢弃 queued messages**；更精细的 per-message 绑定留给后续（若需要）。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 任何 `run internal failure` / `timeout`：对所有 pending reply targets 发送 **一次**错误回执，并清理 `channel_reply_queue` 与 `channel_status_log`。
  - 任何 `final(text != "")`：发送最终文本回执，并清理 `channel_reply_queue` 与 `channel_status_log`。
  - 任何 `final(text == "")`：不发送 Telegram 消息，但仍必须清理 `channel_reply_queue` 与 `channel_status_log`（避免串线）。
  - `immediate failure`：除发送 `⚠️` 外，也必须清理 `channel_reply_queue` 与 `channel_status_log`。
- 不得：
  - 不得在 error/timeout 后遗留 targets/logbook，导致后续串线/漂移。
  - 不得把明显敏感的 raw error（Authorization、完整 body、路径）原样发到 Telegram（必须脱敏/截断）。
- 应当：
  - 错误回执使用纯文本（`ReplyMedium::Text`），避免触发 TTS。

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（推荐，V1 止血）：统一“渠道终止态处理”并强制 drain（含 timeout/silent/immediate）
- 核心思路：
  - 抽一个 helper（例如 `deliver_channel_terminal(...)`），作为 channel session 的**唯一终止态出口**：
    - 先 drain `channel_reply_queue` + drain `channel_status_log`（无论成功/失败/空文本/outbound 是否可用）
    - 再按终止态决定是否发 Telegram：
      - final(text!=empty) → 发文本/语音（现有逻辑）
      - error/timeout → 发短错误文本（固定 ReplyMedium::Text）
      - silent → 不发送
  - 在 run_with_tools error、stream error、timeout、silent success、channel_events immediate Err 等处统一调用。
- 优点：
  - 改动点集中、可测、可回滚；能同时解决“无回执 + 串线 + logbook 串线”。
  - 与更大的 taxonomy/single-egress 方案兼容（未来只需替换 error_text 构造与去重）。
- 风险/缺点：
  - 需要冻结 V1 对 queued messages 的处理（见 Root Cause F）。

#### 方案 2（后续更精细）：reply targets 绑定 run_id / queued message（per-message/per-run queue）
- 核心思路：把 `channel_reply_queue` 从 `Vec<Target>` 升级为 `Vec<{run_id, target}>`（或与 message_queue 合并），deliver 时只处理本 run 的 targets，避免 queued 被误伤。
- 不做为本单 V1：改动面更大，且需要同步调整 channel_events 与 chat.send 的 contract。

### 最终方案（Chosen Approach）
采用 **方案 1（V1 止血）**，并冻结以下行为规范：

#### 行为规范（Normative Rules）
1) **所有终止态都必须 drain**：`channel_reply_queue` 与 `channel_status_log` 不得跨终止态残留。
2) **error/timeout 回执必须脱敏 + 截断**：
   - 来源：优先使用 `parse_chat_error(...)` 的 `title/detail`，再做二次清理（unknown/raw 需截断）。
   - 格式建议：`⚠️ <title>: <detail>`（detail 为空则省略冒号）。
3) **silent success 不回 Telegram，但必须清理状态**（避免串线）。
4) **queued messages 的 V1 降级**：
   - 当某次 run 进入 error/timeout，视为该 session 当前所有 pending 触发都失败：统一回执并清理；随后丢弃/取消该 session 的 queued messages（避免出现“queued replay 成功但无回执”的隐蔽故障）。

#### 接口与数据结构（Contracts）
- 现有 `deliver_channel_replies_to_targets(...)` 可复用（threading/suffix/tts 的实现集中在这里）。
- 需要新增/调整的 helper：
  - `deliver_channel_terminal_success(text, desired_reply_medium)`
  - `deliver_channel_terminal_error(error_text)`（强制 `ReplyMedium::Text`）
  - `drain_channel_terminal_state()`（无论是否发送都要 drain：targets + status_log）

#### 失败模式与降级（Failure modes & Degrade）
- outbound 不可用：仍必须 drain（避免串线），但无法发回 Telegram；需日志明确记录 `outbound unavailable`。
- “尚未 push reply target 就失败”：helper 会发现 targets 为空并跳过；属于可接受的低概率边缘场景。

## 验收标准（Acceptance Criteria）【不可省略】
### 自动化验收（CI/单测可覆盖）
- 失败/超时/空输出的回执与 drain 行为由单元测试覆盖（见“实施现状/已覆盖测试”）。

### 手工验收（可选；不作为关单前置）
- provider 错误（错误 API key / 断网 / provider 4xx/5xx）：Telegram 收到错误回执（thread 对齐）；Web UI 仍能看到 `state="error"`；状态无残留。
- timeout（把 `tools.agent_timeout_secs` 设很小或构造长程工具）：Telegram 收到 timeout 回执；状态无残留。
- silent response（最终文本为空）：Telegram 不收到空消息；下次成功回复不会串线。
- `chat.send` immediate Err（gateway 未 ready 或 chat service 立刻返回 Err）：Telegram 收到 `⚠️`；状态无残留。

## 测试计划（Test Plan）【不可省略】
### Unit（已覆盖）
- immediate failure（channel_events）：`dispatch_to_chat_immediate_failure_drains_reply_targets_and_logbook`
- run_streaming StreamEvent::Error：`run_streaming_error_sends_channel_error_and_drains_state`
- silent success（final text 为空）：`run_streaming_silent_success_drains_state_without_sending`

### 建议补充（非阻塞；单独单测即可）
- `run_with_tools` error 分支（预置 `push_channel_reply` + `push_channel_status_log`，断言 drain + 错误回执）
- timeout 分支（同上；覆盖 `crates/gateway/src/chat.rs` timeout 路径）

### Integration / manual（可选）
- Telegram：断网/错误 key/主动超时，观察回执与 thread 是否正确；之后发送正常消息确认不串线。

## 相关位置（References）
- `crates/gateway/src/channel_events.rs`（channel inbound dispatch / immediate failure）
- `crates/gateway/src/chat.rs`（run_with_tools/run_streaming/timeout/deliver_channel_replies）
- `crates/gateway/src/state.rs`（reply targets + status log queue）

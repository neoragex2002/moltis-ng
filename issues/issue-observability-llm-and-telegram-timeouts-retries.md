# Issue: 增强 LLM 推理与 Telegram 通道的超时/重试/回退可观测性（gateway_timeout / outbound_send / getUpdates）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Updated: 2026-03-08
- Owners:
- Components: gateway / agents / telegram / channels
- Affected providers/models: openai-responses::*（以及其它 provider）

**已实现（如有，写日期）**
- gateway 级别 agent run 硬超时（`tools.agent_timeout_secs`，默认 600s），超时归类为 `FailureStage::GatewayTimeout` 并产生用户侧“Request cancelled or timed out”文案：`crates/gateway/src/chat.rs:2748`、`crates/gateway/src/run_failure.rs:268`
- agent runner 在部分可重试错误下“仅重试 1 次”（固定 2s 延迟）：`crates/agents/src/runner.rs:744`、`crates/agents/src/runner.rs:817`、`crates/agents/src/runner.rs:1452`
- Web UI error 事件载荷已带 `stage/kind/retryable/action/details/raw/egress` 等字段（由 `handle_run_failed_event` 注入）：`crates/gateway/src/chat.rs:4288`
- Telegram long-poll：`getUpdates` 超时 30s，HTTP client 超时 45s：`crates/telegram/src/bot.rs:59`、`crates/telegram/src/bot.rs:139`
- Telegram 出站发送：`send_message`/`edit_message_text`/分块发送/流式占位“…”：`crates/telegram/src/outbound.rs`

**已覆盖测试（如有）**
- gateway agent timeout 单测：`crates/gateway/src/chat.rs:11347`
- runner retry 单测：`crates/agents/src/runner.rs:4279`、`crates/agents/src/runner.rs:4299`

**已知差异/后续优化（非阻塞）**
- 当前用户侧常见回执文案过于笼统（难区分是 LLM 超时、网络断连、用户 cancel、还是 Telegram 发送失败）。
- Telegram 出站发送失败多数路径缺少“明确的 reason code + 关键字段”，并且没有统一的重试/回退语义（本单先补观测性，是否做重试语义另开或本单扩展需冻结）。

---

## 背景（Background）
- 场景：群聊/DM 中出现 `⚠️ Request cancelled or timed out. Please retry.`、以及日志里出现 `telegram getUpdates failed`、`failed to send channel reply` 等网络相关错误。
- 痛点：当前很难判断“到底哪里超时/断了”，也很难把一次故障与具体的 `run_id/trigger_id/chat_id/message_id/provider_request` 关联起来。
- Out of scope（本单默认不做，除非后续在本单冻结范围）：
  - 不引入“可靠投递（at-least-once）”的持久化 outbox 语义（那会显著改变系统行为并引入重复投递风险）。
  - 不重做 Telegram relay/mirror 机制本身（只补故障观测与关联信息）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **agent_timeout_secs**（主称呼）：gateway 对“整次 agent run”的 wall-clock 硬超时上限（默认 600s）。
  - Why：防止一次 run 无限挂起。
  - Not：不是 LLM provider 的 HTTP request timeout；也不是 Telegram send 的超时。
  - Source/Method：configured（配置）+ effective（生效值）
- **FailureStage**（主称呼）：故障发生阶段（`gateway_timeout` / `provider_request` / `provider_stream` / `runner` / `tool_exec` / `channel_delivery`）。
  - Source/Method：effective（由 `run_failure` 归一化推断）
- **Telegram 出站失败**（主称呼）：把回复发送到 Telegram 失败（`send_message` / `edit_message_text` / 分块发送中的任一 chunk 失败）。
  - Source/Method：authoritative（Teloxide/HTTP 返回的错误）
- **观测性（Observability）**（主称呼）：能从日志/事件/UI 追踪一次故障的“发生点、原因、上下文、影响范围、可操作下一步”。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 用户侧（Telegram/Web UI）在发生“超时/取消/网络失败/发送失败”时，能看到**更具体的失败类型**（至少包含 stage/kind 的稳定口径），而不是只有一句“Please retry”。
- [ ] 运维/排障侧（日志）能把一次失败与以下至少 3 类 ID 关联起来：`run_id`、`trigger_id`、`session_id/chan_chat_key`（如适用还包括 `chat_id`、`telegram_message_id`）。
- [ ] 对“发生了重试/发生了 failover/发生了降级（例如流式 edit 失败）”必须有结构化日志（低噪声、可去重）。

### 非功能目标（Non-functional）
- 日志低噪声：只在失败/重试/降级时打关键日志；成功路径避免刷屏。
- 安全与隐私：不得打印 token、完整正文；必要字段仅记录长度/哈希/ID。
- 兼容性：不改变现有对话语义与投递语义（除非明确在本单冻结并给出回滚策略）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 群聊里 bot 回复：`⚠️ Request cancelled or timed out. Please retry.` —— 用户无法判断：
   - 是 `agent_timeout_secs` 触发？
   - 还是上游 provider 网络断了？
   - 还是 Telegram 发送失败？
2) Web UI 已展示结构化字段（如 `stage/kind/action`），但仍缺少 **run_id 可见性**与**一键复制排障信息**能力，导致跨日志/跨渠道定位仍偏慢。
3) 日志里出现 `telegram getUpdates failed`，但无法快速判断是否与“某次回复丢失”有关（链路关联信息不足）。
4) Telegram 流式回复路径中，`edit_message_text` 的错误被吞掉（只剩 “…” 占位的体验难排障），且缺少降级日志。

### 影响（Impact）
- 用户体验：误以为“bot 不稳定/随机坏掉”，并频繁要求人工重试。
- 排障成本：需要人工在多处日志中拼接时间线，缺少稳定的关联键。

## 现状核查与证据（As-is / Evidence）【不可省略】
- “Request cancelled or timed out”文案来源：`crates/gateway/src/run_failure.rs:268`
- gateway 对整次 agent run 的 timeout：`crates/gateway/src/chat.rs:2748`
- Telegram long-poll timeout：`crates/telegram/src/bot.rs:139`（30s）+ client 45s：`crates/telegram/src/bot.rs:59`
- Telegram 出站发送日志：`crates/telegram/src/outbound.rs:73`（start/sent），失败路径多为上层 `warn!(failed to send...)`
- agent runner retry：`crates/agents/src/runner.rs`（仅 1 次，固定 2s）
- Web UI error card 已展示 `stage/kind/action`（但当前看不到 `run_id` / 不便一键复制排障信息）：
  - `crates/gateway/src/assets/js/chat-ui.js:116`
  - `crates/gateway/src/assets/js/websocket.js:499`（WS error frame 仅把 `p.error` 传给 error card，丢失 `p.runId`）
- UI 已收到 retrying 状态但未携带原因字符串：`crates/gateway/src/chat.rs:4935`

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当失败发生时，日志必须包含稳定字段：`failure_stage`、`error_kind`、`retryable`、`action`、以及 `run_id/trigger_id/session_id`（可用则带 `chan_account_key/chat_id/message_id`）。
  - Telegram/Web UI 的用户回执文案必须能区分至少三类：`LLM 超时` / `上游网络失败` / `Telegram 发送失败`（可映射到 stage/kind）。
  - 若发生重试/等待/降级，必须有 1 条结构化日志（可去重/限频）。
- 不得：
  - 不得在日志/回执中泄露 token/完整消息正文。
- 应当：
  - 能在一次 run 的生命周期中，把 provider 请求、runner 重试、channel delivery 失败串成一条可读时间线（以 `run_id` 为主键）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 快速落地建议（Quick Wins，优先级从高到低）
> 目标：不引入新的投递语义（不做可靠 outbox），先让“出了问题就能 30 秒内定位”。

1) **Telegram 失败回执补充“最小诊断码”**
   - 仍以 `normalized.message.user` 为主，但尾部追加一个极短诊断码（不泄露隐私）：
     - 示例：`⚠️ 推理超时（600秒）。code=gateway_timeout`
     - 示例：`⚠️ 上游网络失败，请稍后重试。code=provider_request/network`
     - 示例：`⚠️ Telegram 发送失败，请稍后重试。code=channel_delivery/telegram_send_failed`
   - 可选：仅在 `operator`/debug 模式追加 `run=<short>`（例如前 8 位），避免群里刷长 ID。
   - 实施点（建议）：
     - 生成位置：`crates/gateway/src/chat.rs:4288`（`handle_run_failed_event` 拼接 Telegram 错误回执文本处）
     - 口径来源：`NormalizedError.stage/kind/action + details.timeout_secs`（脱敏）

2) **Web UI：把已有结构化字段“显式展示 + 一键复制”**
   - 至少展示：`stage` / `kind` / `action` / `retryable` / `timeout_secs`（如有）
   - 一键复制：`run_id`、`session_id`、以及一条“排障摘要 JSON”（脱敏）
   - 现状：UI 已展示 `stage/kind/action`，但缺少 `run_id` 以及“一键复制”。
   - 实施点（建议）：
     - WS 处理处把 `p.runId` 注入 error card（例如 `p.error.runId=p.runId` 或把整个 payload 传给渲染层）：`crates/gateway/src/assets/js/websocket.js:499`
     - error card 渲染增加 “Copy diagnostics” 按钮（内容含 `run_id/session_id/stage/kind/action/timeout_secs`）：`crates/gateway/src/assets/js/chat-ui.js:79`

3) **runner 重试可见性补齐**
   - 当前 UI 已广播 `state=retrying`，但丢失了原因字符串。
   - 建议：把 `RunnerEvent::RetryingAfterError(msg)` 的 `msg`（脱敏/截断）附带进 WS payload，并在日志里记录 `event=llm.retrying` + `run_id` + `provider/model`。
   - 实施点（建议）：
     - WS payload：`crates/gateway/src/chat.rs:4935`（当前丢弃了 msg）
     - 日志：`crates/gateway/src/chat.rs` 的 event_forwarder 内新增 1 条低噪声日志（按 `run_id` 去重）

4) **Telegram 出站（send/edit/chunk/stream）失败日志补齐关键字段**
   - 在 TelegramOutbound 内部记录失败（而不是只靠上层 warn）：
     - `op=send_message|edit_message_text|send_document|...`
     - `chat_id` / `reply_to` / `chunk_idx` / `chunk_count` / `text_len`
     - `error_class`（网络/429/403/其它）+ `error_redacted`
   - 对 `send_stream`：当 edit 失败累计达到阈值时打 1 条 warn（低噪声）。
   - 实施点（建议）：`crates/telegram/src/outbound.rs`
   - 补充（强关联，建议一起做）：gateway 在“发送失败”时补齐带 `run_id/trigger_id` 的结构化日志（因为 TelegramOutbound 层拿不到 run_id）：
     - `crates/gateway/src/chat.rs:7390`（`deliver_channel_replies_to_targets(...)` 内的错误分支）

5) **Telegram long-poll（getUpdates）降噪 + 强信号**
   - 连续失败时指数退避（带 jitter）+ 聚合日志（例如每 60s 打一条 `consecutive_failures`），并在恢复时打一条“恢复成功”日志。
   - 实施点（建议）：`crates/telegram/src/bot.rs:98`

#### 诊断码（code）口径冻结（建议）
> 目的：让 Telegram 群里看到错误时，不用看日志就能先判断“是哪一段坏了”。

- `code` 的来源：优先由 `NormalizedError.stage/kind/action` 组合得出（稳定、可测试）。
- 建议编码形式：
  - `gateway_timeout`
  - `provider_request/<kind>`（如 `provider_request/rate_limit`、`provider_request/network`）
  - `provider_stream/<kind>`
  - `channel_delivery/telegram_send_failed`
  - `cancelled`（用户主动 cancel 或 WS 断开导致的取消，需要进一步区分时可扩展）
- `code` 不得包含：provider API key、完整错误正文、用户消息正文。

#### 关联键（Correlation keys）冻结（建议）
- Web UI / 日志主键：`run_id`（必须可见、可复制）
- 次要关联键：
  - `trigger_id`（同一会话并发/队列场景下定位）
  - `session_key` / `chan_chat_key`
  - Telegram：`chan_account_key`、`chat_id`、`reply_to_message_id`、`sent_message_id`（如可得）

#### 中期增强（不影响短期 Quick Wins，但能显著提升排障质量）
- **provider failover 关联 run_id**：
  - 现状：`ProviderChain` 会在 failover 时 `warn!(provider=..., kind=...)`，但缺少 `run_id`（排障时仍需要靠时间线猜）。
  - 建议：将 `run_id` 作为可选字段加入 `LlmRequestContext`，并在 provider_chain 的 failover warn 中输出（脱敏），使“切 provider”可按 run_id 精准关联。
  - 影响面：agents model/context + provider_chain（需要评估兼容性）。
- **LLM 调用耗时日志**：
  - 现状：runner 会输出 tokens，但不输出“provider call duration_ms”（只有 metrics 时才有 histogram）。
  - 建议：在 gateway 侧围绕 `run_with_tools/run_streaming` 的 provider 调用输出一次 `duration_ms`（低噪声，按 run_id 一条）。

#### 行为规范（Normative Rules）
1) **统一 failure 结构化日志**
   - 触发点：`handle_run_failed_event(...)`、channel delivery 失败（Telegram outbound send/edit/chunk）、Telegram polling loop 的连续失败（限频）。
   - 字段最小集：
     - `run_id`、`trigger_id`、`session_id`、`chan_chat_key`（如有）
     - `failure_stage`、`error_kind`、`retryable`、`action`
     - `timeout_secs`/`elapsed_ms`（如有）
     - `provider`、`model`（如有）
     - `chat_id`、`chan_account_key`、`telegram_message_id`（如有）
2) **用户回执文案细分（中文/英文二选一需冻结）**
   - 由 `NormalizedError`（或等价结构）映射生成：
     - LLM 超时：明确显示 `LLM timeout (600s)` 或 `推理超时（600秒）`
     - 上游网络失败：明确显示 `Upstream network error` / `上游网络失败`
     - Telegram 发送失败：明确显示 `Telegram send failed` / `电报发送失败`
3) **流式 Telegram edit 失败的降级可观测性**
   - 当 `edit_message_text` 连续失败达到阈值（例如 3 次）时，记录 `reason code` 并降级为“停止 edit，最终 Done 时直接 send_message 补发一条完整消息”（是否做补发属于行为改变，需在本单冻结；若不做补发，至少要 log 告警）。
4) **重试/等待/Failover 必须可见**
   - runner 的 `RetryingAfterError` 事件必须落到日志（并带 `run_id`）；如 provider chain 发生切换，也必须记录一次（低噪声）。

#### 接口与数据结构（Contracts）
- 复用：`crates/gateway/src/run_failure.rs::NormalizedError`（stage/kind/action/retryable/details）
- 新增/补齐（如需要）：
  - channel delivery 错误的统一归一化（映射到 `FailureStage::ChannelDelivery` + `ErrorKind::Network`/`ProviderUnavailable` 等）

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 复现一次 `agent_timeout_secs` 超时：用户回执与日志都明确显示 “gateway_timeout + timeout_secs=…”（并可用 `run_id` 关联）。
- [ ] 复现一次上游 provider 网络错误（非 gateway timeout）：日志显示 `provider_request|provider_stream` 且带 `provider/model`，用户回执不再混同为“cancelled”。
- [ ] 复现一次 Telegram 出站发送失败：日志显示 `channel_delivery` + `telegram_send_failed`（或等价 reason code），并能关联到 `chan_account_key/chat_id`。
- [ ] runner 发生 retry/failover 时有 1 条结构化日志（低噪声，不刷屏）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `run_failure`：不同 raw_error 能稳定归类到 stage/kind/action（新增网络/超时样例）。
- [ ] Telegram outbound：对“chunk 中途失败/stream edit 失败”的降级路径打单测（用 mock bot 或封装层）。

### Integration
- [ ] 手工：断网/弱网下跑 1 次群聊回复，确认失败归类与字段齐全，且不泄露敏感信息。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先只做“观测性增强”（日志/回执更清晰），避免改变投递语义；若需要引入出站重试/补发，必须开 feature flag。
- 回滚策略：回滚日志/回执映射更改；若引入新行为则必须可关开关。

## 实施拆分（Implementation Outline）
- Step 1: 梳理并冻结“失败类型 → 用户回执文案”映射（中/英口径）。
- Step 2: `handle_run_failed_event` 统一补齐 structured fields（run_id/trigger_id/session_id/chan_chat_key/provider/model/timeout）。
- Step 3: Telegram outbound send/edit/chunk/stream 的失败日志补齐（reason code + 关键关联字段）。
- Step 4: runner retry/failover 的日志落地（低噪声，按 run_id 去重）。
- Step 5: 补齐单测 + 手工验收步骤（断网/弱网）。

## 未决问题（Open Questions）
- Q1: 用户回执文案用中文还是英文？是否需要双语（按 UI locale）？
- Q2: “观测性增强”是否允许轻微行为改变（例如 stream edit 失败后补发一条最终完整消息）？若不允许，则只做日志告警。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

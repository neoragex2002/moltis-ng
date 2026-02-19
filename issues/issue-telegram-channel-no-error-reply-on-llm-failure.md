# Issue: LLM 请求失败时 Telegram 会话无错误回执（且可能积压 reply targets 导致后续串线）

## 背景
当前 Telegram inbound 消息会被 gateway 作为 channel session 触发 `chat.send()` 异步执行，真正的 Telegram 回复通过 `deliver_channel_replies()` 在 LLM 成功完成后再发送回渠道。

在 OpenAI Responses 等 provider 调用失败/流式中断/返回错误时，Web UI 能收到 `state="error"` 的广播，但 Telegram 用户侧往往收不到任何提示，表现为“机器人无响应”。此外，channel reply target 队列在 error 分支没有被 drain，可能导致后续成功回复被发到旧的 Telegram message_id（串线/错回复风险）。

## 现象（Symptoms）
1) Telegram 侧无错误回执：
   - 触发后：Telegram 用户看见长时间 typing（如有），然后没有任何回复消息。
2) 可能的串线：
   - 同一 session 后续再次发送消息并成功返回时，成功回复可能会回到“之前那条失败消息”的 `reply_to`（或重复发送到多个历史 target）。

## 根因分析（Root Cause）
### A. Channel 层只处理“chat.send 立即失败”，不处理“run 内部失败”
`channel_events` 里会在 `chat.send(params).await` 返回 `Err` 时回 Telegram 发 `⚠️ {e}`。

但通常 `chat.send()` 会快速返回 `Ok({runId})` 并异步跑 agent loop；LLM 真正失败发生在 run 过程中（stream error / tool error / provider error），此时 channel_events 看不到 error，也不会发送 Telegram 错误提示。

### B. gateway 的 error 分支未调用 deliver_channel_replies（未 drain targets）
目前只有成功完成时才会调用 `deliver_channel_replies()`：

- tools 模式成功：`crates/gateway/src/chat.rs`（`deliver_channel_replies(...)` 在 final 分支）
- stream-only 模式成功：`crates/gateway/src/chat.rs`（`deliver_channel_replies(...)` 在 Done 分支）

错误分支仅做：
- 记录 run_error
- WebSocket broadcast `state="error"`
- 标记不支持模型等

**没有**向 Telegram 发送任何消息，且未 drain `channel_reply_queue`。

### C. reply targets 队列语义（drain on send）
`deliver_channel_replies()` 会调用 `state.drain_channel_replies(session_key)` 移除并返回所有 pending reply targets。
若 error 不 drain，targets 将残留在内存队列中，可能影响后续回复路由。

## 影响（Impact）
- Telegram 用户体验差：失败时看不到原因，误以为机器人宕机/无响应。
- 可靠性风险：targets 残留导致后续回复串线，出现“回复错消息/重复回复”的错误行为。
- 排障困难：只能看服务器日志才知道 provider 失败。

## 期望行为（Desired Behavior）
当某个 channel session 的 LLM run 在生成阶段失败时：
1) Telegram 必须收到一条错误回执（尽量简短、可读、无敏感信息）。
2) 该 session 的 pending reply targets 必须被及时 drain，避免后续串线。
3) Web UI 保持现有 error 广播行为不变。

## 方案（Proposed Fix）
### 方案 1（推荐）：在 run 的 error 分支也走一次 deliver_channel_replies
在以下错误分支中补充调用：

- `run_with_tools`：agent loop `Err(e)` 分支（broadcast error 后）
- `run_streaming`：`StreamEvent::Error(msg)` 分支（broadcast error 后）

行为：
- 构造一个“用户可见”的错误文本（`ReplyMedium::Text`），例如：
  - `⚠️ Request failed: <detail>`
  - `<detail>` 建议来自 `parse_chat_error()` 的 `detail` 字段（避免直接暴露原始堆栈/内部错误）。
- 调用 `deliver_channel_replies(state, session_key, &error_text, ReplyMedium::Text).await;`
  - 这会 drain targets，并把错误文本发回 Telegram（含 reply_to threading）。

优点：
- 最接近真实生命周期：错误发生在哪里就在哪里处理。
- 不需要在 channel_events 里做复杂的 run 状态订阅。
- 同时解决“无回执”和“targets 残留”两类问题。

注意：
- 如果 error 出现在“尚未 push reply target”之前（理论上可能），`deliver_channel_replies` 会发现 targets 为空并返回；不影响。

### 方案 2（备选）：出错时单独 drain + 显式 outbound 发送
在 error 分支里：
- 先 `state.drain_channel_replies(session_key)` 获取 targets
- 对 targets 逐个调用 outbound 发送错误

不推荐：重复逻辑（tts/logbook/suffix/threading）且易与 `deliver_channel_replies_to_targets` 变更脱节。

## 文案与安全（Error Message Policy）
建议对 Telegram 返回的信息：
- 只包含“用户可操作/可理解”的 detail。
- 不包含完整 HTTP body、token、路径等敏感信息。
- 可以提示重试/换模型（如果已有 failover/模型切换能力）。

## 验收标准（Acceptance Criteria）
- 人为制造 provider 错误（例如错误 API key / 断网 / gateway 返回 400）：
  - Telegram 收到错误回执消息（与触发消息 thread 对齐）。
  - Web UI 仍能看到 `state="error"`。
  - 随后再次发送正常消息成功时，不会发生“回复到旧消息/重复回复”的串线。

## 测试计划（Test Plan）
建议新增单测/集成测试：
1) 单测：模拟 `state.push_channel_reply` 后触发 run error 分支，断言 `state.peek_channel_replies(session_key)` 为空（targets 已 drain）。
2) 单测：用 mock outbound 捕获发送内容，断言错误文本发送一次且带正确 `reply_to`。

如果当前架构不便 mock outbound，可至少测试 drain 行为，另用手工验证覆盖发送行为。

## 相关位置（References）
- channel dispatch error only on immediate `chat.send` Err：`crates/gateway/src/channel_events.rs`
- deliver_channel_replies drains targets：`crates/gateway/src/chat.rs`（`deliver_channel_replies`）
- run_with_tools / run_streaming error branches broadcast but do not deliver to channel：`crates/gateway/src/chat.rs`


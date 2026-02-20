# Issue: Telegram 群聊多 Bot 场景下的 “自我点名语义” / `/cmd@bot` / Privacy Mode 口径不一致（LLM 输入需收敛）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-19；方案 A 落地）
- Priority: P1（群聊多 bot 时会出现“自相矛盾/搞笑回复”、/cmd@bot 行为不一致，且难排障）
- Components: telegram / gateway / agents prompt runtime / sessions / docs

**已实现（代码）**
- Telegram slash command 拦截支持 `/cmd@this_bot`（并校验目标一致性）：
  - Group/Channel：仅处理 addressed command（`/cmd@this_bot`）；`/cmd`（未定向）静默丢弃；`/cmd@other_bot` 静默丢弃
  - DM：`/cmd` 正常执行；`/cmd@other_bot` 静默丢弃
- self-mention stripping：LLM 看到的 user 文本会剥离对“当前 bot 自身”的 mention（优先 entities；无 entities 时使用更严格的 boundary-safe fallback）。
- addressed bot command suffix stripping：对形如 `/cmd@this_bot ...` 的 `BotCommand` entity，会剥离 `@this_bot` 后缀（保留 `/cmd`），避免把“点名噪声”写入 session/输入重建链路。
- self-mention-only 兜底：剥离后为空且无附件时，直接回固定短句 “我在。”（不进入 LLM）。
- identity injection（runtime context）：对 channel-bound session 注入 `channel/channel_account_id/channel_account_handle/channel_chat_id`，用于 system prompt `## Runtime`，帮助模型自洽与排障。

**覆盖测试（已跑通）**
- `cargo test -p moltis-telegram --lib`
- `cargo test -p moltis-agents --lib`
- `cargo test -p moltis-gateway --lib`
  - 关键用例位置（示例）：
    - self-mention stripping：`crates/telegram/src/handlers.rs:2950`
    - addressed command suffix stripping：`crates/telegram/src/handlers.rs:3675`
    - self-mention-only 兜底（“我在。”且不 dispatch）：`crates/telegram/src/handlers.rs:3012`
    - addressed slash command（DM 拦截）：`crates/telegram/src/handlers.rs:3253`
    - addressed command 判定（只认 `/cmd@this_bot`）：`crates/telegram/src/handlers.rs:3659`
    - reply-to-bot 唤醒：`crates/telegram/src/handlers.rs:3725`

**关键改动点（便于 code review）**
- `crates/telegram/src/handlers.rs`：`/cmd@bot` 解析与拦截、自我点名剥离、空文本兜底回复
- `crates/channels/src/plugin.rs`：`ChannelReplyTarget` 新增 `account_handle`（可选）
- `crates/gateway/src/chat.rs`：从 `session_entry.channel_binding` 解析并注入 runtime context
- `crates/agents/src/prompt.rs`：runtime `Host:` 行增补 channel identity 字段

---

## 决策（Decisions）【已冻结；变更需显式注明】
- ✅ self-mention（自我点名）场合采用 **方案 A（自我点名剥离 + identity injection）**，不采用仅注入身份的轻量方案作为最终形态。
- ✅ 群聊中（`mention_mode=mention`）**命令唤醒只认 addressed command**：`/cmd@this_bot`；不将 `/cmd`（不带 `@bot`）默认视为唤醒（避免刷屏/扩大响应面）。
- ✅ 非目标 bot 收到 `/cmd@bot1` 时：必须 `NotMentioned` 拒绝，不进入 LLM，不回复（最多留痕用于排障）。
- ✅ 群聊中收到未定向 `/cmd`（如 `/context`）时：**静默丢弃**（不提示、不回复、不进入 LLM），避免刷屏与节流状态机复杂化。
- ✅ self-mention 剥离后文本为空（用户只发 `@this_bot`）时：**回复固定短句**（不进入 LLM），避免用户误判 bot 离线。
- ✅ EditedMessage（编辑消息）策略：**完全忽略**编辑文本消息（保持现状：仅 live location 走 edited 更新）。编辑不触发 mention/command/LLM，也不额外提示；用户若要触发必须重新发送新消息。

## 待你确认（Open Questions）【避免遗漏；未确认前不实现“行为变化”】【已清空】
- 无（本单的边缘行为已按“最少魔法/最少状态”冻结）。

## 背景（Background）
在 Telegram 群聊中：
- 用户常用 `@bot` 来“点名唤醒”
- 也会用 `/command@bot` 做“定向命令”（群里多个 bot 时常见）

当前代码在门禁层已能做到“只处理被点名的 bot”（见 `issue-telegram-group-mention-gating-not-working.md` 已 DONE）。

但新的用户反馈表明：即使门禁正确，LLM 侧仍可能出现“语义误解”与“命令系统不一致”的问题：
- 用户发送 `@bot 在吗？` 时，bot 回复出现自相矛盾的模板化话术
- `/command@bot` 在 Telegram handler 层未被识别为 slash command，从而落入 LLM（不符合预期）
- BotFather Privacy Mode 的影响与排障口径在用户侧不清晰：用户容易把“没收到入站”与“LLM 无回复”混为一谈

本单目标：收敛 “Telegram 点名/命令 → LLM 输入” 的语义与规则，并补齐可观测性/验证方法。

## 已知复杂性（Known Complexities / Deferred）【先记录；不在本单展开】
- **Topics / message_thread_id**：Telegram supergroup 的 topic（话题）可能需要独立上下文；当前会话通常按 `chat_id` 绑定，topic 混合可能造成上下文串线。本单先不引入 topic 级分桶，统一后继考虑。
- **群聊上下文膨胀与 compaction**：群里同一会话消息量更大，容易触发 auto-compact 或手动 `/compact`。本单只规定“命令/点名语义”，不改变 compaction 策略（见下方“Compaction 规则”补充）。
- **Bot API 的“看不到其他 bot 消息”限制（已冻结为前置约束）**：
  - 即使 BotFather Privacy Mode=OFF，Telegram Bot API 也不会把“其他 bot 发送的消息”投递为 update。
  - 这意味着：如果希望 bot2 “知情 bot1 的最终回复正文/工具结果”，不能依赖 Telegram 平台旁听；必须在 Moltis 侧做“出站同步/共享 transcript”（另开需求时再做）。
  - 官方原文与参考：
    - “Bot admins and bots with privacy mode disabled will receive all messages except messages sent by other bots.”
    - “...bots will not be able to see messages from other bots regardless of mode.”
    - `https://core.telegram.org/bots/faq#what-messages-will-my-bot-get` / `https://core.telegram.org/bots/faq#why-doesnt-my-bot-see-messages-from-other-bots`

## 实施基础（Prerequisites / Readiness）【是否具备 fix 条件】
本单所需信息与能力在当前代码库中已具备（仅涉及补齐少量 plumbing/解析逻辑）：
- **可判定 self-mention / addressed command**：Telegram message 自带 entities/caption_entities；本仓库已在门禁层实现 entities 优先判定（可复用同一解析能力）。
- **可稳定识别 reply-to-bot**：运行态已缓存 bot `user_id`（reply-to 判定基础）。
- **可在 Telegram handler 层拦截 slash command**：现有 `/context` `/compact` 等命令已在 handler 层拦截；仅需补齐 `/cmd@bot` 的解析与“目标一致性校验”。
- **可在 gateway/agents 系统 prompt 注入 runtime**：prompt builder 支持 runtime context（`## Runtime` 段）；只需补齐注入字段与来源（至少 `channel=telegram` + `chat_scope_key`，以及可选的 `bot_handle`）。
- **auto-compact 机制已存在**：网关在发送前有 proactive auto-compact 预检与执行路径；本单只要求渠道体验口径收敛，不要求新增 compaction 算法。

## 概念与口径（Glossary & Semantics）
- **自我点名（self-mention）**：消息文本包含对“当前 bot 自身”的提及（例如 bot handle 为 `@fluffy_tomato_bot`，用户发 `@fluffy_tomato_bot 在吗`）。
- **第三方提及（third-party mention）**：消息里提及了“别的 bot / 别的账号”（例如 `@other_bot`）。
- **addressed command（定向命令）**：`/context@MyBot` 这类写法，语义是“命令发给指定 bot”。
- **Privacy Mode（BotFather 隐私模式）**：平台侧投递开关，决定群聊中哪些消息会被 Telegram 投递给 bot（Receive 层）。
- **四段链路定位法（必须区分）**：
  1) Receive：是否出现 `telegram inbound message received ...`
  2) Gate：是否出现 `handler: access denied reason=...`（或 “access granted”）
  3) LLM：是否出现 `moltis_gateway::chat: chat.send ...` 以及后续 `agent run complete ...`
  4) Outbound：是否出现 `telegram reply delivery starting ...` / `outbound ... sent ...`

### `/cmd@bot1` 对其他 bot 的影响（必须按分层口径说清楚）
以群聊里用户发送 `/context@bot1` 为例（bot1=目标 bot，bot2=其它 bot）：
- Receive（平台投递，代码不可控）：
  - bot2 **是否会收到**该条消息取决于 Telegram/BotFather 的 Privacy Mode、bot 权限/管理员、以及平台的投递策略；
  - 本仓库只能对“收到之后怎么处理”给出确定性规则。
- Gate（本地门禁，代码可控，必须确定）：
  - **bot1**：应判定为“被唤醒”（addressed command 指向自己）→ 放行
  - **bot2**：应判定为“未被唤醒”（addressed command 指向 bot1 不是 bot2）→ `NotMentioned` 拒绝
- LLM（是否进入模型，代码可控，必须确定）：
  - **bot1**：`/context@bot1` 应当被识别为 slash command 并在 handler 层拦截执行（不进入 LLM）
  - **bot2**：不应进入 LLM（因为 Gate 已拒绝；即使未来某处调整 Gate，也必须保证“非目标 bot 不会把 `/cmd@bot1` 当作自己的命令来执行”）
- Outbound（是否对群发消息，代码可控，必须确定）：
  - **bot1**：按命令结果回复
  - **bot2**：不得回复（避免抢答/刷屏）；最多写入服务端日志/留痕用于排障

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 本节把“点名/命令 → LLM 输入”规则一次性写死，避免实现时各处各自推断。

### 群聊（Group/Channel）在 `mention_mode=mention` 下
**唤醒条件（任一满足即可处理）：**
1) `@this_bot`（self-mention）：case-insensitive；优先 entities/caption_entities
2) `/cmd@this_bot`（addressed command）：case-insensitive；命令解析必须保留 args（如 `/model@this_bot gpt-5.2`）
3) reply-to-bot：回复 bot 的消息视为唤醒（需通过 bot user_id 稳定判定）

**非唤醒（必须拒绝）：**
- `@other_bot`、`/cmd@other_bot`：即使 Telegram 平台把消息也投递给本 bot，本 bot 也必须拒绝且不回复
- `/cmd`（不带 `@bot`）：默认不视为唤醒（避免扩大响应面）；按决策静默丢弃（不提示）

### DM（1对1私聊）
- DM 不走 `mention_mode` 门禁（现有系统语义）；`/cmd`（不带 `@bot`）应当正常工作。
- DM 中出现 `@this_bot` 时，也可应用“self-mention 剥离”（减少点名噪声），但这属于体验优化，不作为强制门禁条件。

## 群聊上下文长度与 Compaction（补充规则）【最少魔法/最少状态】
> 目的：解释“群 session 太长需要 compact 时怎么办”，并冻结最简单一致的行为，不把 Telegram 的编辑/删除复杂性引入会话系统。

### 关键事实（避免误解）
- **默认模式（现状）**：只有 **被放行并进入处理链路** 的 bot 会把消息写入该会话的 LLM history；未被点名而被 `NotMentioned` 拒绝的 bot 不会增长其对话上下文。
- **旁听模式（规划）**：若未来启用 `group_ingest_mode=all_messages`（见 `issues/done/issue-telegram-group-ingest-reply-decoupling.md`），则即便 Gate 仍是 `NotMentioned`，该消息也可能被 **ingest-only 写入 session**（但仍不得进入 LLM、不得 outbound 回复）。
- 因此：在同一个群里，即使 Telegram 平台把消息投递给多个 bot，是否会“增长某个 bot 的上下文/触发 compaction”取决于该 bot 的 `group_ingest_mode`（默认不会，旁听会）。
- auto-compact 是对“即将发出的 LLM 请求”的内部预处理（预算触发后会先总结/压缩历史再继续）。

### 冻结行为（群聊场景）
- **auto-compact：静默执行**（不在 Telegram 群里额外提示“正在压缩/已总结”）。
- **compact 失败/仍超限：返回产品化短错误**（而不是底层堆栈）：
  - 建议文案：`上下文太长，自动压缩失败。请用 /new@<bot> 开新会话，或缩短/拆分你的消息。`
- **手动兜底命令（群聊必须定向）**：
  - `/compact@this_bot`：手动触发 compact（不进入 LLM 对话；走 command）
  - `/new@this_bot`：开新会话（最干净的兜底）
  - `/clear@this_bot`：清空历史（破坏性更强，按现有权限策略执行）

> 注：本单不要求实现新的 compaction 逻辑，仅要求 Telegram 渠道在群聊中对定向命令与错误文案保持一致且不刷屏。

### 验收补充（Acceptance Addendum）
- [x] 默认模式（未启用旁听写入）下：在同一群聊中，多 bot 同时收到消息时，只有被放行的 bot 会把该消息写入会话历史并可能触发 compaction；被 `NotMentioned` 拒绝的 bot 不写入历史、不触发 compaction（只留痕/日志）。
  - 旁听写入（规划）：启用 `group_ingest_mode=all_messages` 后，`NotMentioned` 也可能 ingest-only 写入 session（但仍不得进入 LLM、不得 outbound），见：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`。

## 问题陈述（Problem Statement）
### 现象 1：自我点名导致 LLM 误解（搞笑/自相矛盾）
用户在群里发：`@fluffy_tomato_bot 在吗？`
bot 回复为：
- “我在。”
- “但我无法查看 Telegram 里其他账号/机器人的在线状态…所以没法替你确认 `@fluffy_tomato_bot` 在不在…”

该回复在用户体验上是矛盾的：用户点名问的是“你在不在”，模型却把 `@fluffy_tomato_bot` 当作第三方对象。

### 现象 2：`/command@bot` 未被 Telegram handler 当成 slash command 处理
当前 Telegram handler 的命令拦截逻辑仅识别 `cmd` 为 `context/new/...` 等固定字符串。
如果用户发 `/context@MyBot`，则 `cmd` 会变成 `context@MyBot`，无法命中拦截分支，从而落入 LLM。
这与 “群聊定向命令应直接走 command 分支” 的预期不一致。

### 现象 3：Privacy Mode 口径不清，用户把“没收到入站”误判为“LLM 没回”
用户反馈中出现了 “Privacy Mode ON 时 @bot 是否仍能收到？”、“为什么感觉 bot 收不到 LLM 消息？” 等疑问。
这通常是因为没有用一致的日志/分层口径定位到底卡在 Receive / Gate / LLM / Outbound 哪一段。

## 现状证据（Evidence）
用户日志（2026-02-19）显示：
- 同一条群消息被 Telegram 投递给两个 bot（Receive 层同时出现两个 `inbound message received`，account_id 分别为 `fluffy`、`lovely`）。
- `lovely` 被门禁拒绝（`reason=bot was not mentioned`），`fluffy` 放行并进入 LLM（出现 `chat.send ... user_message=@fluffy_tomato_bot 在不在`）。

可确定结论（不依赖猜测）：
- Telegram 平台 **确实**向多个 bot 投递了该条群消息（至少在该例中）。
- Moltis 门禁层对 `lovely` 的拒绝是正确的（未被点名）。
- `fluffy` 的 LLM 输入确实包含 `@fluffy_tomato_bot ...` 原文（导致后续语义误解风险）。

## 根因分析（Root Cause）
至少存在两类根因：
1) **LLM 输入缺少“bot 自身身份”**：system/runtime 未明确告诉模型“你就是 @xxx”，模型容易把 `@xxx` 当第三方对象并输出模板化免责声明。
2) **LLM 输入未区分 mention 的两种语义**：
   - self-mention（点名我）应当被视为“唤醒噪声”，不应干扰用户语义
   - third-party mention（提到别人）才可能需要“无法确认对方在线”等表述
3) **命令系统不一致**：Telegram handler 的 slash command 拦截未兼容 `/cmd@bot`，导致命令落入 LLM。
4) **Privacy Mode 排障口径缺失**：缺少统一的“按日志分段定位”的指导与 debug 输出。
5) **EditedMessage 处理范围过窄**：当前 Telegram polling 对 `EditedMessage` 仅用于 live location 更新（`handle_edited_location`）。若用户“编辑消息补充 @mention / /cmd@bot”，本仓库不会按普通消息链路处理，容易造成“我明明编辑后点名了 bot 但 bot 没反应”的困惑。

## 需求与目标（Requirements & Goals）
- [x] self-mention 文本（`@this_bot ...`）进入 LLM 前应被正确解释为“点名我”，避免第三方免责声明。
- [x] 群聊 `mention_mode=mention` 下：
  - `/cmd@this_bot` 应当作为 slash command 被 handler 拦截并执行（不进入 LLM）。
  - `/cmd`（不带 `@bot`）在群聊中不应默认视为唤醒（避免刷屏），除非未来引入更细策略。
  - `/cmd@other_bot`：本 bot 必须拒绝且不响应（避免多 bot 抢答），即使 Telegram 平台把该消息也投递给了本 bot。
- [x] 提供确定性的排障口径：能从日志一眼定位卡在 Receive/Gate/LLM/Outbound 哪段。
- [x] 可观测性：debug 面板 / /context / 日志能明确显示 bot identity（channel_account_id + bot handle）与输入重写策略是否启用。
- [x] 明确 EditedMessage 的产品策略（至少文档化）：
  - 方案 1：仅支持 live location edited updates（保持现状）
  - 方案 2：支持 edited text 的 mention/command（需要设计去重与“是否应响应编辑”的规则）

## 方案（Proposed Solutions）
### 方案 A（已选定：自我点名剥离 + identity injection）
#### A1) 自我点名剥离（self-mention stripping）
当消息中包含对“当前 bot 自身”的 mention（优先用 entities/caption_entities 精确定位）：
- 在发送给 LLM 的 user 文本中剥离该 mention 片段（只剥离“指向自己”的那段，绝不误删指向他人的 mention）
- 做空白规范化（避免遗留多余空格）
边界：
- 若剥离后文本为空（用户只发 `@this_bot`），直接回复固定短句（不进入 LLM）。

#### A2) 注入 bot 身份（identity injection）
在 system prompt/runtime context 中注入：
- `channel=telegram`
- `channel_account_id=<account_id>`（配置标识）
- `bot_handle=@<bot_username>`（Telegram username）
- `chat_id=<chat_id>`
目标：模型明确知道“自己是谁”，不会把 `@this_bot` 当作第三方对象。

#### A3) addressed command 一致性
在 Telegram handler 的 slash command 解析里支持：
- `/context@this_bot` 解析为 `context` 并执行 command（并校验 `@this_bot` 的目标一致性）
 - `/context@other_bot`：本 bot 不应处理（避免抢答）；若收到该消息，必须静默丢弃（或仅留痕）而不回复
 - `/context`（不带 `@bot`）：群聊默认不视为唤醒；已决策静默丢弃（不提示），不执行命令、不进入 LLM

优点：
- LLM 看到的输入是“用户真正想表达的内容”（去掉点名噪声）
- 规则与 Telegram 平台语义一致、可测、可解释

风险/成本：
- 需要正确实现基于 entities 的“精确删除”，并覆盖 caption_entities
- 需要明确空文本的兜底行为

### 方案 B（不采纳）
仅注入身份不剥离 mention 作为最终方案不采纳（原因：仍较易触发模板化“第三方免责声明”与多 bot 语义混乱）。

## 验收标准（Acceptance Criteria）
- [x] 群聊中 `@this_bot 在吗？` 的回复不再出现“无法确认 @this_bot 是否在场”的第三方免责声明。
- [x] 群聊中 `/context@this_bot` 走 command 分支（不进入 LLM），且能稳定返回 context 卡片。
- [x] 群聊中 `/context@other_bot` 本 bot 不处理（不抢答）。
- [x] “Receive/Gate/LLM/Outbound” 四段定位说明已在文档冻结，且日志字段可对齐（无需猜测）。
- [x] 行为矩阵覆盖：`@this_bot` / `@other_bot` / `/cmd@this_bot` / `/cmd@other_bot` / reply-to-bot / `/cmd`（不带@）在群聊均有明确结果（Receive/Gate/LLM/Outbound）。

## 测试计划（Test Plan）
### Unit
- [x] mention stripping：仅剥离 self-mention，不影响 third-party mention（覆盖：`crates/telegram/src/handlers.rs:2843`）
- [x] `/cmd@bot` 解析：self vs other（覆盖：`crates/telegram/src/handlers.rs:3413`）
- [x] 空文本兜底行为（覆盖：`crates/telegram/src/handlers.rs:2905`）
- [x] 群聊未定向 `/cmd` 不视为唤醒（覆盖：`crates/telegram/src/handlers.rs:3413`）
（非阻塞）EditedMessage 文本忽略：策略已冻结；目前未单测（行为约定，不影响主链路）

### Integration（可选）
（可选）构造 Telegram Message（含 entities/caption_entities）走 handler 命令分支，断言不进入 LLM dispatch
（可选）（若支持 EditedMessage）构造 edited message 场景，验证是否响应/是否去重符合策略

## 行为矩阵（Behavior Matrix）【用于验收与排障；Receive 取决于平台投递】
> 说明：Receive（是否收到 update）由 Telegram/Privacy Mode 等平台策略决定；本矩阵冻结的是 Gate/LLM/Outbound（代码应当保证的行为）。
>
> 符号：
> - Gate：✅=放行处理 / ❌=拒绝（NotMentioned 等）
> - LLM：✅=进入 LLM / ❌=不进入 LLM（走 command 或静默）
> - Outbound：✅=可能回复 / ❌=不回复
>
> 场景假设：同一个群里有 bot1（this_bot）与 bot2（other bot），且两者都可能收到同一条 update（Privacy OFF 时常见）。

| User text (Group) | bot1 Gate | bot1 LLM | bot1 Outbound | bot2 Gate | bot2 LLM | bot2 Outbound | Notes |
|---|---:|---:|---:|---:|---:|---:|---|
| `@bot1 hi` | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | bot1 LLM 输入需剥离 `@bot1`（方案 A） |
| `@bot2 hi` | ❌ | ❌ | ❌ | ✅ | ✅ | ✅ | bot1 不抢答 |
| `@bot1 @bot2 hi` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 两边各自剥离自己的 self-mention |
| `/context@bot1` | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ | bot1 走 command 拦截；bot2 必须拒绝 |
| `/context@bot2` | ❌ | ❌ | ❌ | ✅ | ❌ | ✅ | bot2 走 command；bot1 必须拒绝 |
| `/context` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | 群聊未定向命令：静默丢弃 |
| reply-to-bot1 `"ok"` | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | reply-to-bot 作为唤醒（需要 bot user_id） |
| **edited**: `@bot1 hi` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | Edited text 忽略；若要触发请重新发送新消息 |

## 交叉引用（Cross References）
- `issues/done/issue-telegram-group-mention-gating-not-working.md`（门禁唤醒判定已收敛；本单关注 LLM 输入语义与命令一致性）
- `issues/issue-terminology-and-concept-convergence.md`（identity 字段命名/口径需要收敛）

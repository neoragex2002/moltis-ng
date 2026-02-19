# Issue: Telegram 群聊 `mention_mode=mention` 失效（@mention 误判 / false negative / 导致不响应）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-19）
- Priority: P1（群聊场景直接“无回复”，影响可用性）
- Components: telegram / channels gating / access control
- Affected channels: Telegram（Group / Channel）

**已实现**
- 群聊 `mention_mode=mention` 下，唤醒判定已收敛（实体优先、substring 仅作为 fallback）：
  - `@bot_username`：case-insensitive，优先使用 entities/caption_entities
  - `caption` 中的 `@bot_username`：支持 caption_entities
  - `/command@bot_username`：仅识别“定向命令”（避免把 `/command` 误当作群聊唤醒）
  - reply-to-bot：回复 bot 的消息视为唤醒（通过 bot user_id 稳定判定）
- `NotMentioned` 拒绝日志增强：在不泄露正文的前提下输出必要定位字段（chat/message id、entities count、reply 信息等）。
- 说明：本单只保证“门禁不误拒绝”（Gate/Process 放行）。`/command@bot_username` 的 **slash command 拦截与执行一致性**（例如 `/context@bot` 应走 command 分支而非进入 LLM）已在单独 issue 落地并 DONE：
  - `issues/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
 - 备注：群聊会话更容易触发 auto-compact；compaction 的渠道体验规范（静默 auto-compact、定向命令 `/compact@bot` 等）也在上述 issue 中补充冻结。

**已覆盖测试**
- 已有 access 逻辑单测（不含 “@mention 检测” 本身）：`crates/telegram/src/access.rs`
- 新增 mention 检测单测（覆盖大小写、entity mention、addressed command、TextMention、reply-to-bot、substring fallback 边界）：
  - `crates/telegram/src/handlers.rs`（tests 模块）
    - 典型用例位置（示例）：`crates/telegram/src/handlers.rs:3659`

---

## 背景（Background）
在 Telegram 群/频道中，为避免刷屏，Moltis 提供 `mention_mode` 门禁：
- `mention`：必须 @mention bot 才响应
- `always`：响应所有消息
- `none`：群里不响应

用户反馈：即使在群里明确 `@bot_username`，Moltis 也不响应，表现为“@机制不起作用”。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **mention_mode**：群聊消息是否需要被 @mention 才处理的策略（配置层）。
- **bot_mentioned**：telegram handler 推导出的布尔值，表示本条消息是否“算作提及 bot”。
- **false negative**：用户实际提及了 bot，但 `bot_mentioned=false`，导致访问控制拒绝（NotMentioned）并静默丢弃消息。
- **DM vs Group/Channel**：本单重点讨论 Group/Channel 的“唤醒门禁”；DM 不走 mention 门禁（因此 DM 中 `/command` 不应要求带 `@bot`）。
- **DM（Direct Message）**：私信/一对一私聊（Telegram `Private` chat）。本仓库语义：DM 不走 `mention_mode` 门禁。
- **/command（slash command）**：以 `/` 开头的 bot 命令（如 `/context` / `/help`），用于在 Telegram 客户端触发 bot 功能；命令本身是一段消息文本，Telegram 同时会提供结构化的实体（`BotCommand` entity）。
- **addressed command（定向命令）**：群聊中可写作 `/context@MyBot`，表示该命令“定向”给特定 bot（尤其在群里有多个 bot 时）。是否“算唤醒”必须冻结规则，否则会扩大响应面。
- **entity（实体）**：Telegram 会在 message 中提供 entities（正文）/caption_entities（媒体 caption）来标注 `@mention`、`/command` 等结构化片段；应优先依据实体而非脆弱的 substring。
- **“知道/可见性”的三层口径（必须区分）**：
  1) **收到（Receive）**：Telegram 平台是否把该消息 Update 投递给 bot（受 BotFather Privacy Mode 等平台开关影响）。
  2) **处理（Process）**：Moltis 本地是否放行并进入 LLM/会话上下文/产生回复（受 `mention_mode` 等本地门禁影响）。
  3) **留痕（Persist/Observe）**：即使未处理，服务端是否仍记录/展示该消息（例如 message_log / UI 事件）。
- **重要平台约束（必须牢记）**：Telegram Bot API 不会把“其他 bot 发送的消息”投递为 update（与 Privacy Mode 是否 OFF 无关）。因此“bot 旁听群聊”只能旁听到它实际收到的 updates（通常是人类发言），无法自动旁听其他 bot 的最终回复正文。
  - 官方原文与参考：
    - “Bot admins and bots with privacy mode disabled will receive all messages except messages sent by other bots.”
    - “...bots will not be able to see messages from other bots regardless of mode.”
    - `https://core.telegram.org/bots/faq#what-messages-will-my-bot-get` / `https://core.telegram.org/bots/faq#why-doesnt-my-bot-see-messages-from-other-bots`

补充：本单只修复 **Gate/唤醒判定**。是否“旁听写入 session 历史”（Ingest）属于另一个维度，见：
- `issues/issue-telegram-group-ingest-reply-decoupling.md`

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 当用户在群里 `@bot_username` 时，`mention_mode=mention` 必须放行并触发响应（不再误判）。
- [x] 明确并实现“哪些情况也算唤醒”（规范化）：至少包含
  - [x] 显式文本 `@bot_username`（大小写不敏感）
  - [x] 媒体 caption 中的显式 `@bot_username`（caption mention，大小写不敏感）
  - [x] 回复 bot 的消息（reply_to bot，稳定判定为本 bot）
  - [x] `/command@bot_username`（addressed bot command，仅识别定向命令）
- [x] 当消息被拒绝时，日志必须能解释“为什么”（不泄露用户消息正文）。

### 非功能目标（Non-functional）
- 不得扩大响应面：`mention_mode=mention` 不得退化为 `always`（除非明确配置）。
- 兼容 Telegram 隐私模式（BotFather privacy mode）：在隐私模式下，bot 可能只收到“mention/command/reply”等消息，但本地门禁也必须识别这些唤醒方式，避免“收到却拒绝”。
- 安全隐私：拒绝日志不得记录完整消息文本；可记录实体类型、是否包含 `@`、以及 bot username 等低敏信息。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 群聊里发送 `@bot_username 你好`，bot 无响应。
2) server 日志可见 “access denied: bot was not mentioned”（或同语义），即本地门禁误判。

### 影响（Impact）
- 用户体验：群聊基本不可用（必须 @ 才响应但 @ 又不生效）。
- 排障成本：用户难以区分是 Telegram 平台未投递，还是 Moltis 本地门禁拒绝。

### 复现步骤（Reproduction）
1. 将 bot 加入一个 Telegram 群。
2. 将该 bot 的 `mention_mode` 配置为 `mention`（默认即 `mention`）。
3. 在群里发送：`@<bot_username> ping`
4. 期望：bot 响应。
5. 实际：bot 无响应；日志可能出现 `handler: access denied reason="bot was not mentioned"`。

## 现状核查与证据（As-is / Evidence）【不可省略】
- mention 门禁配置与枚举：
  - `crates/channels/src/gating.rs:52`（`MentionMode::{Mention,Always,None}`）
  - `crates/telegram/src/config.rs:33`（`TelegramAccountConfig.mention_mode` 默认 `Mention`）
- 门禁执行点：
  - `crates/telegram/src/access.rs:72`（群聊下 `MentionMode::Mention` → `bot_mentioned` 为 true 才放行）
- `bot_mentioned` 计算方式：
  - Before（bug）：基于 substring `contains("@{username}")`，大小写敏感，且不解析 entities / 不考虑 reply-to-bot / 不识别 `/command@bot`。
  - After（fixed 2026-02-19）：实体优先（entities/caption_entities），并支持 reply-to-bot 与 `/command@bot`；substring fallback 具备边界判断且仅在 entities 为空时启用。
  - 代码：
    - `crates/telegram/src/handlers.rs`：`check_bot_mentioned()` / `entities_trigger_wakeup()` / `fallback_contains_at_username()`
    - `crates/telegram/src/state.rs` / `crates/telegram/src/bot.rs`：缓存 bot user_id（reply-to-bot 判定基础）
- 被拒绝的日志证据：
  - `crates/telegram/src/handlers.rs:224`：`warn!(... %reason ... "handler: access denied")`（reason 可能为 `NotMentioned`）

## Telegram 机制说明（/command、@mention、以及“上下文如何确定”）【补齐系统语义】
### 1) `/xxx`（slash command）到底是什么？怎么用？
- `/xxx` 只是“消息文本的一种形态”：用户发送一条以 `/` 开头的文本（例如 `/context`）。
- Telegram 客户端会配合 bot 的 `setMyCommands`（本仓库在启动时注册了 `/new` `/sessions` `/model` `/sandbox` `/clear` `/compact` `/context` `/help`）提供命令补全。
- 从 bot 侧看：你会收到一条普通消息；但 Telegram 也会把命令片段标记为结构化实体：`MessageEntityKind::BotCommand`。

### 2) `/cmd` 是什么意思？跟“命令系统”是什么关系？
- 本仓库里 `cmd` 是泛指：比如 `/context` 中的 `context`、`/help` 中的 `help`。
- 用户口中的 “/cmd” 通常是指 “随便举例的某个命令”。在实现规则里应当写具体：例如 `/context`、`/help`、`/compact` 等。

### 3) 为什么群聊里要写成 `/context@bot_username`？
- Telegram 支持 “定向命令”（addressed command）：在群里可写 `/context@MyBot`，表示该命令是发给 `@MyBot` 的。
- 典型用途：一个群里多个 bot 都支持 `/help` 时，用 `/help@MyBot` 来明确目标 bot。
- 风险：如果本地门禁把所有 `/context`（不带 @）都当作唤醒，可能导致 `mention_mode=mention` 变相退化为 `always`（扩大响应面）。
- 因此建议在本单 Spec 明确：群聊的 `mention_mode=mention` 下，**只把 `/command@bot_username` 视为唤醒**（而不是所有 `/command`）。

### 4) `@mention` 在 Telegram 消息里“长什么样”？为什么 substring 会误判？
- Telegram 会把 `@MyBot` 这段标为 `MessageEntityKind::Mention`（或在某些场景为 `TextMention` 等形式），同时提供 offset/length。
- 直接 `text.contains("@MyBot")` 的问题：
  - **大小写敏感**：`@mybot` vs `@MyBot`
  - **误放行**：`@MyBot123` 也会 `contains("@MyBot")`
  - **caption 场景不稳**：媒体 caption 与正文 entities 不同，需要分别看 `caption_entities`
  - **语义缺失**：无法利用 Telegram 已提供的“实体化语义”，也不容易做可靠边界判断
- 结论：应优先使用 entities/caption_entities（结构化）判定唤醒。

### 5) “mention 时给 bot 的上下文如何确定？”
- Telegram 平台层面：`@mention` 只影响“这条消息是否对 bot 可见/是否应被用户理解为指向 bot”，并不携带“会话上下文”的额外结构。
- Moltis 业务层面：会话上下文由 **channel/session key** 决定（通常对 Telegram 是“某个 bot account + 某个 chat_id（群/私聊）”对应一条会话）。
  - 因此：`@mention` 只应该影响 **门禁是否放行**（bot 是否处理本条消息），不应该影响 “采用哪个 session 的历史上下文”。
  - 在群聊中：通常是“群级上下文”（同一个群的消息共享同一个会话历史），不是“每个用户各一份上下文”。
  - 同一个群里的不同 bot（不同 account_id）通常是 **不同 session**，上下文互不共享（分桶粒度可理解为 `(bot_account, chat_id)`）。

### 6) “只 @bot1，bot2 是否会知道？”——必须按三层口径回答
- **收到（Receive）**：由 Telegram/BotFather 决定（Privacy Mode 等），代码库无法强制控制。
- **处理（Process）**：由 Moltis 配置与门禁决定（本单修复范围）。
- **留痕（Persist/Observe）**：由服务端是否记录/展示未处理消息决定；本仓库当前路径下默认启用了 message_log，并且 handler 会在 access deny 之前写入 log（意味着“收到了但未处理”的消息仍可被服务端追溯）。

## 根因分析（Root Cause）
主要风险点（可同时存在）：
- A) **大小写敏感**：Telegram username 比较应视为 case-insensitive；用户输入/客户端补全可能与 `get_me().username` 的大小写不一致。
- B) **未解析 entities**：Telegram message 的 mention 可能以 `MessageEntity` 表达；纯 `contains()` 既脆弱也难以区分“@在 code block/引用里”等情况。
- C) **reply/command 不算唤醒**：在 Telegram 隐私模式下，bot 可能会收到“reply to bot”或“/cmd@bot”的消息，但本地门禁仍以 `@username` 子串判断，导致“收到了却拒绝”。
- D) **bot_username 缺失/异常**：若 `get_me().username` 为 None（理论上少见）或带前缀 `@`（不应发生但可被其它路径污染），则 `contains("@{username}")` 会恒 false。
- E) **substring 误放行**：`contains("@bot")` 会把 `@bot123` 判成提及；导致门禁扩大响应面（安全/刷屏风险）。
- F) **“知道”口径混乱**：没有区分“Telegram 是否投递给 bot”与“Moltis 是否处理/回复/进入会话上下文”，导致排障时误把平台投递问题当作本地门禁问题（反之亦然）。
- G) **未唤醒消息仍可留痕**：当前 handler 在 access deny 之前写 message_log（若启用），导致“未处理但服务端可见/可追溯”，容易与“bot 不知道”混淆。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
### 冻结规则（必须明确区分 DM vs Group/Channel）
#### DM（1对1私聊）
- DM 不走 `mention_mode` 门禁（现有系统语义）。
- DM 中 `/command`（不带 `@bot`）应当正常生效（例如 `/context`），不应强制用户写成 `/context@bot`。

#### Group/Channel（群/频道）
当 `mention_mode=mention` 时，本单冻结为：
- **@mention 唤醒**：识别 `@bot_username`（大小写不敏感），优先 entities/caption_entities，substring 仅作为 fallback。
- **/command 唤醒**：仅识别 **addressed command** `/command@bot_username` 作为唤醒（大小写不敏感）。
  - 不将未定向的 `/command`（不带 `@bot`）视为唤醒，避免扩大响应面导致刷屏（除非未来引入更细策略开关）。
- **reply-to-bot（可选增强）**：若纳入 Spec，应通过 bot user_id 稳定判定“回复对象为本 bot”，不能仅凭文本猜测。

### 其他必须/不得
- 必须：拒绝原因 `NotMentioned` 的日志/调试信息应可定位（不记录消息正文）。
- 不得：不得让 `mention_mode=mention` 退化为 `always`（扩大响应面）。

## 实施基础（Prerequisites / Readiness）
- entities/caption_entities：teloxide 已提供可直接使用的解析接口（`Message::parse_entities()` / `Message::parse_caption_entities()`），无需自行处理 UTF-16 offset。
- reply-to-bot（已实现）：运行态已缓存 bot 的 `user_id`（来自启动时 `get_me()`），用于稳定判断“回复对象为本 bot”。
- Telegram 平台投递开关（BotFather Privacy Mode）：属于平台侧配置，本代码库不具备直接读写/控制的基础；本单只保证“只要 bot 收到了消息，本地唤醒判定不误拒绝”。
- 留痕策略：本仓库当前路径下默认启用了 message_log；并且 handler 在 access deny 之前写入 log。若产品目标是“未唤醒消息对 bot 完全不可见且不落库”，建议另开子 issue 做策略开关/行为调整（不建议与本单强耦合）。

## 方案（Proposed Solution）
> 本单已 DONE；下述 Options 保留作为“为何如此实现”的历史说明。

### 方案对比（Options）
#### 方案 1（快速修复：大小写不敏感 + 更稳的字符串匹配）
- 做法：
  - `check_bot_mentioned()` 改为 case-insensitive（lowercase 比较）。
  - 兼容 `bot_username` 可能带 `@` 的情况（normalize）。
- 优点：改动小、立竿见影。
- 风险/缺点：仍然不解析 entities；reply/command 场景可能仍失败。

#### 方案 2（推荐：基于 Telegram entities 的唤醒判定 + 明确 reply/command 规则）
- 做法：
  - 从 `MessageKind::Common` 中读取 `entities`（文本）与 `caption_entities`（媒体 caption）。
  - 规则优先级：
    1) entities/caption_entities 中存在 `Mention("@xxx")` 且 `xxx == bot_username`（case-insensitive）→ true
    2) entities/caption_entities 中存在 `BotCommand`，且命令形如 `/cmd@bot_username`（case-insensitive）→ true（可选；推荐“仅 addressed command 视为唤醒”）
    3) `reply_to_message` 的发送者是 bot（需要在 AccountState 额外缓存 bot user_id，启动时 `get_me().id`）→ true（可选）
    4) fallback：对 message text 做 case-insensitive 的 `@bot_username` substring（兼容）
- 优点：更符合 Telegram 平台语义；减少误判与回归；对隐私模式更稳。
- 风险/缺点：实现与测试略多；需要确认 teloxide 的 entity 类型/字段在当前版本可用。

### 最终方案（Chosen Approach）
已落地 **方案 2**（entities 优先 + 明确 reply/command 规则），并保留 boundary-safe 的 substring fallback（兼容无 entities 的异常消息）。

#### 可观测性增强（建议）
- 当 access denied 原因为 `NotMentioned` 时，在 warn/debug 日志增加：
  - bot_username（规范化后）
  - `has_entities` / entity kinds 列表（不含正文）
  - `text_has_at_sign`（bool）
  - `chat_id` / `message_id` / `chat_type`

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 群聊 `mention_mode=mention` 下，`@bot_username`（含大小写变化）能稳定触发响应。
- [x] 群聊下 `/context@bot_username` 不再被门禁误拒绝（能放行进入处理链路）；reply-to-bot 也可触发（已实现）。
- [x] `NotMentioned` 的拒绝日志能解释原因且不泄露正文。
- [x] 新增单元测试覆盖 entity/大小写/命令/回复等关键路径。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 为唤醒判定新增单测（覆盖）：
  - [x] 大小写不敏感
  - [x] entities mention 命中
  - [x] `/cmd@bot`（addressed bot command）命中
  - [x] reply-to-bot 命中

### Integration
- [ ] （可选）在 Telegram handler 的现有 tests 模块中构造 `Message`（或 mock）覆盖 access deny/allow 路径，断言 `AccessDenied::NotMentioned` 不再在 mention 场景触发。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用新判定逻辑（不改变 mention_mode 的默认值）。
- 回滚策略：保留旧 substring fallback；若出现异常可快速切回仅 substring（可用 feature flag 或最小回滚 commit）。

## 实施拆分（Implementation Outline）
- [x] Step 1: 明确 spec：reply/command 是否算唤醒（写入本单 Desired Behavior）。
- [x] Step 2: 实现 entities 优先的 mention 检测（含大小写 normalize）。
- [x] Step 3: 增补日志字段（NotMentioned 分支）。
- [x] Step 4: 增补单元测试。

## 交叉引用（Cross References）
- Related docs：
  - `issues/overall_issues_2.md`（Survey: 群聊默认 mention 模式）
  - `issues/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`（self-mention 语义、/cmd@bot、identity injection 与 Privacy Mode 排障口径）
- Related code：
  - `crates/telegram/src/handlers.rs:2167`（`check_bot_mentioned` / mention 判定主入口）
  - `crates/telegram/src/access.rs:72`（门禁判定）

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（唤醒判定稳定）
- [x] 已补齐自动化测试（覆盖 entities/大小写等关键路径）
- [x] 日志可观测性增强到位（不泄露正文）
- [x] 文档/配置说明更新（群聊唤醒规则清晰）

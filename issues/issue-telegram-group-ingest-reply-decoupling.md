# Issue: Telegram 群聊 “是否回复” 与 “是否写入 Session(旁听)” 语义绑死（需二维解耦：reply vs ingest）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-19）
- Priority: P1
- Components: telegram / gateway / sessions / ui / config

## 决策（Decisions）【已冻结；变更需显式注明】
- ✅ 本单只覆盖 **群聊（Group/Supergroup）**；**Channel 先不纳入**（避免平台差异与语义扩散）。
- ✅ `group_reply_mode=never` 时 **不允许** `group_ingest_mode=mentioned_only`（减少组合；never 只允许 `all_messages` 或 `none`）。
- ✅ `group_ingest_mode=all_messages` 时，旁听范围包含 **定向给其他 bot 的消息**（如 `@bot1 ...`、`/cmd@bot1 ...`），以满足“bot2 要知情但不抢答”的群聊诉求。
- ✅ 本单与配置保存语义（`channels.update` stop→restart + 全量 config 替换）**必须一起修**（否则 UI 新增字段会误伤现网配置；见“现状核查与证据/方案”）。
- ✅ `channels.update` 的 merge 规则：**缺省字段=保留旧值**；**显式 `null`=清空/覆盖为 null（如字段允许）**。

**已实现（现状）**
- ✅ 新增群聊“旁听写入”维度：`TelegramAccountConfig.group_ingest_mode`（`mentioned_only|all_messages|none`）：`crates/telegram/src/config.rs`
- ✅ 配置归一化（冻结规则）：
  - `mention_mode=always ⇒ group_ingest_mode=all_messages`
  - `mention_mode=none` 禁止 `group_ingest_mode=mentioned_only`（clamp 为 `none`）
  - `crates/telegram/src/config.rs`
- ✅ Telegram handler 在群聊 access deny（`NotMentioned` / `MentionModeNone`）且 `group_ingest_mode=all_messages` 时走 ingest-only（不触发 LLM、不产生 Telegram 出站）：`crates/telegram/src/handlers.rs`
- ✅ gateway 增加 ingest-only 写入入口：`ChannelEventSink::ingest_only(...)`：
  - trait：`crates/channels/src/plugin.rs`
  - 实现：`crates/gateway/src/channel_events.rs`
- ✅ `channels.update` 改为 merge/patch 语义（避免 UI 未提交字段被重置；先校验后 stop→restart）：`crates/gateway/src/channel.rs`
- ✅ UI（Channels → Edit Telegram Bot）新增 `Group Ingest Mode` 下拉框并提交到服务端：`crates/gateway/src/assets/js/page-channels.js`
- ✅ `/context` 卡片增补群聊配置可观测字段（effective reply/ingest 口径）：`crates/telegram/src/handlers.rs`
- ⚠️ 平台侧 Receive（BotFather Privacy Mode 等）仍不在 Moltis 控制范围内（仅作为前置约束/排障口径）。

**已知差异/后续优化（非阻塞）**
- UI 仍未暴露 `group_policy/group_allowlist/stream_mode/edit_throttle_ms/...` 等所有字段；但服务端已通过 `channels.update` 的 merge/patch 语义保证“未提交字段不被覆盖”，因此不会误伤现网配置：`crates/gateway/src/channel.rs`
- 平台 Receive 前提（Privacy Mode=OFF 等）目前主要通过文档/排障口径提示；如需在 UI 中做强提示/向导可在后续迭代补齐（见 Open Questions）。

---

## 先讲清楚：Telegram 平台投递（Receive） vs Moltis 本地处理（Process/Ingest）
> 本单讨论的是 **Moltis 侧“群消息接收/处理/写入”参数**。但必须先明确：Telegram 平台是否把群消息投递给 bot 与 Moltis 是否处理/写入是两回事。

### Telegram 平台侧（Receive：是否把 update 投递给某个 bot）
这部分属于 Telegram/BotFather/群权限配置，**Moltis 无法控制**，只能“收到以后怎么做”。

- **BotFather Privacy Mode (`/setprivacy`)**
  - **ON（隐私模式开）**：Telegram 的目标行为是：bot 在群里只接收“与 bot 相关”的 update（例如：`@bot`、`/command`、`/command@bot`、reply-to-bot 等）。因此，“群里未点名的普通聊天消息”通常不会投递到 bot。
  - **OFF（隐私模式关）**：Telegram 的目标行为是：bot 能接收更多/全部群消息 update。**本单要实现 `group_ingest_mode=all_messages`（旁听写入），平台前提通常就是 Privacy Mode=OFF**；否则 Moltis 根本收不到未点名消息，就谈不上“旁听写入”。
  - **重要且反直觉的官方限制（必须明确）**：即使 Privacy Mode=OFF，Bot API 也**不会**把“其他 bot 发送的消息”投递为 update；因此 bot2 无法通过“旁听 ingest”自动拿到 bot1 的最终回复正文。
    - 官方原文（Telegram Bots FAQ）：
      - “Bot admins and bots with privacy mode disabled will receive all messages except messages sent by other bots.”
      - “...bots will not be able to see messages from other bots regardless of mode.”
    - 参考：`https://core.telegram.org/bots/faq#what-messages-will-my-bot-get` 与 `https://core.telegram.org/bots/faq#why-doesnt-my-bot-see-messages-from-other-bots`
- **群/频道权限（平台前提）**
  - 群聊（Group/Supergroup）：bot 能否收到消息主要受 Privacy Mode 与群设置影响；Moltis 只能在“已收到 update”之后做 gating/写入/回复。
  - 频道（Channel）：如果期望 bot 处理频道消息，通常需要把 bot 加为管理员/允许接收更新（具体取决于 Telegram 侧设置）；仍属于 Receive 层前置条件。
- **消息实体（entities/caption_entities）**
  - Telegram 会对 `@mention`、`/command` 提供结构化 entity；**Moltis 必须优先读 entity 判定是否点名/是否为定向命令**，而不是 `text.contains("@bot")` 这种 substring。
- **编辑消息（EditedMessage）**
  - Telegram 会以 EditedMessage update 投递“编辑后的消息”。为避免复杂化，本项目应保持一致策略：仅处理 live location 的 edited 更新；编辑后的文本消息忽略。

**Receive 层可验证口径（强制落地到排障）**
- Moltis 是否真的收到了某条群消息：以日志 `moltis_telegram::handlers: telegram inbound message received ...` 为准（没有该日志就说明 Receive 没发生）。
- 本单新增 `group_ingest_mode=all_messages` 的作用域：仅作用在 **Moltis 已收到的 update** 上；无法让 Telegram “多投递”消息。

> 结论：即使 Receive=OFF（平台没投递），Moltis 再怎么配置也没用；反之，即使 Receive=ON（平台投递了），Moltis 仍可能选择不处理或只旁听写入。

### Moltis 侧（Process/Ingest：收到后怎么处理）
- **Process/Reply（处理/回复）**：是否触发命令/LLM 并发出回复（可能刷屏/抢答）。
- **Ingest（写入/旁听）**：是否把该群消息写入该 bot 对应 session 历史用于后续推理（不等于回复）。

## 背景（Background）
在 Telegram 群聊（尤其 multi-bot）里，常见诉求是：
- **只在被点名时才回复**（避免刷屏与抢答）
- 但同时希望 bot 能**旁听群聊上下文**（把未点名消息也写进自身 session），以便被点名时能“知道刚才发生了什么”

当前 Moltis 把“是否回复/处理”与“是否写入 session”绑死在同一个门禁上（`mention_mode`），导致缺少最常见的群聊模式（点名才回复，但仍旁听写入）。

Out of scope：
- Telegram 平台投递（Privacy Mode / admin 权限）本单只作为概念与前置条件说明，不在 Moltis 侧实现“强制收消息”。
- 不在本单扩展“跨 bot 共享上下文/自动转发给其它 bot”。
- **DM（私聊）配置与行为不在本单范围内，必须保持原样**（包括 `dm_policy/allowlist` 等）；本单只收敛“群聊（Group/Supergroup）”接收参数。
- Telegram **Channel**：本单不新增“旁听写入/ingest”能力（先不扩展 scope）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **Receive（平台投递）**：Telegram 是否把某条群消息 update 投递给某个 bot（由 BotFather Privacy Mode、权限、群类型等决定）。
- **Process/Reply（处理/回复）**：Moltis 收到后是否触发命令/LLM 并对群发出回复。
- **Ingest（写入/旁听）**：Moltis 收到后是否把该消息写入该 bot 对应 session 历史（上下文），用于后续推理；ingest 不等于 reply。
- **Group Access（群接入门禁）**：`group_policy/group_allowlist` 作为“更早的一层门禁”，决定该 bot 是否允许在该群里工作；若不允许，则必须既不 reply 也不 ingest（无论 `group_reply_mode/group_ingest_mode` 如何配置）。
- **addressed（定向）**：`@this_bot ...`、`/cmd@this_bot ...`、reply-to-bot 等能明确指向某个 bot 的消息形态。
- **message_log（留痕/观测）**：用于 UI/排障/审计的“收到过什么”；与 `Ingest（写入 session 历史）` 是两条独立路径。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 在群聊中支持“点名才回复，但未点名消息也写入 session”（最常见模式）。
- [ ] 支持 listen-only：群聊不回复，但可选择写入 session（用于纯旁观/审计/后续问答）。
- [ ] 继续支持现有三档行为（Must mention / Always / None），默认行为不变（兼容）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：reply 与 ingest 的口径明确、可组合、可解释。
  - 必须：默认配置在升级后行为不变（不意外扩大响应面/不意外旁听写入）。
  - 不得：为了旁听写入而强制触发 LLM（会导致“旁听=干活”，违背群聊不刷屏）。
- 可观测性：
  - 必须能从日志/`/context`/debug 面板明确看到：当前群聊的 reply 模式与 ingest 模式（以及是否来自配置/默认）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 群聊配置为 “Must @mention bot” 时，未点名消息会被丢弃，bot 被点名时上下文缺失（“不知道刚才发生了什么”）。
2) 想要旁听上下文只能用 “Always respond”，但这会造成刷屏/抢答且把群聊噪声写进 session。

### 影响（Impact）
- 体验：群聊里 bot 只能“被点名才知道”，无法自然参与群讨论。
- 可用性：multi-bot 群聊要么刷屏、要么上下文断裂。
- 维护成本：用户只能靠复杂约定（手工转发/重复粘贴）来补上下文。

### 复现步骤（Reproduction）
1. 在群里把 bot 配为 `mention_mode=mention`（UI：Must @mention bot）。
2. 用户先发一条未点名的普通群消息（例如“刚才我们讨论到 X”）。
3. 随后用户发送 `@my_bot 继续讲讲 X`。
4. 实际：bot 的推理上下文缺失步骤 2（因为被丢弃且未写入 session）。
5. 期望：即便步骤 2 未点名，也能被写入 session（旁听），但不触发 bot 回复；直到步骤 3 点名才回复。

## 现状核查与证据（As-is / Evidence）【不可省略】
- `mention_mode` 门禁同时决定是否放行：`crates/telegram/src/access.rs:55`
- handler 只有 access granted 才 dispatch，从而才写 session：`crates/telegram/src/handlers.rs:444`
- UI 只暴露 `mention_mode`，无“旁听写入”选项：`crates/gateway/src/assets/js/page-channels.js:420`

## 根因分析（Root Cause）
- A. 模型/会话历史的写入路径与 LLM dispatch 绑定：写 session ≈ 触发 LLM。
- B. 群聊门禁（`mention_mode`）被用作“一切开关”，导致无法表达常见的二维组合（reply vs ingest）。
- C. UI/配置未显式表达 “ingest-only” 语义，导致行为不可配置也不可观测。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
### 维度拆分（必须）
群聊行为拆成两个正交维度：
1) **Group Reply Mode**（是否回复/是否处理命令/LLM）
2) **Group Ingest Mode**（是否写入 session 历史）

### 适用范围（必须明确）
- 本单新增的 `group_ingest_mode` 只对 **Group/Supergroup** 生效。
- Telegram **Channel** 先保持现状（reply 与 ingest 仍绑定在门禁上），`group_ingest_mode` 在 Channel 场景下应被忽略/视为默认（避免语义扩散）。

### 门禁顺序（必须冻结，避免“旁听绕过安全门禁”）
> `group_ingest_mode=all_messages` 只是“同群旁听”，不能绕过群接入门禁。

处理顺序必须是：
1) **Group Access（group_policy/group_allowlist）**：不通过则 drop（既不 reply，也不 ingest）。
2) **Group Reply Mode（现有 mention_mode 的口径收敛）**：决定是否触发命令/LLM + 回复。
3) **Group Ingest Mode（新增）**：决定是否写入 session（包括 NotMentioned 场景的 ingest-only）。

### “响应/写入”二维矩阵（必须清晰可解释）
> 本矩阵只描述 Moltis “收到 update 之后”的行为；Receive 是否发生由 Telegram 平台决定（见上文）。

| 模式代号 | Reply/Process | Ingest | 典型用途 |
|---|---|---|---|
| M1（现状默认） | 仅点名 | 仅点名 | 群里只被点名才“看见/才回复”（上下文可能断） |
| M2（最常见，当前缺失） | 仅点名 | 全旁听 | 点名才回复，但旁听群上下文（避免刷屏） |
| M3（现状可配） | 全响应 | 全旁听 | bot 参与所有对话（高风险：刷屏/抢答） |
| M4（当前缺失） | 从不回复 | 全旁听 | 旁观记录/审计/被动问答（不刷屏） |
| M5（现状可配） | 从不回复 | 不写入 | 完全忽略群聊（不处理也不记录） |

### 建议的可组合枚举
#### 收敛既有群参数：把现有 `mention_mode` 明确为 “Group Reply Mode”
> 这是“命名与口径收敛”，不是要求立刻破坏性改字段名。对外（UI/文档）应称为 `group_reply_mode`；实现上可先保持字段名 `mention_mode` 兼容旧配置。

- `mention_only`（兼容旧值 `mention`）：仅 addressed 消息触发回复/处理（@this_bot / /cmd@this_bot / reply-to-bot）
- `always`（兼容旧值 `always`）：所有消息都触发回复/处理（高风险：刷屏/抢答）
- `never`（兼容旧值 `none`）：不触发回复/处理（listen-only 或 ignore）

#### `group_ingest_mode`
- `mentioned_only`：仅 **addressed to this bot** 的消息写入 session（现状行为）
- `all_messages`：所有收到的群消息都写入 session（旁听；包含定向给其他 bot 的消息）
- `none`：不写入 session

### 约束（避免不自洽组合）
- 若 `group_reply_mode=always`，则 `group_ingest_mode` 必须为 `all_messages`（回复却不写入上下文不自洽）。
- 若 `group_reply_mode=mention_only`，`group_ingest_mode` 可为 `mentioned_only`（现状）或 `all_messages`（点名回复+旁听）。
- 若 `group_reply_mode=never`，`group_ingest_mode` **只能**为 `all_messages`（listen-only）或 `none`（ignore），不得为 `mentioned_only`（减少组合、避免含混）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
0) **先收敛配置保存语义（与本单一起做，阻塞）**
   - 服务器端 `channels.update` 必须把“客户端提交的局部 config”与“存量/运行中 config”做 **merge/patch** 后再 stop→restart，避免未提交字段回落默认值：`crates/gateway/src/channel.rs:191`。
   - UI 仍可只提交可编辑字段，但必须保证“没在 UI 展示的字段”不会被重置。
   - 规范（必须冻结）：缺省字段保留旧值；显式 `null` 表示清空/覆盖。
1) **配置层新增** `group_ingest_mode`（默认 `mentioned_only`，确保升级不变）。
2) **reply 口径收敛**（沿用已冻结规则）：
   - 群聊命令（slash command）仍按已冻结规则：只处理 `/cmd@this_bot`；未定向 `/cmd` 静默丢弃；`/cmd@other_bot` 静默丢弃。
3) **实现新增 ingest-only 写入路径**（关键）：
   - 在 gateway/chat/session 层提供一个“只写入 session、不触发 LLM、不产生 channel reply”的入口（例如 `chat.ingest_only(...)` 或在 `ChannelEventSink` 增加 `ingest_only(...)`）。
   - Telegram handler 在 access deny（NotMentioned）时，如果 `group_ingest_mode=all_messages`，则走 ingest-only；否则保持丢弃。

### 接口与数据结构（Contracts）
#### 配置（TelegramAccountConfig）
- 新增字段：
  - `group_ingest_mode = ["mentioned_only" (default), "all_messages", "none"]`
- 现有字段（保持兼容）：
  - `mention_mode` 继续作为群聊 reply 门禁（对外口径收敛为 “Group Reply Mode”；底层字段名先不强改，避免破坏旧配置）。
  - **DM 字段必须保持原样**：`dm_policy/allowlist` 不得被本单方案影响。

#### UI（Channels → Edit Telegram Bot）
- 新增下拉框：`Group Ingest Mode`
  - `Mentioned only (default)` / `Listen (ingest all)` / `Off (ingest none)`
- **必须修复保存语义（与本单一起做，阻塞）**：
  - 服务器端应采用“merge 后再重启”的策略（兼容旧 UI/旧客户端），避免未暴露字段被默认覆盖。
  - UI 侧（可选增强）：Edit 时可先拉取当前完整 config，再做 patch 提交；但即使 UI 未升级，服务端 merge 也必须兜住。

### 失败模式与降级（Failure modes & Degrade）
- 当 `group_ingest_mode=all_messages` 导致上下文膨胀、触发 auto-compact：
  - reply 仍受 `group_reply_mode` 控制（不因 compact 而额外刷屏）。
  - compact 失败/仍超限时：仅在“需要回复的 addressed 消息”场景返回产品化短提示；旁听 ingest-only 不返回任何提示。
- Telegram supergroup “话题/主题（topics, `message_thread_id`）”：
  - 本单暂不扩展为“每个 topic 一个上下文”；先保持现状（同群共享上下文）。
  - 但必须在 Open Questions 中明确：未来要不要把 `(chat_id, message_thread_id)` 作为 session 的进一步分桶键。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 组合模式可表达并可验证：
  - A) mention_only + mentioned_only（现状不变）
  - B) mention_only + all_messages（点名才回复，但旁听写入）
  - C) always + all_messages（现状 always 行为）
  - D) never + all_messages（listen-only）
  - E) never + none（ignore）
- [x] multi-bot 群中：bot2 配为（mention_only + all_messages）时：
  - bot2 不会回复 `@bot1 ...`，但其 session 历史会包含该群消息，可在后续 `@bot2 ...` 时利用上下文。
- [x] `channels.update` 不会重置未提交字段：
  - 例如用户在 UI 只改 `mention_mode`/`group_ingest_mode` 后，`group_policy/group_allowlist/stream_mode/edit_throttle_ms/...` 等未暴露字段必须保持原值不变。
- [x] UI 与 `/context`/debug 明确显示 effective 的 reply/ingest 模式与来源（configured/effective）。
- [x] 默认升级行为不变（旧配置未显式设置时仍等价于 mentioned_only）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 群聊 access deny（NotMentioned）+ ingest_all 时走 ingest-only（且不 dispatch）：`crates/telegram/src/handlers.rs` tests
- [x] mention_mode=none（never）+ ingest_all 仍可旁听写入：`crates/telegram/src/handlers.rs` tests
- [x] 组合约束校验（always ⇒ ingest_all / none forbids mentioned_only）：`crates/telegram/src/config.rs` tests
- [x] `channels.update` merge/patch 语义（缺省字段保留、显式 null 覆盖）：`crates/gateway/src/channel.rs` tests

### Integration
- [ ] （可选）构造端到端群聊序列：先旁听写入，再点名提问，验证上下文包含旁听内容（需要运行态 Telegram 环境，非 CI 必需）

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认关闭（即默认 `group_ingest_mode=mentioned_only`，行为与现状一致）；仅当用户显式选择 “Listen/ingest all” 时启用旁听写入。
- 回滚策略：回滚到无 ingest-only 的版本会导致旁听写入不可用，但不影响基础 reply 功能。

## 实施拆分（Implementation Outline）
- ✅ Step 0: `channels.update` 收敛为 merge/patch 更新（先 merge 再 stop→restart），保证未提交字段不被重置。
- ✅ Step 1: 配置层新增 `group_ingest_mode`（serde 默认 `mentioned_only`，确保升级不变）。
- ✅ Step 2: 在 gateway/chat/session 提供 ingest-only 写入入口（只写 session，不触发 LLM/不产生 reply）。
- ✅ Step 3: Telegram handler 在 NotMentioned/MentionModeNone 分支根据 `group_ingest_mode` 决定 “drop vs ingest-only”。
- ✅ Step 4: UI 增补 `Group Ingest Mode`（仅群聊）。
- ✅ Step 5: `/context`/debug 面板展示 effective 的 reply/ingest 口径字段（configured/effective）。

## 未决问题（Open Questions）
- Q1: 如何在 UI/文档中显式提示平台前提（例如：Privacy Mode=OFF 才可能收到未点名消息，从而 `all_messages` 才“有意义”）？
- Q2: supergroup topics：未来是否需要把 `(chat_id, message_thread_id)` 作为 session 分桶键（同群不同 topic 不共享上下文）？
- Q3: 当旁听写入导致 session 快速膨胀并触发 compact 时，是否需要在 `/context`/debug 中给出明确提示（例如“旁听模式导致上下文增长更快”）？

## 交叉引用（Cross References）
- `issues/issue-telegram-group-mention-gating-not-working.md`（门禁唤醒判定）
- `issues/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`（自我点名剥离、/cmd@bot 行为冻结）
- `issues/issue-terminology-and-concept-convergence.md`（reply/ingest/session/scope 概念收敛）

## Close Checklist（关单清单）【不可省略】
- [x] reply vs ingest 口径已解耦且实现一致
- [x] 默认行为兼容（不扩大响应面/不意外旁听写入）
- [x] 自动化测试覆盖关键分支
- [x] UI/`/context`/debug 可观测字段齐全且标注 source/method
- [x] 安全隐私检查通过（群旁听写入默认关闭，避免意外记录）

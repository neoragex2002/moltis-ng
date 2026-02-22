# Issue: Telegram 群聊 bot@bot 点名“必达触发”缺失（需 Moltis 侧 relay（确定性解析），补齐 Telegram b2bot 限制）

## 实施现状（Status）【增量更新主入口】
- Status: DONE (2026-02-22)
- Priority: P1
- Owners: <TBD>
- Components: telegram / gateway / agents / sessions / ui / config
- Affected providers/models: 可选（仅当 `relay_strictness=loose` 时会调用 LLM 做 mention 打标；默认 strict 不调用）

**已实现（相关前置，写日期）**
- 群聊 reply vs ingest 二维解耦（旁听与回复解耦）：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- 自我点名剥离与 `/cmd@bot` 规则冻结（方案 A）：`issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- bot-to-bot 可见性补偿（出站 mirror into sessions）：`issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`

**本轮已实现（Implementation Done）**
- 群聊配置收敛（按“人类直觉默认”冻结口径落地）：
  - 删除 `mention_mode=none`（仅保留 `mention|always`）。
  - 兼容性：历史配置中残留的 `mention_mode="none"` 仍可反序列化（会按安全口径视为 `mention`，避免旧配置导致启动失败）。
  - 群聊旁听固定开启（未点名时 ingest-only；点名/always 时 dispatch/run）。
  - 群聊默认 open（移除 group allowlist/disable 语义）。
  - Mirror 固定开启（移除开关；仅镜像到“已存在的 (bot, group) session”，避免 phantom sessions）。
  - Reply threading 固定开启（不再提供开关）。
- Relay（bot@bot）固定开启：
  - source bot 出站成功后，在 gateway 侧解析其文本中的 `@bot` 指令并内部触发 target bot run。
  - reply-to 固定为 source bot 的“实际出站 message_id”（线程自然）。
  - 防环：`relay_chain_enabled` + `relay_hop_limit`（默认 3）+ 进程内去重（dedupe key）。
  - relay chain 上下文仅从“最新 user 消息且标记为 relay”的记录继承，避免旧 relay 消息污染后续非 relay run。
  - 误触发防护（收敛口径）：
    - **行首点名**（去掉前导空白后，首 token 为 `@bot`，允许 `@bot:`/`@bot，`）：必 relay（且支持多 bot 点名同一任务）。
    - **非行首点名**：`relay_strictness=strict` 不 relay；`relay_strictness=loose` 才调用 LLM 对“已提取到的 mentions”做 `reference|directive` 打标，判 `directive` 才 relay（失败/解析不出 JSON ⇒ 不 relay）。
    - 共同硬排除：code fences / inline code / 引用行；以及 `@all/@here/...` 等非 bot mention。
- ChannelOutbound 出站 message_id 打通：
  - 新增 `SentMessageRef` 与 `send_*_with_ref` 系列接口；Telegram 实现返回真实 message_id。
  - gateway reply pipeline 改为使用 `send_text_with_ref/send_text_with_suffix_with_ref/send_media_with_ref`。
- Review follow-up 修正（2026-02-22）：
  - 修复 relay chain 上下文“从旧 relay 泄漏到新 run”的正确性问题。
  - 修复 Voice(caption) 路径 logbook HTML 被转义导致 `<blockquote expandable>` 无法渲染的问题。
  - 兼容历史配置 `mention_mode="none"` 的反序列化（按 `mention` 处理），避免旧部署启动失败。
- Telegram Voice（caption）路径的 logbook 跟随消息修正：
  - 当语音转写文本作为 caption 发送后，logbook follow-up 改为走 `send_text_with_suffix("", logbook_html)`，避免 `<blockquote expandable>` 被 Markdown 转义成纯文本。
- UI（Channels -> Edit Telegram Bot）已同步：
  - 移除群聊 ingest/mirror 开关。
  - 增加 relay 的 3 个最小参数：`relay_strictness/relay_chain_enabled/relay_hop_limit`。
- 自动化测试已增补/更新（gateway/telegram）。

**已知差异/后续优化（非阻塞）**
- 本单聚焦 Telegram *群聊*（`chat_id < 0`），topic/thread/channel 先不做。
- 本单不追求跨重启/多实例的严格幂等（不引入 DB outbox），仅做进程内去重/尾部查重。
  - `relay_strictness=loose` 会额外调用一次 LLM 做 JSON 打标（仅对“已提取到的 mentions”逐个标记 `reference|directive`，失败降级为不 relay）。

**本单已冻结的“人类直觉默认”口径（Design Frozen）**
- 群聊：bot 被拉进群即视为启用（群聊默认 open；不再暴露/不再允许 per-bot 群 allowlist/disabled 微管理）。
- 群聊：默认旁听（未点名时 ingest-only；不再暴露开关）。
- 群聊：默认 **点名才回复**（保留 `always` 作为可选项；删除 `mention_mode=none`）。
- 群聊：Mirror 为必备能力（总是开启，不做开关）。
- 群聊：Relay 为必备能力（总是开启，不做 enable 开关）；仅保留“严格度/链式/跳数上限”三项可控参数。
- Reply threading：行为固定为“回复挂在要求提出者下面”（用户→bot：reply_to 用户消息；bot→bot relay：reply_to source bot 消息），不提供 UI 开关。

---

## 背景（Background）
- 场景：同一个 Telegram 群里只有“本 Moltis 实例管理的 bots”（无第三方 bot），希望：
  - 任何成员（人或 bot）在群里 `@` 任意成员（人或 bot），群里其他人都看得到（Telegram 天生满足）
  - **被 @ 的 bot 必须“收到并可处理”**（点名必达 + 必触发），并在群里回复
- 约束（平台硬限制）：Telegram Bot API **不会**把 “bot 发的消息” 作为 update 投递给其他 bot（privacy=OFF 也不行），因此：
  - 人 `@bot2`：bot2 往往能收到 update ✅
  - bot1 `@bot2`：bot2 收不到 update ❌（Moltis 的 inbound handler 也看不到这条消息）
  - 官方 FAQ（证据）：见 `core.telegram.org/bots/faq`（“bots 不会收到 other bots messages”相关条目）
- Out of scope：
  - 不试图改变 Telegram 平台投递行为
  - 不支持“第三方非 Moltis bot”发言触发 Moltis 内部 relay（我们抓不到其 outbound）

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Outbound Mirror**（出站镜像，主称呼）：bot 向群出站成功后，Moltis 把其“最终回复正文（或媒体占位）”以 ingest-only 写入其他 bot 的 session，用于“知情/上下文可见”。
  - Why：补齐 “bot 发言不会投递给其他 bot” 的上下文缺口（但不触发执行）。
  - Not：不是触发被 @ bot 执行；不会导致额外出站刷屏。
  - Source/Method：authoritative（以实际出站成功后的文本为准）。
  - Aliases：mirror / outbound copy

- **Outbound Relay**（出站转交触发，主称呼）：bot 向群出站成功后，Moltis 从该条出站文本中解析“对其他 bot 的点名指令”，并在服务端**内部**触发被点名 bot 的一次 run，使其在群里回复（等价于“人点名 bot”）。
  - Why：补齐 Telegram bot-to-bot update 不投递导致的 “bot@bot 点名不必达”。
  - Not：不是 Telegram 平台的转发；不是让 bot2 收到 bot1 的 update。
  - Source/Method：authoritative（输入来自 bot1 的出站文本）；解析阶段为 estimate（确定性启发式），必须带降级。
  - Aliases：relay / internal dispatch / bot-to-bot trigger

- **可见性（Observe）**：某条群事件是否被写入各 bot 的 session（用于后续上下文）。
- **必达触发（Address）**：被点名的 bot 是否会被实际触发执行并出站回复。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] F1：在 Telegram 群里，若 bot1 的出站文本中包含对 bot2/bot3 的“点名指令”，则 bot2/bot3 必须被 Moltis 内部触发执行，并在群里回复（等价于人点名）。
- [x] F2：支持单 bot 与多 bot 指派（例如同一条里分别给多个 bot 分配不同任务）。
- [x] F3：与现有 Outbound Mirror 兼容：relay 触发的 bot2 回复也应被 mirror 到其他 bots（让大家知情）。
- [x] F4：relay 必须带防环与去重，避免 bot↔bot 互相触发风暴。
- [x] F5：bot-to-bot 线程回复固定启用：relay 触发 bot2 回复时必须 reply-to bot1 的那条消息（对话自然）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：不依赖 Telegram 投递 bot-to-bot update；必须由 Moltis 内部补齐触发。
  - 必须：relay 触发后的 bot2 行为与“人点名 bot2”保持一致（同一组工具权限/门禁/群允许范围）。
  - 不得：mirror/relay 写入不得导致 Web Sessions UI 崩溃（允许不美化，但必须可渲染/可查看）。
  - 不得：relay 不得在解析不确定时误触发（宁可不触发，只 mirror 记账）。
- 兼容性：
  - **不做迁移**：本单允许通过“重新创建/重配 Telegram bot channel”来获得新默认行为；旧配置字段不保证兼容。
- 可观测性：
  - gateway 日志必须能定位：何时发生 relay（source/targets/chat_id/去重 key/strictness/hop/是否降级）。
  - Web UI 的 session 里必须能看见 “relay 输入来源” 的标记（但不污染 Telegram 群文本）。
- 安全与隐私：
  - 日志不得打印 token / 敏感文本全文（可打印哈希/摘要）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 在 Telegram 群里，bot1 发出 `@bot2 ...`，群成员都看得到，但 bot2 不响应，且 bot2 侧没有对应 inbound 日志。
2) 用户期望“群里任一方 @ 任一方，被 @ 的 bot 必达触发”，但 Telegram 平台限制导致 bot@bot 无法成立。

### 影响（Impact）
- 用户体验：bot 间协作/指挥不可用，会议/编排场景无法落地。
- 可靠性：同一“@语义”在 人@bot 与 bot@bot 上行为不一致，导致误解与频繁排障。
- 排障成本：仅靠 Telegram 配置（privacy/off）无法解决，会反复陷入“为什么收不到”的误判。

### 复现步骤（Reproduction）
1. 同一群加入 bot1/bot2（均由本 Moltis 实例管理）。
2. 触发 bot1 回复一条包含 `@bot2` 的文本（例如 bot1 直接在群里发，或在回复中提到）。
3. 期望 vs 实际：
   - 期望：bot2 收到点名并执行，群里回复。
   - 实际：bot2 不会收到 update，不会响应。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 平台证据（已在前置单写明，作为本单前提）：
  - `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`：明确 Telegram Bot API “不投递 other bots messages”。
- 代码证据：
  - Telegram inbound gate 仅能处理 Telegram 投递的 update；bot-to-bot 发言不会进入该路径：`crates/telegram/src/handlers.rs:187`（access denied/ingest-only 逻辑附近）。
  - 目前仅实现 Outbound Mirror（写入 session，不触发 run）：`crates/gateway/src/chat.rs:5639`（`maybe_mirror_telegram_group_reply`）。
- 当前测试覆盖：
  - 已有：mirror 写入/去重/媒体占位：`crates/gateway/src/chat.rs:6674`（相关 tests）。
  - 缺口：没有任何 bot-to-bot relay/内部触发的自动化测试（本单必须补齐）。

## 根因分析（Root Cause）
- A. 上游：Telegram Bot API 不投递 bot-to-bot update，导致 bot2 inbound 侧永远收不到 bot1 的点名。
- B. 中间：Moltis 现有机制只有 Observe（旁听 ingest / outbound mirror），缺少 Address（必达触发）的补偿通路。
- C. 下游：bot@bot `@` 仅在群里“可见”，但无法触发目标 bot 执行，造成语义断裂。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - D1：在 Telegram 群里，任何 Moltis-managed bot 发出的 `@other_bot` 点名指令，必须能触发 other_bot 执行并在群里回复（通过 Moltis 内部 relay）。
  - D2：relay 触发后，目标 bot 的行为应当“等价于被用户点名”（同工具/同门禁/同群允许范围）。
  - D3：支持多 bot 指派（单条消息中可对多个 bot 指派任务，且每个 bot 可得到不同任务片段）。
- 不得：
  - D4：不得在解析不确定时触发（宁可降级为只 mirror，不 relay）。
  - D5：不得引入 bot↔bot 互相触发回环；relay 触发的 run 不得再触发 relay（链式/风暴禁止）。
- 应当：
  - D6：失败/降级应当可观测（日志/metadata 标记）。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1：确定性解析（V1 采用）
- 核心思路：
  - 仅在 source bot 出站成功后，在其“最终回复文本”里做确定性解析：提取 `@bot` + 后续任务片段。
  - 严格模式默认开启：跳过 code fences / inline code / 引用行；并用启发式规则避免示例/教程误触发。
  - 解析失败/不满足护栏：不 relay，只 mirror 记账（保证可见性）。
- 优点：实现简单、稳定、无需额外 LLM 成本；可控性强。
- 缺点：自然语言覆盖有限；需要靠 `relay_strictness` 在误触发与可用性间折中。

#### 方案 2：两阶段（候选提取 + LLM JSON 抽取，后继可选）
- 备注：若 V1 的确定性解析覆盖不足，可再引入 LLM 做“reference vs directive”判别与切分（必须 JSON schema 校验 + 无工具调用 + 失败降级）。

### 最终方案（Chosen Approach）
采用 **方案 1：确定性解析 + 内部 relay（必备、总是开启）**。

#### 行为规范（Normative Rules）
- R1（触发点）：仅在 **Telegram 群聊**（`chat_id < 0`）且 source bot 出站发送成功后尝试解析与 relay。
- R2（候选集）：仅对 “本 Moltis 实例管理的 Telegram bot accounts（排除 source）” 做候选匹配。
- R3（解析护栏）：跳过 code fences / inline code / 引用行；并跳过 `@all/@here` 等非 bot mention。
- R3.1（行首优先）：当 `@bot` 在行首（去掉前导空白后首 token 即为 `@bot`，允许 `@bot:`/`@bot，`）⇒ 必 relay。
- R3.2（非行首）：当 `@bot` 不在行首 ⇒
  - `relay_strictness=strict`：不 relay（只 mirror 知情）
  - `relay_strictness=loose`：调用 LLM 对“已提取到的 mentions”逐个打标 `reference|directive`，仅 `directive` 触发 relay；JSON 解析失败/不合规 ⇒ 不 relay
- R4（等价于人点名）：relay 内部触发必须走与用户点名同等的 run pipeline，并尊重 target bot 的群聊门禁配置（点名才回复 vs always）。
- R5（多 bot 支持）：允许单条出站文本触发多个 bot；每个 bot 可有独立任务片段。
- R6（链式 relay，默认开启）：relay 触发的输入在 session 中带 `channel.relay_*` 标记，并默认允许 “relay→relay” 的链式触发：
  - `relay_chain_enabled=true`（默认）。
  - 必须：使用 hop 计数 + 硬上限，避免无限循环（上限可配置）。
- R7（去重）：同一条 source 出站文本对同一 target bot 的同一任务只触发一次（重试/并发不重复）。
- R8（降级）：任何解析失败/不满足 strictness 护栏/不满足安全约束 → 不 relay，仅记录可观测信息 + 依赖 mirror 保持“知情”。

#### 接口与数据结构（Contracts）
- 群聊行为固定（不再配置/不再暴露开关）：
  - 群聊默认旁听：未点名时 ingest-only；点名/always 时 dispatch/run（因此群消息总会进入 session）。
  - 群聊默认启用：移除 group allowlist/disabled 微管理（按“是否在群里”来决定即可）。
  - Mirror 必备：总是开启（移除 `group_outbound_mirror_enabled` 开关）。
  - Relay 必备：总是开启（不提供 enable 开关）。
  - Reply threading 必备：总是开启且固定 reply-to “要求提出者”（bot→bot relay 时 reply-to source bot 消息）。
- 仅保留 4 个“你确实会调”的群聊参数（最小集合）：
  - `mention_mode: mention | always`（默认 `mention`；删除 `none`）。
  - `relay_strictness: strict | loose`（默认 `strict`）。
  - `relay_chain_enabled: bool`（默认 `true`）。
  - `relay_hop_limit: u8`（默认 `3`；允许配置）。
- Session 写入（可观测）：
  - 对 target bot 的内部触发输入以 `role=user` 写入 session，并带以下字段（仅 session 可见，不污染 Telegram 群文本）：
    - `channel.relay=true`
    - `channel.relay_chain_id=sha256:...`
    - `channel.relay_hop=<n>`
    - `channel.relay_from_account_id=<source account_id>`
    - `channel.relay_from_bot_handle=@...`
    - `channel.relay_source_chat_id=<chat_id>`
    - `channel.relay_source_outbound_message_id=<source telegram message_id (as-sent)>`（用于 reply-to source bot）
    - `channel.relay_source_inbound_trigger_message_id=<source inbound trigger id (best-effort)>`
  - V1 额外说明：relay 注入到 target bot 的用户输入文本会加一个很短的前缀 `（来自 @source_bot）`，便于人类读 session 与 LLM 理解来源。

#### 出站 message_id（前置改造，必须）
> 为支持 “bot2 reply-to bot1 的那条消息”，Moltis 必须拿到 bot1 **实际发送成功后的 Telegram message_id**（as-sent）。

- 现状：`ChannelOutbound::send_text/send_media` 仅返回 `Result<()>`，gateway 无法获知平台生成的 sent message_id。
- V1 必须补齐（两选一）：
  - 选项 A（推荐）：扩展 `ChannelOutbound` 接口，让 send 返回 `Option<SentMessageRef>`（至少包含 `message_id`），Telegram 实现返回真实 message_id；其他 channel 可返回 None。
  - 选项 B（不推荐）：为 Telegram outbound 增加旁路回调/缓存查询 last_message_id（容易串线，不通用）。
 - 本单结论（冻结）：采用 **选项 A**，并把 “as-sent message_id” 作为 relay 与可观测字段的权威来源。

#### 失败模式与降级（Failure modes & Degrade）
- 解析失败 / 不满足 strictness 护栏：
  - 不 relay
  - 记录一条 `telegram outbound relay: skipped` 日志（带原因、去重 key、候选数，不含全文）
  - 保持现有 mirror（Observe）让其他 bots“知情”
- relay 触发失败（target run error）：
  - 不影响 source bot 出站已成功的事实
  - 错误由既有 single egress / run.failure 机制处理（Web UI/日志/Telegram 回执）

#### 安全与隐私（Security/Privacy）
- 禁止在日志打印：
  - bot token / headers / raw request body
  - 源出站文本全文（仅允许 hash/前 N 字符摘要）

## 验收标准（Acceptance Criteria）【不可省略】
- [x] AC1：同一群内（仅 Moltis-managed bots），当 bot1 出站文本包含对 bot2 的明确指令时，bot2 必须被内部触发并在群里回复。
- [x] AC2：支持多 bot 指派（行首）：例如多行 `@bot1 任务1` / `@bot2 任务2` 能正确触发两个 bot 且任务分别不同。
- [x] AC3：不误触发：教程/示例/代码块/引用中的 `@bot2` 不会导致 relay（仅 mirror 知情）。
- [x] AC4：防环生效：链式 relay 开启时，循环点名不会无限触发；超过 `relay_hop_limit` 后停止 relay（仅 mirror + 记日志）。
- [x] AC5：reply-to 行为固定：relay 触发的 bot2 回复必须 reply-to source bot 的那条消息（线程自然）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/gateway/src/chat.rs`：对候选提取/切分（含 code/quote 跳过、connector trim）做单测。
- [x] `crates/telegram/src/handlers.rs`：群聊未点名 → ingest-only；点名 → dispatch，并验证 self-mention strip。
  - [x] loose 模式 LLM 打标路径（mock chat service）：`crates/gateway/src/chat.rs`

### Integration
- CI 不跑 Telegram 端到端（需要真实 bot token/群且不稳定）；以手工验收为准（见下）。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：Telegram 平台端到端在 CI 不稳定/需要真实 bot token。
- 手工验证步骤（最小集）：
  0) 不需要删除/重建任何配置文件或 session 文件（无需清空 jsonl）；只需按下列步骤在 UI 调参与群内交互验收。
  1) Web UI → `Channels` → 分别 `Edit Telegram Bot`（bot1、bot2 都确认一遍）：
     - `Group Mention Mode`：
       - 推荐：`Must @mention bot`
       - 如需抢答：`Always respond`
     - `Group Relay Strictness`（只影响“非行首点名”是否会尝试 LLM 打标）：
       - 推荐默认：`Strict`（行首点名必达；更不误触发）
       - 如需支持非行首点名（如 `请 @bot2 ...`）：把 **bot1** 调为 `Loose`
     - `Relay Chain = on`（按需；建议开）
     - `Relay Hop Limit = 3`
	     - `Save Changes`
	  1.1) 如果你是“文件配置优先”（不用 UI 落盘 bot config），则在 `moltis.toml` 的 `[channels.telegram.<bot_id>]` 里确保包含（重启生效）：
	     - `mention_mode = "mention" | "always"`
	     - `relay_strictness = "strict" | "loose"`
	     - `relay_chain_enabled = true|false`
	     - `relay_hop_limit = 3`
  2) 准备：在群里分别点名一次 bot2（以及其他你希望“旁听/mirror”的 bot），确保其在该群里已创建 session（因为 mirror/relay 仅写入“已存在的 (bot, group) session”）。
     - 例如：`@bot2 说 1 个字`
  3) 行首必达（Strict 即可）：让 bot1 在群里发：`@bot2 列出当前挂载点`（或 `@bot2:` / `@bot2，`）。
  4) 多 bot（可选）：让 bot1 发多行 `@bot1 ...`/`@bot2 ...`，或同一行 `@bot2 @bot3 做同一个任务`，应触发多个 bot 分别回复。
  5) 非行首 + loose（可选）：将 bot1 的 `Group Relay Strictness` 改为 `Loose`，让 bot1 发：`请 @bot2 列出当前挂载点`。
     - 说明：若 LLM/网络不可用会降级为“不 relay，只 mirror”（bot2 不回复是预期降级）。
  6) 误触发防护（应不触发）：让 bot1 发包含引用/代码块/inline code 的 `@bot2`（例如 `> @bot2 ...`、`` `@bot2` ``、fenced code），bot2 不应被触发。
  7) Web UI 落盘验证（可选）：打开 `telegram:bot2:<chat_id>` session，确认存在：
     - bot1 的 mirror 记录（前缀 `[@bot1 mirror] ...`）
     - relay 注入记录（`channel.relay=true`，含 `relay_chain_id/relay_hop/relay_source_outbound_message_id`）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：本单按“直觉默认”会改变群聊默认行为（群聊旁听/mirror/relay/reply-to 固定开启；移除若干旧开关）。
- 迁移策略：**不做迁移**；旧配置字段（例如 group_* / reply_to_message）会被忽略。必要时在 UI 里点一次 Save Changes 触发重启/落盘即可。
- 回滚策略：保留一条紧急全局 kill-switch（仅用于回滚/排障；不作为常规 UI 选项）。
- 上线观测：
  - 日志关键词：`telegram outbound relay`
  - 指标（可选）：relay_trigger_total / relay_skipped_total（按 reason 分桶）

## 实施拆分（Implementation Outline）
- [x] Step 0（收敛/删开关）：删 `mention_mode=none`；群聊旁听/群聊 open/mirror/relay/reply-to 收敛为固定行为（仅保留 relay 的 3 个最小参数）。
- [x] Step 1（前置）：channel outbound 返回 sent message_id（Telegram 必须返回；其余 channel 可 None），gateway 捕获 as-sent message_id。
- [x] Step 2：在 Telegram 出站成功回调处增加 relay pipeline（紧邻 mirror pipeline，复用 “all bots list（本实例账号）”）。
- [x] Step 3：实现确定性候选提取（跳过 code/quote/inline code）+ 去重 + hop 上限防环。
- ✅ Step 4（Done，2026-02-22）：非行首点名在 `relay_strictness=loose` 下引入 LLM JSON 打标（仅对“已提取到的 mentions”逐个标记 `reference|directive`；失败降级为不 relay）。
- [x] Step 5：内部触发 target bot run（等价于人点名），并写入 `channel.relay_*` metadata（仅 session 可见，不污染 Telegram 文本）。
- [x] Step 6：补齐/更新单测；更新本单状态与手工验收步骤。
- 受影响文件（预估）：
  - `crates/gateway/src/chat.rs`（出站回调 + relay pipeline）
  - `crates/telegram/src/config.rs`（新增配置字段）
  - `crates/gateway/src/assets/js/page-channels.js`（UI toggle）
  - `crates/channels/...`（如需要暴露 snapshot 字段）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`
  - `issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
  - `issues/done/issue-telegram-group-ingest-reply-decoupling.md`
  - `issues/issue-telegram-chairbot-meeting-protocol.md`（会议编排可优先走“内部调度”，不依赖 Telegram b2bot update）

## 未决问题（Open Questions）
- Q1：`relay_strictness=loose` 的边界：V1 仍强制跳过 code fences / inline code / 引用行；但对“是否为明确指令”的启发式更宽松（仍以“不确定则不触发”为主）。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（解析为 estimate；触发与落盘为 authoritative）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（旧字段忽略；群聊默认行为更新）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

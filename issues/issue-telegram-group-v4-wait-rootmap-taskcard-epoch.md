# Issue: Telegram 群聊多 Bot 协作补齐 V4 “不断链”（WAIT 挂起续链 / Root 追溯 / TaskCard / 过期输出收敛）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Owners: <TBD>
- Components: telegram / gateway / sessions / agents
- Affected providers/models: <N/A>

**已实现（相关基础能力，写日期）**
- 群聊门禁与旁听：点名/回复 bot 才触发 run；未点名进入 listen-only ingest：`crates/telegram/src/access.rs:55` / `crates/telegram/src/handlers.rs:249`
- ingest-only 写入入口（不触发 LLM）：`crates/channels/src/plugin.rs:24` / `crates/gateway/src/channel_events.rs:361`
- 群聊 bot@bot relay（解析出站 @bot 指令并内部触发目标 bot）：`crates/gateway/src/chat.rs:6394` / `issues/done/issue-telegram-group-bot-to-bot-mentions-relay-via-moltis.md`
- 群聊 bot1→bot2 可见性补偿（outbound mirror into sessions）：`issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`

**已覆盖测试（如有）**
- relay 行首解析、code/quote 跳过、loose 模式 mention 打标：`crates/gateway/src/chat.rs:8433`
- mention gating + reply-to-bot 激活：`crates/telegram/src/handlers.rs:3828`

**已知差异/后续优化（非阻塞）**
- 目前缺少 Bot 级“静默协议”（`<SILENCE:PASS>` / `<SILENCE:WAIT>`）的拦截与持久化动作：仓库代码中未见对应协议字符串（仅存在于 `issues/problems/v4.md`）。
- 目前缺少 WAIT 挂起续链（C 路由）：当 bot2 被点名但需要等待 bot1 交付时，无法登记等待、也无法在后续“未点名但 Reply root”的交付消息到来时自动唤醒 bot2。
- 目前缺少 Root 追溯（MessageRootMap）：无法把“Reply 到非 root 的消息”追溯回 root 进行续链。
- 目前缺少 TaskCard 注入：当 session compact/FIFO 截断后，WAIT 续链可能“失忆”（root 原文与当前状态不可稳定找回）。
- 目前缺少 epoch（过期输出收敛）：慢 bot 的旧快照输出可能在交付到来后仍然被发送（或仍然写入状态），导致竞态与断链风险。

---

## 背景（Background）
- 场景：Telegram 群里有多只 Moltis-managed bots（bot1 查资料、bot2 写代码、Manager 统筹等）。
- 现状能力：已经能做到
  - A 路由：显式 `@bot` / reply-to-bot 才触发 run（降低成本、减少抢话）。
  - listen-only：未点名消息也会写入 session（上下文不断档）。
  - relay：bot1 出站可以触发 bot2（确定性行首，或 loose 模式用 LLM 打标）。
- 仍然缺的关键体验：**不断链**（V4 的 “WAIT 挂起 + Reply 挂链 + 自动追赶 + 竞态收敛”）。
- Out of scope：
  - 不改变 Telegram 平台投递约束（other bots messages 不投递等）。
  - 不在本单引入“看门大爷=LLM”（router 必须是纯程序；已有 `relay_strictness=loose` 的 LLM 打标不在本单扩展，最多做收敛/隔离）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Root**（主称呼）：一条任务链的锚点消息（通常是 Owner 的派活消息），用于把交付与反馈挂在同一条线程下。
  - Why：WAIT 续链与竞态收敛都必须以 root 为 key。
  - Not：不是“当前触发消息 id”；Reply 到非 root 也要能追溯回 root。
  - Source/Method：authoritative（来自 Telegram `reply_to_message_id` + 本地 MessageRootMap 追溯）。

- **MessageRootMap**（主称呼）：`message_id -> root_message_id` 的映射表（同一 chat 内）。
  - Why：Reply 到非 root 时仍可追溯 root。
  - Not：不是 session history（session 可以被 compact；RootMap 必须长期可查）。
  - Source/Method：authoritative（收到入站/出站消息时写入）。

- **TaskCard**（主称呼）：每个 root 的最小“任务便签”（至少含 root 原文与状态）。
  - Why：防止 compact/FIFO 截断导致 WAIT 续链时 bot 失忆。
  - Not：不是完整会话记录；也不是 LLM 总结（V1 可先存原文+少量结构化状态）。
  - Source/Method：authoritative（root 原文来自入站消息文本；状态来自 WAIT 表与交付事件）。

- **Silence Protocol**（主称呼）：Bot 输出的静默信号（严格全等拦截）：
  - `<SILENCE:PASS>`：仅被引用/与己无关 → 丢弃，不发群，不登记状态。
  - `<SILENCE:WAIT>`：任务归我但缺前置/等待交付 → 丢弃，不发群，并登记 WAITING。
  - Why：减少群噪声 + 支持“点名但先别说话”的可控协作。
  - Not：不是自然语言解释；不得输出附加文本（否则会污染群）。
  - Source/Method：authoritative（as-sent LLM output，严格全等匹配）。

- **WAITING**（主称呼）：等待状态记录（WaitingTable）。
  - Why：让“未点名的交付消息”也能唤醒等待的 bot（不断链）。
  - Not：不是队列系统；不是并发调度器；只是“缺前置”的结构化记账。
  - Source/Method：authoritative（由 `<SILENCE:WAIT>` 触发写入）。

- **epoch**（主称呼）：每 bot 单调递增版本号（只认最新快照）。
  - Why：竞态收敛（慢 bot 的旧输出作废）。
  - Not：不是全局时钟；不是消息 id。
  - Source/Method：configured+effective（实现策略决定触发点；行为需冻结在 Spec）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 支持 V4 静默协议拦截：PASS/WAIT 都不进群（严格全等匹配）。
- [ ] 支持 WAIT 挂起续链：bot 输出 WAIT 后，后续交付消息即使未点名，也能在同一 root 下自动唤醒该 bot。
- [ ] 支持 Root 追溯：Reply 到非 root 的消息也能解析出 root，用于 WAIT 续链与可观测。
- [ ] 支持 TaskCard（最小版）：root 原文与 WAITING 状态可稳定注入上下文（不依赖 session 历史“刚好没被 compact”）。
- [ ] 支持 epoch 收敛（最小版）：当同一 root 下有新消息到来时，正在运行的旧快照输出不得落地（至少不得发群；理想是丢弃并重跑）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：静默协议严格全等；不得用包含额外字符的“类似”文本触发。
  - 必须：WAIT 续链不依赖人类“记得 @bot2”；Reply 挂链即可。
  - 不得：listen-only 的普通闲聊导致无故唤醒（只允许命中 WAITING root 的情况下唤醒）。
- 兼容性：
  - 不引入新的群聊开关（默认收敛为“开启”；若必须引入，也只能是 1 个总开关且默认开）。
  - 不破坏现有 relay/mirror 语义（本单只增加 WAIT/Root/TaskCard/epoch，不重写 relay）。
- 可观测性：
  - `/context` 展示：当前 root、WAITING 状态、root 追溯是否命中、最近一次触发来源（mentioned / reply-to-bot / waiter-wakeup）。
  - 日志：必须带 `chat_id/root_message_id/bot_account_handle`；不得打印隐私/全文。
- 安全与隐私：
  - 不在日志打印 Telegram token、完整群消息全文（最多摘要 + hash）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 群里出现依赖任务：Owner 同时点名 bot1 查资料、bot2 写代码，但明确要求 bot2 等资料。
2) bot2 仍会立刻回复（乱抢话/刷屏），或只能靠 prompt “自觉沉默”，但系统无法拦截/续链（任务链会断）。

### 影响（Impact）
- 用户体验：需要人肉提醒/补 @；协作不自然；群噪声高。
- 可靠性：依赖链易断；慢 bot 易输出旧方案或错过交付（lost wakeup）。
- 排障成本：没有 root key、没有 WAIT 记账、没有一致的可观测字段，难以复盘。

### 复现步骤（Reproduction）
1. Owner：`@bot1 查资料；@bot2 等 bot1 结果后写代码`
2. bot2：无法稳定进入 WAIT（即使“口头沉默”，系统也无法登记等待）
3. bot1：交付资料（未点名 bot2，仅 Reply root）
4. 期望 vs 实际：
   - 期望：bot2 被自动唤醒并继续工作
   - 实际：bot2 因未被点名不会 run（只 ingest），任务断链

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/telegram/src/access.rs:55`：群聊 mention gating（未点名 → `NotMentioned`）。
  - `crates/telegram/src/handlers.rs:249`：群聊未点名 → listen-only `ingest_only()`（不触发 LLM）。
  - `crates/gateway/src/channel_events.rs:361`：`ingest_only()` 的语义明确“不触发 LLM run”。
  - `crates/gateway/src/chat.rs:6394`：已存在 bot@bot relay（但无 WAIT/Root/TaskCard/epoch 机制）。
- 当前测试覆盖：
  - 已有：relay 解析/去重/loose 打标等（见 Status 区）。
  - 缺口：静默协议拦截、WAIT 续链、Root 追溯、TaskCard 注入、epoch 收敛均无覆盖。

## 根因分析（Root Cause）
- A. 协议缺失：系统没有一个“可机器拦截的静默信号”，也没有 WAIT 的结构化落地位置。
- B. 索引缺失：系统没有 MessageRootMap，无法把交付事件与 root 绑定，续链无从谈起。
- C. 状态缺失：没有 TaskCard/WaitingTable，compact 后上下文不可稳定找回，竞态也无法收敛。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - Bot 输出严格等于 `<SILENCE:PASS>` 时：系统丢弃该输出（不发群），不写 WAITING。
  - Bot 输出严格等于 `<SILENCE:WAIT>` 时：系统丢弃该输出（不发群），并写入 WaitingTable（key 至少包含 `chat_id + root_message_id + bot_account_handle`）。
  - 当任意新入站消息到来时：
    - 若它能解析到 `root_message_id`，且 WaitingTable 命中当前 bot 在该 root 上 WAITING，则必须触发一次 run（即使未点名）。
  - Root 追溯必须可用：Reply 到非 root 时也能解析 root（通过 MessageRootMap）。
  - TaskCard 至少包含 root 原文，并在构建上下文时注入（无论 session 是否 compact）。
- 不得：
  - 不得让 `<SILENCE:...>` 进入 Telegram 群（任何额外字符都应视为普通文本，不触发拦截/写表）。
  - 不得因为 listen-only 普通消息导致“全 bot 被唤醒”（只有命中 WAITING root 才允许唤醒）。
- 应当：
  - epoch 收敛：同一 root 下若有新消息到来，应当让正在跑的旧快照输出作废（至少不发群；优选丢弃并重跑）。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：按 V4 落地（RootMap + WaitingTable + TaskCard + epoch）
- 核心思路：
  - 为 Telegram 群聊引入“可持久化的 root 索引”和“等待表”，把“不断链”从 prompt 约定变成工程事实。
  - 复用现有 singleflight/message_queue 机制；在此基础上补齐“过期输出作废”的 epoch 语义。
- 优点：行为可冻结、可测试、可观测；能覆盖依赖等待、忘记 @、乱序竞态等典型坑。
- 风险/缺点：需要跨模块新增少量存储/协议字段（Reply root 追溯与 WAIT 状态落地）。

#### 方案 2（最小改动）：仅做 `<SILENCE:PASS>` 拦截，不做 WAIT 续链
- 优点：实现极快、风险低。
- 缺点：无法“不断链”；只能降低噪声，不能解决依赖协作的自动推进。

### 最终方案（Chosen Approach）
- 采用方案 1，但按阶段交付（优先把 WAIT+RootMap 跑通，再补 TaskCard 与 epoch 收敛）。

#### 行为规范（Normative Rules）
- R1：静默协议严格全等匹配（PASS/WAIT），匹配后输出不进入 Telegram。
- R2：WaitingTable 的写入仅由 `<SILENCE:WAIT>` 触发，且必须绑定到 `root_message_id`。
- R3：C 路由（waiter wakeup）：当新入站消息解析出的 `root_message_id` 命中 WAITING 时，即使未点名也触发 run。
- R4：Root 追溯：每条入站消息必须写入 MessageRootMap；每条 bot/system 出站消息也必须写入 MessageRootMap（否则后续 Reply 会断链）。
- R5：TaskCard 注入（最小版）：构建上下文时必须注入 root 原文 + WAITING 状态快照。
- R6：epoch（最小版）：同一 root 下新消息到来时，旧快照输出不得落地（至少不得发群）；实现上允许“丢弃并重跑”或“丢弃并等待下一触发”，但必须冻结一种行为。

#### 接口与数据结构（Contracts）
- Channel inbound 元信息（为 root 追溯提供事实来源）：
  - 为 Telegram 入站消息在 `ChannelMessageMeta`（或其 channel JSON）中增加：
    - `inReplyToMessageId`（可选，字符串）
    - `telegramMessageId`（必填，字符串；即当前入站 message_id）
  - 注意：现有 `ChannelReplyTarget.message_id` 是“出站 reply threading 的目标”，不能用来表达“入站 reply_to”。
- 存储（建议落在 gateway 的 SQLite 体系内，避免随 session compact 丢失）：
  - `telegram_message_root_map(chat_id, message_id) -> root_message_id`
  - `telegram_waiting(chat_id, root_message_id, account_handle) -> {waiting_since_ms, last_seen_message_id, expires_at_ms}`
  - `telegram_task_card(chat_id, root_message_id) -> {original_text, status_json, updated_at_ms}`
- 可观测字段（/context + logs）：
  - `resolved_root_message_id`
  - `wakeup_reason`：`mentioned|reply_to_bot|waiter`
  - `waiter_status`：`NONE|WAITING`
  - `epoch/run_epoch/current_epoch`（若实现 epoch）

#### 失败模式与降级（Failure modes & Degrade）
- RootMap 写入失败 / 查不到 root：
  - 降级：把 `root_message_id = (inReplyToMessageId ?? telegramMessageId)`（best-effort），并禁止跨层追溯；日志记录降级原因。
- WaitingTable/TaskCard 存储不可用：
  - 降级：不触发 waiter wakeup（仍保持现有 mention gating + relay 能力）；不得因此刷屏或报错到群。
- epoch 未实现（阶段 1）：
  - 明确记录缺口与竞态风险；至少保证 WAIT 输出不会进群。

#### 安全与隐私（Security/Privacy）
- 日志不得打印群消息全文；允许 hash + 前 N 字符摘要（N 需冻结，建议 64）。
- TaskCard 只存 root 原文（这本就会进入 session）；仍需遵循现有数据目录与权限策略。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] AC1：当 bot 输出严格等于 `<SILENCE:PASS>`，群里不出现该文本，且不产生任何 WAITING 记录。
- [ ] AC2：当 bot 输出严格等于 `<SILENCE:WAIT>`，群里不出现该文本，且写入 WAITING（可在 /context 看到）。
- [ ] AC3：bot 进入 WAITING 后，后续交付消息只要 Reply 到同一 root（可为非 root 的中间消息），即使未点名，也必须自动唤醒该 bot 并继续工作。
- [ ] AC4：普通未点名群消息不会导致无 WAITING 的 bot 被唤醒（仍为 ingest-only）。
- [ ] AC5（若实现 epoch）：交付消息到来后，慢 bot 的旧快照输出不得落地到群（至少被丢弃）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] Root 追溯：`resolve_root()`（含“Reply 到非 root”链路）单测：`crates/<TBD>/...`
- [ ] 静默协议拦截：PASS/WAIT 严格匹配单测（含“带空格/带前后缀不匹配”）：`crates/<TBD>/...`
- [ ] WaitingTable：写入/命中/过期清理单测：`crates/<TBD>/...`

### Integration
- [ ] Telegram 群聊模拟：未点名消息 ingest-only；命中 WAITING 时触发 run（无需真实 Telegram，使用插件/事件 sink mock）：`crates/<TBD>/...`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若无法在 CI 中模拟 Telegram 更新的 reply 链结构，需要手工验证。
- 手工验证步骤：
  1. 拉起两只 bot 在同一 supergroup。
  2. Owner 派活：`@bot1 ... @bot2 等 bot1 ...`
  3. 验证 bot2 输出 WAIT 不进群（但 /context 可见 WAITING）。
  4. bot1 Reply root 交付；验证 bot2 被唤醒并继续工作。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用（不新增开关）；若风险不可控，允许加 1 个总开关（默认开启）作为紧急回滚阀门。
- 回滚策略：关闭 WAIT/Root/TaskCard/epoch 功能后，系统回到现有 mention gating + listen-only + relay/mirror（不影响基本可用性）。
- 上线观测：新增日志关键词 `telegram_waiter_wakeup` / `telegram_root_resolve`，可在 gateway 日志中检索。

## 实施拆分（Implementation Outline）
- Step 1: 定义静默协议常量与拦截点（出站前）：PASS/WAIT 不进群；WAIT 写表（先不做唤醒）。
- Step 2: 补齐入站元信息（telegramMessageId/inReplyToMessageId），并落地 MessageRootMap。
- Step 3: WaitingTable + waiter wakeup（C 路由）：未点名但命中 WAITING 的入站消息触发 run。
- Step 4: TaskCard（最小版）+ 上下文注入（root 原文 + WAITING 状态）。
- Step 5: epoch 收敛：定义触发点与作废规则；补齐测试与 /context 可观测字段。
- 受影响文件（预估）：
  - `crates/channels/src/plugin.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/sessions/*` 或 `crates/gateway/src/*`（新增 SQLite 表/DAO）

## 交叉引用（Cross References）
- Design doc：`issues/problems/v4.md`
- 相关已完成单：
  - `issues/done/issue-telegram-group-ingest-reply-decoupling.md`
  - `issues/done/issue-telegram-group-bot-to-bot-mentions-relay-via-moltis.md`
  - `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`

## 未决问题（Open Questions）
- Q1：静默协议字符串是否严格采用 `<SILENCE:PASS>` / `<SILENCE:WAIT>`（推荐按 v4 固定），还是要兼容旧形态（例如 `<SILENCE>`）？
- Q2：WAITING 过期策略：默认 TTL 多久（例如 24h）？触发后是否自动清理 WAITING？
- Q3：TaskCard V1 只存 root 原文 + 少量状态是否足够？是否需要“期望交接 expected_handoff”？
- Q4：epoch 的最小可行实现：仅阻止旧输出发群 vs 丢弃并重跑（更一致但更复杂）。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

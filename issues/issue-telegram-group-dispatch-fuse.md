# Issue: Telegram 群聊 bot 协作链缺少根消息共享派发保险丝（root_message_id / dispatch_fuse）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-24
- Owners: Codex
- Components: telegram / config / gateway
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-24：`channels.telegram` 已 hard-cut 收口为 typed `TelegramChannelsConfig { bot_dispatch_cycle_budget, accounts }`，默认预算 `128`，`0` 在配置校验阶段直接拒绝
- 2026-03-24：`crates/telegram/src/state.rs` 已重构为 Telegram 群运行时单点 owner：稳定 `participants` 单独持有，`dedupe_actions`、`message_contexts`、`root_budgets` 继续收口在 per-chat transient runtime 内
- 2026-03-24：`crates/telegram/src/handlers.rs` 与 `crates/telegram/src/outbound.rs` 已接入 `root_message_id` 懒创建、bot-to-bot 准入即扣减、缺根 fail-close、稳定顺序与 chunk 上下文传播
- 2026-03-24：`crates/gateway/src/server.rs` 启动路径已只遍历 `.accounts`，并把共享预算下发到 Telegram plugin 运行时
- 2026-03-24：已补齐 hard-cut 拒绝测试与结构化日志自动化校验，覆盖 `root_dispatch_budget_exceeded` 的 `warn -> info` 级别冻结，以及 `root_dispatch_context_missing` 的 `warn` 语义
- 2026-03-24：`crates/telegram/src/outbound.rs` 已把 Telegram reply 展示目标与协作链 lineage 解耦；非 reply 的受管 bot 群消息只要显式携带 lineage，成功发送后仍会登记到原 `root_message_id`
- 2026-03-24：`crates/telegram/src/state.rs` 已把稳定 `participants` 从 transient per-chat 协作链容器中拆出，继续由同一 runtime owner 单点持有；whole-chat TTL 与 transient chat 容量淘汰都不再连带删除群成员事实
- 2026-03-24：本 issue 已吸收原独立 spec 的冻结规则、人话例子与边界约束；删除重复设计文档，保持本单为唯一事实源

**已覆盖测试（如有）**
- 2026-03-24：`cargo test -p moltis-config --lib -- --nocapture`
- 2026-03-24：`cargo test -p moltis-telegram --lib -- --nocapture`
- 2026-03-24：`cargo test -p moltis-gateway --lib configured_telegram_accounts_uses_typed_accounts_only -- --nocapture`
- 2026-03-24：`cargo test -p moltis-gateway --lib deliver_channel_replies_waits_for_outbound_sends -- --nocapture`
- 2026-03-24：`git diff --check`

**本轮 review 阻塞（已修复）**
- 2026-03-24：`crates/telegram/src/outbound.rs` 已改为显式 lineage 传播；`reply_to_message_id` 只负责 reply 展示/目标识别，不再决定 `sent_message_id -> root_message_id` 是否登记
- 2026-03-24：`crates/telegram/src/state.rs` 已把 `participants` 与 transient chat 容器彻底解耦；whole-chat TTL 与 transient chat 容量淘汰都不会再静默压缩 sibling bot fanout 事实
- 2026-03-24：新增并通过两条阻塞回归单测，issue 恢复完成状态

**已知差异/后续优化（非阻塞）**
- 本单只补 Telegram 群聊 bot-to-bot 扩散保险丝，不恢复旧 relay-chain 机制全集
- 本单不顺手处理 Telegram 群聊正文透传问题；该问题已在 `issues/issue-telegram-group-body-integrity.md` 单独收口
- 结构化日志的 `warn/info + reason_code` 组合已在自动化测试中冻结；仍建议按下方手工口径抽查一轮真实群聊日志，确认生产样式与字段输出符合预期

---

## 背景（Background）
- 场景：Telegram 群聊中，人类或第三方 bot 的外部消息会唤起受管 bot；受管 bot 再在群里正式点名其他受管 bot 时，会继续形成 bot-to-bot 协作扩散。
- 约束：
  - 修复必须严格收敛在 Telegram 适配层与 Telegram 配置接线内闭环。
  - 不允许把 Telegram 群聊专属编排重新放回 gateway/core。
  - 不允许恢复旧 relay-chain 的 hop limit、relay depth、relay path、synthetic cycle id 等整套历史概念。
  - 不向后兼容 raw-map 保留键方案；配置边界直接 one-cut 收口为 typed config。
- Out of scope：
  - 不恢复旧 relay-chain 机制全集。
  - 不修改群聊正文透传语义。
  - 不扩展到非 Telegram 渠道。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **外部根消息**（主称呼）：发送者不属于当前受管 Telegram bot 集合、且实际开启了一条 bot 协作链的群聊消息。
  - Why：它是整条协作链共享预算的唯一合法起点。
  - Not：不是所有外部消息；若这条消息最终没有放行任何 `Dispatch`，它就不会被登记为根。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：external root turn / root message

- **`root_message_id`**（主称呼）：外部根消息自己的 Telegram `message_id`。
  - Why：它天然稳定、天然可追溯，不需要再生成额外的 synthetic cycle id。
  - Not：不是 reply-to 链、不是 session id、不是 UUID。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：dispatch root / root id

- **根预算桶**（主称呼）：`(chat_id, root_message_id)` 对应的一份共享派发预算状态，至少包含 `used`、`budget`、`warned`、`touched_at`。
  - Why：同一条外部根消息后面所有 bot-to-bot 扩散都必须从这一个桶里扣减。
  - Not：不是 per-account 配额，也不是按单条 bot 消息分别计数。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：root budget / dispatch fuse bucket

- **消息上下文**（主称呼）：`(chat_id, message_id)` 对应的一份 Telegram 群运行时事实，至少表达 `root_message_id`、`managed_author_account_handle?` 与 `touched_at`。
  - Why：reply 目标识别与根传播必须共享同一个消息级事实源，不能再拆两套平行缓存。
  - Not：不是 gateway/session 持久化元数据，也不是全量群消息归档。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：message context / message binding

- **群运行时管理器**（主称呼）：Telegram 适配层内持有群聊运行时事实的唯一 owner；其中 `participants` 是按 `chat_id` 单独索引的稳定群成员事实，`dedupe_actions`、`message_contexts`、`root_budgets` 收口在 per-chat transient runtime。
  - Why：这能把预算、上下文、去重、参与者集合关在同一责任边界里，同时避免把稳定事实和临时协作链状态绑进同一淘汰语义。
  - Not：不是跨渠道通用框架，不是新的 gateway/core 抽象，也不是必须新增 actor / 事件总线 / 后台任务。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：per-chat runtime manager

- **准入即扣减**（主称呼）：当某个目标已被判定为应进入 `Dispatch`，且保险丝允许放行时，预算在“放行这一刻”立即记为已消耗。
  - Why：本单目标是保险丝，不是精确结算器；确定性、单点语义与低复杂度优先于“成功后再回补”记账。
  - Not：不是 success accounting，也不是 reserve / commit / rollback 三段式机制。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：admit-time charging

- **保险丝降级**（主称呼）：某个目标原本会进入 `Dispatch`，但因预算耗尽或根上下文缺失，被改为 `RecordOnly`。
  - Why：这是保险丝命中的唯一行为变化。
  - Not：不是 drop，不是正文裁剪，也不是 `addressed` 语义变化。
  - Source/Method：effective

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 为 Telegram 群聊 bot-to-bot 自动派发新增一根共享派发保险丝
- [x] 保险丝以 `root_message_id` 为共享身份，不再引入 synthetic `dispatch_cycle_id`
- [x] 预算耗尽时，把原本会进入 `Dispatch` 的目标降级为 `RecordOnly`
- [x] 受管 bot 消息若缺失有效根上下文时，fail-close 为 `RecordOnly`
- [x] 为预算命中与上下文缺失补齐结构化可观测性
- [x] 把 Telegram 配置边界收口为 typed `TelegramChannelsConfig`

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须只限制 bot-to-bot 自动扩散，不限制外部首轮唤起
  - 必须让同一条外部根消息下的所有 bot 分支共享同一个根预算桶
  - 必须懒创建根预算；外部消息若最终没有放行任何 `Dispatch`，不得分配保险丝状态
  - 必须只追踪两类消息：实际开启协作链的外部根消息、以及由本系统成功发出的受管 bot 群消息
  - 必须把“受管 bot 群消息属于哪个 `root_message_id`”视为显式协作事实；不得再把 Telegram `reply_to_message_id` 形态误当成根传播的唯一事实源
  - 必须保证受管 bot 在群里的非 reply 可见消息，只要它逻辑上仍属于当前协作链，就能继承同一个 `root_message_id`
  - 必须让多目标处理顺序稳定且单次遍历，不能再受 `HashSet` 迭代顺序影响
  - 必须在 Telegram 适配层单点完成根创建、根传播、预算扣减、降级与日志
  - 不得依赖通用 Telegram `reply_to_message_id` 链条去追历史根
  - 不得恢复 hop/depth/path/relay-chain 旧机制
  - 不得把 Telegram 渠道专属复杂性扩散到 gateway/core
  - 不得引入 reserve / commit / rollback 或 handoff 成功回补预算语义
  - 不得把 `participants` 这种稳定群成员事实与 `message_contexts/root_budgets` 这种临时协作链状态绑定到同一个淘汰语义
- 兼容性：
  - 本单为 hard cut 重构；不保留 raw `HashMap<String, Value>` + 保留 key 的旧配置语义
  - 命中 legacy/冲突形状时直接报错，不做 silent degrade
- 可观测性：
  - 任何因保险丝触发的 `Dispatch -> RecordOnly` 降级都必须有结构化日志
  - 不得静默降级
- 安全与隐私：
  - 结构化日志不得打印完整正文或 token
  - 允许记录 `root_message_id`、账号、chat/thread/message 标识与预算数值

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. Telegram 群聊在 one-cut 删除旧 relay-chain 机制后，已经没有一根共享硬保险丝限制 bot-to-bot 扩散总量。
2. 当前参与者集合来自 `HashSet`，多目标处理顺序不稳定；预算若日后接在这条路径上，会天然产生“不确定先放谁”的行为。
3. 当前运行时只记“这条消息是谁发的”，不记“这条消息属于哪一个外部根消息”；因此无法对跨 bot、跨分支、跨回流链共享扣减。
4. 当前配置层把 Telegram 当成 raw map；若继续用保留 key 塞共享配置，会把“共享策略”和“账号项”混成两种事实源。

### 影响（Impact）
- 用户体验：群聊 bot 协作可能出现连续自动对话、多 bot 并发接力或部分消息顺序不稳定
- 可靠性：没有共享上限时，异常协作链会持续占用运行时与 gateway 资源
- 排障成本：若保险丝降级没有固定 `reason_code` 和结构化日志，只能靠群消息反推系统为什么停或不停

### 复现步骤（Reproduction）
1. 在 Telegram 群里由人类正式点名 bot `A`
2. 让 `A` 输出一条继续正式点名 `B`、`C` 的消息；随后 `B` 或 `C` 再继续点名别的 bot
3. 重复构造回流链或多路扇出
4. 期望 vs 实际：
   - 期望：同一外部根消息下面有共享上限；超过上限后只降级为 `RecordOnly` 并留下明确日志
   - 实际：当前只有 dedupe，没有总量保险丝，也没有根级共享扣减模型

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/config/src/schema.rs:942`：`ChannelsConfig` 目前仍把 `channels.telegram` 定义成 `HashMap<String, serde_json::Value>`
  - `crates/config/src/validate.rs:362`：配置校验同样把 `channels.telegram` 当成 raw map-of-leaf 处理，没有共享预算字段的 typed schema
  - `crates/gateway/src/server.rs:1841`：启动阶段直接遍历 `config.channels.telegram` 并把每一项当账号启动，说明“共享配置”和“账号项”当前仍混在同一层 map 语义里
  - `crates/telegram/src/state.rs:66`：`TelegramGroupRuntime` 当前只持有 `participants_by_chat`、`message_authors`、`dedupe`
  - `crates/telegram/src/state.rs:97`：`participants_for_chat()` 直接从 `HashSet` 迭代生成 `Vec`，目标顺序当前不稳定
  - `crates/telegram/src/state.rs:104`：运行时目前只能登记消息作者，不能登记所属 `root_message_id`
  - `crates/telegram/src/outbound.rs:759`：群聊出站路径会拿运行时锁并登记参与者/作者，说明群运行时已经是这类状态的自然收口点
  - `crates/telegram/src/outbound.rs:762`：当前只登记 `message_author`，没有登记根上下文
  - `crates/telegram/src/outbound.rs:807`：当前多目标处理是逐个 target 规划，但没有任何共享预算接线
  - `crates/telegram/src/outbound.rs:1172`：发送文本成功时已经可以拿到新消息的 `MessageId`，因此“发送成功即登记 `(chat_id, sent_message_id) -> root_message_id`”在现有出站链路上是可落地的
  - `crates/telegram/src/adapter.rs:871`：`plan_group_target_action(...)` 当前只基于正文、reply 与 dispatch 开关判定目标，不接收保险丝上下文
- 配置/协议证据（必要时）：
  - `crates/config/src/template.rs:566`：当前模板只展示 `[channels.telegram.<bot>]` 账号配置，没有 Telegram 渠道级共享保险丝配置
- 当前测试覆盖：
  - 已有：`crates/telegram/src/adapter.rs` 附近已有群聊 `Dispatch` / `RecordOnly` 判定测试
  - 缺口：没有测试覆盖根预算桶、根传播、稳定顺序、fail-close、或保险丝日志

## 根因分析（Root Cause）
- A. 旧 relay-chain 机制被 one-cut 删除后，没有补回一根更小、更收敛的 Telegram 适配层保险丝
- B. 当前 `handlers/outbound` 只做 dedupe；dedupe 只能防重复事件，不能限制不同消息之间的链式扩散总量
- C. 当前运行时缺少“消息属于哪个外部根消息”的事实源，因此无法做共享预算
- D. 当前配置边界仍是 raw map，天然鼓励“保留 key + 枚举时记得跳过”的脆弱方案
- E. 当前参与者顺序不稳定；如果预算扣减挂在这条链上，局部耗尽时的放行结果会变成非确定性
- F. 试图通过 generic `reply_to_message_id` 历史链去追根，会把 Telegram 客户端表现差异带进保险丝主路径，机制脆弱且不必要

## 2026-03-24 Review 复盘与修复口径（Blocking Follow-up）
### 回归 1：非 reply 的受管 bot 群消息丢失根传播
- 现象：
  - 当前出站根传播在 `crates/telegram/src/outbound.rs` 上把 `reply_to_message_id` 当成前置条件。
  - 一旦受管 bot 在群里发送“不是 reply，但逻辑上仍属于当前协作链”的可见消息，新 `sent_message_id` 就不会登记 `root_message_id`。
  - 后续这条 bot 消息再正式点名其他 bot 时，会因 `root_dispatch_context_missing` 被错误降级为 `RecordOnly`。
- 第一性判断：
  - `root_message_id` 是协作链事实，不是 Telegram 展示形态。
  - `reply_to_message_id` 只负责 reply 目标识别与线程呈现，不得再承担根传播事实源职责。
- 冻结决策：
  - 根传播必须改为显式 lineage 传递；Telegram 出站主路径要么拿到当前协作链的显式 `root_message_id`，要么严格 fail-close。
  - 禁止在 `reply_to_message_id = None` 时通过“最近一条消息”“thread id”“最后一次发言 bot”等启发式方式猜根。

### 回归 2：whole-chat TTL 误删稳定 `participants`
- 现象：
  - 当前 per-chat runtime 把 `participants`、`message_contexts`、`root_budgets` 绑在同一个 chat 容器里，并按 whole-chat TTL 整体回收。
  - 低活跃共享群一旦超过 TTL，系统会忘掉该群里已有的 sibling bots；之后 source bot 再发消息时，只会重新登记自己，跨 bot fanout 被错误收缩。
- 第一性判断：
  - `participants` 是群内受管 bot 拓扑事实；`message_contexts/root_budgets` 是协作链临时状态。
  - 二者可以继续由同一个 runtime owner 持有，但不得共享同一淘汰语义。
- 冻结决策：
  - 保留 runtime 单点所有权，不新增外部通用抽象。
  - 取消“chat 整体过期 = participants 一并消失”的现实现状；只允许对临时协作链状态做 TTL/上限清理。
  - 禁止为了保住 whole-chat TTL 再引入 participant restore/fallback/猜测恢复逻辑。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 新增 Telegram 渠道级共享配置：
    - `[channels.telegram]`
    - `bot_dispatch_cycle_budget = 128`
  - 内部唯一事实源必须是 typed `TelegramChannelsConfig { bot_dispatch_cycle_budget, accounts }`
  - `bot_dispatch_cycle_budget` 类型必须是 `u32`
  - 未显式配置时默认值必须是 `128`
  - `bot_dispatch_cycle_budget = 0` 必须在配置校验阶段直接报错
  - 协作链身份必须直接使用外部根消息自己的 Telegram `message_id`，即 `root_message_id`
  - 外部消息只有在“至少放行了一个 `Dispatch`”时，才允许懒创建根预算桶与根消息上下文
  - 外部首轮消息即使同时命中多个 bot，这些首轮分支也必须共享同一个 `root_message_id`
  - 外部首轮放行本身不消耗预算；预算只约束后续 bot-to-bot 自动扩散
  - 受管 bot 下游派发必须采用“准入即扣减”：某个目标被放行为 `Dispatch` 的当下，立即把该根预算 `used += 1`
  - 下游 handoff 后续即使失败，也不得回补预算；这是保险丝，不是成功结算器
  - 受管 bot 群消息一旦发送成功，必须立即登记 `(chat_id, sent_message_id) -> { root_message_id, managed_author_account_handle }`
  - 上述登记必须由显式 lineage 上下文驱动，而不是以 `reply_to_message_id` 是否存在作为前置条件
  - 若一次发送因分片/分块产生多个 Telegram `message_id`，则每个成功返回的 `message_id` 都必须登记到同一个 `root_message_id`；不得只登记首条
  - Telegram 分片/分块只影响消息上下文登记，不改变预算计费单位；同一次 source->target 放行无论拆成多少片，都只扣 1 次预算
  - 后续任何由受管 bot 发出的群消息，若要继续触发下游派发，必须优先从“这条消息自己的 `(chat_id, message_id)`”读取 `root_message_id`
  - 这里禁止的是“用 generic reply 历史链追根”，不是禁止现有 reply 目标识别；reply 语义仍只负责判定目标，不负责决定属于哪个根
  - 保险丝追踪范围必须只包含两类消息：实际开启协作链的外部根消息、以及由本系统成功发出的受管 bot 群消息
  - `participants` 必须作为 runtime 持有的稳定群成员事实保留；低活跃或临时协作链状态过期，不得导致 sibling bots 从 fanout 视图中消失
  - 多目标处理必须采用稳定顺序并单次遍历；同一条消息预算不够时，必须稳定地“前面的目标继续 `Dispatch`、后面的目标降级为 `RecordOnly`”
  - 为收敛口径，稳定顺序冻结为：目标账号句柄按字典序升序处理
  - 稳定顺序的产出必须只有一个事实源；实现上应集中在单一 helper 或运行时快照出口完成，禁止在多个调用点各自排序各自解释
  - 受管 bot 消息若原本会触发下游 `Dispatch`，但无法解析出有效 `root_message_id` 或找不到对应根预算桶，必须 fail-close：降级为 `RecordOnly` 并记录结构化日志
  - 进程重启后，旧协作链的运行时状态必须清空；旧 bot 链若继续冒出消息，必须按“上下文缺失” fail-close，而不是尝试跨重启续链
  - 任何因保险丝触发的 `Dispatch -> RecordOnly` 降级都必须输出结构化日志
- 不得：
  - 不得再引入 synthetic `dispatch_cycle_id`
  - 不得把预算按 bot 账号分别配置或分别计算
  - 不得依赖 generic Telegram `reply_to_message_id` 历史链去追根
  - 不得引入 reserve / commit / rollback、成功回补、或 `handle_inbound() -> Result<()>` 这类为精确结算服务的额外契约
  - 不得追踪所有群消息
  - 不得在 raw `HashMap<String, Value>` 上继续做“保留 key + 启动时跳过”的旧方案
  - 不得在预算耗尽或上下文缺失时 drop 消息、裁剪正文、伪造新的 session/system message、或静默绕过保险丝
- 应当：
  - 应当把 `participants` 与 transient 协作链状态继续收口在同一个 Telegram runtime owner 内，但 `participants` 不得再和 `dedupe_actions`、`message_contexts`、`root_budgets` 共用 per-chat transient 容器
  - 应当沿用现有 Telegram 出站成功拿到 `MessageId` 的主路径做根传播，而不是额外追 Telegram reply 历史链
  - 应当为 `message_contexts` 与 `root_budgets` 提供统一的 TTL / 上限清理，确保进程内状态有界

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐：`root_message_id` 共享预算）
- 核心思路：
  - 用外部根消息自己的 `message_id` 当作唯一根身份
  - 用 Telegram runtime 单点 owner 持有稳定参与者集合，并为每个 chat 持有 transient 去重、消息上下文、根预算桶
  - 外部根消息在“首次真的放行了某个 `Dispatch`”时懒创建根预算桶
  - bot-to-bot 目标一旦被放行就立即扣减预算，不再做成功回补
  - 受管 bot 消息发送成功后立刻把新 `message_id` 绑定到同一个 `root_message_id`
- 优点：
  - 没有 synthetic id，没有 rollback 契约，没有 reply 链追溯，机制最短
  - 根、预算、消息上下文都收口在 Telegram 运行时单点，天然高内聚
  - 进程重启后的行为天然安全：状态丢了就 fail-close，不会偷偷续链
- 风险/缺点：
  - 下游 handoff 失败也会消耗预算，这是刻意选择的保险丝语义
  - 需要把配置边界从 raw map 一次性收口为 typed config

#### 方案 2（不推荐：沿 reply-to 历史链找根）
- 核心思路：依赖 Telegram 消息自带的 `reply_to_message_id` 一路回溯，试图从历史消息里找外部首轮根
- 风险/缺点：
  - Telegram 客户端消息不保证都有 reply-to
  - 任意历史链断点都会让保险丝主路径变脆弱
  - 会把“是否 reply 样式”这种 UI/客户端差异误当成系统级事实源

#### 方案 3（不推荐：synthetic cycle id + success accounting）
- 核心思路：重新引入 `dispatch_cycle_id`，并做 reserve / commit / rollback 三段式精确记账
- 风险/缺点：
  - 纯概念膨胀：要额外引入 synthetic id、回滚路径、成功信号契约
  - 为了“精确结算”把复杂度扩散到 gateway/core 或 bridge 契约，得不偿失
  - 与本单“高内聚、单点收口、最小闭环”的目标相反

### 最终方案（Chosen Approach）
- 采用方案 1

#### 行为规范（Normative Rules）
- 规则 1：配置入口冻结为 `channels.telegram.bot_dispatch_cycle_budget`，内部唯一事实源为 typed `TelegramChannelsConfig`
- 规则 2：Telegram 账号枚举、校验、启动都只能遍历 `TelegramChannelsConfig.accounts`
- 规则 3：外部消息先按现有 planner 产出候选目标；如果最终没有任何 `Dispatch` 被放行，则不创建任何根状态
- 规则 4：外部消息一旦至少放行了一个 `Dispatch`，就以该消息自己的 Telegram `message_id` 作为 `root_message_id` 懒创建根预算桶；同一消息首轮命中的多个 bot 共用这一根
- 规则 5：bot-to-bot 下游目标按 `target_account_handle` 字典序升序处理，预算判定与降级只能沿这一个稳定顺序单次遍历；这里故意不用正文 mention 出现顺序，避免再引入第二套排序事实源
- 规则 6：某个 bot-to-bot 目标被放行为 `Dispatch` 的当下，立即消耗 1 个预算单位；不做 commit / rollback
- 规则 7：受管 bot 群消息发送成功后，立即把新 `sent_message_id` 写入消息上下文，并继承当前 `root_message_id`
- 规则 8：后续处理受管 bot 群消息时，只能用“当前消息自己的 `(chat_id, message_id)`”查根；不回溯 generic reply 历史链
- 规则 9：若受管 bot 消息原本会 `Dispatch`，但查不到有效根上下文或根预算桶，则 fail-close 为 `RecordOnly`
- 规则 10：根预算桶与消息上下文都是纯进程内状态；TTL/上限淘汰与重启丢失都按 fail-close 处理
- 规则 11：保险丝日志只在“原本会 dispatch，但因预算耗尽或上下文缺失被降级”为真时触发

#### 接口与数据结构（Contracts）
- API/RPC：
  - 无新增外部 API
  - `TelegramCoreBridge::handle_inbound(...)` 不做成功/失败返回值改造；预算语义不依赖它
- Telegram 内部出站协作上下文：
  - Telegram 出站内部主路径必须显式携带“当前消息属于哪个 `root_message_id`”的 lineage 上下文
  - 该上下文是 Telegram 适配层内部 contract，不向 gateway/core 扩散新的通用概念
  - `reply_to_message_id` 继续用于 Telegram reply 展示/目标识别，但不得再充当 lineage 唯一事实源
- 配置：
  - 外部 TOML：
    - `[channels.telegram]`
    - `bot_dispatch_cycle_budget = 128`
  - 内部 typed config：`TelegramChannelsConfig { bot_dispatch_cycle_budget, accounts }`
  - `accounts` 是 Telegram 账号唯一来源；共享保险丝配置不得继续混在 raw map 枚举路径里
- 运行时状态：
  - `TelegramGroupRuntime` 应重构为 Telegram 群运行时单点 owner
  - 其中：
    - `participants` 是按 `chat_id` 索引的稳定群成员事实
    - `dedupe_actions`、`message_contexts`、`root_budgets` 是 per-chat transient 协作链状态
  - 允许它们继续由同一 runtime owner 持有，但不得共享“chat 整体 TTL 到期后一并删掉”或“transient chat 容量淘汰连带删除”的淘汰语义
  - `message_contexts[(chat_id, message_id)]` 至少表达：
    - `root_message_id`
    - `managed_author_account_handle: Option<String>`（外部根消息为 `None`，受管 bot 消息为 `Some(...)`）
    - `touched_at`
  - `message_contexts` 既是 reply 目标识别的作者事实源，也是根传播事实源；不得再保留平行的 `message_authors` 旧表
  - `root_budgets[(chat_id, root_message_id)]` 至少表达：
    - `used`
    - `budget`
    - `warned`
    - `touched_at`
- UI/Debug 展示（如适用）：
  - 本单不新增 UI 面板
  - 结构化日志是首要可观测性闭环

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - `bot_dispatch_cycle_budget = 0`：配置校验直接报错
  - 根预算耗尽：目标从 `Dispatch` 降级为 `RecordOnly`
  - 有受管 bot 消息、且原本会 `Dispatch`，但查不到有效根上下文或根预算桶：fail-close 为 `RecordOnly`
  - 进程重启或 TTL 淘汰后旧链再冒消息：同样按“上下文缺失” fail-close
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - dedupe hit：不占预算，不新增根状态
  - 外部消息最终没有放行任何 `Dispatch`：不新增根状态
  - 根预算与消息上下文：按统一 TTL / 上限淘汰，避免无界增长

#### 安全与隐私（Security/Privacy）
- 日志级别：
  - `root_dispatch_budget_exceeded`：同一 `(chat_id, root_message_id)` 首次命中 `warn`，后续命中 `info`
  - `root_dispatch_context_missing`：固定 `warn`
- 默认日志字段：
  - `event = "telegram.group.dispatch_fuse"`
  - `decision = "downgrade_to_record"`
  - `policy = "group_record_dispatch_v3"`
  - `reason_code`
  - `root_message_id`
  - `used`
  - `budget`
  - `chat_id`
  - `thread_id`
  - `source_account_handle`
  - `target_account_handle`
  - `message_id`
- 可选字段：
  - `remediation = "start a new external turn or increase channels.telegram.bot_dispatch_cycle_budget"`
- 禁止打印字段：
  - 完整正文
  - token
  - 其他敏感认证信息

## 人话例子（Examples）
### 例子 A：最基本单链
```text
人类消息 m100 -> A
A 发消息 m101 -> B
B 发消息 m102 -> A
A 发消息 m103 -> C
```

- 若 `bot_dispatch_cycle_budget = 2`，则 `m100` 首轮放行给 A 不扣预算
- `m101 -> B` 放行，扣 1
- `m102 -> A` 放行，扣 1
- `m103 -> C` 再想放行时，预算已满，只能降级为 `RecordOnly`

### 例子 B：为什么根传播不靠 reply-to
```text
人类消息 m100 -> A
A 成功发出 m101，并在正文里点名 B
```

- send 成功那一刻就登记 `m101 -> root = m100`
- 后面看到 `m101` 时，直接查 `m101` 自己的上下文
- 查到 `root = m100`，就从 `m100` 这桶预算里继续扣
- 整个过程不需要依赖 `m101.reply_to_message_id`

### 例子 C：一次性点名多个 bot
```text
人类消息 m100 -> A
A 发消息 m101，正文同时点名 C、B、D
```

- 假设此时根预算只剩 `2`
- 稳定顺序冻结为 `B / C / D`
- 所以结果必须稳定为：`B -> Dispatch`、`C -> Dispatch`、`D -> RecordOnly`

### 例子 D：进程重启后的旧链
```text
人类消息 m100 -> A
A 成功发出 m101 -> B
此时进程重启
稍后 Telegram 又把 m101 相关后续事件送到系统
```

- 重启后内存状态已清空，系统已不知道 `m101` 属于哪个 `root_message_id`
- 所以后续不能继续 bot-to-bot 放行
- 正确行为是：`RecordOnly + root_dispatch_context_missing`

## 并发与回收边界（Concurrency & Eviction Boundaries）
- 同一 `chat` 的运行时状态必须由同一个 runtime owner 串行化访问
- 根预算、消息上下文、参与者集合、dedupe 必须在这个边界内统一更新
- 不允许把“写消息上下文”和“扣预算”分散到多个互不知情的锁或缓存里
- `message_contexts` 与 `root_budgets` 必须共享统一的 `touched_at` 刷新与淘汰策略
- 淘汰后的行为统一按 fail-close 处理，不做磁盘持久化，也不跨重启恢复

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `channels.telegram` 已收口为 typed `TelegramChannelsConfig`，Telegram 账号枚举只来自 `.accounts`
- [x] `bot_dispatch_cycle_budget` 默认值为 `128`，且 `0` 会被明确拒绝
- [x] 外部消息只有在至少放行一个 `Dispatch` 时才会创建根预算桶
- [x] 同一条外部根消息首轮同时命中多个 bot 时，共享同一个 `root_message_id` 与根预算桶
- [x] 外部首轮放行本身不消耗预算
- [x] bot-to-bot 单链派发按“每个放行目标 1 次”扣减
- [x] 同一条 bot 消息多目标处理时，顺序稳定且预算不足时会稳定地前放后拦
- [x] 下游 handoff 失败不会回补预算；该行为有明确测试和文档冻结
- [x] 受管 bot 群消息即使不是 reply 发送，只要逻辑上仍属于当前协作链，也会用显式 lineage 继承同一个 `root_message_id`
- [x] 根传播 contract 已与 `reply_to_message_id` 解耦；reply 形态不再决定是否登记 `sent_message_id -> root_message_id`
- [x] 继续派发时读取的是“当前消息自己的上下文”，而不是 generic reply 历史链
- [x] 受管 bot 消息缺失有效根上下文或根预算桶时，会 fail-close 为 `RecordOnly`
- [x] 低活跃共享群即使临时协作链状态过期，`participants` 仍保留 sibling bot fanout 所需事实，不会因 whole-chat TTL 被静默压缩
- [x] 进程重启/状态淘汰后的旧链消息不会绕过保险丝，而是按上下文缺失处理
- [x] 预算耗尽时，正文与 `addressed` 保持不变，只降级 `mode`
- [x] 同一根预算首次命中日志为 `warn`，后续命中为 `info`

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/config/src/schema.rs` / `crates/config/src/validate.rs`：覆盖 `TelegramChannelsConfig` 反序列化、默认值 `128`、`bot_dispatch_cycle_budget = 0` 校验失败
- [x] `crates/gateway/src/server.rs` 或相邻测试：覆盖 Telegram 启动只遍历 `TelegramChannelsConfig.accounts`
- [x] `crates/telegram/src/state.rs`：覆盖 per-chat 运行时管理器的根预算桶懒创建、消息上下文写入、稳定目标顺序、chunk 预算口径与首次/重复溢出语义
- [x] `crates/telegram/src/handlers.rs` 或相邻单测：覆盖外部消息无 `Dispatch` 时不创建根状态
- [x] `crates/telegram/src/handlers.rs` 或相邻单测：覆盖同一外部根消息首轮多目标共享同一个 `root_message_id`
- [x] `crates/telegram/src/state.rs` / `crates/telegram/src/outbound.rs`：覆盖 bot-to-bot 单链扣减
- [x] `crates/telegram/src/outbound.rs` 或相邻单测：覆盖单条 bot 消息多目标的稳定顺序与局部耗尽降级
- [x] `crates/telegram/src/state.rs`：覆盖“准入即扣减”，即 handoff 成功与否都不回补预算
- [x] `crates/telegram/src/outbound.rs` 或相邻单测：覆盖受管 bot 消息发送成功后根上下文即时传播
- [x] `crates/telegram/src/outbound.rs` / `crates/telegram/src/state.rs`：覆盖消息分片/分块发送时，每个成功返回的 `message_id` 都继承同一个 `root_message_id`
- [x] `crates/telegram/src/state.rs`：覆盖单次 source->target 派发即使被 Telegram 分成多片，也只扣 1 次预算
- [x] `crates/telegram/src/outbound.rs` 或相邻单测：覆盖受管 bot 消息缺失根上下文/根预算桶时 fail-close 为 `RecordOnly`
- [x] `crates/telegram/src/outbound.rs` / `crates/telegram/src/handlers.rs`：覆盖 `root_dispatch_budget_exceeded` 与 `root_dispatch_context_missing` 的日志级别和 `reason_code`
- [x] `crates/telegram/src/outbound.rs`：新增单测覆盖“受管 bot 群消息在 `reply_to_message_id = None` 时，只要显式 lineage 存在，新的 `sent_message_id` 仍会继承正确 `root_message_id` 并继续下游 fanout”
- [x] `crates/telegram/src/state.rs` / `crates/telegram/src/outbound.rs`：新增单测覆盖“临时协作链状态过期后，低活跃共享群仍保留 `participants`，source bot 后续出站仍能看到 sibling bots 并继续稳定 fanout”
- [x] `crates/telegram/src/state.rs` / `crates/telegram/src/outbound.rs`：新增单测覆盖“transient chat 命中容量淘汰后，稳定 `participants` 仍保留，群可见出站不会因 runtime chat 回收而丢失 sibling bot fanout”

### Integration
- [x] `cargo test -p moltis-config --lib -- --nocapture`
- [x] `cargo test -p moltis-telegram --lib -- --nocapture`
- [x] `cargo test -p moltis-gateway --lib configured_telegram_accounts_uses_typed_accounts_only -- --nocapture`

### UI E2E（Playwright，如适用）
- [x] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：结构化日志当前未做自动 tracing 输出捕获；此外 Telegram 真实群聊上的跨消息 runtime 行为仍建议用真群再压一轮
- 手工验证步骤：
  - 配置较小预算，例如 `bot_dispatch_cycle_budget = 4`
  - 在测试群中构造“外部消息 -> A -> B/C -> ...”的单链回流与多目标分叉
  - 验证预算耗尽后目标变为 `RecordOnly`，且日志字段、级别、正文与 `addressed` 语义符合预期
  - 重启进程后再让旧链 bot 发消息，验证其因 `root_dispatch_context_missing` 被 fail-close

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随 Telegram adapter/config 修复发布，不加 feature flag
- 回滚策略：若需回滚，仅回滚 `crates/config` 与 `crates/telegram` 相关改动；配置项删除后恢复现状
- 上线观测：重点观察 `event = "telegram.group.dispatch_fuse"`、`reason_code = "root_dispatch_budget_exceeded"`、`reason_code = "root_dispatch_context_missing"`

## 实施拆分（Implementation Outline）
- Step 1: 在 `crates/telegram/src/outbound.rs` 盘点所有“群可见出站后会调用 `register_group_visible_outbound_contexts()` / `emit_group_visible_outbound_event()`”的主路径，冻结唯一内部 lineage contract，禁止继续由 `reply_to_message_id` 隐式决定根传播
- Step 2: 在 `crates/telegram/src/handlers.rs`、`crates/telegram/src/outbound.rs` 与相邻调用点接入显式 lineage 传递；保证受管 bot 的非 reply 群消息只要仍属于当前协作链，就能登记 `sent_message_id -> root_message_id`
- Step 3: 在 `crates/telegram/src/state.rs` 把 `participants` 与临时协作链状态的淘汰语义拆开；继续保持 runtime 单点所有权，但删除“whole-chat TTL 连带删除 participants”的现实现状
- Step 4: 先补两条阻塞回归的 failing tests，再做最小实现，再补日志/回归验证；禁止顺手引入新的通用抽象、fallback 或 legacy guard
- Step 5: 通过 review 复核“唯一事实源是否只剩显式 lineage + runtime participants”“是否仍保持 Telegram 适配层单点闭环”，再恢复 issue 完成状态
- 受影响文件：
  - `crates/config/src/schema.rs`
  - `crates/config/src/validate.rs`
  - `crates/config/src/template.rs`
  - `crates/gateway/src/server.rs`
  - `crates/telegram/src/state.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/outbound.rs`
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/plugin.rs`
- 本轮阻塞回归修复的直接触点应收敛为：
  - `crates/telegram/src/outbound.rs`
  - `crates/telegram/src/state.rs`
  - `crates/telegram/src/handlers.rs`
  - 相邻 Telegram 单测
- 除非在实施中发现显式 lineage contract 无法在 Telegram 适配层内闭合，否则不得再把本轮修复扩散回 `crates/config` 或 `crates/gateway`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-telegram-group-body-integrity.md`
  - `issues/issue-telegram-group-relay-hop-limit-blocks-return-activation.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- 无新的产品/架构未决项。本轮 review 后，待实现项已收敛为两条阻塞性回归修复：
  - 显式 lineage 根传播替代 `reply_to_message_id` 前置
  - `participants` 与临时协作链状态淘汰语义拆分
- 除此之外，不再扩写新概念，不新增兼容层，不扩大到 Telegram 之外。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

# Issue: V3 Telegram 群聊执行计划 one-cut（record / dispatch / TG adapter）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-23
- Owners: TBD
- Components: telegram/gateway/config/docs
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-20：Telegram 主入站已经通过 `TelegramCoreBridge` 把 `TgInboundMode::RecordOnly|Dispatch` 送入 gateway，bridge 基础已具备：`crates/telegram/src/handlers.rs:942`、`crates/gateway/src/channel_events.rs:1869`
- 2026-03-20：gateway/core 侧已经具备统一执行 `ingest_only` / `dispatch_to_chat` 的能力，可作为最终执行器复用：`crates/gateway/src/channel_events.rs:1881`
- 2026-03-22：TG 群聊 record/dispatch 边界设计已冻结为“TG adapter 出计划，gateway/core 只执行”：`docs/src/refactor/telegram-record-dispatch-boundary.md:1`
- 2026-03-23：实施计划已二次收敛，明确不新增跨渠道通用 execution-plan 抽象；仅在 Telegram 侧复用现有 `TgInboundMode` / `TgInboundRequest` / `TelegramCoreBridge` 闭环落地。
- 2026-03-23：TG 群聊 planner 已收口到 Telegram 侧：`crates/telegram/src/adapter.rs:1199`、`crates/telegram/src/handlers.rs:866`、`crates/telegram/src/outbound.rs:808`；连续行首 mention、reply-to、presence、同 bot 去重与 merge 统一由同一路径处理。
- 2026-03-23：gateway 已删除旧 TG 群聊 mirror/relay 主链编排，仅保留 `record` / `dispatch` 执行器与严格 bucket/session 绑定：`crates/gateway/src/chat.rs`、`crates/gateway/src/state.rs`、`crates/gateway/src/channel_events.rs`。
- 2026-03-23：Telegram 渠道配置/UI 已只保留 `group_line_start_mention_dispatch` 与 `group_reply_to_dispatch` 两个群聊 dispatch 开关：`crates/gateway/src/channel.rs:75`、`crates/gateway/src/assets/js/page-channels.js:531`、`crates/gateway/src/assets/js/onboarding-view.js:2028`。

**已覆盖测试（如有）**
- `check_bot_mentioned` 已覆盖 reply-to-bot 触发：`crates/telegram/src/handlers.rs:6786`
- Telegram bridge 已覆盖 `RecordOnly` / `Dispatch` 两种模式进入 gateway：`crates/telegram/src/handlers.rs:4503`
- planner 单测已覆盖多 mention / mention+reply-to 去重 / presence / 关闭 dispatch 开关：`crates/telegram/src/adapter.rs:1199`、`crates/telegram/src/adapter.rs:1219`、`crates/telegram/src/adapter.rs:1256`、`crates/telegram/src/adapter.rs:1276`、`crates/telegram/src/adapter.rs:1296`
- handlers / outbound 已覆盖群聊入站与 group-visible 发言共用 planner：`crates/telegram/src/handlers.rs:5605`、`crates/telegram/src/outbound.rs:3019`
- gateway 严格 bucket/session 路径已补齐回归：`crates/gateway/src/channel_events.rs:2585`、`crates/gateway/src/channel_events.rs:2894`、`crates/gateway/src/channel_events.rs:3071`、`crates/gateway/src/channel_events.rs:3202`、`crates/gateway/src/channel_events.rs:3357`
- 2026-03-23：已执行 `cargo test -p moltis-telegram`、`cargo test -p moltis-gateway`、`cargo check -p moltis --bin moltis` 全绿。

**已知差异/后续优化（非阻塞）**
- 本单不处理非消息型事件的完整产品策略；相关口径先留白，但不得因此引入 `record` / `dispatch` 之外的新执行语义。
- 本单不重做 Telegram 发送侧 retry/typing/location 等既有通道执行机制，只收口群聊入站规划职责。

---

## 背景（Background）
- 场景：Telegram 群聊原生入站消息，以及本地 bot 在同一群里的 group-visible 发言事件，都必须先由 TG adapter 展开成 bot 视角 `execution_plan`，再交给 gateway/core 逐条执行。
- 约束：
  - 严格遵守 v3 one-cut 口径，不保留 fallback、alias、compat shim、silent degrade。
  - gateway/core 与 Telegram 之间的执行语义只允许保留 `record` / `dispatch`。
  - 群聊正文在进入 gateway/core 前必须已由 TG adapter 整理为最终文本；gateway/core 不再二次理解 Telegram 群策略。
- Out of scope：
  - 不改 DM 主语义，DM 仍然是一条入站对应一个 `dispatch`。
  - 不改最终持久化承载格式。
  - 不在本单实现非消息型事件（reaction、delete、edit、button click）的完整产品策略。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`execution_plan`**（主称呼）：TG adapter 针对一条 Telegram 入站展开出的 `0..N` 条 bot 视角动作集合。
  - Why：这是本单的核心交付物，gateway/core 只消费它。
  - Not：不是新的跨渠道公共业务概念，也不是新的执行语义；本单也**不要求**它必须落成一个新的公共 Rust 类型。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：planner output / TG plan

- **`record`**（主称呼）：将该条 bot 视角消息写入会话/历史，但不触发 run。
  - Why：群聊环境事实必须能进入上下文。
  - Not：不是“忽略”或“静默丢弃”。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：listen-only / ingest-only

- **`dispatch`**（主称呼）：将该条 bot 视角消息写入会话/历史，并触发 run。
  - Why：这是唯一允许触发 bot 主处理链的群聊执行语义。
  - Not：不是 mirror / relay 的别名。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：wake / activate

- **`addressed`**（主称呼）：对某个 bot 来说，这条群聊消息明确要求它处理。
  - Why：它决定某条 bot 视角消息应是 `dispatch` 还是仅 `record`。
  - Not：不等于“消息里出现过 bot 名字”，也不等于“gateway 以后要不要跑补偿逻辑”。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：explicitly targeted

- **`tg_dedupe`**（主称呼）：TG adapter 在把执行计划交给 gateway/core 前做的 Telegram 侧去重/合并。
  - Why：同一 TG 入站 + 同一 bot 不能因为多个触发原因被重复执行。
  - Not：不是 gateway/core 的通用消息去重。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：planner dedupe

- **authoritative**：来自 Telegram 事件本身或 adapter 已持有的真实路由/线程/消息引用。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给下游执行器的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] TG adapter 为群聊入站产出 `execution_plan`，将一条 Telegram 入站展开为 `0..N` 条 bot 视角的 `record` / `dispatch`。
- [x] planner 的输入范围必须同时覆盖：Telegram 原生群聊入站消息，以及本地 bot 在同一群中发出的 group-visible 消息事件（旧 mirror / relay 的真实来源）。
- [x] gateway/core 不再在主链中自行判断 Telegram 群聊 mirror / relay / mention / reply-to 规则，只按计划执行。
- [x] 支持两个显式开关：
  - 行首 mention（包括人->bot、bot->bot、连续多个行首 `@`）是否触发 `dispatch`
  - reply-to（包括人 reply bot、bot reply bot）是否触发 `dispatch`
- [x] TG adapter 在计划阶段完成 Telegram 侧去重与合并：
  - 同一 TG 入站 + 同一 bot，最终交给 gateway/core 的动作只能是 `0` 或 `1` 条
  - `dispatch` 优先级高于 `record`
  - mention 与 reply-to 同时命中同一个 bot时，只允许一次最终 `dispatch`

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：TG 群聊专属复杂规则只存在于 TG adapter，不再散落在 gateway chat 主链。
  - 必须：同一 bot 在同一条群聊消息里不得因多个触发原因收到多次 `dispatch` 或多次重复 `record`。
  - 必须：连续行首多个 `@bot_a @bot_b @bot_c` 能为多个 bot 生成多条独立动作，但每个 bot 至多一条。
  - 不得：gateway/core 再次解析 Telegram 群聊 mention/reply-to 来“猜”是否应触发。
  - 不得：用 mirror / relay / reply-to wakeup 作为新的公共执行语义透传给 core。
- 兼容性：本单按 strict one-cut 实施，不保留 gateway 旧 mirror/relay 编排与 TG planner 并行的双轨路径。
- 可观测性：
  - 计划命中、去重、合并、硬拒绝、策略拦截都必须记录结构化日志。
  - 日志至少包含 `event`、`reason_code`、`decision`、`policy`；上下文允许时补 `chat_id`、`thread_id`、`target_account_key`、`message_id`。
- 安全与隐私：
  - 日志不得打印完整消息正文。
  - 如需辅助排障，只允许正文短预览或哈希。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) gateway 仍在主链中自行实现 Telegram 群聊 mirror/relay 编排、hop/budget/dedupe 与跨 session 注入，TG adapter 只负责很薄的一层模式判断。
2) `reply-to-bot` 当前直接并入 `check_bot_mentioned()` 的唤醒判定，缺少“reply-to 是否 dispatch”的独立用户开关。
3) 同一条帖子里多个行首 mention、或 mention + reply-to 共同命中同一 bot，现有结构仍可能导致重复转派或重复记录。

### 影响（Impact）
- 用户体验：
  - 同一帖子可能对同一 bot 触发两次 relay/dispatch。
  - 用户无法明确配置“行首 mention”和“reply-to”两种触发条件。
- 可靠性：
  - TG 群聊策略散落在 gateway 与 telegram 两层，后续 one-cut 实施时极易漏改。
  - 同一 TG 入站在不同路径下可能被重复执行或被不一致地展开。
- 排障成本：
  - 现在既要看 `crates/telegram/src/handlers.rs`，又要看 `crates/gateway/src/chat.rs` 才能理解一条群消息为何被 mirror/relay。

### 复现步骤（Reproduction）
1. 在 Telegram 群中，由 `bot_c` 发出：
   ```text
   @bot_a 请处理 A

   @bot_b 请处理 B
   ```
2. 继续构造同帖 `mention + reply-to` 同时命中同一 bot 的场景。
3. 期望 vs 实际：
   - 期望：每个目标 bot 最终只收到一次 `dispatch`，且由 TG adapter 产出计划后交给 gateway 执行。
   - 实际：gateway 仍自行构建 relay directives，并仅按 `(target, task)` 粗粒度去重。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/chat.rs:7129`：`maybe_relay_telegram_group_mentions()` 仍在 gateway 主链解析 Telegram 群聊 mention 并构建 relay directives。
  - `crates/gateway/src/chat.rs:7384`：当前只按 `(target_account_id, task_text)` 去重，无法满足“同一 TG 入站 + 同一 bot 最终只一条动作”的口径。
  - `crates/gateway/src/chat.rs:7729`：`maybe_mirror_telegram_group_reply()` 仍在 gateway 主链做 TG 群聊 mirror 补偿写入。
  - `crates/gateway/src/chat.rs:8073`、`crates/gateway/src/chat.rs:8085`：channel reply 发送后仍直接回调 gateway 内的 mirror/relay 逻辑。
  - `crates/telegram/src/handlers.rs:942`：当前群聊只按 `bot_mentioned` 与 `mention_mode` 在 `RecordOnly` / `Dispatch` 间二选一，尚未产出多 bot 执行计划。
  - `crates/telegram/src/handlers.rs:3667`：`check_bot_mentioned()` 当前把 reply-to-bot 直接视为显式激活。
  - `crates/telegram/src/config.rs:71`、`crates/telegram/src/config.rs:83`：现有 Telegram config 有 `mention_mode`、`relay_chain_enabled`、`relay_hop_limit`、`epoch_relay_budget`、`relay_strictness`，但没有独立的 mention / reply-to dispatch 开关。
- 当前测试覆盖：
  - 已有：`crates/telegram/src/handlers.rs:6786` 覆盖 reply-to-bot 被视为 mentioned；`crates/telegram/src/handlers.rs:4503` 覆盖 bridge 模式分流。
  - 缺口：缺少“同一 TG 入站 + 同一 bot 最终只有一次动作”“mention + reply-to 同时命中去重”“连续行首多个 mention 多 bot 展开”的 planner 级测试。

## 根因分析（Root Cause）
- A. 现有 bridge 只把单条入站压成一个 `TgInboundMode`，没有“多 bot 视角展开”的正式执行计划层。
- B. 旧 mirror/relay 语义长期堆在 gateway 主链，导致 gateway 不得不理解 Telegram 群聊规则、去重、hop/budget 与跨 bot 注入。
- C. Telegram 侧缺少统一的 planner/dedupe 规则与配置开关，reply-to 与 mention 仍混在同一“是否被点名”的粗粒度判定里。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - TG adapter 针对每条群聊消息产出 `execution_plan`，每条动作只允许是 `record` 或 `dispatch`。
  - gateway/core 只执行计划，不再在主链中解析 Telegram 群聊 mirror/relay/mention/reply-to。
  - DM 入站仍是一条消息对应一条 `dispatch`。
  - Group 入站允许展开为 `0..N` 条动作；每个 bot 至多一条最终动作。
  - 同一 bot 若同时命中 `record` 与 `dispatch`，最终只保留 `dispatch`。
  - 同一 bot 若命中多个触发片段，最终只发送一次动作，正文按原始出现顺序合并有效任务片段。
- 不得：
  - 不得让 gateway/core 再持有 Telegram 群聊 mirror/relay 的独立执行编排函数。
  - 不得用“双轨模式”同时保留新 planner 和旧 gateway 编排作为长期稳态。
  - 不得对同一 TG 入站 + 同一 bot 产生重复 `record` 或重复 `dispatch`。
- 应当：
  - 应保留 hop/budget/strictness，但它们应当成为 TG planner 的内部策略与观测字段，而不是 gateway/core 公共概念。
  - 应把硬拒绝场景显式记录为结构化决策，而不是静默吞掉。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 在 TG adapter 增加正式的群聊 planner。
  - planner 负责多 bot 展开、`record` / `dispatch` 判定、配置开关、去重与合并。
  - gateway/core 只接收 planner 已整理好的动作集合并执行。
- 优点：
  - 与 `docs/src/refactor/telegram-record-dispatch-boundary.md` 完全一致。
  - 职责清晰，后续并行实施与 review 边界稳定。
- 风险/缺点：
  - 需要一次性把 gateway chat 主链中的旧 mirror/relay 编排彻底清理掉。

#### 方案 2（备选）
- 核心思路：
  - 保留 gateway 中的 mirror/relay 主链，只在 Telegram 侧补少量 helper 与配置开关。
- 优点：
  - 短期改动看起来更少。
- 风险/缺点：
  - 与 one-cut 口径冲突。
  - 会继续让 gateway 理解 Telegram 群聊语义，不具备后续实施基础。

### 最终方案（Chosen Approach）
- 采用方案 1。

#### 收敛实施约束（Implementation Constraints）
- 必须复用现有 `TgInboundMode`、`TgInboundRequest`、`TelegramCoreBridge`、gateway `ingest_only` / `dispatch_to_chat` 执行链；**不得**为本单再引入新的跨渠道 execution-plan 公共类型、额外调度总线或新的 gateway/core 语义层。
- Telegram 群聊规则必须收口在 `crates/telegram/src/adapter.rs`、`crates/telegram/src/handlers.rs`、`crates/telegram/src/outbound.rs`；gateway 侧本单只允许做“删除旧 TG 专属逻辑 + 复用既有执行器 + 必要配置/UI 清理”。
- 配置面只允许保留两个群聊 dispatch 开关：`group_line_start_mention_dispatch`、`group_reply_to_dispatch`；**不得**继续扩出第三套群聊策略配置。
- 测试面只保留 4 类关键路径：planner 单测、handlers 入站集成、outbound group-visible 集成、gateway 回归/编译验证；**不得**用大量 case 堆砌 legacy/fallback 已移除这一事实。
- 本单不再额外抽象 hop/budget/strictness 对外契约；若仍保留，只允许继续作为 Telegram 内部状态或观测字段存在。

#### 明确不做（Non-goals for This Implementation）
- 不新建跨渠道通用 `ExecutionPlan`/`DispatchPlan`/`RecordPlan` 类型。
- 不为非消息型事件补齐完整产品策略。
- 不保留 gateway mirror/relay 与 Telegram planner 并行运行的双轨模式。
- 不因为本单去改造 session/persistence 主格式或引入自动迁移。

#### 行为规范（Normative Rules）
- 规则 1：DM 入站
  - 一律产出 1 条 `dispatch`。
  - 不走群聊 planner 多 bot 展开。
- 规则 2：Group 入站
  - TG adapter 先识别参与 bot 集合，再按 bot 视角生成 `record` / `dispatch` 候选。
  - 参与 bot 集合的真值来源，必须是 Telegram runtime 当前注册的 bot 账户快照与该事件可见的 group context；不得由 gateway 通过扫描 session 或历史去反推。
  - planner 的输入事件必须同时覆盖：Telegram 原生群聊入站消息，以及本地 bot 在同一群中的 group-visible 发言事件。
  - 候选计划在交给 gateway/core 前必须做 TG 侧去重与合并。
- 规则 3：dispatch 开关
  - 行首 mention 是否触发 `dispatch`，由独立配置控制。
  - reply-to 是否触发 `dispatch`，由独立配置控制。
  - 两种开关对人->bot 与 bot->bot 一视同仁。
- 规则 4：多 mention
  - 同一条消息中，连续行首多个 mention 可以同时命中多个 bot。
  - 每个 bot 最终只能得到一条动作。
- 规则 5：去重优先级
  - 同一 TG 入站 + 同一 bot：
    - 重复 `record` 仅保留一条。
    - 重复 `dispatch` 仅保留一条。
    - 若同时存在 `record` 与 `dispatch`，仅保留 `dispatch`。
- 规则 6：显式硬拒绝
  - 命中 TG 明确拒绝规则的消息可以产出 `0` 条动作。
  - 这些场景必须留下结构化拒绝日志。

#### 接口与数据结构（Contracts）
- 约束说明：
  - `execution_plan` 在本单里是**概念口径**，允许直接由 Telegram 侧以 `0..N` 次既有 `TgInboundRequest` / `TelegramCoreBridge` 调用来表达；不要求新增独立的公共 plan 结构体。
- TG planner 的输入真值：
  - `telegram runtime registered bot snapshots`
  - 当前 group / thread 路由事实
  - 原生群聊入站正文或本地 bot group-visible 发言正文
- TG planner 对 gateway/core 的最小交付语义：
  - `target_account_key`
  - `decision`：`record` | `dispatch`
  - `text`：已整理好的最终正文
  - `route/private_target/reply_target`：执行所需的 Telegram 路由事实
  - `reason_code`：仅用于内部观测，不作为新的公共执行语义
- gateway/core 对上述计划的责任边界：
  - `record` -> 写历史，不触发 run
  - `dispatch` -> 写历史，并触发 run
  - 群聊 `text` 视为 TG adapter 已产出的最终正文（TG-GST v1 或该 issue 冻结的群聊文本口径），不再追加 Telegram 群聊语义判断
- 配置：
  - 新增两个 Telegram 群聊 dispatch 开关：
    - `group_line_start_mention_dispatch`
    - `group_reply_to_dispatch`
  - `mention_mode` 不再承担 TG 群聊 planner 的 dispatch 决策；实现本单时应从群聊路径与对应设置项中移除，避免与新开关并存造成双重口径。
  - `relay_chain_enabled`、`relay_hop_limit`、`epoch_relay_budget`、`relay_strictness` 如继续保留，只能作为 TG planner 内部策略，gateway/core 不感知。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - planner 内部错误：拒绝生成计划并记录结构化失败日志，不允许退回 gateway 旧 mirror/relay 主链。
  - 配置非法：启动或验证阶段直接报错，不做 alias/fallback。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - gateway 删除旧 `maybe_mirror_telegram_group_reply()` / `maybe_relay_telegram_group_mentions()` 主链调用后，不再保留“失败时再走旧链”的补偿路径。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 记录 TG planner 决策时只打印短 preview 或 hash。
- 禁止打印字段清单：
  - 完整正文
  - token / secret
  - 非必要的完整 channel binding blob

## 验收标准（Acceptance Criteria）【不可省略】
- [x] TG 群聊 planner 成为唯一的多 bot 展开与 `record` / `dispatch` 决策入口。
- [x] gateway/core 主链不再包含 Telegram 专属 mirror/relay 编排逻辑。
- [x] 行首 mention 与 reply-to 两个 dispatch 开关可独立控制，TG 群聊路径不再依赖 `mention_mode` 做粗粒度 dispatch 判定。
- [x] 同一 TG 入站 + 同一 bot 最终只会进入一次 `record` 或一次 `dispatch`。
- [x] mention + reply-to 同时命中同一 bot 时，最终只保留一次 `dispatch`。
- [x] 结构化日志能说明“为什么这条 bot 视角消息被 record / dispatch / reject / merged”。
- [x] 本地 bot 在群中的 group-visible 发言事件也走同一 planner，而不是绕回 gateway 旧 mirror / relay 主链。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] Telegram planner：DM 入站一律产出 1 条 `dispatch`。
- [x] Telegram planner：群聊普通消息仅产出 `record`。
- [x] Telegram planner：连续行首多个 mention 为多个 bot 生成多条动作，但每个 bot 仅 1 条。
- [x] Telegram planner：同一 bot 同时命中 mention 与 reply-to 时，仅产出 1 条 `dispatch`。
- [x] Telegram planner：若同一 bot 同时命中 `record` 与 `dispatch`，最终仅保留 `dispatch`。

### Integration
- [x] `crates/telegram/src/handlers.rs`：群聊入站通过 planner 生成 `0..N` 条动作，并经 `TelegramCoreBridge` 交给 gateway。
- [x] 本地 bot group-visible 发言事件也进入同一 planner 路径，并完成同口径的去重与 `record` / `dispatch` 决策。
- [x] `crates/gateway/src/channel_events.rs`：gateway 仅执行 `record` / `dispatch`，不再解析 Telegram 群聊策略。
- [x] `crates/gateway/src/chat.rs`：旧 gateway mirror/relay 主链调用被移除，相关回归测试迁移到 TG planner 路径。

### UI E2E（Playwright，如适用）
- [x] `crates/gateway/src/assets/js/page-channels.js` 关联的 Telegram 渠道设置页增加两个 dispatch 开关，保存与回显正确，且旧 `mention_mode` 的群聊控制入口被同步移除。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - Telegram 真机群聊多 bot 行为仍需要一次手工验收，以确认 planner 输出与真实群内表现一致。
- 手工验证步骤：
  1. 在群内构造“普通讨论”“连续行首多 mention”“reply-to bot”“mention + reply-to 同时命中”四类消息。
  2. 观察每个 bot 的 session history 与执行次数。
  3. 确认每个 bot 在同一条 TG 入站下至多执行一次。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 作为 one-cut 主线改动一次切换，不保留长期双轨。
- 回滚策略：
  - 代码级回滚到旧版本；不提供运行时开关切回 gateway mirror/relay 主链。
- 上线观测：
  - 关注 `telegram.execution_plan.*`、`telegram.dispatch_policy.*`、`telegram.plan_rejected`、`telegram.plan_deduped` 类日志。

## 实施拆分（Implementation Outline）
- Task A（Telegram planner 单点收口）：
  - 只在 `crates/telegram/src/adapter.rs` 保留群聊 `record` / `dispatch` 判定核心。
  - `crates/telegram/src/handlers.rs` 与 `crates/telegram/src/outbound.rs` 只能复用同一 planner，不各自长出第二套 TG 群聊判定。
  - 先补最小失败测试，再修 mention / reply-to / 去重 / presence / multi-mention 主路径。
- Task B（gateway 只保留执行器）：
  - 从 `crates/gateway/src/chat.rs`、`crates/gateway/src/state.rs` 删除旧 mirror/relay 主链与其专属状态。
  - `crates/gateway/src/channel_events.rs` 只保留对 `record` / `dispatch` 的执行，不再补 Telegram 语义。
  - `crates/gateway/src/channel.rs`、`crates/gateway/src/assets/js/page-channels.js`、`crates/gateway/src/assets/js/onboarding-view.js` 只做与新双开关一致的最小清理，不做 UI 形态重设计。
- Task C（聚焦验证与文档收口）：
  - 先跑 `moltis-telegram` 定向/全量测试，再跑 `moltis-gateway --no-run`。
  - 通过后只更新本 issue 与设计文档中的实施现状/勾选项，不扩写新概念。
- 受影响文件：
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/config.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/assets/js/page-channels.js`
  - `docs/src/refactor/telegram-record-dispatch-boundary.md`
  - `issues/issue-v3-telegram-group-execution-plan-record-dispatch-one-cut.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/refactor/telegram-record-dispatch-boundary.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `issues/issue-v3-c-telegram-core-boundary-and-context-bridge.md`
  - `issues/issue-telegram-group-relay-duplicate-replies-on-concurrent-mentions.md`
  - `issues/issue-telegram-group-multi-bot-nl-collaborative-orchestration.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- Q1:
  - 无阻塞性未决问题；本单的实现口径已冻结。
- Q2:
  - 非消息型事件后续若进入 planner，仍必须服从“只产出 `record` / `dispatch`”这一总口径。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

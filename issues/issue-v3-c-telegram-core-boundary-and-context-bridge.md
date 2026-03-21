# Issue: V3 C 阶段先收敛 Telegram / core 边界（telegram / gateway / sessions）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-20
- Owners: TBD
- Components: telegram/gateway/channels/sessions/docs
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-20：A+B 已完成，Telegram adapter 边界与 bucket/session 语义已先行落地：`issues/issue-v3-telegram-adapter-and-session-semantics.md:1`
- 2026-03-20：V3 设计、roadmap、上下文分层、Telegram 边界等文档，已经统一成“C 阶段先不改落盘，先收敛 Telegram / core 边界”的口径：`docs/src/refactor/v3-roadmap.md:1`
- 2026-03-20：TG 主路径已经改走正式 `TelegramCoreBridge`，Telegram runtime 在 server/plugin/bot/state 四处都显式挂接 `core_bridge`，不再靠 handler 直接跨层调用旧入口：`crates/telegram/src/adapter.rs:103`、`crates/telegram/src/plugin.rs:68`、`crates/gateway/src/server.rs:1822`
- 2026-03-20：Telegram handler 不再直接做 TgGstV1 最终 speaker/envelope 塑形，只负责整理 bridge hint 后 dispatch/ingest：`crates/telegram/src/handlers.rs:544`
- 2026-03-20：core 在统一入口接管群聊文本整理；`dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` 都先走同一套格式化：`crates/gateway/src/channel_events.rs:121`
- 2026-03-20：gateway 通过 `TelegramCoreBridge` 实现把 `tg_inbound` / `tg_route` / follow-up target 收口进统一 bridge；Telegram handler 的主消息、callback、edited live location、voice/location follow-up 都已先走 bridge，再落到 gateway 内部实现：`crates/gateway/src/channel_events.rs:1800`、`crates/telegram/src/handlers.rs:4743`
- 2026-03-20：运行时回投/回声主链改成 `ChannelTurnContext`，reply target/status log 不再挂在 session+trigger 旧队列上：`crates/gateway/src/state.rs:92`
- 2026-03-20：`ChannelTurnContext` 已进一步收紧为按 `session_key + turn_id` 隔离，避免不同 session 复用同一个 `_channelTurnId` 时串线回投：`crates/gateway/src/state.rs:554`
- 2026-03-20：web UI channel echo 改成从当前 session 自己的 binding 重建 turn context，不再受同 chat 其他 bucket 的 active session 影响：`crates/gateway/src/chat.rs:2344`
- 2026-03-20：Telegram bridge hint 只在运行时使用；进入 `channel` 元数据和 session history 前会被剥离，不改现有落盘口径：`crates/gateway/src/channel_events.rs:145`
- 2026-03-20：Telegram typing keepalive 已收回 `telegram/outbound` 帮助函数；gateway 主链只保留调用，不再自带独立 typing 生命周期实现：`crates/telegram/src/outbound.rs:370`、`crates/telegram/src/outbound.rs:451`、`crates/gateway/src/channel_events.rs:513`
- 2026-03-20：`tg_gst_v1_system_prompt_block_for_binding`、relay route、relay reply 等 Telegram 专项 helper 已集中到 adapter；gateway/core 只消费 helper 结果，不再重复保留散点实现：`crates/telegram/src/adapter.rs:147`、`crates/telegram/src/adapter.rs:163`、`crates/telegram/src/adapter.rs:188`
- 2026-03-20：review 已确认 `MsgContext` / `routing` / `auto-reply` 不再参与 TG 主路径；当前 TG 主路径只落在 `telegram` / `gateway` / `channels` 这条实现链上，旧模型已与本单主路径脱钩。
- 2026-03-20：补回升级兼容：当旧部署只有 `active_session_id`、还没回填 `bucket_session_id` 时，TG 主路径会在命中同 chat/thread 的旧会话后自动回填 bucket 映射，避免升级后平白分叉新 session：`crates/gateway/src/channel_events.rs:210`、`crates/gateway/src/chat.rs:7538`
- 2026-03-20：topic/thread typing 已恢复按目标 thread 发送，避免 forum/topic 场景把 typing 丢到根 chat：`crates/telegram/src/outbound.rs:355`

**已覆盖测试（如有）**
- TgGstV1 文本整理已移到 core，普通文本/ingest/图片 caption 三条入口都有回归：`crates/gateway/src/channel_events.rs:1989`
- TgGstV1 listen-only / addressed / presence reply 保持原结果：`crates/telegram/src/handlers.rs:4931`
- TG 主入站、callback、edited live location 都已证明优先走 `core_bridge`，不会再误打到 legacy `event_sink` 主路径：`crates/telegram/src/handlers.rs:4743`、`crates/telegram/src/handlers.rs:4816`、`crates/telegram/src/handlers.rs:4942`
- callback / edited live location 继续保留 bucket_key，不会退回 chat 级 active session 猜测：`crates/telegram/src/handlers.rs:5877`
- 旧部署仅有 `active_session_id` 时，会命中兼容回填而不是平白新建会话；但不同 bucket 不会错误复用旧活跃会话：`crates/gateway/src/channel_events.rs:2219`、`crates/gateway/src/chat.rs:13503`
- web UI channel echo 在多 bucket 同 chat 下仍命中原 session：`crates/gateway/src/chat.rs:13510`
- `_channelTurnId` 即便跨 session 重号，也不会共享 reply target / status log：`crates/gateway/src/state.rs:946`
- gateway typing 生命周期回归已补齐；start/run/error feedback 三段都继续保活到正确结束点：`crates/gateway/src/channel_events.rs:2658`、`crates/gateway/src/channel_events.rs:2747`
- topic/thread typing 回归已补齐，前台和后台 typing loop 都继续带 thread_id：`crates/telegram/src/outbound.rs:3193`、`crates/telegram/src/outbound.rs:3222`
- polling liveness 仍以真实 runtime 状态为准：`crates/telegram/src/plugin.rs:523`
- 验证命令已跑绿：`cargo test -p moltis-telegram --lib`、`cargo test -p moltis-gateway --lib`

**已知差异/后续优化（非阻塞）**
- 本单明确不改最终落盘格式，不引入 `session_event` 持久化替换。
- `_chanChatKey`（V2 跨域桥）已退出 TG 主路径真值判断，但当前仍残留在工具上下文与部分 router/sandbox 辅助链路；彻底删除与 `session_key/session_id` 统一改名，跟踪在：`issues/issue-v3-session-ids-and-channel-boundary-one-cut.md:1`
- `ChannelEventSink` 仍保留给 OTP 审批、实时 UI 事件这类旁路能力使用；但它已不再承担 TG 主消息 / callback / live location / voice 主路径跨层语义。
- relay / mirror / mention / listen-only / addressed / topic-thread / reply continuity、voice / photo / unsupported attachment 继续沿用现有行为；本单只收职责边界，不重写规则。

**当前阻塞关单的差异**
- 无。除落盘承载外，本单范围内的阻塞项已清零。
- 说明 1：`TelegramCoreBridge` 已成为 TG 主路径的正式跨层入口；`ChannelEventSink` 仅剩 gateway 内部复用和旁路事件用途，不再承担 TG 主路径跨层语义：`crates/telegram/src/adapter.rs:103`、`crates/gateway/src/channel_events.rs:1800`
- 说明 2：稳态下 `active session fallback` 已退出 TG 主路径真值判断；仅对“升级前遗留、尚未回填 bucket 映射”的旧 Telegram 会话保留一次性兼容回填，回填完成后仍以 bucket/thread-aware 路径为准：`crates/gateway/src/channel_events.rs:210`、`crates/gateway/src/chat.rs:7538`

---

## 背景（Background）
- 场景：A+B 做完后，Telegram 的会话分桶和 follow-up 基础已经通了，但“最终给模型看的上下文由谁负责”这件事还没完全收干净。
- 约束：
  - C 阶段的第一优先级，是尽快把 Telegram 适配层和 core 的职责切清。
  - 这次先不改落盘，但允许把现有 `SessionStore` / `PersistedMessage` 只当作过渡承载继续用。
  - 群聊文本格式先保持现状，不能借这次顺手改文案协议。
  - 复杂群策略先保持现状，不能借这次顺手重写 mention / relay / mirror / topic-thread 规则。
  - 媒体路径先保持现状，不能借这次顺手改 voice / photo / unsupported attachment 口径。
  - 本单必须把“职责切干净”的完成口径写清；其中运行时旧桥接必须在本单里切掉，允许留下的尾巴只剩落盘承载。
- Out of scope：
  - `session_event` 统一持久化
  - 历史数据迁移
  - 非 Telegram 渠道接入
  - 借机重做群聊文案协议或复杂群策略规则

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Telegram adapter**（主称呼）：负责 Telegram 原生协议收发、消息归一化、路由分桶、回复目标恢复，以及 typing / callback / live location / liveness / retry 这类 Telegram 专项逻辑。
  - Why：这次要先把它和 core 的职责切清。
  - Not：不是最终给模型拼上下文的地方。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：TG adapter

- **core 上下文整理**（主称呼）：负责把会话里的事实整理成最终给模型的上下文。
  - Why：这部分职责必须回到 core。
  - Not：不是 Telegram 收发逻辑，也不是 Telegram 文案协议本身。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：context bridge / context engine bridge

- **过渡桥接**（主称呼）：在不改落盘格式的前提下，暂时借现有 `SessionStore` / `PersistedMessage` 作为承载，把数据交给 core 做上下文整理。
  - Why：这是这次能快速推进 C 阶段的前提。
  - Not：不是继续让旧链路决定语义，也不是最终保存层方案。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：兼容桥接 / legacy persistence bridge

- **一次消息处理上下文**（主称呼）：一次 Telegram / Web 入站在运行时从入口带到 run、回声和最终回投的一组字段，至少包括 `session_id`、`bucket_key`、`thread_id`、`reply_target`、`echo_policy`。
  - Why：运行时旧桥接要被明确替换掉，不能再把回投和回声建立在隐藏字段或 chat 级猜测上。
  - Not：不是新的落盘格式，也不是 Telegram 专有协议对象。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：runtime request context / turn context

- **群聊文本格式**（主称呼）：当前 Telegram 群聊给模型前使用的文本形态，只认现有 `Legacy` / `TgGstV1` 两种格式。
  - Why：这次必须先保行为不变。
  - Not：不是这次要重新设计的新协议。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：group transcript format

- **复杂群策略**（主称呼）：当前代码里已经存在的 mention / relay / mirror / listen-only / addressed / topic-thread / callback / location / reply 连续性等行为规则。
  - Why：这次要保留行为，只切职责归属。
  - Not：不是这次要重新讨论的新策略。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：group strategy

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

补一句人话：

- 本单后面凡是写“兼容 / bridge / 过渡”，意思都只是“短期借旧承载托底”。
- **不是**“继续让 Telegram 旧分支、旧文本塑形、旧触发链路来定义最终语义”。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 把“最终给模型看的上下文整理”收回 core，不再由 Telegram / gateway 零散拼文本。
- [x] Telegram adapter 继续负责 Telegram 专项逻辑：协议收发、路由分桶、reply target 恢复、typing、callback、live location、liveness、retry。
- [x] 在不改落盘的前提下，用过渡桥接先把新边界跑通。
- [x] 群聊文本格式先保持当前 `Legacy` / `TgGstV1` 行为不变。
- [x] 复杂群策略先保持当前行为不变，只清职责边界。
- [x] voice / photo / location / unsupported attachment 等现有媒体路径先保持当前行为不变，只清职责边界。
- [x] listen-only ingest、web UI channel echo、final reply 回投这类现有行为先保持不变，但它们背后的**运行时旧桥接**必须在本单内替掉。
- [x] TG / core 必须直接使用正式跨层契约，不接受长期保留过渡壳、双轨路径或“新契约外面再套旧入口”的做法；Telegram 专项状态不得再散落在 `gateway` 主链。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须先保现有行为，再做职责收敛。
  - 不得因为收边界而改变 Telegram 群聊文本格式口径。
  - 不得因为收边界而把 Telegram 专项字段继续扩散成 core 公共概念。
- 唯一允许保留的尾巴：现阶段只允许继续借用现有 `SessionStore` / `PersistedMessage` 作为持久化承载。
- 不允许保留的尾巴：`dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` 的旧语义桥接、`_triggerId` / `_chanChatKey` / “按 session+trigger 管 reply target / status log 的旧桥接” / `channel_binding` 这类运行时旧桥接，关单前必须退出主路径。
- 可观测性：对“命中候选但被兼容分支/降级分支接管”的情况补齐结构化日志，必须带 `reason_code`，且不得打印完整正文；最低要能区分上下文整理回退、web echo / final reply 找不到目标、location follow-up 找不到原会话、非图片附件拒绝、typing 发送失败、polling 失活恢复。
- 安全与隐私：日志只允许短预览或哈希，不打印 token、完整消息正文、完整上游返回。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 群聊相关的文本塑形现在分散在 `telegram` 和 `gateway` 两边，不好判断谁该对最终模型上下文负责。
2) relay / mirror / listen-only 等复杂群逻辑，既参与 Telegram 投递，也在影响最终入模文本，边界不干净。
3) typing / callback / live location / liveness 这些 Telegram follow-up 已经是专项能力，但和“上下文整理”还没完全拆开。

### 影响（Impact）
- 用户体验：一旦后面继续改群聊策略或群聊文本，很容易牵一发而动全身。
- 可靠性：职责不清时，修 Telegram follow-up、修 session、修上下文，容易互相带回归。
- 排障成本：出现“为什么模型看到的是这段文本”时，要跨多个文件倒推。

### 复现步骤（Reproduction）
1. 看 Telegram 群聊文本入口，会发现同一类语义在 Telegram handler 和 gateway chat 两边都在改写。
2. 再看 relay / mirror / typing / callback / location / liveness，会发现 Telegram 专项逻辑和上下文路径交织。
3. 期望 vs 实际：期望是 Telegram 只管 Telegram，core 统一整理上下文；实际是当前仍有一部分“给模型看的文本”散在 Telegram / gateway 链路里。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/telegram/src/handlers.rs:544`：Telegram handler 现在只把 sender/chat/transcript hint 封成 `ChannelMessageMeta`，不再直接产出 TgGstV1 最终文本。
  - `crates/telegram/src/handlers.rs:1482`：普通 dispatch 路径也改成只传 raw body + bridge hint，最终文本交给 core。
  - `crates/gateway/src/channel_events.rs:121`：core 统一用 `format_channel_inbound_text()` 生成 TgGstV1 最终文本。
  - `crates/gateway/src/channel_events.rs:488`：普通文本 dispatch 先在 core 做格式整理，再广播/入模。
  - `crates/gateway/src/channel_events.rs:717`：listen-only ingest 也走同一套 core 格式整理，再落现有 `SessionStore`。
  - `crates/gateway/src/channel_events.rs:1033`：图片附件 caption 同样先在 core 整理，再进入多模态 content。
  - `crates/gateway/src/channel_events.rs:145`：Telegram bridge hint 在广播/落盘前会被剥离，不改现有持久化 JSON 口径。
  - `crates/gateway/src/state.rs:92`：`ChannelTurnContext` 成为运行时回投/回声主链的显式上下文。
  - `crates/gateway/src/chat.rs:2344`：web UI channel echo 改成从当前 session 自身 binding 重建 turn context，不再靠 chat 级 active session 猜测。
  - `crates/gateway/src/chat.rs:7010`：relay 回投也改成先登记 turn context，再走统一 `channel_turn_id` 主链。
  - `crates/telegram/src/plugin.rs:523`：liveness / probe 仍保持 Telegram runtime 专项逻辑。
- 当前测试覆盖：
  - 已有：
    - `crates/gateway/src/channel_events.rs:1989`：core 接管 TgGstV1 普通文本格式回归。
    - `crates/gateway/src/channel_events.rs:2056`：core 接管 listen-only + 图片占位文本回归，且 bridge hint 不落盘。
    - `crates/gateway/src/channel_events.rs:3100`：core 接管图片 caption 的 TgGstV1 文本整理回归。
    - `crates/gateway/src/chat.rs:13510`：web UI channel echo 在多 bucket 同 chat 下仍命中原 session。
    - `crates/telegram/src/handlers.rs:4931`：listen-only TgGstV1 文本结果与现状一致。
    - `crates/telegram/src/handlers.rs:5074`：addressed TgGstV1 文本结果与现状一致。
    - `crates/telegram/src/handlers.rs:5240`：self mention only + presence reply 行为不回归。
    - `crates/telegram/src/handlers.rs:5877`：edited live location 保留 bucket key。
    - `crates/telegram/src/handlers.rs:6544`：callback follow-up 保留 bucket key。
    - `crates/telegram/src/plugin.rs:523`：polling liveness 判定回归。
  - 缺口：
    - 真实 Telegram 网络、多 bot 群、长时间 polling 断续恢复，仍需要手工验收。

## 根因分析（Root Cause）
- A. A+B 先把 Telegram adapter 边界和 session 分桶跑通了，但还没有把“最终上下文整理”彻底从 Telegram / gateway 的散点逻辑里拿出来。
- B. 当前代码因为还借着旧 `SessionStore` / `PersistedMessage` 在跑，导致 Telegram / gateway 多处直接拼接或改写文本。
- C. 因为还没进入最终落盘改造阶段，这次不能靠改 `session_event` 一步到位，只能先做一层过渡桥接，把职责先切清。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - Telegram adapter 必须只负责 Telegram 专项能力，不再负责最终给模型的上下文整理。
  - core 必须统一负责最终给模型看的上下文整理。
  - 在 C 阶段里，core 必须允许继续读取现有 `SessionStore` / `PersistedMessage`。
  - 运行时主链必须改成围绕“一次消息处理上下文”来传递 session、bucket、reply target、channel echo 信息。
  - 旧 `SessionStore` / `PersistedMessage` 只允许继续托底；`_triggerId` / `_chanChatKey` / “按 session+trigger 管 reply target / status log 的旧桥接” / `channel_binding` 这类运行时旧桥接必须在本单内退出主路径。
  - 群聊文本格式必须先对齐当前 `Legacy` / `TgGstV1`。
  - relay / mirror / mention / listen-only / addressed / topic-thread / callback / location / typing / liveness / retry 的行为必须先保持当前口径。
  - voice / photo / unsupported attachment 的行为必须先保持当前口径。
- 不得：
  - 不得在本单里改最终落盘格式。
  - 不得在本单里顺手重写复杂群策略规则。
  - 不得把 Telegram 内部字段继续扩成 core 长期公共字段。
- 应当：
  - 过渡桥接应当尽量薄，只作为过渡层。
  - 新增降级路径应当有结构化日志，日志里要能直接看出走了哪条兼容分支。
  - 若中间步骤为了编译或小步提交临时保留旧入口名，这些名字应当只剩无状态薄转发，不能继续保留独立语义或独立状态。

## “切干净”完成口径（Done Definition）
- C 阶段关单时，必须做到的“切干净”：
  - Telegram 不再决定最终给模型看的文本。
  - core 接管最终上下文整理。
  - relay / mirror / mention / reply continuity 等“入模怎么组织”的职责回到 core。
  - Telegram 侧只保留协议、路由、follow-up、投递、媒体协议处理。
  - `dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` 的旧分流语义、`_triggerId` / `_chanChatKey`、“按 session+trigger 管 reply target / status log 的旧桥接”、`channel_binding` / `session_matches_channel_binding` 这类运行时旧桥接，必须已经退出主路径和真值判断。
  - 关单后唯一允许继续存在的尾巴，只剩 `SessionStore` / `PersistedMessage` 这一层落盘承载。
- 只有到了后续“替换旧保存层”的阶段，才算**彻底切干净**：
  - core 不再依赖 `SessionStore` / `PersistedMessage` 做上下文整理。
  - Telegram / core 之间只剩收敛后的新边界对象和新主链，连落盘尾巴也被新保存层替掉。
- 对应当前 roadmap：
  - 本单完成：等于“**职责切干净**”。
  - 后续保存层替换完成：才等于“**彻底切干净**”。
- 在“落盘暂不做”的前提下，时间点再说死一点：
  - **最晚到 C6 结束**：运行时旧桥接必须全部退出主路径和真值判断。
  - **到 C7 关单**：运行时旧桥接的清理、回归验证、文档、验收必须全部收口。
  - **落盘尾巴** 不在本单完成；它天然要等后续保存层改造阶段。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：先不改落盘；先在现有保存层之上加一层 core 侧的过渡桥接，把最终上下文整理收回 core，同时保留 Telegram 专项 follow-up 能力在 adapter 一侧。
- 优点：
  - 能最快收敛 Telegram / core 边界。
  - 不会把落盘重构、事件模型重构、群策略重构混在一个单子里。
  - 方便先做“行为对齐现状”的回归测试。
- 风险/缺点：
  - 过渡期会同时存在旧保存层和新的 core 上下文整理层，需要用测试和日志把边界看清。

#### 方案 2（备选）
- 核心思路：直接把 `session_event` 持久化和上下文整理一起重做。
- 风险/缺点：
  - 范围过大，会拖慢 Telegram / core 边界收敛。
  - 这次用户已明确“会话保存先不改”，不适合作为当前方案。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：先保当前行为，再做职责切换。
- 规则 2：群聊文本格式以当前 `Legacy` / `TgGstV1` 为兼容基线，不借这次改协议。
- 规则 3：复杂群策略以当前代码行为为兼容基线，不借这次改规则。
- 规则 4：Telegram adapter 保留 Telegram 专项 follow-up；core 只接收结构化输入并统一整理最终上下文。
- 规则 5：只有旧落盘承载允许暂留；旧运行时桥接必须在本单内替成“一次消息处理上下文”主链，不能留到后续。

#### 接口与数据结构（Contracts）
- API/RPC：
  - Telegram -> core：必须直接走正式跨层契约，不允许“正式契约外再套旧入口壳”。本单关单前，TG 主路径跨层只允许使用 `tg_inbound` / `tg_route`；旧 `dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments`、`ChannelEventSink`、`ChannelMessageMeta`、`ChannelReplyTarget` 不得再承担 TG 主路径跨层语义。
  - Telegram follow-up -> core：callback、edited live location 这类 follow-up 的 session 命中，必须保留原来的 `bucket_key` / `thread_id` 线索，不能退回 chat 级 active session 猜测。
  - core -> Telegram：必须直接走 `tg_reply`；最终回投和 web echo 的真值必须来自“一次消息处理上下文”，不能再靠 `_triggerId` / `_chanChatKey` / chat 级活动会话猜测。
  - Web UI -> core：当消息发往一个已经绑定 Telegram 的 session 时，必须从该 session 自己的绑定和 bucket/thread 线索重建“一次消息处理上下文”，不能因为同 chat 里别的 bucket 更活跃就改投到别的会话。
- 存储/字段兼容：
  - 继续使用现有 `SessionStore` / `PersistedMessage`。
  - 继续使用现有 bucket -> session bridge。
  - 不新增要求迁移的落盘字段。
- UI/Debug 展示（如适用）：
  - 应补充“当前上下文是否走过渡桥接、当前 transcript format、是否命中 relay / mirror 兼容分支、回声/回投是否命中新主链、是否触发降级”的可观测字段。

#### 正式跨层契约（必须冻结）
- `tg_inbound`：TG adapter -> core 的唯一入站契约。
  - 最小字段：
    - `kind`
    - `mode`
    - `body`
    - `private_source`
  - 其中 `body` 至少稳定包含：
    - `text`
    - `has_attachments`
    - `has_location`
  - 其中 `private_source` 至少稳定包含：
    - `account_handle`
    - `chat_id`
    - `message_id`
    - `thread_id`
    - `peer`
    - `sender`
    - `addressed`
- `tg_route`：TG adapter -> core 的唯一路由契约。
  - 最小字段：
    - `peer`
    - `sender`
    - `bucket_key`
    - `addressed`
- `tg_reply`：core -> TG adapter 的唯一出站契约。
  - 最小字段：
    - `output`
    - `private_target`
  - 其中 `private_target` 至少稳定包含：
    - `account_handle`
    - `chat_id`
    - `message_id`
    - `thread_id`
- 明确禁止：
  - 不允许再让 `ChannelEventSink` / `ChannelMessageMeta` / `ChannelReplyTarget` 承担 TG 主路径跨层契约。
  - 不允许再让 `_triggerId` / `_chanChatKey` / `channel_binding` 这类兼容字段承担正式跨层语义。
  - 不允许保留“新契约 + 旧跨层入口壳并存”的双轨主路径。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - core 侧如果无法按新路径整理上下文，只允许回退到“继续读取现有 `SessionStore` / `PersistedMessage` 的过渡桥接”，不得回退到 Telegram / gateway 旧文本塑形链路，并记录 `reason_code`。
  - Telegram callback / location / typing / liveness / retry 继续按现有 Telegram 专项错误处理返回，不改变用户可见口径。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 现有 reply target、bucket binding、typing keepalive、callback binding 的清理规则继续沿用，不在本单改语义。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 默认脱敏，只允许短预览或哈希。
- 禁止打印字段清单：
  - token
  - 完整消息正文
  - 完整 provider 回包

## 实施前冻结项（Pre-implementation Freeze）
- 文本兼容基线：
  - DM 文本链路先保持现状，不借本单改用户可见文案。
  - Group 文本链路必须以当前 `Legacy` / `TgGstV1` 的实际输出为基线。
  - listen-only / addressed 文本，以 `crates/telegram/src/handlers.rs:512`、`crates/telegram/src/handlers.rs:1454` 当前行为为准。
  - relay / mirror 文本，以 `crates/gateway/src/chat.rs:7342`、`crates/gateway/src/chat.rs:7688` 当前行为为准。
  - mention strictness / ambiguous mention 处理，以 `crates/gateway/src/chat.rs:7176` 当前行为为准。
- 媒体与引用兼容基线：
  - voice / audio：继续由 Telegram adapter 做下载和转写，core 不接手 Telegram 文件协议细节。
  - photo / image：继续由 Telegram adapter 产出规范化附件，core 只接手“这些内容如何进入上下文”。
  - live location follow-up：继续由 Telegram adapter + channel event sink 处理回复目标和 pending request。
  - 非图片附件拒绝：当前行为先保持不变，不借本单扩成全量多媒体支持。
  - reply continuity / quote 关系：从这阶段开始，归 core 负责如何进入上下文；Telegram adapter 只负责提供可用的回复引用线索。
- 职责冻结：
  - Telegram adapter 继续保留：原生 update 解析、route / bucket_key、reply target、callback、live location、typing、liveness、retry。
  - Telegram adapter 继续保留：voice 转写、photo 下载、unsupported attachment 反馈、topic/thread 原生字段提取。
  - core 接管：最终给模型的上下文整理、speaker/envelope 组装、group transcript format 选择、relay / mirror / mention / reply continuity 的入模整理。
  - `crates/gateway/src/channel_events.rs` 在本单收尾时只允许保留 session bridge 与正式契约编排；Telegram typing 生命周期、relay 投递编排、transcript-format 特判不得继续留在 `gateway` 主链。
- 首轮代码落点冻结：
  - 第一版 core 侧过渡桥接，先落在现有 `crates/gateway/src/chat.rs` 附近，原因是当前 prompt/context assemble 主链已经在这里。
  - 本单不要求现在就抽新 crate，也不要求现在就把 generic trait 全部做实。
  - 若后续需要拆模块，优先做同 crate 内部的小步抽取，不做大搬家。
- “切干净”的定义冻结：
  - Telegram 不再决定最终给模型看的文本。
  - core 接管最终上下文整理。
  - 旧落盘承载可以暂留；旧 trigger、旧 echo、旧 chat 级路由猜测不允许带到关单状态。
- 兼容 bridge 输入冻结：
  - 第一波允许借现有入口保编译和过渡，但收尾时 TG 主路径必须只剩正式跨层契约；旧入口只允许彻底删除，或退化成 adapter 内部私有薄适配，不得继续承担跨层主路径。
  - 正式契约至少要稳定拿到：文本、图片附件、消息种类、发送者展示信息、`message_id` / `thread_id` / `bucket_key`。
  - 一次消息处理上下文至少要稳定拿到：`session_id`、`bucket_key`、`thread_id`、reply target、channel echo 策略。
  - 实施过程可以短暂借旧字段保编译和过渡，但到 C6 结束前，final reply 回投和 web UI channel echo 必须切到新主链；`_triggerId` / `_chanChatKey` / chat 级 active session 猜测不能再是真值。
  - 如果现有公共字段不够表达 reply continuity / quote 线索，优先补 Telegram 私有 bridge hint，不直接扩成新的长期公共字段。
  - `group_session_transcript_format` 仍先从 Telegram 配置读取，但它只作为 bridge 输入，不再代表 Telegram 长期拥有“最终文本格式决定权”。
- session 真值边界冻结：
  - 以下场景禁止继续使用 `active session fallback` 作为真值：
    - TG 主路径消息路由
    - callback / edited live location 等 follow-up 命中
    - web UI channel echo
    - final reply 回投
    - relay / mirror 的目标会话命中
  - `active session fallback` 只允许用于“新 route 首次建 session”的兜底创建，或与 TG 主路径无关的只读兼容展示；只要参与 TG 主路径命中判断，就视为本单未完成。
- 旧跨层模型冻结：
  - `MsgContext` / `routing` / `auto-reply` 这套旧模型，如果仍在 TG 主路径上被调用，本单必须一起清掉。
  - 如果它们只是仓库内历史遗留、且不再参与 TG 主路径，本单不要求顺手删除代码文件，但必须明确与 TG 主路径完全脱钩，不能继续承担 TG / core 契约职责。
- 验收口径冻结：
  - 本单默认要求“文本结果对齐现状”，不是只保语义大概一致。
  - 如果某一步必须改动文本细节，必须单独开差异说明，并补专项回归测试。
- 明确不做：
  - 不改 `SessionStore` / `PersistedMessage` 落盘格式。
  - 不改 `concepts-and-ids.md`。
  - 不顺手推进全渠道统一 trait 落地。

## 运行时旧桥接清单（关单前必须清掉）
- 正式契约外的旧跨层路径：
  - `ChannelEventSink` / `ChannelMessageMeta` / `ChannelReplyTarget` 当前仍是 TG 主路径跨层壳；本单关单前必须退出 TG 主路径跨层。
  - `MsgContext` / `routing` / `auto-reply` 若仍参与 TG 主路径，也视为旧跨层路径，必须一并退出；若不参与，则必须明确与 TG 主路径脱钩。
- 入口分流旧桥接：
  - `dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` 现在分别带着旧语义进主链；本单关单前必须收敛为统一入口，只保留“会不会触发 run”的显式模式差异，不能再各自偷带一套文本语义。
- 隐藏参数旧桥接：
  - `_triggerId` / `_chanChatKey` 现在仍在运行时承担回投和回声相关语义；本单关单前必须退出真值路径和主路径入参/出参。实施中能删就删；临时删不掉也只能被动透传，不能再驱动任何路由或命中判断。
- reply target 旧桥接：
  - 旧的 session + trigger reply queue / status log 关联关系，之前会直接驱动最终回投；本单关单前必须改成由“一次消息处理上下文”或其显式映射来驱动。若迁移中暂时保留 `push_channel_reply` 这类函数名，也只能是 `ChannelTurnContext` 上的薄写入入口，不能再自己维持独立真值和独立状态。
- session 绑定旧桥接：
  - `channel_binding` / `session_matches_channel_binding` 现在仍参与 web UI channel echo 和回投判定；本单关单前必须改成 bucket/thread-aware 的显式绑定判断，不能再靠 chat 级 active session 猜测。实施过程中若短期仍借现有 binding 存 reply target 快照，它也只能当静态载体，不能再兼任 session 选择真值。
- 清理口径：
  - 若某个旧函数名为了小步提交暂时保留，最多只能是无状态薄转发；只要它还保留独立语义、独立状态或独立真值判断，就视为本单未完成。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] Telegram DM / Group 的最终上下文整理路径已经收回 core。
- [x] 当前 `Legacy` / `TgGstV1` 群聊文本结果与现状一致。
- [x] 当前 relay / mirror / mention / listen-only / addressed / topic-thread / callback / location / typing / liveness / retry 行为与现状一致。
- [x] 当前 voice / photo / image attachment / unsupported attachment 行为与现状一致。
- [x] 当前 reply continuity / quote 进入上下文的行为与现状一致，至少不能比现状更差。
- [x] 当前 listen-only ingest、web UI channel echo、final reply 回投在 bucket/thread-aware 路径下与现状一致。
- [x] gateway 中残留的 Telegram typing / relay / transcript-format 专项逻辑已收回正确层次，不再由 `gateway` 主链直接承担 Telegram adapter 职责。
- [x] TG / core 已切到正式跨层契约；`tg_inbound` / `tg_route` / `tg_reply` 成为主链唯一边界，不再同时保留旧跨层路径或旧入口壳参与主路径。
- [x] `active session fallback` 已退出 TG 主路径真值判断；它不再参与消息路由、follow-up 命中、web echo、final reply、relay/mirror 目标命中。
- [x] `MsgContext` / `routing` / `auto-reply` 若曾触达 TG 主路径，已被清掉；若未触达，也已在实现与 review 中明确证明与 TG 主路径脱钩。
- [x] 运行时旧桥接清单里的项目都已退出主路径和真值判断；关单后唯一允许剩下的尾巴只有 `SessionStore` / `PersistedMessage`。
- [x] 本单未引入落盘格式变更，也不要求历史数据迁移。
- [x] 对过渡桥接与降级分支补齐了结构化日志，且日志不泄露敏感内容。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 群聊文本兼容测试：分别校验 `Legacy` / `TgGstV1` 在 core 接管后输出仍与当前一致。
- [x] DM / Group 上下文整理测试：校验 core 通过现有 `PersistedMessage` 读取后，生成的入模上下文与当前一致。
- [x] relay / mirror 兼容测试：校验 core 接管后文本结果不回归。
- [x] mention strictness / ambiguous mention 兼容测试：校验复杂 mention 判别不回归。
- [x] voice / photo / location / unsupported attachment 兼容测试：校验媒体路径职责切换后行为不回归。
- [x] reply continuity / quote 兼容测试：校验引用关系进入上下文的行为不回归。
- [x] callback / live location 定位测试：校验 follow-up 继续保留原 `bucket_key` / `thread_id`，不会退回 chat 级 active session 猜测。
- [x] 回声 / 回投主链测试：校验 web UI channel echo、final reply 回投、listen-only ingest 不再依赖 `_triggerId` / `_chanChatKey` / chat 级 active session 猜测。
- [x] 降级日志测试：校验命中过渡桥接/降级分支时会打印带 `reason_code` 的结构化日志。
- [x] 正式契约测试：校验 TG 主路径跨层只走 `tg_inbound` / `tg_route` / `tg_reply`，旧跨层壳不再承担主路径语义。
- [x] session 真值边界测试：校验 TG 主路径不再依赖 `active session fallback` 做消息路由、follow-up 命中、web echo、final reply、relay/mirror 目标命中。
- [x] 旧模型退场测试/证明：校验 `MsgContext` / `routing` / `auto-reply` 不再参与 TG 主路径；如仅保留历史代码，需在 review 记录中证明已脱钩。

### Integration
- [x] Telegram DM 普通文本链路。
- [x] Telegram Group listen-only / addressed 链路。
- [x] Telegram Group relay / mirror 链路。
- [x] Telegram Group mention strictness / ambiguous mention 链路。
- [x] Telegram Group topic-thread / reply 链路。
- [x] Telegram photo / voice / location 链路。
- [x] callback / live location follow-up 链路。
- [x] web UI channel echo / final reply 回投链路。
- [x] typing 生命周期与 run 结束联动链路。
- [x] liveness / probe 状态判定链路。

### UI E2E（Playwright，如适用）
- [x] 不适用；本单聚焦后端边界与 Telegram 适配链路。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 真实 Telegram 网络、多 bot 群环境、长时间 polling 恢复，不适合在当前仓库里稳定自动化。
- 手工验证步骤：
  1. 在 Telegram DM 中发送普通文本，确认回复内容、typing、session 归属与当前一致。
  2. 在 Telegram 群中分别验证 listen-only、addressed、relay、mirror、mention strictness，确认模型看到的文本结果与当前一致。
  3. 在群里验证 reply、topic-thread、callback、live location follow-up，确认仍命中原会话。
  4. 发送 photo、voice、location，确认媒体路径行为与当前一致；非图片附件仍按当前口径拒绝。
  5. 从 web UI 和 Telegram 双向来回发消息，确认 channel echo、final reply 回投、bucket/thread 绑定都不回归。
  6. 人为制造短暂网络异常，确认 liveness / retry / polling 恢复日志能看出原因码，且不会无声断联。

## 发布与回滚（Rollout & Rollback）
- 发布策略：按小步提交推进；每一步都先补对应回归测试，再切实际代码路径；默认不新增新配置开关，也不保留长期双轨主路径。
- 回滚策略：若 core 接管后的上下文整理出现回归，直接按提交粒度回退本单改动；不在稳态代码里长期保留旧 runtime 主链兜底；A+B 已完成的 session 分桶与 follow-up 修复不回滚。
- 上线观测：重点看 transcript format 命中、过渡桥接命中、mention / relay / mirror 分支命中、web echo / final reply 新主链命中、callback/location fallback、voice/photo/unsupported attachment 分支命中、typing 失败恢复、polling liveness reason code。

## 实施拆分（Implementation Outline）
- Step 1: 冻结当前行为基线，先把 `Legacy` / `TgGstV1`、listen-only、addressed、mention、relay、mirror、媒体路径的现状补成对照测试。
- Step 2: 在 core 侧补一层过渡桥接，并建立“一次消息处理上下文”主链，先用现有 `SessionStore` / `PersistedMessage` 统一整理 DM / Group 上下文。
- Step 3: 先把 DM / Group 基础文本路径收回 core，再收 mention / relay / mirror / topic-thread / reply continuity。
- Step 4: 替掉 `_triggerId` / `_chanChatKey` / 旧的 session+trigger reply queue / `channel_binding` 这类运行时旧桥接的真值角色，保证 listen-only ingest、web echo、final reply、callback/location 路由不中断。
- Step 5: 在不改 Telegram 协议职责的前提下，明确 photo / voice / location / unsupported attachment 的边界归属，并补齐日志、测试和文档同步。
- Step 6: 把 `gateway` 中残留的 Telegram typing 生命周期、relay 投递编排、transcript-format 特判继续收回 Telegram adapter / core bridge 的正确一侧。
- Step 7: 把 TG / core 主链直接切到正式跨层契约，删除旧跨层路径；不保留“正式契约外再套旧壳”的双轨方案。
- Step 8: 跑完新增回归测试、更新 issue / gap 文档，再判断是否具备关单条件。
- 受影响文件：
  - `crates/channels/src/plugin.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/sessions/src/store.rs`
  - `docs/src/refactor/v3-design.md`
  - `docs/src/refactor/v3-roadmap.md`
  - `docs/src/refactor/v3-gap.md`

## 实施子任务清单（Ready-to-code）
- C1：补行为基线测试
  - 目标：先把当前行为钉住，防止后面“看起来收边界，实际偷偷改行为”。
  - 交付：
    - `Legacy` / `TgGstV1` 群聊文本对照测试。
    - listen-only / addressed 对照测试。
    - mention / relay / mirror 对照测试。
    - photo / voice / location / unsupported attachment 对照测试。
  - 主文件：
    - `crates/telegram/src/handlers.rs`
    - `crates/gateway/src/chat.rs`
  - 完成标准：
    - 至少能直接证明“当前文本长什么样”，后面重构只允许把这些测试继续跑绿。

- C2：定 core 侧过渡桥接入口
  - 目标：先把“最终上下文整理”收口到一个明确入口，别再散在 Telegram / gateway 多处。
  - 交付：
    - 在 `crates/gateway/src/chat.rs` 附近收出一个明确的 bridge 入口，专门负责读取旧记录并产出最终入模上下文。
    - 收出“一次消息处理上下文”，把 session、bucket、thread、reply target、echo 策略放到同一条主链里传递。
    - 明确 bridge 的输入、输出、调用点。
    - 明确现有公共字段不够时，优先加 Telegram 私有 bridge hint，而不是顺手扩公共字段。
    - 明确 `dispatch_to_chat`、`ingest_only`、`dispatch_to_chat_with_attachments` 三条旧入口如何收敛到这层 bridge，并写清它们的淘汰口径。
  - 主文件：
    - `crates/gateway/src/chat.rs`
    - `crates/channels/src/plugin.rs`
    - `crates/gateway/src/channel_events.rs`
  - 完成标准：
    - 代码里能明确看出“最终入模上下文从这里出”，而不是 Telegram handler / relay 分支各自拼一段。

- C3：先收回 DM / Group 基础上下文整理
  - 目标：先处理最基础的普通聊天链路。
  - 交付：
    - DM 普通文本改走 core 侧上下文整理。
    - Group 普通文本、listen-only、addressed 改走 core 侧上下文整理。
    - 保证 listen-only ingest 仍只入 session、不触发 run。
  - 主文件：
    - `crates/telegram/src/handlers.rs`
    - `crates/gateway/src/chat.rs`
    - `crates/gateway/src/channel_events.rs`
  - 完成标准：
    - `handlers.rs` 不再负责最终 speaker/envelope 塑形，只保留结构化事实和 dispatch。
    - DM / Group 基础链路进入 run、web echo、final reply 时，都已经走同一份“一次消息处理上下文”。

- C4：收回群聊复杂文本的入模整理
  - 目标：把最容易污染边界的群聊复杂文本整理，从 gateway 散点逻辑里收回来。
  - 交付：
    - relay 的入模文本整理改走 core 侧 bridge。
    - mirror 的入模文本整理改走 core 侧 bridge。
    - mention strictness / ambiguous mention / topic-thread / reply continuity 的上下文整理改走 core 侧 bridge。
    - 保留 Telegram 出站和回声投递逻辑，不改用户可见发回路径。
  - 主文件：
    - `crates/gateway/src/chat.rs`
  - 完成标准：
    - `chat.rs` 中与 relay / mirror / mention / reply continuity 相关的逻辑，能清楚分成“会不会发”“往哪发”和“给模型看什么”两层。
    - 复杂群文本进入模型时，不再依赖 Telegram handler 或旧 trigger 字段做隐式补语义。

- C5：收口媒体链路边界
  - 目标：把 photo / voice / location 这类路径的职责也切清，但先不改现有用户口径。
  - 交付：
    - 明确 voice 转写继续留在 Telegram adapter，core 只接手转写后的文本和上下文整理。
    - 明确 photo / image 附件继续由 Telegram adapter 规范化，core 只接手如何进入上下文。
    - 明确 live location follow-up 与 location 内容入模是两条责任链，不再混写。
    - 保持非图片附件拒绝的当前口径，不在本单扩战场。
  - 主文件：
    - `crates/telegram/src/handlers.rs`
    - `crates/gateway/src/channel_events.rs`
    - `crates/gateway/src/chat.rs`
  - 完成标准：
    - 媒体链路里“Telegram 协议处理”和“最终入模整理”两层职责能清楚分开。
    - live location follow-up 仍命中原会话，但 location 内容怎么进入模型只由 core 决定。

- C6：清理残余 Telegram 文本塑形并补可观测性
  - 目标：把不该留在 Telegram adapter 的最终文本职责和运行时旧桥接一起清掉，同时补足降级日志。
  - 交付：
    - 清掉 Telegram handler 中不该保留的最终 transcript shaping。
    - 把 `dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` 的旧分流语义收敛掉，主路径只认统一入口。
    - 把 `_triggerId` / `_chanChatKey` / 旧的 session+trigger reply queue / `channel_binding` / chat 级 active session 猜测，从 web echo、final reply、follow-up 命中的真值路径里替掉。
    - bridge 命中、过渡回退、文本格式选择、relay / mirror 降级等场景，都有结构化日志和 `reason_code`。
    - web UI channel echo、final reply 回投、location follow-up、typing / liveness 异常恢复，都有必要的结构化日志。
    - 保留 callback / live location / typing / liveness / retry / reply target 等 Telegram 专项逻辑。
  - 主文件：
    - `crates/gateway/src/chat.rs`
    - `crates/gateway/src/channel_events.rs`
    - `crates/telegram/src/handlers.rs`
    - `crates/telegram/src/plugin.rs`
  - 完成标准：
    - Telegram 侧只剩“原生协议 + 路由 + follow-up + 投递”职责；运行时旧桥接清单全部退出主路径；命中候选但被过渡分支接管时，日志能直接看出原因，且不打印完整正文。

- C7：做回归验证并收口文档
  - 目标：确保这个阶段收得住，可以直接关单并进入后续落盘阶段。
  - 交付：
    - 跑完相关单测 / 集成测试。
    - 补手工验收记录。
    - 同步 issue 状态、已实现项、已覆盖测试、已知差异。
  - 主文件：
    - `issues/issue-v3-c-telegram-core-boundary-and-context-bridge.md`
    - `docs/src/refactor/v3-gap.md`
  - 完成标准：
    - 这张 issue 单本身能直接作为施工和 review 的依据，不需要再补关键口径。

- C8：收回 gateway 中残留的 Telegram 专项生命周期
  - 目标：把现在还挂在 `gateway` 主链里的 Telegram typing / relay 投递 / transcript-format 特判收回正确层次。
  - 交付：
    - 把 `crates/gateway/src/channel_events.rs` 里的 Telegram typing keepalive 编排收回 Telegram adapter / outbound。
    - 把 `crates/gateway/src/chat.rs` 里的 Telegram relay 投递与目标路由细节移出 core 主链，只保留“给模型看什么”的整理职责。
    - 把 `maybe_append_tg_gst_v1_system_prompt()` 这类直接读取 Telegram snapshot 的逻辑收口，避免 core 主链继续直接依赖 Telegram 配置细节。
  - 主文件：
    - `crates/gateway/src/channel_events.rs`
    - `crates/gateway/src/chat.rs`
    - `crates/telegram/src/handlers.rs`
    - `crates/telegram/src/outbound.rs`
  - 完成标准：
    - `gateway` 主链不再直接承担 Telegram typing / relay 投递 / transcript-format 配置判定这类适配层职责。

- C9：切到正式跨层契约并删除旧跨层路径
  - 目标：让 TG / core 之间只剩一套正式主契约，不留半成品，不留双轨，不留旧路径继续穿层。
  - 交付：
    - 直接以 `tg_inbound` / `tg_route` / `tg_reply` 作为 TG / core 主链契约，旧的 `ChannelEventSink` / `ChannelMessageMeta` / `ChannelReplyTarget` 不再参与主路径跨层。
    - 明确并清理正式契约外的旧跨层清单；至少逐项处理：
      - `crates/channels/src/plugin.rs`
      - `crates/gateway/src/channel_events.rs`
      - `crates/common/src/types.rs`
      - `crates/routing/src/resolve.rs`
      - `crates/auto-reply/src/reply.rs`
    - 收掉不该长期暴露在公共契约里的 Telegram 专项字段；必须保留的，只能留在 Telegram 私有对象或 bridge hint。
    - 删除“正式契约外再套旧入口壳”的跨层旧路径，避免调用点继续两套并存。
    - 写清新的主调用链：Telegram adapter 产出什么、core 接什么、core 回给 Telegram adapter 什么；这条链路必须能单独成立，不依赖旧桥接补语义。
  - 主文件：
    - `crates/telegram/src/adapter.rs`
    - `crates/channels/src/plugin.rs`
    - `crates/gateway/src/channel_events.rs`
    - `crates/common/src/types.rs`
    - `crates/routing/src/resolve.rs`
    - `crates/auto-reply/src/reply.rs`
    - `docs/src/refactor/telegram-adapter-boundary.md`
  - 完成标准：
    - review 时可以明确说清楚“TG -> core 用什么对象交接”，且代码里只剩这一条正式跨层路径。

## 代码改造落点建议（首轮）
- `crates/telegram/src/handlers.rs`
  - 保留：原生 update 解析、route / bucket_key、reply target、callback、location、voice/photo 处理、dispatch。
  - 逐步移出：最终 speaker/envelope 文本塑形。
- `crates/channels/src/plugin.rs`
  - 目标：从 TG 主路径跨层接口中退场，或收成正式契约的内部适配层。
  - 不允许：继续承担 TG 主路径正式契约。
- `crates/gateway/src/chat.rs`
  - 新增或收口：core 侧过渡桥接入口、一次消息处理上下文、DM / Group 入模上下文整理、mention / relay / mirror / reply continuity 入模文本整理。
  - 不新增：新的 Telegram 协议细节分支。
- `crates/gateway/src/channel_events.rs`
  - 保持：session bridge、正式契约编排。
  - 必须移出：typing keepalive、Telegram relay 投递编排、任何新的 Telegram transcript-format 特判。
- `crates/common/src/types.rs`
  - 只允许：保留与 TG 主路径无关的历史兼容用途。
  - 不允许：继续作为 TG 主路径跨层模型。
- `crates/routing/src/resolve.rs` / `crates/auto-reply/src/reply.rs`
  - 若仍触达 TG 主路径：本单一起清掉。
  - 若不再触达 TG 主路径：必须在实现和 review 中明确证明已脱钩。
- `crates/telegram/src/plugin.rs`
  - 保持：polling、probe、liveness、自愈相关逻辑。
  - 不承担：上下文整理职责。
- `crates/sessions/src/store.rs`
  - 只允许：为了 bridge 读取方便做极小的辅助调整。
  - 不允许：变更落盘格式或引入迁移。

## 实施顺序与依赖（Execution Order）
- 必须先做 C1，再做 C2；没有行为基线就不要开始改路径。
- C3 完成并跑绿后，再做 C4；不要一上来先动 relay / mirror。
- C5 只能在 C3+C4 基本稳定后做；不要在基础文本路径还没稳时先碰 photo / voice / location。
- C6 不放到最后一次性补；每切一段路径，就补对应日志并顺手清理一段残余文本塑形。
- C8 必须先于最终关单；如果 `gateway` 里还留着 Telegram typing / relay / transcript-format 直连逻辑，就不允许把本单改回 DONE。
- C9 必须在 C8 之后收口；如果正式契约没有成为唯一跨层路径，本单就不允许关单。
- C9 完成时，必须同步给出“旧跨层路径退场结果”：
  - 哪些旧路径已删除
  - 哪些旧路径保留但已与 TG 主路径脱钩
- 任何一步只要碰到 `dispatch_to_chat` / `ingest_only` / `_triggerId` / `_chanChatKey` / 旧的 session+trigger reply queue / `channel_binding`，都必须同步验证 channel echo 和 final reply 回投。
- C7 是关口，不是可选项。

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-v3-telegram-adapter-and-session-semantics.md`
  - `docs/src/refactor/v3-design.md`
  - `docs/src/refactor/v3-roadmap.md`
  - `docs/src/refactor/v3-gap.md`
  - `docs/src/refactor/session-context-layering.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `docs/src/refactor/channel-adapter-generic-interfaces.md`
  - `docs/src/refactor/session-event-canonical.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- 当前无阻塞性未决问题。
- 已冻结决定：
  - core 侧过渡桥接首轮先落在 `crates/gateway/src/chat.rs` 附近，不额外起大模块搬迁。
  - `Legacy` / `TgGstV1`、listen-only、addressed、mention、relay、mirror 的验收口径，默认按“文本结果对齐现状”执行。
  - voice / photo / location / unsupported attachment 先保持当前行为，不借本单扩战场。
  - `dispatch_to_chat` / `ingest_only` / `_triggerId` / `_chanChatKey` / 旧的 session+trigger reply queue / `channel_binding` 这条旧运行时桥接，本单里必须替掉，不能拖到后面。
  - `gateway` 中残留的 Telegram typing / relay 投递 / transcript-format 特判，也视为本单必须清掉的非落盘尾巴，不能留到后续。
  - TG / core 之间必须直接切到正式跨层契约；不接受“新契约 + 旧跨层路径并存”的收口方式。
  - `active session fallback` 不得继续参与 TG 主路径的消息路由、follow-up 命中、web echo、final reply、relay/mirror 目标命中真值判断。
  - `MsgContext` / `routing` / `auto-reply` 若仍参与 TG 主路径，属于本单必须清掉的旧路径；若不参与，也必须在实现中证明已脱钩。
  - 本单完成后唯一允许剩下的尾巴，只是 `SessionStore` / `PersistedMessage` 这一层落盘承载；真正“彻底切干净”要等后续把这一层也替掉。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] 运行时旧桥接清单已清零，关单后仅剩落盘尾巴
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

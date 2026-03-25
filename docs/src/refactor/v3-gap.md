# 当前代码现状与 V3 目标差距

> 补充说明（2026-03-25）：
> 本文档是历史差距盘点，不再定义当前 one-cut 规范。若本文与 `docs/src/refactor/session-key-bucket-key-one-cut.md`、`issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`、`docs/src/refactor/channel-info-exposure-boundary.md`、`docs/src/refactor/telegram-adapter-boundary.md` 冲突，以后者为准。

## 2026-03-20 最新结论

以当前代码为准，如果先把“落盘改造 / `session_event` 持久化替换”排除掉，V3 C 阶段这次要收的东西已经基本收口：

- TG 主消息、callback、edited live location、voice/location follow-up，都已经先走正式 `TelegramCoreBridge`，不再由 Telegram handler 直接跨层打旧入口：`crates/telegram/src/adapter.rs:103`、`crates/gateway/src/channel_events.rs:1800`、`crates/telegram/src/handlers.rs:4743`
- gateway/core 侧的群聊文本整理、`ChannelTurnContext`、bucket/thread-aware web echo/final reply 真值链，已经改成新主链，不再依赖 chat 级 active session 猜测：`crates/gateway/src/channel_events.rs:126`、`crates/gateway/src/state.rs:92`、`crates/gateway/src/chat.rs:2322`
- Telegram typing keepalive、transcript-format helper、relay route/reply helper 已收口到 Telegram adapter / outbound；gateway 只消费 helper 或 bridge，不再自带第二套独立语义：`crates/telegram/src/outbound.rs:370`、`crates/telegram/src/adapter.rs:147`、`crates/telegram/src/adapter.rs:163`
- 升级兼容也已补回：如果老数据只有 `active_session_id`、还没回填 `bucket_session_id`，主路径会先复用匹配的旧 Telegram 会话并自动回填 bucket 映射，不会因为升级平白断上下文；但不同 bucket 仍不会错误共用：`crates/gateway/src/channel_events.rs:210`、`crates/gateway/src/chat.rs:7538`
- topic/thread typing 也已修正回 thread-aware；论坛 topic 里的长回复不会把 typing 丢到根 chat：`crates/telegram/src/outbound.rs:355`
- `_triggerId` / legacy tool-chat key / `channel_binding` 这批旧运行时桥接已经退出 TG 主路径真值判断；允许保留的尾巴只剩旧落盘承载：`crates/gateway/src/channel_events.rs:2182`、`crates/gateway/src/chat.rs:13515`

一句更准确的人话：

- **除落盘之外，Telegram adapter / core 的正式契约、职责归位、旧路径退场，这一轮已经收口。**
- **后面剩下的主要硬差距，就是保存层本身还没换。**

说明：

- `ChannelEventSink` 还存在，但它对 Telegram 来说已经退到 gateway 内部复用和 OTP/UI 旁路事件，不再承担 TG 主路径跨层语义。
- 这里说“active session fallback 退出真值路径”，指的是稳态真值；为兼容升级前旧行数据，仍保留一次性 bucket 回填，不算重新把 chat 级 fallback 带回主路径。
- 下文的“历史差距拆解”已按 2026-03-20 的最新代码现状逐条回填状态；不会再出现“上面说收口、下面又在说没做”的口径冲突。

本文档只定义一件事：

- 当前代码库离 V3 目标到底还差什么

本文档不讨论：

- V3 设计原则本身
- V3 分阶段实施顺序
- 落盘改造如何做

也就是说，本文档讨论的是：

- **当前已经做到什么**
- **除落盘之外，还差哪些硬差距**

而不是：

- **V3 应该怎么设计**
- **每一步具体怎么落地**

## 一句话结论

如果把“落盘改造”先排除掉，当前代码已经不再卡在“正式契约、职责归位、旧路径退场”这几个 C 阶段核心问题上。

更准确的判断是：

- **行为层已经对齐 V3 C 阶段目标**
- **边界层和契约层也已收口到“只剩落盘尾巴”的状态**

一句人话：

- **现在已经可以说：除落盘之外，Telegram adapter / core 这一刀已经切到位了。**

## 现状总评

从“除落盘外的 V3 终态”看，当前状态已经不是“两头都没收完”，而是：

- 已经到位的：
  - Telegram 入站归一化与 route/bucket 解析
  - callback / live location / liveness / retry 这些 follow-up 基础能力
  - 群聊最终文本整理已经回到 core 统一入口
  - TG 主路径跨层已经显式走 `TelegramCoreBridge`
  - web echo / final reply 的运行时主链已经切到 `ChannelTurnContext`
  - active-session fallback 已退出 TG 主路径真值判断
- 仍未到位的：
  - 旧保存层还没有替换

因此，当前更准确的定位是：

- **Telegram / core 运行时主链已经进入 V3 C 阶段完成态**
- **剩余差距主要落在保存层，而不是边界层/契约层**

## 当前已经完成的部分

## 1. Telegram 专项入站对象已经存在

当前 Telegram 侧已经有比较明确的专项对象：

- `TgInbound`：`crates/telegram/src/adapter.rs:46`
- `TgRoute`：`crates/telegram/src/adapter.rs:54`
- `resolve_tg_route(...)`：`crates/telegram/src/adapter.rs:211`

handlers 里也确实是先构造 `TgInbound`，再解析 route，而不是继续全靠散点字段临时拼：

- `build_tg_inbound(...)`：`crates/telegram/src/handlers.rs:3409`
- 主入站使用 `build_tg_inbound(...) + resolve_tg_route(...)`：`crates/telegram/src/handlers.rs:529`, `crates/telegram/src/handlers.rs:801`, `crates/telegram/src/handlers.rs:943`

这说明 Telegram adapter 的“先归一化、再分桶”这一步已经成形。

## 2. 群聊最终文本整理已经主要回到 core

当前真正做 `TgGstV1` 群聊最终文本整理的是 core 侧统一入口：

- `format_channel_inbound_text(...)`：`crates/gateway/src/channel_events.rs:126`

Telegram handler 侧现在主要做的是：

- 提取 Telegram 原生事实
- 计算 route / bucket
- 构造 reply target
- 构造 metadata
- 决定是 `dispatch` 还是 `ingest`

关键点在：

- `build_channel_message_meta(...)`：`crates/telegram/src/handlers.rs:3070`
- 普通分发路径：`crates/telegram/src/handlers.rs:1482`
- listen-only 路径：`crates/telegram/src/handlers.rs:529`

这比旧链路已经前进了一大步：Telegram 不再直接负责最终 speaker/envelope 的大部分落地文本。

## 3. follow-up 基础链路已经较稳定

当前这些 Telegram 专项能力仍主要留在 Telegram adapter：

- callback：`crates/telegram/src/handlers.rs:2629`
- edited live location：`crates/telegram/src/handlers.rs:1797`
- polling liveness：`crates/telegram/src/plugin.rs:523`
- retry reason / retry budget：`crates/telegram/src/bot.rs:35`

这部分符合 V3 想要的方向：Telegram 专项 follow-up 没有继续被塞回 core 统一语义层。

## 4. 运行时回投/回声主链已经明显改进

当前 reply/echo 的运行时主链已经切到 `ChannelTurnContext`：

- `ChannelTurnContext`：`crates/gateway/src/state.rs:92`
- 作用域已经修正为 `session_key + turn_id`：`crates/gateway/src/state.rs:554`

这比旧的 session + trigger reply queue 明显更接近 V3 要求的“显式一次消息处理上下文”。

## 历史差距拆解（已基本收口）

下面按本轮实施前的拆解顺序，把每一项都用“已收口 / 仅剩落盘尾巴 / 非阻塞”标注清楚；避免出现“顶部说已收口、下文又在说还没做”的自相矛盾。

## 1. TG / core 正式跨层主契约（已收口）

这曾经是最大的差距：新对象已经有了，但 TG 主路径还在走旧入口壳。

截至 2026-03-20，这一项已经收口为：

- TG 主消息、callback、edited live location、voice/location follow-up 都优先走 `TelegramCoreBridge`：`crates/telegram/src/adapter.rs:103`、`crates/telegram/src/handlers.rs:943`、`crates/telegram/src/handlers.rs:2622`、`crates/telegram/src/handlers.rs:1770`
- gateway 侧通过 `impl TelegramCoreBridge for GatewayChannelEventSink` 承接跨层入口：`crates/gateway/src/channel_events.rs:1844`
- 回归已锁死“即便 legacy `event_sink` 仍存在，TG 主路径也会优先走 `core_bridge`”：`crates/telegram/src/handlers.rs:4743`

一句人话：**TG -> core 的主入口已经切到 `TelegramCoreBridge`，并被测试锁住。**

## 2. `gateway` 残留 Telegram adapter 职责（已收口到“core 只调 helper/bridge”）

这项历史差距的核心不是“gateway 里不能出现任何 TG 字样”，而是：

- 不要在 gateway 再养一套“Telegram 自己的独立语义实现”（typing lifecycle、route/reply 的第二套判断、prompt 注入散点）
- gateway 如果需要 TG 专项能力，应只通过 bridge 或 TG adapter/outbound 的 helper 完成

截至 2026-03-20，已收口为：

- typing keepalive：gateway 侧只负责“何时需要保活”，发送细节统一走 `telegram/outbound` 的 targeted typing loop（带 thread/topic）：`crates/telegram/src/outbound.rs:355`、`crates/gateway/src/channel_events.rs:557`
- TG-GST v1 system prompt 注入：gateway 侧不再直接读 Telegram 私有配置字段，只调用 `tg_gst_v1_system_prompt_block_for_binding(...)`：`crates/gateway/src/chat.rs:937`、`crates/telegram/src/adapter.rs:147`
- group relay 的 bucket_key / reply 文本生成：已集中到 TG adapter helper，gateway 只消费结果：`crates/telegram/src/adapter.rs:163`、`crates/telegram/src/adapter.rs:188`、`crates/gateway/src/chat.rs:7347`

同时说明一个“看起来还在 gateway，其实是预期归属”的点：

- group relay / mirror / mention 这类“入模怎么组织、怎么写入 session history”的逻辑，本轮按 C 阶段口径仍归 core（因为它直接改写会话事实流/历史），并不等同于“Telegram 协议细节泄漏到 core”。

## 3. core 直接依赖 Telegram 配置/专项语义（已收敛为 snapshot + helper）

目标不是“core 完全看不见 Telegram”，而是“core 不要直接依赖 Telegram 的 raw update / secret config / 散点策略分支去决定主链语义”。

截至 2026-03-20，core 对 Telegram 的依赖已收敛成：

- 只读 `TelegramBusAccountSnapshot`（无 token 等 secret）来做 group scope / relay/mirror 的必要决策：`crates/telegram/src/config.rs:166`
- 通过 TG adapter helper 解析：bucket_key、transcript-format system prompt block 等，不再散点重复实现：`crates/telegram/src/adapter.rs:132`、`crates/telegram/src/adapter.rs:147`、`crates/gateway/src/chat.rs:949`

一句人话：**core 不再“反向摸 Telegram 细节字段”，而是只吃“归一化后的 snapshot + helper 输出”。**

## 4. session 主链仍是 bridge 形态（仅剩落盘尾巴）

截至 2026-03-20，运行时“真值链”已收口为：

- session 解析优先使用 `bucket_session_id`（bucket_key 真值），而不是 chat 级 active session 猜测：`crates/gateway/src/channel_events.rs:215`、`crates/gateway/src/chat.rs:7518`
- `active_session_id` 仅保留“升级兼容一次性回填”，并带结构化日志标注：`crates/gateway/src/channel_events.rs:231`、`crates/gateway/src/chat.rs:7543`
- `ChannelTurnContext` 以 `session_key + turn_id` 隔离，避免跨 session 串线回投：`crates/gateway/src/state.rs:554`

仍未替换的部分主要是：

- session history / metadata 仍建立在旧 `SessionStore` / `PersistedMessage` + metadata table 之上
- `channel_binding` / legacy tool-chat key 仍作为“旧保存层/工具链”兼容载体存在（但已退出 TG 主路径真值判断）

因此这条差距在本文档的归类里属于：**落盘尾巴（后续保存层替换阶段再清）**。

## 5. 旧跨层模型仍在系统里并存（已与 TG 主路径脱钩 / 非阻塞）

`MsgContext`/旧 routing/auto-reply 仍存在于仓库中，但截至 2026-03-20：

- TG 主路径已经不再依赖这些旧模型完成路由与上下文（它们对本单来说是“系统里还在，但不参与主链”）
- 是否要清理它们属于全局收口问题，不是 Telegram 垂直切的阻塞项

因此这里的差距状态是：**非阻塞（可在后续做全局清理）**。

## 6. Telegram transcript-format / 群聊最终文本整理归属（已收口：格式在 core，hint 在 adapter）

截至 2026-03-20：

- TG handler 不再输出 TgGstV1 的最终 speaker/envelope 文本，core 统一入口负责最终格式化：`crates/telegram/src/handlers.rs:943`、`crates/gateway/src/channel_events.rs:126`
- TgGstV1 的 system prompt 注入通过 TG helper 从 `channel_binding` + snapshot 决定：`crates/gateway/src/chat.rs:937`、`crates/telegram/src/adapter.rs:147`

一句人话：**Telegram 侧只决定“怎么收/怎么发”，core 决定“模型看到什么文本”。**

## 7. 落盘虽然不在本阶段做，但当前上下文主链仍深度依赖旧消息流（仍未做 / 真实剩余最大差距）

这一项就是你问“除了落盘之外”的“落盘”本体：

- 仍在大量依赖 `SessionStore` / `PersistedMessage`：`crates/sessions/src/store.rs:24`、`crates/sessions/src/message.rs:15`
- gateway `chat.rs` 仍以旧消息流读写历史并组装上下文：`crates/gateway/src/chat.rs:2309`、`crates/gateway/src/chat.rs:5005`

也因此：

- **除落盘之外**：这轮已经不再被“边界/契约/旧路径”卡住
- **包含落盘**：仍差保存层替换与历史迁移这一整段工程

## 当前距离 V3 终态还有多远

如果你问的是本文档开头强调的口径——**先不算落盘**——那答案已经很明确：

- **除落盘外：已经收口**
- **包含落盘：剩余几乎都集中在保存层替换（`session_event` + migration + context bridge 从旧消息流脱钩）**

一句人话：**现在的差距不是“边界还差一段”，而是“保存层还差一段”。**

## 当前可直接复用的基础

当前可以直接复用的部分仍然成立（并且多数已处于“完成态”，不需要再回头修边界）：

当前可以直接复用的部分有：

- Telegram 入站归一化对象：`crates/telegram/src/adapter.rs:46`
- Telegram route/bucket 解析：`crates/telegram/src/adapter.rs:211`
- callback / live location follow-up 保持 bucket/thread-aware：`crates/telegram/src/handlers.rs:2622`、`crates/telegram/src/handlers.rs:1770`
- polling liveness / retry：`crates/telegram/src/plugin.rs:523`、`crates/telegram/src/bot.rs:35`
- core 侧群聊文本整理入口：`crates/gateway/src/channel_events.rs:126`
- `ChannelTurnContext` 运行时回投主链：`crates/gateway/src/state.rs:92`, `crates/gateway/src/state.rs:554`
- 现有 `SessionStore` / `PersistedMessage` 作为本阶段允许继续借用的承载：`crates/sessions/src/store.rs:24`, `crates/sessions/src/message.rs:15`

也就是说：下一步完全可以围绕“保存层替换”推进，而不是再回头补边界与主链语义。

## 建议把 gap 分成三类看（避免再混口径）

为了避免后面继续混淆，建议把 gap 永久分成三类看：

### A. 本阶段已清掉的 gap（DONE）

这些是“除落盘外”本轮必须清掉的；截至 2026-03-20 已全部收口：

- TG / core 正式跨层契约切到 `TelegramCoreBridge`
- `ChannelTurnContext` 真值链替代 session+trigger 旧队列
- web echo / final reply / follow-up 全链路 bucket/thread-aware
- active-session fallback 退出稳态真值路径（仅保留升级一次性回填）

### B. 后续阶段再做的 gap（PENDING）

这些是明确留给后续的：

- `session_event` 统一事件记录
- `SessionStore` / `PersistedMessage` 替换
- 历史数据迁移

### C. 非阻塞但建议后续清理的事项（Optional）

这些不阻塞“除落盘外已收口”的结论，但会影响后续保存层替换的成本与长期可维护性：

- 逐步退出 legacy tool-chat key / `_triggerId` 这类 legacy 工具/前端兼容字段（目前已退出真值路径，但仍在 payload 中存在）
- 把“TG 专项 helper 的归属”在文档里说更死：哪些算 adapter 内部、哪些算稳定边界工具（避免后续又长出第二套）

## 相关文档

- `docs/src/refactor/v3-design.md`
- `docs/src/refactor/v3-roadmap.md`
- `docs/src/refactor/telegram-adapter-boundary.md`
- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

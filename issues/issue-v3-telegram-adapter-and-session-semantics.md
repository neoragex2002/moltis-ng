# Issue: V3 Telegram adapter 边界重构与会话语义落地（telegram / gateway）

> SUPERSEDED BY:
> - 设计真源：`docs/src/refactor/session-key-bucket-key-one-cut.md`
> - 治理主单：`issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
> - 本单仅保留历史背景与实施证据，不再定义当前实现口径或规范优先级。

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-20
- Owners: TBD
- Components: telegram/gateway/channels/sessions
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-20：在 `crates/telegram/src/adapter.rs:63` 落地 TG 专项入站/路由/bucket helper，并在 `crates/telegram/src/handlers.rs:534` / `crates/telegram/src/handlers.rs:802` / `crates/telegram/src/handlers.rs:944` 接入真实消息链路。
- 2026-03-20：在 `crates/telegram/src/config.rs:89`、`crates/telegram/src/plugin.rs:99`、`crates/gateway/src/channel.rs:303` 打通 `dm_scope` / `group_scope` 配置、快照与更新入口。
- 2026-03-20：在 `crates/sessions/src/metadata.rs:324` 新增 `session_buckets` 持久化映射，并在 `crates/gateway/src/channel_events.rs:46` / `crates/gateway/src/channel_events.rs:90` 用统一 bridge helper 收口 session 解析与 binding 持久化。
- 2026-03-20：在 `crates/channels/src/plugin.rs:299` 增补 target-aware 出站接口，在 `crates/telegram/src/outbound.rs:1562`、`crates/gateway/src/chat.rs:7782` / `crates/gateway/src/chat.rs:8328` 与 `crates/gateway/src/channel_events.rs:177` 接入 thread-aware 文本/媒体/位置/typing 投递。
- 2026-03-20：在 `crates/gateway/src/chat.rs:6929` / `crates/gateway/src/chat.rs:7555` 收紧 Telegram relay/mirror 会话桥接，按 bucket/session bridge 与 topic thread 兼容路径选择目标会话。
- 2026-03-20：修复 bucket-aware follow-up 回归；在 `crates/gateway/src/chat.rs:2360` 让 web UI 回声按 bucket binding 判活，在 `crates/gateway/src/chat.rs:7412` 增量刷新既有 session 的 bucket/thread-aware `channel_binding`，并在 `crates/telegram/src/handlers.rs:1814` / `crates/telegram/src/handlers.rs:2641` 为 edited live location 与 callback query 回复目标补回 `bucket_key`，同时通过 keyboard message -> bucket 绑定与 callback sender hint 保留 callback 的原会话归属。

**已覆盖测试（如有）**
- Telegram adapter route / bucket：`crates/telegram/src/adapter.rs:129`
- Telegram callback typing / follow-up 回归：`crates/telegram/src/handlers.rs:6392`、`crates/telegram/src/handlers.rs:6776`
- Telegram callback / live-location bucket follow-up 回归：`crates/telegram/src/handlers.rs:5800`、`crates/telegram/src/handlers.rs:5860`、`crates/telegram/src/handlers.rs:6527`、`crates/telegram/src/handlers.rs:6665`
- Telegram outbound thread-aware 回归：`crates/telegram/src/outbound.rs:2616`
- Gateway bucket bridge 回归：`crates/gateway/src/channel_events.rs:1944`
- Gateway web UI channel echo bucket 判活回归：`crates/gateway/src/chat.rs:13483`
- Gateway 旧 binding 升级回归：`crates/gateway/src/chat.rs:13517`
- Sessions bucket 映射回归：`crates/sessions/src/metadata.rs:1055`
- Telegram group mirror / relay 回归：`crates/gateway/src/chat.rs:9217`、`crates/gateway/src/chat.rs:9960`
- 全量验证：`cargo test -p moltis-telegram -p moltis-gateway -p moltis-sessions`

**已知差异/后续优化（非阻塞）**
- 当前 Telegram runtime / session key / bucket key 规范，已统一迁移到 `docs/src/refactor/session-key-bucket-key-one-cut.md` 与 `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`。
- 本单明确不处理第三版内核收口（统一事件记录 / core 上下文整理 / 最终落盘格式重构）。
- 本单允许阶段性复用现有 `SessionStore`、`PersistedMessage`、`session_metadata` 桥接新边界。
- `ChannelReplyTarget` / `channel_binding` / server-session-sandbox 侧旧兼容载体在 A+B 阶段继续保留；统一替换放到 C 阶段。
- `docs/src/refactor/channel-adapter-generic-interfaces.md` 对应的通用 trait / 对象正式落地放到 C 阶段。

---

## 背景（Background）
- 场景：当前 Telegram 相关能力分散在 `crates/telegram`、`crates/gateway`、`crates/channels`、`crates/sessions` 四处；V3 需要先把 Telegram 渠道适配层做实，再把 TG 的 DM / Group 会话语义整体跑通。
- 约束：
  - 本单只做 V3 的 A+B：
    - A：`telegram` / `gateway` 边界重切
    - B：Telegram 的会话语义与 DM / Group 全链路落地
  - 本单的设计主依据是 `docs/src/refactor/telegram-adapter-boundary.md`。
  - `docs/src/refactor/channel-adapter-generic-interfaces.md` 在本单里只作为回看/校验参考，不作为直接施工蓝图。
  - A+B 实施期间不以 generic interface 落地为目标；除文档同步外，不以 generic 命名或 trait 反向驱动 Telegram 专项实现。
  - 现阶段继续复用现有配置来源、现有 `chat.send` 主链、现有会话存储。
  - 修改方案必须收敛，不得把第三版内核重构混入本单。
- Out of scope：
  - `session_event` 统一事件记录重构
  - core 上下文管理 / render / compact 全面替换
  - `docs/src/refactor/channel-adapter-generic-interfaces.md` 对应的 generic trait / generic object 正式落地
  - 非 Telegram 渠道接入
  - 历史数据迁移、前向兼容清理

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Telegram adapter**（主称呼）：第三版中承接 Telegram 原生协议、Telegram 本地策略、会话路由解析、回复投递的渠道适配层。
  - Why：这是本单的主改造对象。
  - Not：不是“整个渠道系统的终版抽象层”，也不是 core。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：TG adapter

- **聊天入站对象**（主称呼）：Telegram adapter 交给上层聊天主链的最小结构化输入。
  - Why：它决定 adapter -> core 的边界是否收敛。
  - Not：不是 Telegram raw update，也不是最终给 LLM 的 transcript 文本。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`tg_inbound` / `NormalizedMessage`

- **路由解析结果**（主称呼）：由 Telegram adapter 根据入站对象与当前 scope 解析出的 `peer` / `sender` / `bucket_key` / `addressed`。
  - Why：它决定 DM / Group 最终进入哪个逻辑会话桶。
  - Not：不是旧的 `chan_chat_key` / `chan_account_key` 直推 session。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`tg_route` / `ResolvedRoute`

- **会话策略**（主称呼）：当前 Telegram account 导出的 `dm_scope` / `group_scope`。
  - Why：它决定“同桶 / 异桶”的上层语义要求。
  - Not：不是具体渠道字段拼接模板。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：session policy

- **bucket_key**（主称呼）：Telegram adapter 在当前 scope 下给出的稳定分桶结果。
  - Why：这是 TG 会话语义的最终实现结果。
  - Not：不是上层概念来源；core 不应反向解析其内部 TG 结构。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：session subkey

- **回复出站对象**（主称呼）：Telegram adapter 接收的 Telegram 专项回复出站对象。
  - Why：它冻结“core 表达回复语义、Telegram adapter 表达投递目标”的边界。
  - Not：不是跨渠道 generic reply 接口，也不是 Telegram API 请求体。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`tg_reply`

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 让 `telegram` crate 成为 Telegram 渠道真实 adapter 主体，统一承接 TG 入站归一化、TG 路由解析、TG 控制输入、TG 回复投递。
- [ ] 让 `gateway` 从“直接持有大量 TG 私有语义”退回到“桥接 TG adapter 与现有 chat/session 主链”的角色。
- [ ] 跑通 Telegram `dm` 与 `group` 两类会话语义，并让 `dm_scope` / `group_scope` 真正决定 `bucket_key`。
- [ ] 将 Telegram group 复杂能力一并收口到新边界内，包括：listen-only、mention、relay、mirror、topic/thread、reply path、media/voice/location。
- [ ] 在不重构内核落盘模型的前提下，完成 Telegram 第三版外层与会话语义落地。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须保持 Telegram DM / Group 正常收发不回退。
  - 必须保证现有 TG 复杂能力迁移后仍可观测、可测试。
  - 不得在 `gateway` 中继续扩散新的 Telegram 私有字段和策略分支。
  - 不得在本单内把 `session_event`、上下文管理、其他渠道一起重构。
- 兼容性：现阶段兼容现有 `TelegramAccountConfig`、现有 `chat.send` 主链、现有 `SessionStore` / `session_metadata`。
- 可观测性：凡是被策略拦截、降级、旁听、relay/mirror 触发、topic/thread 分桶切换、reply path 恢复失败的路径，必须有结构化日志与 `reason_code`。
- 安全与隐私：日志不得打印 token、完整敏感正文、未经截断的原始 update；必要时只打短预览或结构化摘要。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 相关逻辑并未收束在 Telegram adapter 内部，而是分散在 `telegram`、`gateway`、`sessions` 等多个模块。
2) 当前 session 绑定主链仍然主要由 `channel_type + account_handle + chat_id -> session_id` 驱动，不是由 `type/scope -> bucket_key` 驱动。
3) Telegram group 的 listen-only、mirror、relay、transcript 格式、reply path 等复杂能力与 gateway/chat 主链强耦合。
4) 当前 `channels` crate 更像旧二版壳层，不代表已经收敛好的第三版 adapter 契约。

### 影响（Impact）
- 用户体验：
  - TG DM / Group 行为口径不够统一，复杂能力容易出现边界漂移。
  - 后续引入其他渠道时，高概率继续复制“渠道细节渗入 core/gateway”的问题。
- 可靠性：
  - 一处 Telegram 特化改动容易影响 chat/session/reply 主链。
  - 复杂能力分散使回归面扩大、故障定位困难。
- 排障成本：
  - 同一条 TG 行为链横跨多个 crate，多点判断 session、reply、mirror/relay，排障需要来回追链路。

### 复现步骤（Reproduction）
1. 阅读 Telegram inbound 入口：`crates/telegram/src/handlers.rs:307`
2. 阅读 gateway 渠道桥接：`crates/gateway/src/channel_events.rs:380`
3. 阅读 gateway 内 Telegram mirror/relay/session 辅助逻辑：`crates/gateway/src/chat.rs:6929`
4. 阅读 session active 映射：`crates/sessions/src/metadata.rs:555`
5. 期望 vs 实际：
   - 期望：Telegram adapter 提供收敛边界对象与路由结果，gateway 只桥接。
   - 实际：Telegram 语义由多个模块共同拼装，session 仍主要由旧 channel/chat 绑定驱动。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/telegram/src/handlers.rs:307`：group listen-only / transcript 改写仍直接在 Telegram handlers 中处理。
  - `crates/telegram/src/handlers.rs:1507`：Telegram 入站最终仍通过 `dispatch_to_chat` 桥接现有 chat 主链，没有独立 core 接口替换。
  - `crates/gateway/src/channel_events.rs:380`：gateway 仍按 reply target / chat_id / bucket mapping 决定 session、broadcast、channel binding，并调用 `chat.send`。
  - `crates/gateway/src/chat.rs:6929`：Telegram relay / mirror / target session 辅助逻辑仍大量驻留在 gateway。
  - `crates/sessions/src/metadata.rs:555`：chat 级 active session 仍由 `(channel_type, account_handle, chat_id)` 直接映射，作为 bucket bridge 的兼容回退。
  - `crates/channels/src/plugin.rs:88`：`ChannelEventSink` 仍代表“渠道消息直接打进 gateway”的旧接口形态。
- 配置/协议证据（必要时）：
  - `crates/telegram/src/config.rs:115`：`group_session_transcript_format` 仍由 Telegram 配置直接驱动会话文本行为。
  - `crates/telegram/src/config.rs:168`：relay / mirror / transcript policy 仍作为 gateway 侧 group 行为快照对外暴露。
- 当前测试覆盖：
  - 已有：
    - `crates/telegram/src/handlers.rs:4666`
    - `crates/gateway/src/chat.rs:9113`
    - `crates/gateway/src/chat.rs:9860`
  - 缺口：
    - 暂无阻塞性自动化缺口；核心 bucket/session/follow-up 回归已补齐到 unit/integration。
    - 仍未覆盖真实 Telegram 网络与进程重启后的端到端恢复，只通过 mock API 与 sender-hint / runtime-binding 缺失回归做替代验证。

## 根因分析（Root Cause）
- A. 第二版是从“单 agent 残缺实现”逐步补成“多 agent + 多 Telegram bot”，但没有先把 core / adapter / session 语义边界冻结。
- B. `telegram` crate 只承接了一部分 Telegram 原生与策略逻辑，`gateway` 继续承接了会话定位、reply 路由、mirror / relay 等大量 TG 私有语义。
- C. `channels` crate 当前接口表达的是旧架构下的“渠道消息如何直接打进 gateway”，不是第三版要求的“adapter -> core 最小契约”。
- D. `sessions` 当前 active session 模型天然偏向 `(channel/account/chat)` 绑定，尚未由 `scope -> bucket_key` 驱动，因此 Telegram 会话语义无法自然收口。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - Telegram adapter 必须成为 Telegram 渠道语义的唯一主实现层。
  - gateway 必须只消费收敛后的 Telegram adapter 产物，并桥接到现有 chat/session 主链。
  - Telegram `dm` / `group` 必须由会话策略与路由解析结果决定最终 `bucket_key`。
  - Telegram group 复杂能力必须挂在统一 adapter 边界内，而不是继续散落在 gateway 多处分支。
  - 本单实施后，TG DM 与 TG Group 应能在第三版外层边界下整体稳定运行。
- 不得：
  - 不得把 Telegram 私有字段继续直接扩展成 core / gateway 的长期公共字段。
  - 不得在本单中同时重构统一事件记录、最终上下文引擎、其他渠道。
  - 不得为了抽象而先行设计一套大而全的跨渠道接口体系。
- 应当：
  - `channels` crate 只应提炼 Telegram 已压实的稳定接口，不应成为主战场。
  - `sessions` / 旧落盘 / 旧上下文链路应尽量桥接复用，待后续第三版内核阶段再统一处理。
  - `private_source` / `private_target` 在 A+B 阶段应保持 Telegram adapter 私有 opaque carrier 语义，不在 `gateway` / `sessions` / `channels` 中展开内部字段。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：将 V3 的 A+B 合并实施；先在 Telegram 真实链路上完成 adapter 边界与会话语义落地，再桥接旧会话主链。
- 优点：
  - 避免 A 阶段只做抽象壳子、B 阶段再返工接口。
  - 可一次性收口 TG DM / Group / complex features 的边界。
  - 对用户讨论成本更低，整体推进效率更高。
- 风险/缺点：
  - 范围比单做 A 更大，必须严格锁住 Out of scope，防止 C 阶段内核改造混入。

#### 方案 2（备选）
- 先只做 A，再单独做 B。
- 风险/缺点：
  - 接口极可能停留在半抽象状态，后续 B 阶段仍要返工 `telegram` / `gateway` 边界。
  - 用户参与讨论与 review 成本更高。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（TG-first）：先以 Telegram 真实需求压实边界，再把稳定部分提炼进 `channels` crate。
- 规则 2（外向内）：先改 `telegram` / `gateway`，后改 `sessions` / core 内核。
- 规则 3（桥接旧主链）：现阶段新边界输出应兼容现有 `chat.send`、`SessionStore`、`session_metadata`。
- 规则 4（复杂能力一并收口）：listen-only、mention、relay、mirror、topic/thread、reply path、media/voice/location 都必须进入新边界，不再新增散点 TG 分支。
- 规则 5（不混入 C）：统一事件记录、上下文管理、跨渠道推广不进入本单。

#### 接口与数据结构（Contracts）
- API/RPC：
  - 现阶段复用现有 Telegram account 配置入口与 update/start/stop 生命周期。
  - 现阶段不要求对外 RPC 立即切换到第三版终版接口。
- 存储/字段兼容：
  - 继续使用现有 `session_metadata` active session 映射与 `SessionStore`。
  - Telegram adapter 内部产生的新边界对象、`private_target` / reply path 恢复结果、route 结果，可先通过桥接层映射到旧字段。
- UI/Debug 展示（如适用）：
  - 调试展示优先补充“adapter 边界命中/降级原因、scope、bucket_key、record_only/dispatch、addressed、reply path 恢复状态”。

#### 已冻结的 Telegram 专项边界对象（Implementation-frozen）
- 聊天入站对象：
  - `tg_inbound { kind, mode, body, private_source }`
  - `tg_content { text, attachments, location }`
- 路由解析：
  - `resolve_tg_route(tg_inbound, scope) -> tg_route`
  - `tg_route { peer, sender, bucket_key, addressed }`
- 控制输入：
  - `tg_control`
- 回复出站：
  - `tg_reply { output, private_target }`
  - `send_tg_reply(tg_reply)`

#### Bridge 约束（Implementation-frozen）
- `telegram` crate 拥有：
  - `private_source`
  - `private_target`
  - `bucket_key` 的 Telegram 内部生成逻辑
  - reply path / reply target 恢复逻辑
- `gateway` 只允许：
  - 消费 `tg_inbound` / `tg_route` / `tg_control` / `tg_reply`
  - 桥接到现有 `chat.send` / `SessionStore` / `session_metadata`
  - 做 run 编排、队列与现有 session 主链桥接
- `channels` / `sessions` 在 A+B 阶段只允许 bridge-only 修改：
  - 不新增 Telegram 业务语义
  - 不为 generic interface 落地而重构
  - 不解析 `private_source` / `private_target` 内部结构

#### 实施前收口（Pre-implementation Freeze）
- session 选择主键：
  - Telegram 新主链的 session 选择真值改为 `bucket_key`，不再继续以 `(channel_type, account_handle, chat_id)` 作为 Telegram `group_scope` 主路径的最终判定。
  - `sessions` 可新增 bridge-only 的 bucket -> `session_id` 持久化映射，但该映射只保存字符串，不承载 Telegram 语义解析。
  - 旧 `(channel_type, account_handle, chat_id)` active session 映射继续保留，仅作为旧路径兼容与回退兜底，不再代表第三版 Telegram 会话语义本身。
- gateway 桥接方式：
  - `crates/gateway/src/channel_events.rs` 必须收口为一个统一的 Telegram bridge helper，负责：session 选择、channel binding 持久化、reply target 入队、`chat.send` 桥接、失败清理。
  - `dispatch_to_chat` / `ingest_only` / `dispatch_to_chat_with_attachments` / `dispatch_command` 不得继续各自复制一份 Telegram session 解析流程。
- 旧兼容载体策略：
  - `private_target` 是 Telegram adapter 的真实投递目标。
  - `ChannelReplyTarget` / `channel_binding` 在 A+B 阶段只保留为旧外围链路兼容载体，主要服务于：reply queue、session 列表、sandbox router、server 侧 location prompt、旧 UI 展示。
  - 除 Telegram adapter 与 gateway bridge 恢复点外，其他模块不得依赖或扩展 Telegram 私有字段。
- branch / sender 缺失时的固定降级：
  - `sender` 缺失而 scope 依赖 sender 时，按“去掉 sender 维度”降级，并记录结构化 `reason_code`。
  - `branch` 缺失而 scope 依赖 branch 时，按“去掉 branch 维度”降级，并记录结构化 `reason_code`。
  - `group_scope = per_branch_sender` 时，若两者都缺失，最终降级到 `group`；不得伪造 sender / branch。
- topic/thread 口径：
  - core 只看 `bucket_key`，不看 Telegram 原生 topic/thread 字段。
  - Telegram adapter 内部可继续使用 forum topic / thread / reply path 等原生概念完成 `branch` 维度实现。
  - 真正的 Telegram forum topic/thread 收发支持仍属于 A+B 范围，不下放到 C；下放到 C 的仅是“跨渠道统一 branch 抽象”和“移除旧兼容载体”。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - Telegram raw update 解析失败：记录结构化日志并按失败类型决定忽略/重试/用户反馈。
  - route 解析缺关键字段：必须记录 `reason_code`，并按 scope 语义做可解释降级。
  - reply path 恢复失败：不得静默吞没；必须记录 `reason_code` 并走安全降级。
  - relay / mirror / mention / topic 判定未命中：命中候选但被策略拦截时必须可观测。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 不改现有 chat run 队列/状态机主逻辑。
  - 新边界桥接失败时，必须清理 reply target / typing 生命周期残留状态。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - Telegram token、原始 update、未截断正文、外部敏感标识不得直接打印。
  - 复杂策略日志默认仅打印结构化标识、短预览、关键 id。
- 禁止打印字段清单：
  - bot token
  - 原始 update 完整 JSON
  - 未截断的正文全文
  - 未脱敏的外部认证/审批信息

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `telegram` crate 成为 Telegram adapter 主体，TG 入站 / 路由 / 控制 / 回复边界清晰，gateway 不再新增 TG 私有语义分支。
- [x] Telegram `dm` 与 `group` 均由会话策略与 route 结果决定最终 `bucket_key`，而不是继续零散依赖旧 `chat_id/account` 推导。
- [x] Telegram DM 文本、Group 文本、listen-only、mention、relay、mirror、topic/thread、reply path、media/voice/location 均已跑通新边界。
- [x] `channels` crate 与 `sessions` crate 仅发生 bridge-only 修改，不承载新的 Telegram 业务逻辑，也不以 generic interface 落地为目标。
- [x] 现有 `SessionStore` / `session_metadata` / `chat.send` 继续可用，且桥接路径行为可观测、可回滚。
- [x] 与本单相关的关键路径均有自动化测试；如确实无法自动化，已记录缺口和手工验收步骤。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] Telegram 入站归一化：`crates/telegram/src/handlers.rs:534`
- [x] Telegram route 解析（DM / Group / addressed / sender missing / topic/thread）：`crates/telegram/src/adapter.rs:63`
- [x] Telegram reply target / reply path 恢复：`crates/telegram/src/handlers.rs:3412`、`crates/telegram/src/outbound.rs:1562`
- [x] `group_scope` / `dm_scope` -> `bucket_key` 结果：`crates/telegram/src/adapter.rs:90`

### Integration
- [x] Telegram DM 文本走新 adapter 边界并桥接旧 `chat.send`
- [x] Telegram Group 文本走新 adapter 边界并正确区分 `dispatch` / `record_only`
- [x] relay / mirror / mention / topic-thread / media / voice / location 回归
- [x] callback query / edited live location / web UI channel echo 在多 bucket 群聊下保持同桶会话归属
- [x] gateway 桥接层不再直接承担新的 Telegram 私有策略决策

### UI E2E（Playwright，如适用）
- [x] 暂不新增；以 gateway / Telegram integration tests 和手工在线验证为主

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 真实 Telegram 在线链路、topic/thread 组合、bot-to-bot relay 与外部网络环境存在自动化覆盖边界。
- 手工验证步骤：
  - 启动至少两个 Telegram bot account 与一个真实 TG group。
  - 验证 DM：文本、语音、图片、位置、reply path。
  - 验证 Group：未点名旁听、点名 dispatch、mention command、relay、mirror、topic/thread、media、voice、location。
  - 验证日志：所有被策略拦截或降级的路径均有结构化 `reason_code`。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 分支内一次性完成 A+B 代码与测试后再合并。
  - 优先在 Telegram 场景回归，不扩大到其他渠道。
- 回滚策略：
  - 保持旧 `SessionStore` / `session_metadata` / `chat.send` 主链不变。
  - 若新 adapter 边界不稳定，可回退到改造前的 `telegram` / `gateway` 组合实现。
- 上线观测：
  - Telegram inbound normalization / route resolve / bucket resolve / 回复投递 结构化日志
  - relay / mirror / mention / topic-thread 判定日志
  - typing / send / dispatch / ingest_only / queued followup 日志

## 实施拆分（Implementation Outline）
- Step 1（冻结 Telegram 专项边界对象）：
  - 在 `telegram` crate 内先落地 `tg_inbound` / `tg_route` / `tg_control` / `tg_reply` 这组内部对象与配套 helper。
  - 优先在 `crates/telegram/src/handlers.rs`、`crates/telegram/src/outbound.rs`、`crates/telegram/src/plugin.rs` 内完成内部收口；如确有必要，再新增 Telegram crate 内部模块文件。
  - 退出条件：Telegram 专项边界对象名称、字段、职责与文档一致，且不要求 `gateway` 先理解其内部私有字段。
- Step 2（收 Telegram 入站与回复归属）：
  - 将聊天入站归一化、reply path / reply target 恢复、TG 私有投递信息恢复收回 `telegram` crate。
  - `private_source` / `private_target` 只允许在 `telegram` crate 内构造和解释。
  - 退出条件：`gateway` 不再新增对 Telegram reply target / raw chat routing 细节的直接分支，且旧 `ChannelReplyTarget` 仅保留兼容桥接用途。
- Step 3（收 Telegram route 与 bucket_key）：
  - 将 DM / Group 的 route 解析、`bucket_key` 生成、`addressed` 语义判定收敛到 Telegram adapter。
  - `gateway` 只接 `tg_route` 结果，并据此桥接现有 session 主链。
  - `sessions` 侧如需新增 bucket -> `session_id` 持久化映射，只允许以 bridge-only 字符串映射形式落地。
  - 退出条件：不再新增 `channel_type + account + chat_id` 风格的 Telegram 新分支作为主路径，且 `group_scope` 能真实表达 `per_sender` / `per_branch` / `per_branch_sender`。
- Step 4（切 DM 与 Group 基础链路）：
  - 先切 Telegram DM 文本与基础 reply。
  - 再切 Group 文本、`dispatch` / `record_only`、`sender`、基础 topic/thread 场景。
  - 退出条件：DM / Group 基础文本链路均通过新边界进入现有 `chat.send` / `SessionStore`。
- Step 5（切 Group 复杂能力）：
  - 将 listen-only、mention、relay、mirror、topic/thread、reply path、media/voice/location 统一挂到 Telegram adapter 边界下。
  - 复杂策略只允许附着在 Telegram adapter 主链外侧，不得重新改写 core 会话主定义。
  - 退出条件：复杂能力已迁移，`gateway` 中不再保留新的 Telegram 复杂策略散点实现。
- Step 6（bridge-only 收尾与回归）：
  - `channels` / `sessions` 只做必要 bridge-only 适配。
  - 补齐单测、集成测试、文档同步与真实 TG 手工回归清单。
  - 退出条件：本单 Acceptance Criteria 全部可逐条验收。
- 受影响文件：
  - Telegram 主战场：
    - `crates/telegram/src/handlers.rs`
    - `crates/telegram/src/outbound.rs`
    - `crates/telegram/src/plugin.rs`
    - `crates/telegram/src/config.rs`
    - `crates/telegram/src/lib.rs`
  - Gateway 桥接层：
    - `crates/gateway/src/channel_events.rs`
    - `crates/gateway/src/chat.rs`
    - `crates/gateway/src/session_labels.rs`
  - Bridge-only：
    - `crates/channels/src/plugin.rs`
    - `crates/sessions/src/metadata.rs`
  - 文档：
    - `docs/src/refactor/telegram-adapter-boundary.md`
    - `docs/src/refactor/v3-roadmap.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/refactor/v3-design.md`
  - `docs/src/refactor/v3-roadmap.md`
  - `docs/src/refactor/v3-gap.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `docs/src/refactor/channel-adapter-generic-interfaces.md`
  - `docs/src/refactor/dm-scope.md`
  - `docs/src/refactor/group-scope.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - `../openclaw/docs/design/session-channel-mental-model.md`
  - `../openclaw/docs/design/session-key-example-walkthrough.md`

## 未决问题（Open Questions）
- 当前实现前口径已冻结，A+B 可直接开工。
- 暂无阻塞性未决问题；以下实现前提已视为已定：
  - `channels` crate 在 A+B 阶段仅保留旧接口桥接，不以 generic interface 落地为目标。
  - `private_source` / `private_target` 在 bridge 阶段保持 Telegram adapter 私有 opaque carrier，不做最小公共壳展开。
  - Telegram 内部可继续使用 `topic/thread` 等原生称呼实现细节；对 core 仅暴露 `bucket_key`、`addressed`、`sender` 等专项边界结果。
- 以下遗留问题明确下放到 C 阶段，不阻塞 A+B：
  - 用统一第三版会话记录替换当前 `ChannelReplyTarget` / `channel_binding` 兼容载体。
  - 将 server / session / sandbox / prompt runtime 对旧 channel binding 的依赖整体迁移到第三版正式会话对象。
  - 将 `docs/src/refactor/channel-adapter-generic-interfaces.md` 落成真正的通用接口壳。
  - 统一 Telegram 之外渠道的 adapter -> core 契约与回复出站模型。
  - 统一事件记录、上下文整理、落盘格式与跨渠道 branch 抽象。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

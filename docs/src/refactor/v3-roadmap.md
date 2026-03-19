# V3 实施路线图

本文档只定义一件事：

- 第三版架构应如何按一条**外向内、TG 优先、渐进替换**的路径落地

本文档不讨论：

- 历史数据迁移
- 前向兼容
- 其他渠道如何并行接入
- 某个阶段内的具体代码 diff
- 第三版整体设计原则本身

也就是说，本文档讨论的是：

- **第三版的实施顺序与阶段边界**

而不是：

- 某个模块的局部实现细节

## 一句话结论

第三版不应从“先重构整个 core”开始。

更稳妥的落地路径应当是：

- 先从 **Telegram 渠道适配边界** 开始
- 再把 **Telegram 的 session 分桶/定位** 单独抽出
- 期间阶段性复用现有 **会话记录** 与 **旧上下文链路**
- 然后按 **TG DM -> TG Group -> TG Group 复杂能力**
  的顺序逐段替换
- 最后再把内部旧会话记录与旧上下文管理替换成第三版目标形态

一句人话：

- **先收 Telegram 外壳，再逐步往里换核心**

第三版整体设计原则，见：

- `docs/src/refactor/v3-design.md`

## 路线总原则

### 1. 从外向内

不要一开始就改最内层核心契约。

先改：

- Telegram 收消息入口
- Telegram 到 core 的边界

后改：

- 统一事件记录模型
- core 上下文管理

### 2. 先垂直切 Telegram，不横扫全系统

现阶段只聚焦：

- Telegram adapter
- 与 Telegram 直接相连的 core 上层

现阶段暂不处理：

- Feishu / Slack / Discord 等其他渠道

### 3. 先收边界，再换内核

前几个阶段应优先做到：

- Telegram 不再直接主导“给模型的文本”语义
- Telegram 不再在多个模块里零散决定 session
- Telegram 入口只负责归一化与交付

在边界稳定之前，不急着改：

- 最终事件记录模型
- 最终上下文管理

### 4. 阶段性复用旧能力

第三版落地初期，允许阶段性复用：

- 现有 `SessionStore`
- 现有 `PersistedMessage`
- 现有 chat.send / chat run 主链
- 现有上下文整理链路

但复用方式必须是：

- **新边界输出兼容旧接口**

而不是：

- 继续让旧接口反过来定义第三版边界

### 5. 按复杂度递进

Telegram 侧的切换顺序应固定为：

1. DM 文本
2. Group 文本
3. Group relay / mirror / mention 复杂策略
4. 多媒体 / voice / location / reply_ref

不要倒着做。

## 整体步骤

第三版建议分成 7 个阶段。

### 阶段 1：抽取 Telegram adapter 边界

先把 Telegram 从当前散落逻辑中收边界。

这一阶段只回答一个问题：

- **Telegram 收到一条原生消息后，如何把它整理成面向 core 的统一输入**

### 阶段 2：抽取 Telegram session 分桶/定位模块

把“这条 TG 消息应该进哪个 session”从零散逻辑中单独收出来。

这一阶段只回答一个问题：

- **Telegram DM / Group 的会话定位规则如何统一收口**

### 阶段 3：用新 Telegram 边界桥接旧会话记录与旧上下文

在不改内核存储格式的前提下，让新的 Telegram 输入边界先跑起来。

这一阶段只回答一个问题：

- **新 TG adapter 能否先稳定接上现有 `SessionStore` 与 chat 主链**

### 阶段 4：先切 Telegram DM

第一批真正进入第三版路径的流量，应当是：

- Telegram DM 文本消息

这一阶段的目标是：

- 用最简单的业务切片验证新边界和 session 定位

### 阶段 5：再切 Telegram Group 基础链路

第二批进入第三版路径的流量，应当是：

- Telegram Group 普通文本消息

这一阶段先不碰复杂群策略，只先把：

- `group_scope`
- `peer`
- `per_branch` / `per_branch_sender` 的黑盒落地
- `sender`
- listen-only / addressed run

这些基础能力理顺。

### 阶段 6：切 Telegram Group 复杂能力

当 Group 基础链路稳定后，再处理：

- relay
- mirror
- mention 触发策略
- topic / thread / forum topic
- reply_ref
- 多媒体 / voice / location

这一阶段的目标是：

- 把 Telegram 复杂能力也收进第三版边界，而不是继续散落在 gateway/chat 特化分支里

### 阶段 7：最后替换内部旧内核

当 Telegram 全链路都稳定跑在第三版边界上后，再逐步替换：

- `session_event` 统一事件记录
- core 上下文管理
- 旧会话文本驱动的保存/拼接路径

也就是说：

- **最里面的内核改造，放到最后**

## 各阶段的具体工作

下面按阶段展开。

## 阶段 1：抽取 Telegram adapter 边界

### 本阶段目标

先把 Telegram 入口整理干净。

让 Telegram 这一层只负责：

- 解析原生 update
- 识别消息类型
- 提取 Telegram 原生字段
- 归一化出一个面向 core 的 Telegram 输入对象

### 本阶段完成后应达到的状态

Telegram adapter 输出的对象，至少应能稳定表达：

- 当前消息属于 `dm` 还是 `group`
- 当前消息体 `body`
- 当前附件 `attachments`
- 当前 reply / quote / source message 信息
- 当前消息属于普通消息、命令、listen-only、relay 候选中的哪一类
- 一份仅供 Telegram adapter 自己后续解析的 `private_source` 私有载荷

这里故意**不**要求阶段 1 直接产出：

- `peer`
- `sender`
- `branch`
- `bucket_key`
- `addressed`

这些应在下一阶段通过：

- `resolve_tg_route(tg_inbound, scope) -> tg_route`

再由 Telegram route resolver 产出。

### 本阶段不要做什么

- 不在聊天主入口对象里提前塞入 `peer` / `sender` / `bucket_key`
- 不在聊天主入口对象里提前塞入 `branch` / `addressed`
- 不改最终落盘格式
- 不改上下文管理
- 不改全局 session key 体系
- 不改其他渠道

### 本阶段为什么先做

因为当前 Telegram 相关语义仍散落在：

- `crates/telegram/src/handlers.rs`
- `crates/gateway/src/channel_events.rs`
- `crates/gateway/src/chat.rs`

必须先把 Telegram 入口与 core 的责任切开，后续才能稳定推进。

## 阶段 2：抽取 Telegram session 分桶/定位模块

### 本阶段目标

把“Telegram 消息到底应该落到哪个会话”收成一个明确模块。

这一模块至少要统一处理：

- Telegram DM
- Telegram Group
- Telegram account
- Telegram chat
- Telegram thread/topic
- Telegram sender

### 本阶段完成后应达到的状态

Telegram 侧不再在多个模块零散决定：

- 是不是新 session
- 当前 active session 是谁
- 当前消息应进入哪个 session

而应统一改成：

- Telegram adapter 输出归一化对象
- Telegram route resolver 先返回 `tg_route`
- 再由 Telegram session resolver 基于 `tg_route + scope` 返回当前消息对应的 session 定位结果

### 本阶段允许复用什么

本阶段允许继续复用：

- 当前 `channel_sessions`
- 当前 `sessions` metadata
- 当前 active session 记录方式

原因是：

- 这一阶段的目标是先收口“谁来决定 session”，不是立刻改底层存储模型

### 本阶段不要做什么

- 不引入最终版 `session_event`
- 不引入全渠道统一 resolver
- 不处理其他渠道

## 阶段 3：桥接旧会话记录与旧上下文

### 本阶段目标

让新 Telegram 边界先稳定接上旧核心。

也就是说：

- 新 Telegram adapter
- 新 Telegram session resolver

在这一阶段后面，先继续桥接到：

- 现有 `PersistedMessage`
- 现有 `SessionStore`
- 现有 chat.send / agent run 主链
- 现有上下文整理逻辑

### 本阶段完成后应达到的状态

第三版前半截已经建立：

- Telegram 入口边界是新的
- Telegram session 决策边界是新的

但第三版后半截还没替换：

- 会话记录仍可先写旧格式
- 上下文仍可先走旧链路

### 本阶段为什么关键

这是整条路线能否收敛的关键。

如果没有这一步，就只能二选一：

- 要么一开始就大改最内层核心
- 要么第三版边界永远被旧链路反向污染

桥接层的作用就是：

- **让外层先变对，内层稍后再换**

## 阶段 4：切 Telegram DM

### 本阶段目标

先让 Telegram DM 文本消息跑到新链路。

建议本阶段只覆盖：

- 文本入站
- 基础 reply
- 基础 session 进入
- 基础上下文使用

### 本阶段为什么先切 DM

因为 DM 是当前 Telegram 场景里最简单、变量最少的一条链。

它最适合验证：

- adapter 边界是否合理
- session 定位是否合理
- 旧桥接层是否稳定

### 本阶段不要做什么

- 不碰 group
- 不碰 relay / mirror
- 不碰复杂媒体

## 阶段 5：切 Telegram Group 基础链路

### 本阶段目标

在 DM 跑稳后，再把 Group 基础文本链路切到新边界。

本阶段优先处理：

- addressed group message
- listen-only ingest
- group 下的 `peer`
- group 下的 `sender`
- group 下的子线判别结果
- `group_scope` 的基础落地

### 本阶段的核心要求

这一阶段开始，Telegram Group 不应再由 Telegram handler 直接主导：

- 给模型的文本格式
- 上下文语义
- session 分桶

它只应负责：

- 提供 Telegram 原生信息
- 交给上层 session resolver 与桥接层

### 本阶段不要做什么

- 不先碰 relay / mirror
- 不先碰 topic/thread 的全部复杂边角
- 不先碰多媒体和 voice

## 阶段 6：切 Telegram Group 复杂能力

### 本阶段目标

把 Telegram Group 的复杂能力也收进第三版边界。

建议这一阶段逐步吸收：

- relay
- mirror
- mention 相关策略
- topic / thread / forum topic 细化规则
- reply_ref
- 多媒体
- voice / location

### 本阶段的要求

复杂策略可以继续存在，但必须：

- 通过 Telegram adapter 输出结构化信息
- 通过 Telegram session resolver 和 core 上层接管语义
- 不再把复杂语义直接写死在会话文本或给模型的文本里

## 阶段 7：最后替换内部旧内核

### 本阶段目标

在 Telegram 外层已经稳定后，再替换第三版真正的内核目标：

- `session_event`
- 统一事件记录持久化
- core 上下文管理

### 本阶段为什么最后做

因为这些属于最内层改造。

只有在 Telegram 外层已经稳定后，再替换内层，才能避免：

- 一边改内核
- 一边还在反复改 Telegram 入口边界

### 本阶段完成后意味着什么

到这一步，第三版才算真正完成了：

- 外层 Telegram adapter 已收敛
- 中层 session 分桶/定位已收敛
- 内层统一事件记录与上下文管理已完成替换

## 每阶段的复用策略

为了保证路径收敛，复用策略应明确固定：

### 阶段 1 ~ 3

明确复用：

- 现有 `SessionStore`
- 现有 `PersistedMessage`
- 现有 chat.send / chat run
- 现有上下文整理

### 阶段 4 ~ 6

优先复用：

- 现有落盘
- 现有上下文整理

只替换：

- Telegram adapter 边界
- Telegram session resolver
- Telegram 业务链路

### 阶段 7

再真正替换：

- 统一事件记录模型
- 上下文管理

## 本路线明确不建议的做法

不建议：

- 一开始就全局重构所有 core 契约
- 一开始就把所有渠道一起纳入第三版
- 一开始就把旧会话记录和旧上下文全部替换
- 在 Telegram Group relay/mirror 还没收边界前先改最内层上下文管理
- 按零散函数、零散字段、零散命名做碎片化替换

## 最后收口

如果只记住一句话，就记住这句：

- **第三版应先从 Telegram 外围收边界开始**
- **期间阶段性复用旧会话记录与旧上下文**
- **按 TG DM -> TG Group -> TG Group 复杂能力 的顺序推进**
- **最后再替换最里面的事件记录持久化与上下文管理**

## 相关文档

- `docs/src/concepts-and-ids.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`
- `docs/src/refactor/session-scope-overview.md`
- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

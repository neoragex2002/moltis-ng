# 会话保存与上下文整理

本文档只定义一件事：

- 在第三轮目标形态下，系统应如何拆开会话保存与推理上下文整理的职责

本文档不讨论：

- `dm_scope` / `group_scope` 的具体取值
- `session_key` 的具体拼接字符串细节
- Telegram / Feishu / Slack 等渠道的原生字段长什么样
- 具体某个 adapter 的实现代码

也就是说，本文档讨论的是：

- **会话保存与上下文整理的责任边界**

而不是：

- 某个具体渠道今天怎么临时拼 prompt

## 一句话结论

最终形态应当是：

- `type` 拥有会话语义与上下文语义
- `core` 拥有会话记录格式与完整的上下文管理
- `adapter` 只拥有渠道归一化与渠道收发协议

更具体地说：

- 会话保存应尽量是统一、结构化、可重放的
- 推理上下文应当是根据结构化会话事件**整理出来**的，而不是直接把 adapter 拼好的 transcript 当作唯一事实来源

## C 阶段实施口径（当前优先级）

当前 C 阶段先解决的是：

- Telegram adapter / core 的职责边界
- 最终给模型的上下文整理归属
- Telegram 残余 transcript shaping 的清理

当前 C 阶段暂不要求：

- 立即落地 `session_event` 持久化
- 立即替换 `SessionStore` / `PersistedMessage`
- 立即完成全渠道统一实现

也就是说，C 阶段允许采用：

- **legacy persistence bridge**

它的准确含义是：

- 保存层暂时继续写旧记录
- core 负责把旧记录桥接成当前需要的上下文输入
- adapter 只负责归一化、路由语义与协议收发

当前更准确的链路应当是：

```text
Telegram raw update
  -> tg_inbound / tg_route
  -> core session resolve
  -> legacy persistence write/read
  -> core context bridge
  -> model input
  -> core reply result
  -> tg reply delivery
```

## 必须先分开的三件事

第三轮里，至少要把以下三件事彻底分开。

### 1. 会话身份

它回答的是：

- 这条消息属于哪个逻辑桶

它对应的核心对象是：

- `type`
- `scope`
- `session_key`
- `session_id`

### 2. 会话记录格式

它回答的是：

- 这条消息/事件在磁盘里如何被保存

这里的目标应是：

- 结构化
- 稳定
- 可重放
- 尽量不掺杂面向模型的临时文本包装

### 3. 推理上下文格式

它回答的是：

- 在当前一次 agent run 里，应把哪些已保存事实以什么形式提供给模型

这里的目标应是：

- 面向 LLM
- 按 `type` 区分
- 可随上下文管理规则演进
- 不反过来污染保存层

这三者若混在一起，就会出现典型问题：

- adapter 直接拼大段 transcript，保存与推理耦死
- 某个渠道一改格式，历史重放和新 prompt 行为一起漂移
- 同一个 `type` 在不同渠道上产生不同语义

## 最终目标形态

目标形态可以收敛成 4 段职责。

### 第一段：会话语义

这一层由 core 定义。

它负责回答：

- 当前是哪一种 `type`
- 当前采用哪个 `scope`
- 应按哪些语义轴决定同桶/异桶
- 当前消息应映射到哪个 `session_key`
- 当前 `session_key` 指向哪个 `session_id`

这一层只处理语义，不处理渠道字段。

### 第二段：渠道归一化

这一层由 adapter 定义。

它负责回答：

- 这个原生入站事件对应哪个逻辑 `peer`
- 这个事件里是否存在 `branch`
- 这个事件的 `sender` 是谁
- 这个事件对应哪个 `account`
- 原生 reply / thread / topic / media / message id 如何映射到统一字段

这一层输出的应当是：

- **结构化归一化事件**

而不是：

- 一段已经定稿、不可逆的 prompt 文本

### 第三段：统一记录

这一层由 core 定义。

它负责决定：

- 会话事件如何持久化
- 哪些字段是统一字段
- 哪些字段只是 adapter 的补充信息

这一层的保存对象，可以称为：

- `session_event`

它应该是结构化对象，而不是“已经给模型看的最终文本”。

### 第四段：上下文整理

这一层由 core 定义。

它整体负责：

- 把已保存的 `session_event` 整理成当前 run 需要的上下文块
- 处理不同 `type` 的上下文表达
- 选择哪些事件进入当前窗口
- 做截断、压缩、摘要、compaction
- 组装最终喂给模型的 messages / context blocks

这里要明确：

- `dm` 与 `group` 可以有不同的 render 规则
- 但这些都属于 core 内部的上下文管理
- 不需要再在架构层面额外拆成多个并列“大块”

## C 阶段必须冻结的边界

为了让 C 阶段能直接实施，下面几条边界需要先冻结。

### 1. adapter 不再主导 LLM 可见文本

尤其是 Telegram，不应再负责：

- speaker/envelope 的最终文本格式
- group transcript format 的最终选择
- relay / mirror 的 LLM 可见前缀文本
- “给模型看什么文本”的最终塑形

它可以继续负责：

- 原生消息解析
- 结构化内容归一化
- route / reply target / control follow-up 恢复
- typing / retry / liveness / threading 这类协议行为

### 2. core 必须拥有 context assemble / render

从 C 阶段开始，core 至少应统一负责：

- 读取当前 session 历史
- 识别 `dm` / `group` 不同上下文规则
- 处理 speaker、reply continuity、引用关系、窗口控制
- 生成最终喂给模型的 messages / context blocks

### 3. 保存层允许先 bridge，不要求先 final

在 C 阶段里：

- 可以继续写旧 `PersistedMessage`
- 可以继续读旧 `SessionStore`
- 但不允许再由 Telegram 反向定义保存层与上下文层边界

## 推荐的责任边界

### 先讲清一条方向性原则

第三轮里，core 和 adapter 的关系必须长期保持为：

- **core 只定义核心语义**
- **adapter 只封装渠道细节并输出归一化结果**

这条原则意味着：

- core 不需要知道 Telegram / Feishu / Slack 的原生字段长什么样
- core 不需要知道某个渠道内部到底如何识别 account / peer / thread / topic
- core 只关心这些渠道细节最后是否被可靠地归一化成核心语义字段
- core 不应把所有 type-specific 语义轴一开始就平铺成一份膨胀的“核心概念总表”

也就是说：

- 核心层可以有自己的概念体系
- 渠道层也可以有自己的内部概念体系
- 但渠道层内部概念不应直接上升为 core 的长期公共概念

更准确地说：

- core 应先冻结少量稳定概念，例如 `agent`、`type`、`scope`、`peer`
- `session_key`、`session_id` 是 core 管理的会话身份结果
- `sender`、`account`、`channel` 等，则应由不同 `type` / `scope` 在需要时按语义引入
- `branch` 不应被抬成强 core 公共概念；它更适合作为 adapter 为 `per_branch` 一类语义返回的黑盒子线标识

一句话：

- **core 负责定义“要什么语义”**
- **adapter 负责实现“本渠道怎么把这些语义做出来”**

### `core` 应负责什么

- `type` 与 `scope` 语义
- `session_key` / `session_id` 生命周期
- `session_event` 统一结构
- 完整的上下文管理（包括 render / assemble / compact / ingest）

### `adapter` 应负责什么

- 原生渠道事件解析
- 原生 reply / thread / topic / media / sender / account 提取
- 结构化字段归一化
- 对 group-like 场景返回满足 `per_branch` 语义的子线判别结果
- 出站协议格式与投递
- 少量渠道特定补充信息

### `adapter` 不应负责什么

- 主导最终落盘格式
- 主导最终 LLM 上下文文本格式
- 让不同渠道各自定义一套长期稳定 transcript 语义
- 把本渠道内部概念直接扩散成 core 公共概念

### C 阶段里 Telegram 还可以保留什么

当前不需要为了 C 阶段，强行把以下能力从 Telegram adapter 挪走：

- reply target 恢复
- callback / location / control follow-up
- typing keepalive
- retry / backoff / liveness
- thread/topic/reply_to 的 Telegram 协议细节

这些能力的边界要求只是：

- **它们属于 Telegram 的协议与策略职责**
- **但它们不再决定最终给模型看的上下文文本**

## 为什么“保存格式”和“上下文格式”不能是同一层

因为两者服务的目标不同。

### 保存格式追求什么

- 稳定
- 完整
- 可审计
- 可重放
- 能支撑将来的上下文管理规则演进

### 上下文格式追求什么

- token 效率
- LLM 可读性
- 当前 `type` 的对话语义
- 当前模型窗口与 compaction 策略

因此更合理的关系应是：

- **保存层是唯一事实来源**
- **上下文是按需整理出来的模型输入**

而不是：

- 保存层直接等于某一代 prompt 文本

## 推荐的数据模型方向

第三轮不一定要一次性冻结所有字段，但建议从一开始就按“结构化事件 + 类型特定扩展”设计。

更完整的 `session_event` 字段表，见：

- `docs/src/refactor/session-event-canonical.md`

### 基础层

每条 `session_event` 至少应有：

- `session_key`
- `session_id`
- `type`
- `direction`
- `event_kind`
- `ts`
- `body`
- `attachments`
- `reply_ref`
- `source`
- `adapter_hints`

### `dm` 侧语义字段

建议至少有：

- `peer`
- `channel`
- `account`

### `group` 侧语义字段

建议至少有：

- `peer`
- `branch`
- `sender`

其中：

- `peer` 表示 agent 面对的多人共享会话对象
- `branch` 表示 adapter 返回的黑盒子线标识，而不是需要 core 深度理解的统一对象模型
- 这些是保存层与上下文整理会消费的结构化事实，不等于 core 必须长期公开维护一张同样大小的核心概念总表

### 关于 `adapter_hints`

允许 adapter 提供少量本地提示，例如：

- 原生 thread id
- 原生 topic root id
- 原生 provider message id
- 某些必须保留但又不该上升为核心概念的边界信息

但它只能是：

- 补充信息

不能反过来成为：

- 上层语义定义本身

## `dm` 和 `group` 为什么需要不同上下文规则

因为这两个 `type` 的上下文问题不同。

### `dm` 的核心问题

`dm` 更关心：

- 对端是谁
- 入口来自哪个 `channel`
- 是否区分 `account`

因此它的上下文管理更关心：

- 1v1 关系
- reply continuity
- 账户/渠道隔离语义

### `group` 的核心问题

`group` 更关心：

- agent 当前面对的是哪个多人共享会话对象
- 当前是否在某个 `branch`
- 发言人是谁

因此它的上下文管理更关心：

- 群内说话人标识
- adapter 提供的子线上下文边界
- 群提示词、群内激活模式、群内引用关系

所以：

- `dm` 与 `group` 可以共用底层保存框架
- 但不应共用同一套最终上下文表达策略

## 出站也应遵循类似分层

入站之外，出站也应做同样切分。

### core 负责

- 语义回复内容
- 结构化 reply payload
- 是否需要引用上一条消息
- 是否需要工具结果、卡片、媒体等语义对象

### adapter 负责

- 本渠道如何发送 text / media / location / card
- 本渠道如何表示 reply threading
- 本渠道有哪些长度限制、格式限制、重试策略

也就是说：

- core 产出“发什么”
- adapter 决定“怎么按本渠道协议发出去”

## 对 OpenClaw 当前架构的对比

OpenClaw 当前已经明显朝这个方向走了一步，但还没有完全走到你想要的最终形态。

### 1. OpenClaw 确实有 channel adapter 概念

这一点是明确成立的。

- channel 可通过 `registerChannel(...)` 注册
- 实际 channel plugin / extension 负责各自渠道接入

所以在“是否存在渠道适配层”这个问题上，OpenClaw 的答案是：

- 有，而且是显式存在的

### 2. OpenClaw 也已经区分了结构化入站上下文与上下文编排

这一点也成立。

OpenClaw 的 adapter 不是只丢一段字符串给 core，它会先构造结构化上下文对象：

- `FinalizedMsgContext`
- `finalizeInboundContext(...)` 会统一补齐 / 规范化字段

这些字段里已经包含：

- `BodyForAgent`
- `BodyForCommands`
- `InboundHistory`
- `SessionKey`
- `ConversationLabel`
- `GroupSystemPrompt`
- media / reply / thread / sender 等结构化字段

与此同时，OpenClaw 还显式定义了上下文引擎：

- 上下文引擎负责 ingest / assembly / compaction
- 接口中显式区分 `bootstrap` / `ingest` / `assemble` / `compact` / `afterTurn`

而 core run pipeline 也会显式调用：

- `bootstrap(...)`
- `assemble(...)`
- `afterTurn(...)` / `ingest(...)`

所以在“是否有结构化输入与 context assemble/render 区分”这个问题上，OpenClaw 的答案是：

- 有，而且已经是显式机制

### 3. 但 OpenClaw 当前仍保留了部分 adapter-shaped prompt 痕迹

这一点也要看清。

虽然 OpenClaw 已经有结构化上下文和上下文引擎，但 adapter 今天仍会构造一些面向模型的字段，例如：

- `BodyForAgent`
- `InboundHistory`
- 各种 provider-specific 的上下文拼装

例如 Telegram adapter 在构造 `ctxPayload` 时就会直接填这些字段：

- 这说明 adapter 仍在参与一部分面向模型的输入整理

core 随后消费这些字段：

- `dispatchReplyFromConfig(...)` 直接吃 `FinalizedMsgContext`
- session prompt 会选择 `BodyForAgent` / `BodyForCommands` 等字段
- reply run 再结合 group-specific meta 生成运行期上下文

另外，OpenClaw runtime 自己也写明了一个方向性判断：

- 更推荐 `BodyForAgent + structured user-context blocks`
- 不再推荐 plaintext inbound envelope

这说明 OpenClaw 也意识到了：

- 纯 plaintext envelope 不是最终方向

但它当前仍未完全收敛到：

- adapter 只产出纯结构化事实
- core 全权 render 最终模型上下文

### 4. 因此，OpenClaw 当前状态更准确的评价是

- 已经具备 channel adapter
- 已经具备结构化 inbound context
- 已经具备上下文引擎，负责 ingest / assemble / compact 编排
- 但 adapter 与 core 在面向模型的输入上仍有部分混写

也就是说，它更接近：

- **半解耦**

而不是：

- **彻底纯解耦**

## 与 OpenClaw 的收敛关系

对第三轮 Moltis 而言，更合理的目标不是照搬 OpenClaw 当前细节，而是：

- 继承它已经走对的方向
- 再把边界继续收紧

可以把这件事概括成：

| 主题 | OpenClaw 当前状态 | 第三轮建议目标 |
| --- | --- | --- |
| channel adapter | 已存在且显式 | 保留 |
| 结构化 inbound context | 已存在 | 保留并进一步收紧成统一事件输入 |
| 上下文引擎 | 已存在 | 作为 core 内部上下文管理能力保留，并更明确只面向统一事件工作 |
| adapter prompt shaping | 仍然存在一部分 | 继续下沉为 normalization / hints，不主导最终 render |
| plaintext transcript 作为主事实来源 | 已弱化但未完全退出 | 彻底退出统一字段层 |

## 最后收口

如果只记住一句话，就记住这句：

- **第三轮的正确方向不是“每个渠道各自定义会话 transcript”**
- **而是“adapter 先整理事实，core 再统一保存事件，并由 core 统一负责完整的上下文管理”**

## 相关文档

- `docs/src/refactor/session-scope-overview.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`

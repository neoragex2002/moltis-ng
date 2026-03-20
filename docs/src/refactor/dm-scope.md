# `dm_scope`：`dm` 类型的分桶语义

本文档只定义一件事：

- 当 `type = dm` 时，系统如何判断两条 1v1 对话消息应该落进同一个逻辑会话桶

本文档**不**讨论：

- 群聊、频道、thread、topic 如何分桶
- cron、subagent、hook 等非 `dm` 类型如何分桶
- Telegram、Feishu 等渠道内部到底使用哪些原生字段
- `session_key` 后半截具体长什么样

也就是说，本文档讨论的是：

- `dm` 类型的**分桶语义**

而不是：

- 具体渠道的**分桶实现细节**

## 一句话结论

`dm_scope` 是上层核心对 `dm` 类型分桶语义的定义。

它只规定：

- 哪些 `dm` 消息应当同桶
- 哪些 `dm` 消息必须异桶

它不规定：

- 渠道适配模块内部如何用本地字段实现这个判定

因此：

- 上层负责定义语义
- 渠道适配模块负责实现语义
- `session_key` 只是最终编码结果

## 前提与边界

本文档始终在以下前提下讨论：

- 固定单个 `agent`
- 固定 `type = dm`
- 讨论对象是 agent 与单个外部参与者之间的 1 对 1 会话
- `session_key` 是逻辑会话桶名
- `session_id` 是该逻辑桶当前指向的 transcript / 持久会话代次

这里故意不把 Telegram、Feishu、Slack 等渠道内部对象直接拉进核心概念层。

## 核心概念

### `agent`

处理这段对话的逻辑 agent。

它是最大的思考与隔离边界。

在本文档里，所有规则默认都带着一句隐含前提：

- “对这个固定的 `agent` 来说”

### `peer`

与这个 `agent` 进行 1 对 1 `dm` 对话的单个外部参与者。

这里强调的是：

- `peer` 是上层逻辑概念
- 它不是某个具体渠道里的原生 id 字段

也就是说：

- Telegram 上怎么定位这个 `peer`
- Feishu 上怎么定位这个 `peer`

属于下层实现问题，不属于本文档定义范围。

### `channel`

当前这条 `dm` 消息来自哪个渠道。

例如：

- Telegram
- Feishu

`channel` 是上层已知语义输入之一，但它不等于渠道内部的原生分桶字段。

### `account`

某个对象在某个 `channel` 上的账号表示。

这里只保留 `account` 这个核心概念，不在本文档继续展开更多相关概念。

对 `dm` 分桶来说，更重要的结论是：

- `account` 的具体识别、解析、比较，属于渠道适配模块内部职责

上层不需要知道 Telegram / Feishu 到底如何确定“当前是哪一个本地 account”。

### `session_key`

逻辑会话桶的稳定名字。

它回答的是：

- 这条消息应该落进哪个逻辑桶

本文档不把 `session_key` 当成核心语义来源。

更准确地说：

- `session_key` 是分桶语义被实现并编码后的结果

### `session_id`

`session_key` 当前指向的那一代持久 transcript / 会话正文。

因此必须区分：

- `session_key`：逻辑桶是谁
- `session_id`：这个逻辑桶当前挂着哪一代正文

## 三层分工

为了避免概念膨胀、避免渠道细节泄漏到上层，`dm` 分桶应明确拆成三层。

### 第一层：语义层

这一层只定义上层语义输入。

对 `dm`，上层核心只关心：

- 是哪个 `agent`
- 对端是谁，也就是哪个 `peer`
- 当前消息来自哪个 `channel`
- 当前会话类型是 `dm`
- 当前分桶要求是什么

这一层**不**关心：

- Telegram 用的是 user id、chat id 还是别的字段
- Feishu 用的是 open id、chat id 还是别的字段
- 当前本地 account 到底是怎么从渠道事件里解析出来的

### 第二层：适配实现层

这一层由具体 `dm` 渠道适配模块负责。

它接收上层给出的分桶要求，然后自己处理本渠道细节，例如：

- 我这里哪个原生字段能唯一确定这个 `peer`
- 我这里哪个原生字段能唯一确定当前本地 `account`
- 我这里是否还有额外本地边界细节需要纳入分桶

然后，它返回一个稳定的 `dm` 分桶结果，也就是：

- `dm_subkey`

这里的 `dm_subkey` 是实现接口概念，不是新的上层核心概念。

术语对齐说明：

- 在实现代码与运行时表里，这个“黑盒分桶结果”也常被称为 `bucket_key`
- `bucket_key` 可以理解为 `dm_subkey` 的具体编码值；上层同样只应比较相等/不相等，不解析其内部结构

上层只把它当作：

- 一个黑盒结果

也就是：

- 可以比较相等/不相等
- 可以持久化
- 可以用于拼接 `session_key`
- 但不应依赖其内部结构

### 第三层：编码装配层

这一层只做统一装配。

例如可以把最终结果表达成：

```text
session_key = <agent-prefix> + :dm: + <dm_subkey>
```

其中：

- `<agent-prefix>` 由上层统一决定
- `<dm_subkey>` 由对应渠道适配模块返回

重点在于：

- `session_key` 只是编码结果
- 真正需要先冻结的是“同桶/异桶语义”

## `dm_scope` 的输入与输出

对 `dm` 分桶，上层核心向渠道适配模块发出的，其实不是“请你按某种字符串模板拼 key”，而是一个**分桶请求**。

这个请求至少包含：

- 固定的 `agent`
- 固定的 `type = dm`
- 当前逻辑 `peer`
- 当前 `channel`
- 当前 `dm_scope`

渠道适配模块收到这个请求后，返回：

- 满足该语义约束的 `dm_subkey`

## `dm_scope` 的四种取值

当前文档冻结四种 `dm` 分桶语义：

1. `main`
2. `per_peer`
3. `per_channel`
4. `per_account`

这是当前 1v1 `dm` 场景下最小、最收敛的一组语义。

这四种取值与 OpenClaw 现有 DM 分桶语义一一对应：

- `main` 对应 OpenClaw 的 `main`
- `per_peer` 对应 OpenClaw 的 `per-peer`
- `per_channel` 对应 OpenClaw 的 `per-channel-peer`
- `per_account` 对应 OpenClaw 的 `per-account-channel-peer`

这里的差异只在命名与分层表达，不在语义本身：

- OpenClaw 把 `channel` / `peer` / `account` 更显式地展开进模式名
- 本文档把这些维度收敛进核心概念定义与适配层职责里
- 因此两边的同桶/异桶判定保持等价，但本文档的命名更短、信息隐藏边界更清晰

### `main`

含义：

- 对固定 `agent` 来说，所有 `dm` 都进入同一个逻辑桶

这里不区分：

- `peer`
- `channel`
- 当前本地 `account`

因此它表达的是：

- `dm` 主线完全塌缩

对渠道适配模块的行为要求是：

- 只要分桶请求是 `main`，它返回的 `dm_subkey` 必须对所有 `dm` 输入保持相同

### `per_peer`

含义：

- 对固定 `agent` 来说，同一个逻辑 `peer` 的 `dm` 进入同一个逻辑桶

这里不区分：

- `channel`
- 当前本地 `account`

这里区分：

- `peer` 是否相同

因此它表达的是：

- `dm` 按逻辑对端分桶

对渠道适配模块的行为要求是：

- 如果两条 `dm` 消息对应同一个逻辑 `peer`，返回的 `dm_subkey` 必须相同
- 如果对应不同逻辑 `peer`，返回的 `dm_subkey` 必须不同

这里特别要注意：

- `peer` 是否相同，是上层语义输入
- 适配模块负责实现，不负责改写这条语义

### `per_channel`

含义：

- 对固定 `agent` 来说，同一个逻辑 `peer` 的 `dm`，若 `channel` 不同，则必须分桶

这里区分：

- `peer` 是否相同
- `channel` 是否相同

这里不继续区分：

- 同一 `channel` 下的当前本地 `account`

因此它表达的是：

- `dm` 按“逻辑对端 + 渠道”分桶

它与 OpenClaw 里的 `per-channel-peer` 是同一层语义。

对渠道适配模块的行为要求是：

- 如果两条 `dm` 消息对应同一个逻辑 `peer`，且 `channel` 也相同，返回的 `dm_subkey` 必须相同
- 如果 `channel` 不同，则返回的 `dm_subkey` 必须不同

这里要注意：

- 在 `dm` 语义下，`per_channel` 不是“只按 channel 分桶”
- 它的真实含义是“同一个 `peer` 在不同 `channel` 上不共桶”
- 也就是：`peer + channel`

### `per_account`

含义：

- 对固定 `agent` 来说，同一个逻辑 `peer` 的 `dm`，若当前本地 `account` 不同，则必须分桶

这里区分：

- `peer` 是否相同
- `channel` 是否相同
- 当前本地 `account` 是否相同

因此它表达的是：

- `dm` 按“逻辑对端 + 渠道内当前本地接入账号”分桶

它与 OpenClaw 里的 `per-account-channel-peer` 是同一层语义，只是这里把 `channel` 内含进 `account` 的定义里，不再单独展开进模式名。

对渠道适配模块的行为要求是：

- 如果两条 `dm` 消息对应同一个逻辑 `peer`，且当前本地 `account` 也相同，返回的 `dm_subkey` 必须相同
- 如果当前本地 `account` 不同，则返回的 `dm_subkey` 必须不同

这里仍然要注意：

- `account` 是某个对象在某个 `channel` 上的账号表示，因此 `per_account` 天然包含 `channel` 边界
- 上层不需要知道“当前本地 `account`”在 Telegram / Feishu 里到底是怎么解析出来的
- 这正是适配模块的信息隐藏边界

## 四种语义的等价关系

对固定 `agent`、固定 `type = dm`，两条消息是否同桶，可以直接写成：

### `main`

同桶当且仅当：

- 它们都属于这个 `agent` 的 `dm` 主线

### `per_peer`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑 `peer`

### `per_channel`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑 `peer`
- 它们来自同一个 `channel`

### `per_account`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑 `peer`
- 它们对应同一个当前本地 `account`

由于 `account` 是 channel-scoped，这也隐含要求：

- 它们位于同一个 `channel`

## 适配模块的黑盒契约

既然 `dm_subkey` 由渠道适配模块负责生成，那就必须明确黑盒契约。

渠道适配模块至少必须满足：

### 1. 语义正确

它返回的 `dm_subkey` 必须满足上层定义的 `dm_scope` 语义。

也就是：

- 该同桶时必须同桶
- 该异桶时必须异桶

### 2. 稳定

在相同语义输入和相同本地事实下，它必须返回相同结果。

否则：

- 同一条会话线会漂移到不同 `session_key`

### 3. 局部封装

它可以自由决定内部使用哪些渠道字段，但这些字段不应泄漏为上层核心概念。

换句话说：

- 上层不应依赖 Telegram/Feishu 专有字段来解释 `dm` 分桶语义

### 4. Opaque

上层可以使用 `dm_subkey`，但不应要求解析它的内部结构。

因此，上层对 `dm_subkey` 的依赖应限制在：

- 比较
- 拼接
- 存储

而不应包括：

- 反向解释其内部渠道细节

## 例子

下面的例子只说明语义，不说明某个具体渠道到底怎么拼内部字段。

### `main`

上层请求：

- `agent = alma`
- `type = dm`
- `dm_scope = main`

则：

- Telegram 上的 Alice
- Feishu 上的 Bob
- Telegram 上的 Carol

都应进入同一个 `session_key`

### `per_peer`

上层请求：

- `agent = alma`
- `type = dm`
- `dm_scope = per_peer`

则：

- 同一个逻辑 `peer`，无论从 Telegram 还是 Feishu 发来，只要上层判定仍是同一个 `peer`，就应进入同一个 `session_key`
- 不同逻辑 `peer` 必须分桶

### `per_channel`

上层请求：

- `agent = alma`
- `type = dm`
- `dm_scope = per_channel`

则：

- 同一个逻辑 `peer`，若分别从 Telegram 和 Feishu 发来，应进入不同 `session_key`
- 但同一个逻辑 `peer` 在同一个 `channel` 上，仍应进入同一个 `session_key`

### `per_account`

上层请求：

- `agent = alma`
- `type = dm`
- `dm_scope = per_account`

则：

- 同一个逻辑 `peer`，如果它是通过同一 `channel` 下不同本地接入 `account` 与这个 `agent` 发生 `dm`，就应进入不同 `session_key`

这里的“不同本地接入 `account`”具体意味着什么，由对应渠道适配模块负责解释和实现。

## 对 OpenClaw 当前架构的对比

OpenClaw 在 `dm` 语义上，与本文档是一一对应的：

- `main`
- `per-peer`
- `per-channel-peer`
- `per-account-channel-peer`

也就是说：

- 在 `dm` 的同桶/异桶判定上，两边语义是等价的

但在架构分层上，OpenClaw 当前更接近：

- adapter 先构造结构化 inbound context
- core 再消费这些结构化字段并交给上下文引擎做 assemble / compact

这说明它已经明显区分了：

- 渠道接入层
- 结构化入站上下文
- 会话上下文编排层

但它当前仍保留了一部分 adapter-shaped prompt 痕迹，例如：

- adapter 仍会填写 `BodyForAgent`
- adapter 仍会填写 `InboundHistory`

所以更准确地说，OpenClaw 当前在 `dm` 上是：

- **语义已对齐**
- **分层已部分建立**
- **最终 prompt render 仍未完全 core-owned**

而本文档对应的第三轮目标是：

- 保持与 OpenClaw 等价的 `dm` 分桶语义
- 继续把最终上下文 render 收回 core / `type` renderer
- 让 adapter 更专注于 `peer` / `channel` / `account` 的归一化与 `dm_subkey` 实现

更完整的上下文分层讨论，见：

- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

## 最后收敛

如果只记住一句话，就记住这句：

- `dm_scope` 定义的是 `dm` 类型的同桶/异桶语义
- 渠道适配模块负责把这个语义实现成一个稳定的 `dm_subkey`
- `session_key` 只是把这个结果编码出来
- `session_id` 则是该逻辑桶当前指向的那一代 transcript

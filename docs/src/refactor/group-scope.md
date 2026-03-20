# `group_scope`：`group` 类型的分桶语义

本文档只定义一件事：

- 当 `type = group` 时，系统如何判断两条群消息应该落进同一个逻辑会话桶

本文档**不**讨论：

- `dm` 如何分桶
- `channel` 类型如何分桶
- cron、subagent、hook 等非群聊类型如何分桶
- Telegram、Feishu、Slack 等渠道内部到底用哪些原生字段
- `session_key` 后半截具体长什么样

也就是说，本文档讨论的是：

- `group` 类型的**分桶语义**

而不是：

- 具体渠道的**分桶实现细节**

## 一句话结论

`group_scope` 是上层核心对 `group` 类型分桶语义的定义。

它只规定：

- 哪些群消息应当同桶
- 哪些群消息必须异桶

它不规定：

- 渠道适配模块内部如何用本地字段实现这个判定

因此：

- 上层负责定义语义
- 渠道适配模块负责实现语义
- `session_key` 只是最终编码结果

## 前提与边界

本文档始终在以下前提下讨论：

- 固定单个 `agent`
- 固定 `type = group`
- 讨论对象是 agent 与多个外部参与者之间的共享会话
- `session_key` 是逻辑会话桶名
- `session_id` 是该逻辑桶当前指向的 transcript / 持久会话代次

## 核心概念

### `agent`

处理这段群上下文的逻辑 agent。

它是最大的思考与隔离边界。

### `peer`

当前这段 `group` 会话对应的多人共享会话对象。

例如：

- Telegram 群
- Feishu 群
- Slack channel
- Discord channel

这里强调的是：

- `peer` 是上层逻辑概念
- 它不是某个具体渠道里的原生 id 字段

### `branch`

adapter 为 `per_branch` 一类语义返回的黑盒子线标识。

它常见地承接不同渠道里的：

- `topic`
- `thread`
- forum topic
- reply-root 形成的子线

这里强调的是：

- 上层只关心“是否存在应参与分桶的独立子线”
- 它是 `group_scope` 局部需要的语义槽位，不是 core 的强公共概念
- core 不需要深度理解它的内部对象模型
- 至于某个渠道到底叫 `topic` 还是 `thread`，属于适配层职责

### `sender`

群内当前消息的逻辑发言人。

这里同样强调：

- `sender` 是上层逻辑概念
- 它不是某个具体渠道里的原生字段名

### `session_key`

逻辑会话桶的稳定名字。

它回答的是：

- 这条群消息应该落进哪个逻辑桶

### `session_id`

`session_key` 当前指向的那一代持久 transcript / 会话正文。

因此必须区分：

- `session_key`：逻辑桶是谁
- `session_id`：这个逻辑桶当前挂着哪一代正文

## 三层分工

为了避免概念膨胀、避免渠道细节泄漏到上层，`group` 分桶应明确拆成三层。

### 第一层：语义层

对 `group`，上层核心只关心：

- 是哪个 `agent`
- 当前消息属于哪个多人共享会话对象 `peer`
- 当前消息是否命中了某个应参与分桶的子线判别结果（也就是 `branch`）
- 当前消息的 `sender` 是谁
- 当前会话类型是 `group`
- 当前分桶要求是什么

这一层**不**关心：

- Telegram 用什么字段识别 forum topic
- Slack / Discord 用什么字段识别 thread
- Feishu 用什么字段识别 sender

### 第二层：适配实现层

这一层由具体群聊渠道适配模块负责。

它接收上层给出的分桶要求，然后自己处理本渠道细节，例如：

- 我这里哪个原生字段能唯一确定这个共享会话对象 `peer`
- 我这里哪个原生字段能稳定地产生这个 `branch`
- 我这里哪个原生字段能唯一确定这个 `sender`
- 我这里是否还有额外本地边界细节需要纳入分桶

然后，它返回一个稳定的 `group` 分桶结果，也就是：

- `group_subkey`

这里的 `group_subkey` 是实现接口概念，不是新的上层核心概念。

术语对齐说明：

- 在实现代码与运行时表里，这个“黑盒分桶结果”也常被称为 `bucket_key`
- `bucket_key` 可以理解为 `group_subkey` 的具体编码值；上层同样只应比较相等/不相等，不解析其内部结构

### 第三层：编码装配层

这一层只做统一装配。

例如可以把最终结果表达成：

```text
session_key = <agent-prefix> + :group: + <group_subkey>
```

其中：

- `<agent-prefix>` 由上层统一决定
- `<group_subkey>` 由对应渠道适配模块返回

重点在于：

- `session_key` 只是编码结果
- 真正需要先冻结的是“同桶/异桶语义”

## `group_scope` 的输入与输出

对 `group` 分桶，上层核心向渠道适配模块发出的，其实不是“请你按某种字符串模板拼 key”，而是一个**分桶请求**。

这个请求至少包含：

- 固定的 `agent`
- 固定的 `type = group`
- 当前逻辑 `peer`
- 当前 `group_scope`
- 若命中子线语义则包含 `branch`
- 当前 `sender`

渠道适配模块收到这个请求后，返回：

- 满足该语义约束的 `group_subkey`

## `group_scope` 的四种取值

当前文档冻结四种 `group` 分桶语义：

1. `group`
2. `per_sender`
3. `per_branch`
4. `per_branch_sender`

这四种取值与 OpenClaw 当前 group 侧语义一一对应：

- `group` 对应 OpenClaw 的 `group`
- `per_sender` 对应 OpenClaw 的 `group_sender`
- `per_branch` 对应 OpenClaw 的 `group_topic`
- `per_branch_sender` 对应 OpenClaw 的 `group_topic_sender`

差异只在命名与抽象层次，不在语义本身：

- OpenClaw 目前在 group 侧主要以 `topic` 命名
- 本文档把 `topic` / `thread` / 类似子线统一抽象成 `branch`
- 但这里的 `branch` 更适合被理解成 adapter 返回的黑盒判别结果，而不是强 core 对象模型

### `group`

含义：

- 对固定 `agent` 来说，同一个逻辑群 `peer` 的消息进入同一个逻辑桶

这里不区分：

- `branch`
- `sender`

因此它表达的是：

- 按共享会话对象分桶

### `per_sender`

含义：

- 对固定 `agent` 来说，同一个逻辑群 `peer` 内，不同 `sender` 必须分桶

这里区分：

- `peer` 是否相同
- `sender` 是否相同

这里不区分：

- `branch`

因此它表达的是：

- 按“共享会话对象 + 发言人”分桶

### `per_branch`

含义：

- 对固定 `agent` 来说，同一个逻辑群 `peer` 内，不同 `branch` 必须分桶

这里区分：

- `peer` 是否相同
- `branch` 是否相同

这里不区分：

- `sender`

因此它表达的是：

- 按“共享会话对象 + 子线判别结果”分桶

这里要注意：

- `branch` 是统一命名，不要求所有渠道都显式叫 `topic` 或 `thread`
- 它更像 adapter 返回的子线判别结果，而不是强 core 对象
- 若当前消息不属于任何独立 `branch`，则它退化回群根级语义

### `per_branch_sender`

含义：

- 对固定 `agent` 来说，同一个逻辑群 `peer` 内，不同 `branch` 或不同 `sender` 都必须分桶

这里区分：

- `peer` 是否相同
- `branch` 是否相同
- `sender` 是否相同

因此它表达的是：

- 按“共享会话对象 + 子线判别结果 + 发言人”分桶

这里要注意：

- 若当前消息不属于任何独立 `branch`，则该模式退化为“共享会话对象 + 发言人”

## 四种语义的等价关系

对固定 `agent`、固定 `type = group`，两条消息是否同桶，可以直接写成：

### `group`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑群 `peer`

### `per_sender`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑群 `peer`
- 它们属于同一个逻辑 `sender`

### `per_branch`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑群 `peer`
- 它们属于同一个逻辑 `branch`

### `per_branch_sender`

同桶当且仅当：

- 它们属于同一个 `agent`
- 它们属于同一个逻辑群 `peer`
- 它们属于同一个逻辑 `branch`
- 它们属于同一个逻辑 `sender`

## 适配模块的黑盒契约

渠道适配模块至少必须满足：

### 1. 语义正确

它返回的 `group_subkey` 必须满足上层定义的 `group_scope` 语义。

也就是：

- 该同桶时必须同桶
- 该异桶时必须异桶

### 2. 稳定

在相同语义输入和相同本地事实下，它必须返回相同结果。

否则：

- 同一条会话线会漂移到不同 `session_key`

### 3. 局部封装

它可以自由决定内部使用哪些渠道字段，但这些字段不应泄漏为上层核心概念。

### 4. Opaque

上层可以使用 `group_subkey`，但不应要求解析它的内部结构。

## OpenClaw 的 thread / topic 做法

OpenClaw 当前并没有再单独定义一套通用的 `thread_scope` 或 `topic_scope` 枚举。

它的做法更接近两类：

- 对 DM：用 `dm_scope` 控制 DM 是否按人、渠道、账号拆桶
- 对 group：在 group 侧通过 `group` / `group_sender` / `group_topic` / `group_topic_sender` 表达是否把 `topic` 纳入分桶

而 `thread` / `topic` 本身更多体现在：

- 某些渠道把它们作为父容器之下的附加分桶维度写进 key
- 某些渠道直接把 thread 自己当作独立 peer / conversation

所以从上层抽象看，更稳妥的做法不是再发明：

- `thread_scope`
- `topic_scope`

而是把它们统一折叠进 `branch` 这个命名槽位，由适配层决定本渠道如何落地。

## 对 OpenClaw 当前架构的对比

OpenClaw 在 group 侧当前暴露的语义，和本文档是一一对应的：

- `group`
- `group_sender`
- `group_topic`
- `group_topic_sender`

也就是说：

- 在 group 的同桶/异桶判定上，两边语义是等价的

本文档与它的主要差异，不在语义，而在抽象收敛：

- OpenClaw 目前更偏 `topic` 命名
- 本文档把 `topic` / `thread` / 类似子线统一抽象成 `branch`
- 同时明确 `branch` 更接近 adapter 提供的子线判别结果

从架构分层看，OpenClaw 当前也已经明显分出了：

- channel adapter
- 结构化 inbound context
- 上下文引擎

但它今天的 group adapter 仍会构造一部分面向模型的字段，例如：

- `BodyForAgent`
- `InboundHistory`
- `ConversationLabel`
- `GroupSystemPrompt`

因此更准确地说，OpenClaw 当前在 group 上是：

- **语义已对齐**
- **结构化入站上下文已存在**
- **最终给模型的文本仍未完全由 core 接管**

而本文档对应的第三轮目标是：

- 保持与 OpenClaw 等价的 group 分桶语义
- 把 `topic` / `thread` 统一收敛为 `branch`
- 继续把最终上下文 render 收回 core / `type` renderer
- 让 adapter 更专注于 `peer` / `branch` / `sender` 的归一化与 `group_subkey` 实现

更完整的上下文分层讨论，见：

- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

## 最后收敛

如果只记住一句话，就记住这句：

- `group_scope` 定义的是 `group` 类型的同桶/异桶语义
- 它只关心共享会话对象、子线判别结果、发言人三件事
- 渠道适配模块负责把这个语义实现成稳定的 `group_subkey`
- `session_key` 只是把这个结果编码出来

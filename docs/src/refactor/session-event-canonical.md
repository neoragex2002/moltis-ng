# `session_event` 字段表

本文档只定义一件事：

- 第三轮目标形态下，`session_event` 的统一持久化字段应如何收敛

本文档不讨论：

- `session_key` 的具体拼接规则
- `dm_scope` / `group_scope` 的具体取值
- 某个具体渠道的原生 webhook / SDK 字段
- 某次 agent run 里最终给模型看的 prompt 文本长什么样

也就是说，本文档讨论的是：

- **保存层的统一事件模型**

而不是：

- 某个 adapter 今天临时怎么拼上下文

## 一句话结论

`session_event` 应当是：

- 统一的
- 结构化的
- 可重放的
- 与最终 prompt 文本解耦的

因此：

- 它必须保存稳定事实
- 但不应把 `BodyForAgent`、`InboundHistory`、plaintext envelope 这类给模型看的临时文本直接当作统一字段

这里还要明确区分：

- `session_event` 的字段表是保存层字段表
- 它不等于 core 的最小公共概念表
- 保存层为了可重放与可审计，可以比 core 公共概念更丰富

## 与 C 阶段的关系

本文档描述的是：

- **最终持久化目标**

它不是：

- **C 阶段当前实施的前置条件**

也就是说：

- C 阶段可以先不改落盘
- C 阶段可以继续复用 `SessionStore` / `PersistedMessage`
- C 阶段的第一优先级仍是 Telegram adapter / core 边界收敛，以及 context ownership 收敛

当前更准确的理解是：

- `session_event` 是后续替换 legacy persistence bridge 的目标形态
- 而不是 C 阶段必须同轮落地的内容

## 设计原则

### 1. 保存层是唯一事实来源

`session_event` 保存的是：

- 发生了什么事实

而不是：

- 这次准备怎么向模型讲述这些事实

### 2. 上下文是按需整理出来的

core 的上下文管理应从 `session_event` 整理出：

- `dm` 上下文
- `group` 上下文

而不是反过来让保存层依赖某一版上下文文本。

### 3. 渠道字段只保留必要统一字段，其余放补充信息

凡是会被多处核心逻辑稳定依赖的字段，才应进入统一字段层。

其余渠道细节：

- 放进 `adapter_hints`

### 4. 顶层字段数量要收敛

顶层只保留核心字段。

更细的内容应进入受控嵌套对象，而不是不断平铺新字段。

## 顶层字段表

| 字段 | 必填 | owner | 含义 |
| --- | --- | --- | --- |
| `event_id` | 是 | core | 当前 `session_event` 的稳定唯一标识 |
| `session_key` | 是 | core | 该事件所属的逻辑会话桶 |
| `session_id` | 是 | core | 该事件写入时对应的会话代次 |
| `type` | 是 | core | 当前事件所属会话类型，如 `dm`、`group` |
| `direction` | 是 | core | 事件方向，如 `inbound`、`outbound`、`system` |
| `event_kind` | 是 | core | 事件种类，如普通消息、系统事件、工具事件 |
| `ts` | 是 | core | 该事件的规范时间戳 |
| `channel` | 否 | adapter -> core | 事件来源渠道；用于说明来源，不等于一定参与分桶 |
| `account` | 否 | adapter -> core | 当前渠道账号表示；channel-scoped |
| `peer` | 视 `type` 而定 | adapter -> core | 逻辑对端；在 `group` 下表示多人共享会话对象 |
| `branch` | 否 | adapter -> core | adapter 返回的子线判别结果，承接 `topic` / `thread` 等 |
| `sender` | 否 | adapter -> core | 逻辑发言人 |
| `body` | 是 | adapter -> core | 该事件的统一文本正文；无文本时可为空字符串 |
| `attachments` | 否 | adapter -> core | 规范化附件列表 |
| `reply_ref` | 否 | adapter -> core | 该事件引用 / 回复的上游对象 |
| `source` | 是 | adapter -> core | 原生来源与采集信息 |
| `adapter_hints` | 否 | adapter | 不上升为核心语义的渠道局部信息 |

这些字段里，像 `channel`、`account`、`branch`、`sender` 是否出现，首先取决于保存这一类事实是否有价值；
这并不表示它们都应被抬成 core 长期冻结的强公共概念。

## 推荐的顶层枚举约束

### `type`

当前至少应支持：

- `dm`
- `group`
- `cron`
- `heartbeat`

未来可扩展其他类型，但不应让不同类型共用一套含混不清的语义字段。

### `direction`

当前建议冻结：

- `inbound`
- `outbound`
- `system`

含义分别是：

- `inbound`：来自外部用户 / 外部系统进入当前会话
- `outbound`：当前 agent / 系统向外发出的回复或动作结果
- `system`：不属于正常入站/出站消息，但应进入会话事实流的系统事件

### `event_kind`

当前建议先收敛为少量稳定值：

- `message`
- `control`
- `tool`
- `notice`

要求是：

- `event_kind` 只表达事件类别
- 不把渠道细节塞进枚举名

## 引用对象

以下几个字段建议统一使用相同风格的对象，而不是随处散落裸字符串。

### `account`

```text
account = {
  id,
  label?
}
```

约束：

- `id` 是稳定黑盒 id
- `label` 仅用于展示，不参与语义判定

### `peer`

```text
peer = {
  id,
  label?
}
```

约束：

- `id` 是逻辑对端 id；在 `group` 下可表示多人共享会话对象
- 不要求它等于某个渠道原生字段

### `branch`

```text
branch = {
  id,
  label?
}
```

约束：

- `branch` 是统一命名槽位
- 它更接近 adapter 返回的黑盒子线标识
- 不要求区分 `topic` / `thread`
- 是否来自 topic、thread、reply-root，可放入 `adapter_hints`

### `sender`

```text
sender = {
  id,
  label?
}
```

约束：

- `id` 是逻辑发言人标识
- `label` 仅用于展示

## `attachments` 结构

建议 `attachments` 为数组，每项至少包含：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `id` | 是 | 当前附件的稳定标识 |
| `kind` | 是 | 附件大类，如 `image`、`audio`、`video`、`file`、`location` |
| `uri` | 是 | 当前附件的规范访问位置或存储位置 |
| `mime_type` | 否 | MIME 类型 |
| `name` | 否 | 原始文件名或展示名 |
| `size_bytes` | 否 | 大小 |

要求：

- `attachments` 保存的是统一附件事实
- 不在这里提前塞入“给模型看的媒体提示语”

## `reply_ref` 结构

建议 `reply_ref` 至少包含：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `event_id` | 否 | 若已解析到本地统一事件，则指向对应 `event_id` |
| `source_message_id` | 否 | 原生渠道里的被回复消息 id |
| `snippet` | 否 | 引用片段的短文本快照 |

要求：

- `reply_ref` 的目标是保存“引用关系”
- 不是把整段 reply envelope 提前拼好
- 也不是 adapter 出站时用于回投消息的私有投递路径

后一类“怎么把回复发回原渠道”的投递信息，应留在 adapter 自己的回复私有对象里，例如 Telegram 的 `tg_reply.private_target`，而不是写进统一保存字段。

## `source` 结构

`source` 用于保存原生来源信息。

建议至少包含：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `adapter` | 是 | 该事件来自哪个 adapter |
| `message_id` | 否 | 原生 provider message id |
| `conversation_id` | 否 | 原生 provider conversation/chat id |
| `thread_id` | 否 | 原生 provider thread/topic id |
| `ingest_mode` | 否 | 采集方式，如正常消息、listen-only、relay 等 |

要求：

- `source` 解决来源追踪和排障问题
- 不应被误当作上层会话语义定义

## `adapter_hints` 的边界

`adapter_hints` 允许存在，但必须受约束。

可以放进去的内容：

- 某渠道 reply threading 的本地额外信息
- 某渠道 topic/forum 的局部标记
- 某渠道必须保留但暂时没必要上升为统一字段的元数据

不该放进去的内容：

- 已被核心层长期依赖的稳定语义字段
- 最终给模型的文本
- 会影响同桶/异桶判定、但又没有升为统一字段的关键字段

一句话：

- `adapter_hints` 是缓冲区，不是语义垃圾桶

## `dm` / `group` 的必填矩阵

| 字段 | `dm` | `group` | 说明 |
| --- | --- | --- | --- |
| `peer` | 必填 | 必填 | `dm` 是逻辑对端；`group` 是多人共享会话对象 |
| `channel` | 建议必填 | 建议必填 | 用于说明来源，也常参与下游策略 |
| `account` | 条件必填 | 条件必填 | 多账号渠道下应明确记录 |
| `branch` | 通常为空 | 条件必填 | `group` 在 topic/thread/子线场景下应有值 |
| `sender` | 通常为空 | 入站建议必填 | `group` 入站消息通常需要标识发言人 |

这里要注意：

- `dm` 中通常不需要 `sender`
- `group` 中通常不需要把 `account` 当作主分桶轴，但仍可能需要保留其来源信息

## 哪些字段不应进入统一字段层

以下内容不应直接成为统一持久化字段：

- `BodyForAgent`
- `BodyForCommands`
- `InboundHistory`
- `ConversationLabel`
- `GroupSystemPrompt`
- 任意 plaintext inbound envelope
- 任意“本轮给模型看的最终拼接文本”

原因很简单：

- 这些都是上下文整理阶段才会生成的内容
- 不是保存层事实本身

## `session_event` 与上下文管理的关系

关系应当固定为：

```text
adapter 原生事件
  -> 归一化
  -> session_event 统一持久化
  -> core 上下文管理
  -> 最终模型上下文
```

也就是说：

- `session_event` 位于保存层
- 这些都属于 core 的上下文管理内部职责

它们不能倒置。

## 对 OpenClaw 当前架构的对比

OpenClaw 当前已经明显有：

- channel adapter
- 结构化 inbound context
- 上下文管理

但它当前更接近：

- 用 `FinalizedMsgContext` 作为结构化入站上下文对象
- 由 adapter 仍提供一部分面向模型的字段
- core 再继续做 assemble / compact

这说明 OpenClaw 已经走向：

- 结构化输入
- context orchestration 分层

但它当前还没有完全收敛到：

- `session_event` 作为唯一统一保存对象
- 面向模型的字段完全退出统一字段层

因此，第三轮更合理的目标是：

- 继承 OpenClaw 已经建立的“结构化输入 + 上下文引擎”方向
- 再把保存层继续收紧到 `session_event`

## 最后收口

如果只记住一句话，就记住这句：

- **`session_event` 应只保存稳定事实**
- **最终上下文应由 core 的上下文管理根据这些事实整理出来**

## 相关文档

- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-scope-overview.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`

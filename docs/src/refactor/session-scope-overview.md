# 会话 scope 总表

本文档只做一件事：

- 把当前已经冻结的会话类型、scope 枚举和语义轴并排收拢成一张总表

它不展开具体渠道实现，也不讨论 `session_key` 的编码细节。

## 一句话结论

当前上层 scope 设计分成两类：

- `dm_scope`：解决 1v1 对话在“人 / 渠道 / 账号”维度上的分桶歧义
- `group_scope`：解决共享群会话在“共享会话对象 / 子线判别 / 发言人”维度上的分桶细化

两者关注的语义轴不同，不应混成一套枚举。

## 总表

| `type` | scope 值 | 同桶判定所看的语义轴 | 说明 |
| --- | --- | --- | --- |
| `dm` | `main` | `agent` | 所有 DM 塌缩到同一主线 |
| `dm` | `per_peer` | `agent + peer` | 同一逻辑对端跨渠道可共桶 |
| `dm` | `per_channel` | `agent + peer + channel` | 同一对端在不同渠道不共桶 |
| `dm` | `per_account` | `agent + peer + account` | 同一对端在不同接入账号不共桶；`account` 内含 `channel` |
| `group` | `group` | `agent + peer` | 同一共享群会话对象共桶 |
| `group` | `per_sender` | `agent + peer + sender` | 同群内按发言人拆桶 |
| `group` | `per_branch` | `agent + peer + branch` | 同群内按 adapter 识别出的子线拆桶 |
| `group` | `per_branch_sender` | `agent + peer + branch + sender` | 同群内按子线和发言人共同拆桶 |

## 语义轴解释

### `agent`

最大的思考与隔离边界。

### `peer`

逻辑对端。

在 `dm` 中，它表示 agent 面对的单个外部参与者。  
在 `group` 中，它表示 agent 面对的多人共享会话对象。

### `channel`

消息来自哪个渠道。

例如：

- Telegram
- Feishu
- Slack

### `account`

某个对象在某个 `channel` 上的账号表示。

因此：

- `account` 天然带有 `channel` 边界

### `branch`

由 adapter 识别出的群内子线判别结果。

- 它服务于 `per_branch` / `per_branch_sender` 语义
- 它是 group-scope 局部语义槽位，不是 core 的强公共概念
- 它本身不要求 core 深度理解其内部结构
- 它常见地承接：

- `topic`
- `thread`
- forum topic
- reply-root 形成的子线

### `sender`

群内消息的逻辑发言人。

## 为什么不能把两类 scope 混成一套

因为它们解决的是不同问题。

### `dm_scope` 解决什么

`dm_scope` 的核心问题是：

- 同一个人跨渠道要不要共桶
- 同一个人跨账号要不要共桶

所以它的语义轴天然集中在：

- `peer`
- `channel`
- `account`

### `group_scope` 解决什么

`group_scope` 的核心问题是：

- 同一群里要不要按子线拆
- 同一群里要不要按发言人拆

所以它的语义轴天然集中在：

- `peer`
- adapter 返回的子线结果
- `sender`

## 当前收敛原则

### 1. 先按 `type` 分开定义

不要试图发明一套覆盖所有类型的大而全 scope 枚举。

当前至少应保持：

- `dm_scope`
- `group_scope`

分别定义、分别收敛。

### 2. 上层只定义语义轴

上层只定义：

- 哪些维度参与同桶判定
- 哪些维度不参与同桶判定

上层不定义：

- 具体渠道用哪些原生字段完成判定

### 3. 渠道适配层负责黑盒落地

适配层负责回答：

- 这个 `peer` 在本渠道怎么识别
- 这个子线判别结果在本渠道怎么识别
- 这个 `account` 在本渠道怎么识别
- 这个 `sender` 在本渠道怎么识别

然后返回稳定的 subkey 给上层装配 `session_key`。

## 与 OpenClaw 的关系

`dm` 侧是一一对应的：

- `main` ↔ `main`
- `per_peer` ↔ `per-peer`
- `per_channel` ↔ `per-channel-peer`
- `per_account` ↔ `per-account-channel-peer`

`group` 侧也是一一对应的：

- `group` ↔ `group`
- `per_sender` ↔ `group_sender`
- `per_branch` ↔ `group_topic`
- `per_branch_sender` ↔ `group_topic_sender`

差异主要在命名收敛：

- 我们把 `topic` / `thread` / 类似子线统一抽象成 `branch`
- 并明确它更适合作为 adapter 落地时返回的子线结果
- 我们把较长的 OpenClaw 模式名收敛成更短的 snake_case 形式

## 相关文档

- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`

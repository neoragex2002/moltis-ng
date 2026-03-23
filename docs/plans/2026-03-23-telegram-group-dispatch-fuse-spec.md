# Telegram 群聊派发保险丝规范

**状态：** 已冻结，可进入实现评审

**范围：** 仅限 Telegram 适配层（`crates/telegram/*`）中的群聊 bot 之间自动派发扩散控制

**不在范围内：** 不恢复旧 relay-chain 整套机制；不新增 gateway/core 的 Telegram 群聊编排；不修改正文透传规则（该内容见独立规范）

---

## 1. 目标

为 Telegram 群聊中的 bot 协作补上一根共享的派发保险丝，防止单个协作循环无限扩散或多路分叉失控。

这根保险丝只负责限制扩散总量，不负责目标识别、正文改写、会话路由或群聊转写语义。

---

## 2. 配置

冻结后的配置形态：

```toml
[channels.telegram]
bot_dispatch_cycle_budget = 128
```

### 配置语义

- 作用域：Telegram 渠道级共享策略
- 不是按 bot 账号分别配置
- 由所有 `channels.telegram.<bot>` 账号共享
- 类型：`u32`
- 默认值：`128`
- `0` 为非法值，必须在配置校验阶段直接报错

### 配置落地约束

- `channels.telegram` 在当前配置结构里已经承担 Telegram bot 账号总表的语义
- `bot_dispatch_cycle_budget` 冻结为这个总表下的保留元字段，不是一个 bot 账号 ID
- 任意 Telegram bot 账号 ID 都不得命名为 `bot_dispatch_cycle_budget`；若发生重名，配置校验必须直接报错
- 账号枚举、账号加载、账号校验时，必须显式跳过 `bot_dispatch_cycle_budget`
- 除 `bot_dispatch_cycle_budget` 外，本次不在 `channels.telegram` 同层再扩写新的共享元字段

### 为什么必须是共享配置

这个预算约束的是一条跨 bot 协作链，不能正确建模为按账号各算各的配置。因为一条协作循环可能跨越多个 bot：

```text
A -> B -> C
B -> D
C -> E
E -> A
```

这条链上的所有成功 bot 到 bot 派发，都必须从同一个共享预算桶里扣减。

---

## 3. 问题定义

在 one-cut 删除旧 relay-chain 机制后，Telegram 群聊规划路径已经没有一根共享的硬保险丝，去限制同一个协作循环中 bot 到 bot 下游派发的总扩散量。

当前风险：

- `A -> B -> A -> C -> A -> ...` 这类链条，只要每一步都还满足派发条件，就可能继续跑下去。
- 一条 bot 消息如果同时唤醒多个 bot，会产生激进的多路扩散。
- 现有 dedupe 只能阻止同一事件被重复处理，不能充当总量保险丝。

---

## 4. 设计原则

1. 保险丝按 Telegram 群聊协作循环共享，而不是按单个 bot 账号计算。
2. 保险丝只限制 bot 到 bot 扩散，不限制人对 bot 的首次唤醒。
3. 保险丝命中后，只做 `Dispatch -> RecordOnly` 降级，不丢正文，不篡改 `addressed` 语义。
4. 任何由保险丝导致的降级，都必须有结构化可观测性，不能静默发生。
5. 保险丝实现必须继续收敛在 Telegram 适配层，不得把旧 Telegram 专属编排放回 gateway/core。

---

## 5. 协作循环定义

### `dispatch_cycle_id`

`dispatch_cycle_id` 表示一个 Telegram 群聊 bot 协作循环的身份标识。

### 循环起点

当一条新的、由人发出的 Telegram 群聊消息唤醒或正式指向某个受管 Telegram bot，并进入 Telegram 适配层规划路径时，开启一个新的协作循环。

### 循环传播

协作循环一旦建立，所有由该循环派生出的、由 bot 发出的 Telegram 群聊下游派发，都必须携带同一个 `dispatch_cycle_id`。

### 扇出冻结规则

同一条由人发出的 Telegram 群聊原始消息，即使首轮同时命中了多个 bot，也只能创建一个共享的 `dispatch_cycle_id`。

这意味着：

- 人类首消息的多目标扇出，仍然属于同一个协作循环
- 后续各条 bot 分支继续共享同一个预算桶
- 不允许为 `A` 分支、`B` 分支各自新建独立预算

否则一次人类输入就会在多条分支上重复拿到多份预算，保险丝会失去意义

### 循环重置

之后任意新的、由人发出的 Telegram 群聊消息，都必须开启新的协作循环，不能继承上一个循环的预算消耗。

### 人与 bot 的区别

- 人对 bot 的发起会开启协作循环，但不消耗预算。
- bot 对 bot 的下游派发会消耗预算。

---

## 6. 预算计数规则

### 计数单位

计数单位冻结为：

> 一次成功的 bot 对 bot 下游 `Dispatch`

以下都不是计数单位：

- 一条消息
- 一个 mention 候选
- 一个过滤前的目标候选
- 一次 `RecordOnly`

### 多目标消息

如果一条 bot 消息成功派发到多个目标 bot，每个成功目标都消耗 1 个单位。

例子：

```text
@bot_b 处理日志
@bot_c 处理配置
@bot_d 处理发布差异
```

如果这三个目标都成功派发，则本条消息总共消耗 `3` 个单位。

### 精确扣减时机

冻结规则：

- 先做去重。
- 目标仍然可派发时，在下游交接之前预留 1 个预算单位。
- 下游交接失败时，退回这个预留单位。

这里的准确语义是：

- 对外可见的最终记账单位，仍然是“一次成功的 bot 对 bot 下游 `Dispatch`”
- “预留 1 个预算单位”只是实现期保证精确记账的执行手段，不改变计数语义本身

这样可以保证：

- dedupe hit 不消耗预算
- `RecordOnly` 不消耗预算
- 没有命中目标不消耗预算
- planner drop 不消耗预算
- 下游交接失败不会留下永久泄漏

### 局部成功规则

预算按目标逐个应用，而不是按整条源消息整体应用。

如果一条源 bot 消息命中了 3 个目标，但只剩 2 个预算单位：

- 前两个目标可以继续派发
- 剩余目标必须降级为 `RecordOnly`

不能把整条消息整体作为“要么全放，要么全拦”的粗粒度判断。

---

## 7. 预算耗尽时的行为

当某个目标原本会进入 `Dispatch`，但该协作循环的预算已经耗尽时：

- 将该目标从 `Dispatch` 降级为 `RecordOnly`
- 保留原始正文
- 保留该目标的 `addressed` 语义
- 不再继续自动唤醒该目标的下游 bot 扩散

### `addressed` 保留规则

冻结规则：

- 如果 planner 判定这条消息确实是正式指向该目标，则 `addressed` 必须继续保持为真
- 保险丝只改变执行模式，不改变“这条消息是不是正式指向了该 bot”这件事的语义真相

---

## 8. 可观测性与告警

任何由保险丝触发的 `Dispatch -> RecordOnly` 降级，都必须输出结构化日志。

### 触发条件

仅当以下条件同时满足时，触发这类保险丝告警：

1. 当前链路是由 bot 发出的 Telegram 群聊流程
2. 当前目标原本会进入 `Dispatch`
3. 当前 `dispatch_cycle_id` 的预算已耗尽
4. 系统因此把该目标降级为 `RecordOnly`

### 不应触发此类告警的情况

- 正常的 `RecordOnly`
- 人发起的第一轮派发
- dedupe hit
- planner drop
- 没有命中目标
- 未耗尽预算的普通下游交接失败

### 日志级别

- 同一个 `dispatch_cycle_id` 第一次发生降级：`warn`
- 同一个 `dispatch_cycle_id` 后续再次发生降级：`info`

### 固定日志字段

- `event = "telegram.group.dispatch_fuse"`
- `reason_code = "dispatch_cycle_budget_exceeded"`
- `decision = "downgrade_to_record"`
- `policy = "group_record_dispatch_v3"`
- `dispatch_cycle_id`
- `used`
- `budget`
- `chat_id`
- `thread_id`
- `source_account_handle`
- `target_account_handle`
- `message_id`

可选字段：

- `remediation = "start a new human turn or increase channels.telegram.bot_dispatch_cycle_budget"`

### 降噪规则

- 每个 `dispatch_cycle_id` 的首次命中打一条 `warn`
- 同一协作循环中后续目标的降级只打 `info`
- 不自动向群里发警告消息
- 不为保险丝告警注入伪 session/system message

---

## 9. 示例场景

### 示例 A：单链循环

```text
人 -> A
A -> B
B -> A
A -> C
C -> A
人 -> A
```

若 `bot_dispatch_cycle_budget = 4`：

- 第一个协作循环中计数的派发为：`A->B`、`B->A`、`A->C`、`C->A`
- 总消耗：`4`
- 后面的新人类消息会开启新循环
- 新循环的预算重新从 `0` 开始

### 示例 B：多目标分叉

```text
人 -> A
A -> B/C/D
C -> E
D -> B
```

如果 `A -> B/C/D` 成功派发到三个目标，则它消耗的是 `3` 个预算单位，而不是 `1`。

### 示例 C：人类首轮多目标

```text
人 -> A/B
A -> C
B -> D
C -> E
```

若 `bot_dispatch_cycle_budget = 4`：

- `人 -> A/B` 会开启一个新的 `dispatch_cycle_id`
- 这次人类首轮虽然命中了 `A` 和 `B`，但它们共享同一个协作循环
- 后续计数的是 `A->C`、`B->D`、`C->E`
- 这些分支不能各自再拿一份独立预算

---

## 10. 适配层归属

这个修复属于 Telegram 适配层 / 运行时边界。

预期归属面：

- Telegram 配置与 schema 接线（为了接入这条共享策略）
- Telegram 入站 / 出站规划
- Telegram 共享群运行时状态（用于循环计数）
- Telegram 测试

明确不允许：

- 把 Telegram 专属 relay-chain 编排重新放回 gateway/core
- 新增 gateway-owned Telegram 群聊兜底逻辑

---

## 11. 必备测试

实现至少必须覆盖：

1. 人类新消息会开启新协作循环，并拥有全新预算。
2. bot 对 bot 单链派发按“每次成功下游派发”计数。
3. 一条源 bot 消息派发到多个目标时，按“每个成功目标”计数。
4. 当预算在一条多目标消息处理中途耗尽时，前面的目标可以派发，后面的目标会降级为 `RecordOnly`。
5. dedupe hit 不消耗预算。
6. 下游交接失败会退回预留预算。
7. 保险丝耗尽时，保留原文与 `addressed` 真值，只降级 mode 为 `RecordOnly`。
8. 同一循环首次命中打 `warn`，后续命中打 `info`。

---

## 12. 收敛性评估

这份规范已经足够收敛，可以直接进入实现。

仍然存在的进一步收敛空间，已经属于低价值实现细节，而不是产品语义问题，例如：

- `dispatch_cycle_id` 在内部挂在哪个运行时字段
- 预留 / 退回预算 helper 的函数命名与结构体布局
- `[channels.telegram]` 如何接入当前 schema plumbing

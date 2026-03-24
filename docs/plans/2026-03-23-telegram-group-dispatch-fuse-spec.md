# Telegram 群聊派发保险丝规范（`root_message_id` 核心机制）

**状态：** 已冻结，可进入实现评审

**范围：** 仅限 Telegram 适配层与 Telegram 配置接线（`crates/telegram/*`、`crates/config/*`、相关启动接线）中的群聊 bot-to-bot 自动派发扩散控制

**不在范围内：** 不恢复旧 relay-chain 整套机制；不新增 gateway/core 的 Telegram 群聊编排；不修改正文透传规则（该内容见独立规范）

---

## 1. 目标

为 Telegram 群聊中的 bot 协作补上一根共享派发保险丝，防止单个外部根消息引出的 bot-to-bot 协作链无限扩散或多路分叉失控。

这根保险丝只负责限制 bot-to-bot 扩散总量，不负责目标识别、正文改写、会话路由或群聊转写语义。

---

## 2. 核心冻结结论

这次收口后的核心机制只有 6 条：

1. **根身份直接用外部根消息自己的 `message_id`**
   - 不再生成 `dispatch_cycle_id`
   - 正文统一称呼 `root_message_id`

2. **每个 Telegram chat 只有一个运行时管理器**
   - 同一 chat 内的 `participants`、`dedupe_actions`、`message_contexts`、`root_budgets` 必须收口在一起
   - 不允许再拆成几张彼此平行、各自清理的状态表
   - 这是逻辑所有权，不要求为每个 chat 新增 actor、事件总线或后台任务

3. **根预算懒创建**
   - 外部消息只有在“至少放行了一个 `Dispatch`”时，才创建 `root_budgets[(chat_id, root_message_id)]`
   - 外部消息如果最终只是噪声或 `RecordOnly`，不得平白创建保险丝状态

4. **预算采用“准入即扣减”**
   - 某个 bot-to-bot 目标被放行为 `Dispatch` 的当下，立即 `used += 1`
   - 不做 reserve / commit / rollback
   - 下游 handoff 后续即使失败，也不回补预算

5. **根传播只靠本系统自己写入的消息上下文**
   - 受管 bot 群消息发送成功后，立即登记 `(chat_id, sent_message_id) -> root_message_id`
   - 后续继续派发时，只查“当前消息自己的上下文”
   - 不依赖 generic Telegram `reply_to_message_id` 历史链

6. **缺上下文就 fail-close**
   - 受管 bot 消息如果原本会触发下游 `Dispatch`，但找不到有效 `root_message_id` / 根预算桶，必须降级为 `RecordOnly` 并告警
   - 进程重启后旧链状态清空，旧链消息继续冒出来时同样按 fail-close 处理

---

## 3. 配置

冻结后的外部配置形态：

```toml
[channels.telegram]
bot_dispatch_cycle_budget = 128
```

### 配置语义

- 作用域：Telegram 渠道级共享策略
- 不是按 bot 账号分别配置
- 由所有 Telegram bot 账号共享
- 类型：`u32`
- 默认值：`128`
- `0` 为非法值，必须在配置校验阶段直接报错

### 配置落地约束

- 内部必须收口为 typed `TelegramChannelsConfig { bot_dispatch_cycle_budget, accounts }`
- Telegram 账号的唯一来源是 `accounts`
- 不再允许继续保留 raw `HashMap<String, Value>` + 保留键跳过的旧方案
- `bot_dispatch_cycle_budget` 虽然名称里保留了 `cycle` 一词，但它在本规范中的实际语义就是“根消息共享预算”；不再对应任何 synthetic cycle id

### 为什么必须是共享配置

这个预算限制的是一条跨 bot 协作链，而不是某个 bot 自己一天能发几次。比如：

```text
人 -> A
A -> B
B -> C
B -> D
D -> E
E -> A
```

上面这些 bot-to-bot 放行次数，必须从同一个根预算桶里扣。

---

## 4. 运行时状态

概念上的收口形态如下：

```text
TelegramGroupRuntime
└── chats[chat_id]
    ├── participants
    ├── dedupe_actions
    ├── message_contexts[message_id]
    └── root_budgets[root_message_id]
```

### `message_contexts`

`message_contexts[(chat_id, message_id)]` 至少表达：

- `root_message_id`
- `managed_author_account_handle: Option<String>`
- `touched_at`

只追踪两类消息：

1. **实际开启协作链的外部根消息**
   - `managed_author_account_handle = None`
2. **由本系统成功发出的受管 bot 群消息**
   - `managed_author_account_handle = Some(account_handle)`

不追踪其余所有群消息。

### `root_budgets`

`root_budgets[(chat_id, root_message_id)]` 至少表达：

- `used`
- `budget`
- `warned`
- `touched_at`

语义：同一个 `root_message_id` 后面所有 bot-to-bot 放行，都从这一个桶里扣。

### 资源回收

- `message_contexts` 与 `root_budgets` 必须有统一的 TTL / 数量上限清理
- 清理、淘汰、进程重启造成的状态丢失，都是允许的
- 一旦旧状态不存在，系统必须 fail-close，而不是试图“猜测恢复”

---

## 5. 根创建与传播

### 5.1 外部消息如何成为根

一条外部消息进入 Telegram 群聊规划路径后：

- 如果最终没有任何目标被放行为 `Dispatch`
  - 不创建根预算桶
  - 不登记根消息上下文
- 如果最终至少有一个目标被放行为 `Dispatch`
  - 这条消息立刻成为外部根消息
  - 其自己的 Telegram `message_id` 冻结为 `root_message_id`
  - 创建 `root_budgets[(chat_id, root_message_id)]`
  - 同时登记 `message_contexts[(chat_id, root_message_id)]`

### 5.2 首轮多目标共享同一根

如果同一条外部消息首轮同时放行给多个 bot：

```text
人类消息 m100 -> A / B
```

冻结规则：

- `m100` 只产生一个 `root_message_id = m100`
- A、B 只是同一个根下面的两条首轮分支
- 首轮放行本身不消耗预算
- 后续 `A -> ...`、`B -> ...` 都要共用 `m100` 这一个根预算桶

### 5.3 bot 消息如何继承根

当受管 bot 在群里成功发出一条消息时：

- 系统已经知道这条发送动作当前属于哪个 `root_message_id`
- 一旦 Telegram send 成功并返回新的 `sent_message_id`
- 就立刻登记：

```text
(chat_id, sent_message_id) -> { root_message_id, managed_author_account_handle }
```

后续再看到这条 bot 消息时：

- 直接查这条消息自己的 `(chat_id, message_id)`
- 得到 `root_message_id`
- 再去对应根预算桶扣减

如果一次发送因分片/分块产生多个 Telegram `message_id`，则每个成功返回的 `message_id` 都必须登记到同一个 `root_message_id`；不能只记第一条。

### 5.4 为什么不走 reply-to 历史链

因为 Telegram 客户端消息不保证都有 reply-to。

换句话说：

- “这条 bot 消息是不是 reply 样式”不是可靠事实源
- “这条 bot 消息是不是我们自己成功发出去过，并且当时属于哪个根”才是可靠事实源

所以根传播冻结为：

> 只信本系统自己在发送成功时写入的消息上下文，不追历史 reply 链。

这里禁止的是“用 generic reply 历史链追根”，不是禁止现有 reply 目标识别；reply 语义仍只负责判定目标，不负责决定属于哪个根。

---

## 6. 预算扣减规则

### 6.1 计数单位

计数单位冻结为：

> 一次被放行的 bot-to-bot `Dispatch`

以下都不是计数单位：

- 一条消息
- 一个 mention 候选
- 一个 `RecordOnly`
- 外部首轮放行
- dedupe hit
- 同一次 source->target 放行产生的多个 Telegram 分片

### 6.2 扣减时机

冻结规则：

- 对某个 bot-to-bot 目标，先完成目标识别与 dedupe
- 如果它原本应进入 `Dispatch`，并且保险丝允许放行
- 则在“放行这一刻”立即 `used += 1`

如果这一次放行在 Telegram 传输层被拆成多个消息分片：

- 预算仍然只扣 1 次
- 分片只影响消息上下文登记
- 不得按分片数量重复扣减

这就叫“准入即扣减”。

### 6.3 为什么不回补预算

这不是精确结算器，而是保险丝。

如果还要做到“下游 handoff 失败就回补”，就必须额外引入：

- reserve / commit / rollback
- 成功/失败返回契约
- 更多跨层状态同步

这会把机制重新做复杂。

本单明确不这么做，直接冻结为：

> 只要已经放行，就算已经消耗预算；后续失败也不回补。

### 6.4 多目标处理顺序

为避免“预算只剩 1 个时，到底放行 B 还是 C”这种不确定性，必须冻结稳定顺序。

最终规则：

- 候选目标按 `target_account_handle` 字典序升序排列
- 沿这一顺序单次遍历
- 能放就放，放行即扣减
- 后续预算不够的目标降级为 `RecordOnly`

之所以不用正文里的 mention 出现顺序，是为了不把正文解析副产物再变成第二套排序事实源。
稳定顺序本身也必须只有一个事实源：应当集中在单一 helper 或运行时快照出口产出，禁止多个调用点各自排序。

例子：

```text
剩余预算 = 2
当前 bot 消息命中目标 = C / B / D
稳定顺序冻结后 = B / C / D
```

则结果必须稳定为：

- `B -> Dispatch`
- `C -> Dispatch`
- `D -> RecordOnly`

不是这次放 `C`、下次放 `B`。

---

## 7. 降级与告警

### 7.1 预算耗尽

当某个目标原本会进入 `Dispatch`，但根预算已耗尽时：

- 将该目标从 `Dispatch` 降级为 `RecordOnly`
- 保留原始正文
- 保留该目标的 `addressed` 真值
- 记录结构化日志

### 7.2 根上下文缺失

当某条受管 bot 消息原本会继续触发下游 `Dispatch`，但系统拿不到有效根状态时：

- 将该目标从 `Dispatch` 降级为 `RecordOnly`
- 保留原始正文
- 保留 `addressed`
- 记录结构化日志

“拿不到有效根状态”包括：

- 当前消息没有消息上下文
- 当前消息上下文里拿不到 `root_message_id`
- `root_message_id` 有了，但根预算桶已经不存在（例如 TTL 淘汰或进程重启后）

### 7.3 固定日志口径

固定日志字段：

- `event = "telegram.group.dispatch_fuse"`
- `decision = "downgrade_to_record"`
- `policy = "group_record_dispatch_v3"`
- `reason_code`
- `root_message_id`
- `used`
- `budget`
- `chat_id`
- `thread_id`
- `source_account_handle`
- `target_account_handle`
- `message_id`

固定 `reason_code`：

- `root_dispatch_budget_exceeded`
- `root_dispatch_context_missing`

日志级别：

- 同一 `(chat_id, root_message_id)` 首次预算耗尽：`warn`
- 同一根后续再次预算耗尽：`info`
- `root_dispatch_context_missing`：固定 `warn`

禁止：

- 打完整正文
- 打 token
- 往群里额外发“系统警告消息”
- 注入伪 session/system message

---

## 8. 人话例子

### 例子 A：最基本单链

```text
人类消息 m100 -> A
A 发消息 m101 -> B
B 发消息 m102 -> A
A 发消息 m103 -> C
```

若 `bot_dispatch_cycle_budget = 2`：

- `m100` 首轮放行给 A，不扣预算
- `m101 -> B` 被放行，扣 1
- `m102 -> A` 被放行，扣 1
- `m103 -> C` 再想放行时，预算已满，降级为 `RecordOnly`

### 例子 B：为什么根传播不靠 reply-to

```text
人类消息 m100 -> A
A 成功发出 m101，并在正文里点名 B
```

系统在 send 成功那一刻就登记：

```text
m101 -> root = m100
```

后面系统看到 `m101`，要不要继续派发给 B？

- 直接查 `m101` 自己属于哪个 root
- 查到 `root = m100`
- 从 `m100` 这桶预算里扣

这里完全不需要 `m101.reply_to_message_id`。

### 例子 C：一次性点名多个 bot

```text
人类消息 m100 -> A
A 发消息 m101，正文同时点名 C、B、D
```

假设此时根预算只剩 `2`：

- 稳定顺序冻结为 `B / C / D`
- `B -> Dispatch`，扣 1
- `C -> Dispatch`，扣 1
- `D -> RecordOnly`

### 例子 D：进程重启后的旧链

```text
人类消息 m100 -> A
A 成功发出 m101 -> B
此时进程重启
稍后 Telegram 又把 m101 相关后续事件送到系统
```

重启后内存状态已清空：

- 系统已不再知道 `m101` 属于哪个 `root_message_id`
- 因此不能继续 bot-to-bot 放行
- 正确行为是：`RecordOnly + root_dispatch_context_missing`

这是刻意的安全收口，不是 bug。

---

## 9. 并发与回收边界

### 并发边界

- 同一 chat 的运行时状态必须由同一个 per-chat 管理器统一串行化访问
- 根预算、消息上下文、参与者集合、dedupe 必须在这个边界内一起更新
- 不允许把“写消息上下文”和“扣预算”分散到多个互不知情的锁或缓存里

### 回收边界

- `message_contexts` 与 `root_budgets` 必须有统一的 `touched_at` 刷新与淘汰策略
- 淘汰后的行为统一按 fail-close 处理
- 不做磁盘持久化，不跨重启恢复

---

## 10. 必备测试

实现至少必须覆盖：

1. typed `TelegramChannelsConfig` 的默认值、解析与 `0` 拒绝。
2. 外部消息无 `Dispatch` 时，不创建根预算桶。
3. 同一外部根消息首轮多目标共享同一个 `root_message_id`。
4. bot-to-bot 单链派发按“每个放行目标”扣减。
5. 一条 bot 消息派发到多个目标时，按稳定顺序单次遍历，预算不足时前放后拦。
6. 受管 bot 消息发送成功后，新的 `sent_message_id` 会即时绑定到正确根。
7. 消息分片/分块发送时，每个成功返回的 `message_id` 都会绑定到同一个根。
8. 单次 source->target 放行即使被 Telegram 分成多片，也只扣 1 次预算。
9. 下游 handoff 失败不会回补预算。
10. 预算耗尽时，保留原文与 `addressed` 真值，只降级 `mode` 为 `RecordOnly`。
11. 受管 bot 消息缺失有效根状态时，会 fail-close 并打 `root_dispatch_context_missing`。
12. 同一根首次预算耗尽打 `warn`，后续命中打 `info`。
13. 进程重启/状态淘汰后的旧链不会绕过保险丝。

---

## 11. 收敛性结论

这份规范已经进一步收敛到最小闭环：

- 没有 synthetic id
- 没有 rollback 契约
- 没有 generic reply 链追溯
- 没有 per-account 预算
- 没有把 Telegram 复杂性外溢到 gateway/core

剩余实现自由度只在低价值工程细节，例如：

- `participants` 最终用 `BTreeSet` 还是等价稳定集合承载
- TTL / 数量上限取多少更合适
- helper 函数与局部结构体怎么命名

这些都不再影响产品语义，不需要再继续发散机制。

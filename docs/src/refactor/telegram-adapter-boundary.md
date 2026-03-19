# Telegram Adapter Boundary

本文档只定义一件事：

- Telegram 渠道适配层在第三版早期阶段，应当如何与 core 切边界

本文档讨论的是：

- TG adapter 的边界面
- TG 侧对象与命名
- 哪些事情属于 TG adapter
- 哪些事情属于 core
- 当前阶段如何尽量复用已有配置链路与已有代码

本文档不讨论：

- 其他渠道的最终统一接口
- 最终版 `session_event` 全字段表
- 全局一次性重构顺序

也就是说，本文档讨论的是：

- **TG-first 的边界收敛方案**

而不是：

- **所有渠道立即共用的最终接口**

本文档与下列通用接口文档配套使用：

- `docs/src/refactor/channel-adapter-generic-interfaces.md`

两者关系是：

- 本文档先定义 Telegram 这条真实链路的专项边界
- 通用接口文档再从本专项边界中提炼稳定接口壳

如果两者暂时出现冲突，应先以本专项边界文档为准，再回修通用接口文档

## 一句话结论

TG adapter 与 core 之间，当前应明确拆成四个边界面：

- 配置面
- 聊天入站面
- 控制面
- 回复出站面

其中，真正进入 core 聊天主链的对象，应先收敛为：

```text
tg_inbound {
  kind
  mode
  body
  private_source
}
```

并由 TG adapter 额外提供：

```text
resolve_tg_route(tg_inbound, scope) -> tg_route
```

以及：

```text
send_tg_reply(tg_reply)
```

一句话：

- **TG adapter 负责理解 Telegram 协议与本地策略，core 负责会话语义、会话记录与最终上下文整理**

## 设计目标

这一轮收敛主要解决四个问题：

- 不再把 Telegram 私有字段直接抬成 core 公共概念
- 不再把 mirror / relay / typing / reply threading 这类 TG 策略直接散落在 gateway/chat 主链
- 不再让 TG adapter 直接主导最终 LLM 可见 transcript
- 在不大改配置来源链路的前提下，先把 TG adapter 的职责边界钉死

## 命名原则

当前阶段，这套接口**不是**所有渠道共用的最终抽象接口。

因此命名原则应明确为：

- TG adapter 自有对象、函数、策略，统一使用 `tg_` 前缀
- core 语义结果，不使用 `tg_` 前缀
- 少用过泛的词，例如：
  - `payload`
  - `meta`
  - `context`
  - `channel_*`
- 尽量使用见名知意的短词，例如：
  - `kind`
  - `mode`
  - `body`
  - `private_source`
  - `route`
  - `output`
  - `private_target`

## 四个边界面

### 1. 配置面

负责回答：

- TG adapter 的运行时配置从哪里来
- 哪些配置由 TG adapter 自己消费
- 哪些配置最终要影响 core 的会话策略

当前阶段的原则是：

- **尽量复用现有配置来源与更新链路**
- **先切逻辑归属，不急着改存储结构**

也就是说，当前仍继续复用：

- `crates/telegram/src/config.rs`
- `crates/gateway/src/channel.rs`
- `crates/telegram/src/plugin.rs`

继续沿用现有 `TelegramAccountConfig` 作为：

- 持久化配置结构
- RPC 更新输入
- TG plugin/runtime 的启动配置

但在职责上，应把它拆成两个逻辑视图：

- `tg_runtime`
- `tg_policy`

#### `tg_runtime`

由 TG adapter 自己消费的配置视图。

典型包括：

- token
- polling / reconnect / backoff
- outbound retry / typing / stream
- DM allowlist / OTP
- group mention / listen
- relay / mirror / hop limit / budget / strictness
- TG 侧 group transcript policy

#### `tg_policy`

由 TG adapter 提供给 core 的会话策略视图。

当前主要包括：

- `dm_scope`
- `group_scope`

如果后续需要把 transcript policy、默认 model / persona / session defaults 进一步收口到 core，再单独推进。

当前阶段不需要先改：

- 配置存储位置
- 配置 RPC schema
- 配置热更新入口

### 2. 聊天入站面

负责回答：

- 哪些 Telegram 入站属于聊天主链
- 它们应以什么最小对象进入 core

这里的原则是：

- 聊天主链只接收真正需要进入会话/推理链的消息
- TG 私有的 transport 细节与局部策略信息，不直接扩散成 core 公共字段

### 3. 控制面

负责回答：

- 哪些 Telegram 输入根本不属于聊天正文

这一面不应混进 `tg_inbound`。

典型包括：

- slash command
- inline keyboard callback
- OTP challenge / approval flow
- access denied / security feedback
- account health / runtime state 事件

这些输入应走：

- `tg_control`

或直接由 TG adapter 内部策略链处理，而不是塞进聊天主对象。

### 4. 回复出站面

负责回答：

- core 产出回复后，TG adapter 如何把它送回 Telegram

这一面不应让 core 直接理解：

- `chat_id`
- `message_id`
- `reply_to_message_id`
- topic/thread 细节
- 用哪个 bot account 发

这些都属于 TG adapter 自己的投递责任。

## TG 聊天主入口对象

当前建议收敛为：

```text
tg_inbound {
  kind
  mode
  body
  private_source
}
```

### `kind`

取值：

- `dm`
- `group`

这里的 `kind` 只表达：

- **这条 Telegram 聊天输入属于 DM 还是群聊**

它不表达：

- `cron`
- `heartbeat`
- 其他非 TG 入站类型

这些是 core 的全局会话类型，不应混入 TG 聊天入站对象。

### `mode`

取值：

- `dispatch`
- `record_only`

含义：

- `dispatch`：进入 run / 推理链
- `record_only`：只记会话，不触发 run

它回答的是：

- **这条聊天输入是否要触发 agent run**

因此：

- addressed group message 通常是 `dispatch`
- listen-only group message 通常是 `record_only`
- mirror 派生输入通常是 `record_only`
- relay 派生输入通常是 `dispatch`

当前阶段，不再把 `origin` 抬成聊天主对象字段。

原因是：

- `origin = relay | mirror | native` 更像 TG adapter 的内部来历
- 它与 `mode` 放在同一层很容易膨胀
- 当前 core 不需要先知道这些细分来历

### `body`

定义为：

```text
tg_content {
  text
  attachments
  location
}
```

它只表达：

- 这条聊天输入的通用消息内容

这里的原则是：

- TG adapter 先把 Telegram raw update 消化成通用内容
- 再把结果装进 `body`

典型包括：

- 文本消息 -> `text`
- 语音消息 -> 先转写，再落到 `text`
- 图片消息 -> 图片字节/附件对象进入 `attachments`
- 位置共享 -> 进入 `location`

这里**不**应直接放：

- mention entity 原始结构
- Telegram file id
- Telegram update 原文
- TG-GST v1 最终 transcript 文本
- legacy mirror / relay 前缀文本

也就是说：

- `body` 负责表达“消息内容是什么”
- 不负责表达“Telegram 原始协议长什么样”
- 也不负责表达“最终给 LLM 的文本长什么样”

### `private_source`

`private_source` 是：

- **TG adapter 私有的不透明附带对象**

它回答的是：

- 这条聊天输入在 Telegram 侧来自哪里
- TG adapter 后续如何继续解析路由、会话分桶与回复路径

当前建议把所有 TG 私货都收进这里，而不是散落成聊天主对象的公共字段。

例如：

- bot account
- chat id
- message id
- sender 原生引用
- reply_to message 引用
- thread / topic 引用
- source time
- relay info
- mirror info
- 其他 dedupe / local policy 所需信息

当前阶段，不要求 `private_source` 的字段一开始就完全冻结成所有渠道共用结构。

它的定位就是：

- **TG adapter 私有、core 不透明、但可被 core 携带与回传的不透明载体**

## TG 路由解析

仅有 `tg_inbound` 还不够。

因为：

- `peer` 不是 TG adapter 在聊天入口对象里天然就能直接给出的最终 core 语义
- `sender` 也不是所有场景下都稳定可得
- `bucket_key` 取决于当前 `scope`

因此需要第二步：

```text
resolve_tg_route(tg_inbound, scope) -> tg_route
```

### `tg_route`

建议收敛为：

```text
tg_route {
  peer
  sender
  bucket_key
  addressed
}
```

### `peer`

这是 core 语义结果。

它回答的是：

- 这条消息属于哪个逻辑对端

这里要明确：

- `peer` 不是从配置里直接读出来的
- `peer` 不是从旧 session 反推出来的

更合理的来源是：

- 入站 `private_source`
- identity link / identity 解析链
- 当前 `kind`

也就是说：

- `dm` 下，`peer` 指向单个外部参与者
- `group` 下，`peer` 指向共享群会话对象

### `sender`

这也是 core 语义结果，但允许为空。

它回答的是：

- 这条群消息是谁说的

这里要明确：

- `sender` 缺失不代表 listen-only
- listen-only 由 `mode` 表达
- `sender` 为空只是协议边角或解析失败时的允许状态

如果某个 `scope` 依赖 `sender`，而当前又没有稳定 `sender`，则应由 TG route resolver 在内部决定：

- 如何降级计算 `bucket_key`

而不是让 core 伪造一个假 `sender`。

### `bucket_key`

这是 TG adapter 在当前 `scope` 下返回的稳定分桶结果。

它回答的是：

- 这条消息在当前 scope 下应落入哪个逻辑会话桶

这里故意不再抬出：

- `account`
- `branch`
- `topic`
- `thread`

这些都属于 TG adapter 内部实现细节。

当前更收敛的做法是：

- core 知道 `scope`
- TG adapter 返回 `bucket_key`

至于：

- Telegram 用 chat/account/topic/thread 怎样实现这个结果

不需要 core 知道。

### `addressed`

这是一个群聊语义结果。

它回答的是：

- 这条群消息在语义上是否明确对 agent 说

这里要和 `mode` 区分：

- `mode`：要不要触发 run
- `addressed`：这条话是不是对 agent 说

例如：

- group listen-only 旁听消息：`mode = record_only`，`addressed = false`
- group 被点名消息：`mode = dispatch`，`addressed = true`
- always-respond 策略下未点名消息：可能 `mode = dispatch`，但 `addressed = false`

因此 `addressed` 仍有必要作为 route 解析结果保留给 core。

## 谁负责把群聊消息变成最终上下文文本

这一点必须明确：

- **TG adapter 不负责最终 LLM 可见 transcript**
- **core 负责把结构化群消息整理成会话记录，再由 core renderer 生成最终上下文**

也就是说，应拆成三步：

1. TG adapter 归一化 raw update
   - 产出 `tg_inbound`

2. TG adapter 解析 TG 路由语义
   - 产出 `tg_route`

3. core 生成会话记录并整理上下文
   - `dm_record`
   - `group_record`
   - 最终由对应 renderer 生成 LLM 可见上下文

这意味着：

- TG adapter 不应继续直接主导最终 transcript 拼接
- TG-GST v1 / legacy 这类文本整理逻辑，长期应由 core 的 group renderer 接管

当前阶段可以复用现有代码逻辑，但归属目标应明确：

- **逻辑可暂复用**
- **职责应逐步迁回 core**

## `dm_record` / `group_record` 属于 core

在这条边界里，建议区分：

- TG adapter 输入对象
- core 会话记录对象

也就是说：

- `tg_inbound` / `tg_route` 是 TG adapter -> core 的边界对象
- `dm_record` / `group_record` 是 core 内部对象

例如：

```text
group_record {
  peer
  sender
  addressed
  body
}
```

这里回答的是：

- core 以后怎样保存这条群消息
- core 以后怎样整理这条群消息进入 LLM 上下文

这一层不应继续由 TG adapter 直接决定最终文案格式。

## TG 控制面

TG 里有大量输入并不属于聊天正文。

这些应从聊天主入口中剥离，进入：

```text
tg_control
```

它主要覆盖：

- slash command
- inline callback
- OTP challenge / self-approval
- access denied / allowlist feedback
- account health / runtime control

这样做的好处是：

- `tg_inbound` 不再承载命令与控制语义
- 命令处理链不会继续污染会话主链
- TG adapter 的控制逻辑可以单独内聚

## TG 回复出站面

回复出站面建议定义为：

```text
tg_reply {
  output
  private_target
}
```

并由：

```text
send_tg_reply(tg_reply)
```

负责最终发回 Telegram。

### `output`

`output` 是 core 产出的回复结果。

它表达的是：

- 发什么
- 回哪条已保存上游事件
- 是否静默投递

例如：

- 文本
- 媒体
- 位置
- 流式回复结果

这里建议把“回复语义”理解成：

- core 只表达这次回复的语义结果
- core 不直接表达 Telegram API 的发送细节

更具体地说：

- 如果这次回复需要“引用上一条”，core 应表达为“回哪条本地已保存事件”
- TG adapter 再把这个结果恢复成 Telegram 自己的 `reply_to_message_id`

这样可以保持：

- core 只依赖本地事实链
- TG 协议细节继续封装在 adapter 内部

### `private_target`

这里的 `private_target` 是 TG adapter 私有的回复投递对象。

它表达的是：

- 往哪发
- 用哪个 bot account 发
- 是否 reply_to 某条 TG message
- 是否需要 thread/topic 细节

这部分可以：

- 从入站 `private_source` 派生
- 或从已绑定 session 的 TG 投递信息恢复

但都不要求 core 解析这些内部字段。

这里故意不用：

- `target`

是因为在回复出站语义里，`private_target` 能同时表达两件事：

- 这是一个投递目标对象
- 这是 TG adapter 私有且不透明的对象

## mirror / relay 的归属

mirror / relay 都应被视为：

- **TG adapter 的 group policy**

而不是：

- core 的公共语义对象

### mirror

`mirror` 的本质是：

- 同群多 bot 会话同步策略

它不应在 core 公共接口里变成一套单独概念。

当前更合理的方式是：

- mirror 是否触发，由 TG adapter 自己决定
- 如果需要写入 core，会生成一条新的 `tg_inbound`
- 通常这条输入是 `mode = record_only`

mirror 私有信息，例如：

- mirror key
- dedupe seed
- source bot handle

都应留在 `private_source` 中，而不是抬成 core 公共字段。

### relay

`relay` 的本质是：

- Telegram 群内 bot-to-bot delegation 策略

它同样不应先上升为 core 公共概念。

当前更合理的方式是：

- mention 解析、strict/loose、hop limit、epoch budget、dedupe，都留在 TG adapter 内部
- 如果判定要委派，则由 TG adapter 生成一条新的 `tg_inbound`
- 通常这条输入是 `mode = dispatch`

relay 私有信息，例如：

- chain id
- hop
- source outbound ref

都应留在 `private_source` 中，而不是抬成 core 公共字段。

## 当前代码与目标边界的大致映射

### 当前已偏向 TG adapter 的部分

主要在：

- `crates/telegram/src/config.rs`
- `crates/telegram/src/plugin.rs`
- `crates/telegram/src/bot.rs`
- `crates/telegram/src/outbound.rs`
- `crates/telegram/src/access.rs`
- `crates/telegram/src/handlers.rs`

这些部分已经覆盖了：

- TG 连接与 polling
- TG outbound 发送
- TG access / allowlist / OTP
- TG raw message 解析
- TG media 提取
- TG slash command / callback

### 当前仍散在 gateway 的部分

主要在：

- `crates/gateway/src/channel_events.rs`
- `crates/gateway/src/chat.rs`
- `crates/gateway/src/session_labels.rs`

这些部分今天仍承载了大量 TG 特有逻辑，例如：

- active session 绑定
- TG typing lifecycle
- TG channel-bound session label
- TG group mirror
- TG group relay
- TG 回复投递细节
- TG-GST v1 prompt 注入

第三版早期阶段，TG adapter 的主要改造目标不是“重写一切”，而是：

- **把这些仍散在 gateway 的 TG 特性，逐步并回 TG adapter 边界之内**

## 当前阶段的收敛结论

这一轮先冻结以下几点：

### 1. 边界面固定为四个

- 配置面
- 聊天入站面
- 控制面
- 回复出站面

### 2. TG 聊天主对象先收敛到四字段

```text
tg_inbound {
  kind
  mode
  body
  private_source
}
```

### 3. `peer` / `sender` / `bucket_key` 不直接塞进入站对象

而是通过：

```text
resolve_tg_route(tg_inbound, scope) -> tg_route
```

再产出：

- `peer`
- `sender`
- `bucket_key`
- `addressed`

### 4. 最终群聊 transcript 归 core

- TG adapter 负责归一化与路由解析
- core 负责 `group_record` / `dm_record`
- core 负责最终上下文整理与 renderer

### 5. 配置来源尽量复用现有代码

当前阶段先不改：

- `TelegramAccountConfig`
- 现有 channel add/update/start 流程
- 现有 plugin runtime 配置链

先只改：

- 配置的逻辑归属
- 读取与消费边界

## 后续直接可落的实现方向

基于本文档，下一步最自然的工程动作是：

1. 在 TG adapter 内部显式引入：
  - `tg_inbound`
  - `tg_route`
  - `tg_control`
  - `tg_reply`

2. 先把 `handlers.rs` 中的聊天主链整理成：
  - raw update -> `tg_inbound`

3. 再把当前散在 gateway 的 TG route / reply / mirror / relay 逻辑逐步收回：
   - `crates/gateway/src/channel_events.rs`
   - `crates/gateway/src/chat.rs`

4. 最后再把 group record / renderer 责任从 TG transcript shaping 中彻底抽回 core

## 相关文档

- `docs/src/refactor/v3-design.md`
- `docs/src/refactor/v3-gap.md`
- `docs/src/refactor/v3-roadmap.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`
- `docs/src/refactor/session-context-layering.md`

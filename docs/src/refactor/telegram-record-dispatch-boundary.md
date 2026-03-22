# Telegram `record` / `dispatch` Boundary

本文档用于收敛并冻结以下问题：

- TG 适配层与 gateway/core 的责任分工
- TG 适配层与 gateway/core 之间的最小协议界面
- `DM` / `Group` 下的交互方式
- `mention` / `reply-to` 的触发规则
- `record` / `dispatch` 的判定顺序
- TG 适配层的去重口径

本文档只讨论一件事：

- **TG 适配层先判定，再把结果以 `record` / `dispatch` 交给 gateway/core 执行**

本文档不讨论：

- 代码模块如何命名
- 现有函数具体迁到哪个文件
- 其他渠道的完整实现细节
- UI/配置页最终长什么样
- 编辑/删除/reaction/按钮点击等非消息型事件的完整产品策略

但有一条边界先冻结：

- 后续若要支持这些事件，也不得因此向 gateway/core 引入 `record` / `dispatch` 之外的新执行语义

## 一句话结论

TG 适配层与 gateway/core 之间，执行语义只保留两种：

- `record`
- `dispatch`

除此之外不再引入新的执行概念。

TG 适配层负责：

- 理解 Telegram 协议与群聊规则
- 把一条 Telegram 事件展开成 `0..N` 条面向 bot 视角的消息
- 给每条消息标明 `record` 或 `dispatch`
- 在交给 gateway/core 之前完成 **TG 侧去重**

gateway/core 负责：

- 按 `record` / `dispatch` 执行
- 写入会话/历史
- 触发 run（仅 `dispatch`）
- 承担系统级可观测性、失败处理和通用执行编排

一句话：

- **TG 适配层负责判断和展开，gateway/core 负责照章执行**

## 设计目标

这一轮要解决的问题只有两个：

- 不再让 gateway/core 理解 Telegram 群聊专属复杂性
- 不再让 `mirror` / `relay` / `listen-only` / `reply-to wakeup` 继续膨胀成 gateway/core 的独立执行概念

这一轮明确不做的事：

- 不把 Telegram 的私有规则提升成跨渠道公共业务概念
- 不把 gateway/core 改造成“理解 Telegram 群聊俚语”的地方

## 最小责任分工

### TG 适配层负责什么

TG 适配层负责判断“这条 Telegram 事件对哪些 bot 来说意味着什么”。

具体包括：

- 判断这是不是一个值得进入 bot 会话环境的 Telegram 事件
- 判断它对每个 bot 是 `record` 还是 `dispatch`
- 判断 `mention` / `reply-to` 是否构成明确指向
- 判断代码块、引用、空文本、系统事件、按钮点击、编辑事件等 Telegram 细节
- 维护 Telegram 自己的 `reply-to` / `thread` / `message_id` 事实
- 对最终要产出的 `record` / `dispatch` 做 TG 去重

TG 适配层不负责：

- 直接操作通用会话主链
- 直接承担系统级 run 执行器
- 直接把 Telegram 私有理由扩散成 gateway/core 公共概念

### gateway/core 负责什么

gateway/core 只负责执行，不负责理解 Telegram 群聊语义。

具体包括：

- 接收 TG 适配层产出的消息
- 对 `record`：写入会话/历史，但不触发 run
- 对 `dispatch`：写入会话/历史，并触发 run
- 维护通用执行生命周期、失败处理、日志、回执、状态流

gateway/core 不负责：

- 自己解析 Telegram 群聊里的 `mention`
- 自己判断 reply-to bot 是否应唤醒
- 自己决定 Telegram 群聊的多 bot 展开规则

## 最小协议界面

TG 适配层给 gateway/core 的每条消息，最小只需要表达：

- 这是哪一个 bot 视角下的一条消息
- 这条消息是 `record` 还是 `dispatch`
- 这条消息的正文/事实内容是什么
- 必要的渠道事实（例如 reply-to/thread/message_id）

这里最重要的不是字段名，而是语义收敛：

- **执行语义只保留 `record` / `dispatch`**
- `mirror` / `relay` / `listen-only` / `reply-to wakeup` 都不再是执行语义
- 它们如果还需要出现，也只能作为 TG 内部原因或日志原因存在

## 交给 gateway/core 的正文文本要求

这里要再冻结一条非常关键的边界：

- `gateway/core` 消费的是 TG adapter 已经整理好的最终 `text`，而不是 Telegram 原始字段拼装任务
- **群聊 `record` / `dispatch` 交给 gateway/core 的正文，必须严格遵守 TG-GST v1**
- **DM 不使用 TG-GST v1**，仍按 DM 自己的自然正文口径交给 gateway/core
- TG-GST v1 的落地与正确性，由 TG adapter 全权负责；gateway/core 不再二次改写 speaker / `-> you` / transcript 头

群聊最终文本口径：

- 形式是 `<speaker><addr_flag>: <body>`
- `speaker` 必须是发言人的用户可识别本体标识
- `addr_flag` 只有命中“这条 bot 视角消息明确指向你”时才能写成 ` -> you`
- `body` 是 TG adapter 清洗、裁剪、合并后的最终正文

关于 `speaker`，再明确两条：

- 应优先使用发言人的人类可读名称/身份标识，而不是内部渠道账号键、session key、`telegram:xxxx` 这类内部标识
- 换句话说，交给 gateway/core 的群聊正文里，不应把发言人写成 `telegram:123456789` 这种渠道账号名来污染模型上下文

关于 ` -> you`，再明确两条：

- 群聊里，只有当前这条 bot 视角消息确实明确指向该 bot 时，才允许出现 ` -> you`
- 同一条 TG 入站若被展开给多个 bot，其中只有被明确点名/明确 reply-to 的那个 bot 视角应带 ` -> you`；其他 bot 若只是环境 `record`，不得误写 ` -> you`

人话：

- TG adapter 不只是决定发不发 `record` / `dispatch`，还必须把群聊最终入模文本写对；尤其是 speaker 和 ` -> you` 不能错

## TG 的 name 体系与 speaker 规则

这里要把 TG 常见的几种 name 彻底拆开，否则实现时最容易把“匹配身份”和“显示给模型看”混成一团。

### TG 里常见的几种 name

对同一个 Telegram 发言者，系统里常见的名字至少有 5 类：

1. **本体名 / identity `display_name`**
   - 这是 link identity 命中的逻辑身份名
   - 例子：`风险助手`、`Alice Zhang`
   - **用途：**优先作为群聊 speaker 最终显示名

2. **稳定 ID / `telegram_user_id` / `chanUserId`**
   - 这是 Telegram `user.id` 这类稳定身份值
   - 例子：`1234567890`
   - **用途：**优先用于 link identity / sender 匹配
   - **禁止：**直接作为最终 speaker 文本给模型看

3. **`username` / `telegram_user_name`**
   - 这是 Telegram 的 `@risk_bot_cn` 这类用户名主体
   - 例子：`risk_bot_cn`、`alice_99`
   - **用途：**`@mention`、手工识别、匹配回退、显示回退
   - **问题：**可变、可能为空、常常机器味很重

4. **渠道展示名 / `telegram_display_name` / `sender_name`**
   - 这是 Telegram first/last/title 拼出的可读名字
   - 例子：`风险助手中文`、`Alice Zhang`
   - **用途：**在没命中本体名时，作为人类可读显示回退
   - **问题：**可变、可能重名、不可作为稳定匹配主键

5. **内部账号键 / account key / session key / binding key**
   - 例子：`telegram:1234567890`
   - **用途：**系统内部路由、绑定、存储
   - **禁止：**进入群聊 speaker / TG-GST 正文

### 一个 bot 到底有几个名字、几个账号

必须明确：

- 一个 **bot 本体** 可以对应多个渠道账号
- 一个 **TG 账号** 又可能同时拥有：稳定 ID、username、display name、内部 key
- 给模型看的 speaker，目标是“稳定表达这个发言者是谁”，不是把这些原始名字原样透传出去

例子：

- 本体名：`风险助手`
- TG 账号 A：
  - `telegram_user_id=1234567890`
  - `username=risk_bot_cn`
  - `telegram_display_name=风险助手中文`
  - `account_key=telegram:1234567890`
- TG 账号 B：
  - `telegram_user_id=2233445566`
  - `username=risk_helper_backup`
  - `telegram_display_name=风险助手中文备用`
  - `account_key=telegram:2233445566`

如果 A / B 都 link 到同一个本体，那么群聊 speaker 的首选都应回到本体名 `风险助手`，而不是把渠道账号名直接暴露给模型。

### speaker 解析分两步

TG adapter 对群聊 speaker 的处理，必须拆成两步：

1. **先识别发言者本体是谁**
2. **再决定最终怎么显示给 gateway/core**

这两步不能混。

### 第一步：speaker 本体匹配优先级

TG adapter 应按下面顺序做 sender / identity 解析：

1. 优先用稳定 `telegram_user_id` / `chanUserId` 命中 link identity
2. 如果没有稳定 ID 或当前事件拿不到，再退到 `username`
3. 如果仍未命中，则把它当成“未链接 Telegram 发言者”

这里必须强调：

- `display name` 只适合显示，不适合作为稳定匹配主键
- 内部 `account_key` / `session_key` 不参与 speaker 匹配口径

### 第二步：speaker 最终显示优先级

一旦 TG adapter 已经知道“这是谁”，群聊 TG-GST v1 的 speaker 应按下面顺序渲染：

1. **若命中 link identity：使用本体 `display_name`**
2. 否则，若有 `telegram_display_name`：使用它
3. 否则，若有 Telegram 原生 `sender_name`：使用它
4. 否则，若有 `username`：使用裸 `username`，不带 `@`
5. 最后才允许技术性兜底：
   - 人：`tg-user-<short_id>`
   - bot：`tg-bot-<short_id>`

硬要求：

- 不允许把 `telegram:1234567890` 这类内部 key 当 speaker
- 不允许把 `session_key` / `bucket_key` / binding 文本塞进 speaker
- 一旦命中 link identity，最终 speaker 应优先显示本体名，而不是继续显示某个渠道账号名

### `(bot)` 规则

`(bot)` 也必须收成硬规则，不要模糊处理：

- **仅在群聊 TG-GST v1 speaker 中，bot 发言统一追加 `(bot)`**
- 人类发言永远不带 `(bot)`
- DM 不使用 TG-GST v1，因此不适用这条 speaker 后缀规则

也就是说：

- 人：`Alice Zhang: hello`
- bot：`风险助手(bot): 已处理`

### ` -> you` 规则

` -> you` 表达的是“当前 bot 视角下，这条消息明确指向你”，而不是 speaker 的固有属性。

因此必须满足：

- 只有当前 bot 视角消息确实明确命中该 bot 时，才允许带 ` -> you`
- 同一条 TG 入站若被展开给多个 bot，只有命中目标 bot 的那条 `dispatch` 才带 ` -> you`
- 其他 bot 若只是环境 `record`，即使看到同一原始消息，也不得误写 ` -> you`

### 同名冲突时怎么办

如果两个不同 TG 发言者最终渲染出的 speaker 同名，TG adapter 必须做**最小可读消歧**，而不是退回内部 key。

建议顺序：

- 优先追加可读 `username`：如 `风险助手(bot)[risk_bot_cn]`
- 如果连 `username` 都没有，再追加技术兜底短 ID：如 `风险助手(bot)[2233445566]`

这里同样禁止：

- `风险助手(bot)[telegram:2233445566]`

### 功能实现归属

关于群聊正文转写、speaker 匹配与格式化，职责必须冻结为：

- **TG adapter 负责：**
  - 解析 Telegram 原始 sender 信息
  - 执行 link identity 匹配
  - 选择最终 speaker 显示名
  - 决定是否追加 `(bot)`
  - 决定是否追加 ` -> you`
  - 做必要的同名消歧
  - 产出最终 TG-GST v1 文本

- **gateway/core 负责：**
  - 只消费最终 `text`
  - 不再重跑 speaker 匹配
  - 不再把群聊文本重新格式化
  - 不再二次判断是否带 `(bot)` / ` -> you`

一句话：

- **TG 名字很脏，但脏东西必须在 TG adapter 内部消化完，不能扩散给 gateway/core**

### 示例

#### 示例 1：已命中本体 identity 的 bot

已知：

- 本体名：`风险助手`
- `telegram_user_id=1234567890`
- `username=risk_bot_cn`
- `telegram_display_name=风险助手中文`

群聊 speaker 应显示：

- `风险助手(bot): 已处理`

不应显示：

- `risk_bot_cn(bot): 已处理`
- `telegram:1234567890(bot): 已处理`

#### 示例 2：普通人类用户

已知：

- 未命中 identity link
- `telegram_display_name=Alice Zhang`
- `username=alice_99`

群聊 speaker 应显示：

- `Alice Zhang: 大家看下`

#### 示例 3：只有 username 的回退场景

已知：

- 未命中 identity link
- 无 display name
- `username=alice_99`

群聊 speaker 应显示：

- `alice_99: 大家看下`

#### 示例 4：只剩稳定 ID 的兜底场景

已知：

- 未命中 identity link
- 无 display name
- 无 username
- `telegram_user_id=987654321`

群聊 speaker 可兜底显示：

- `tg-user-987654321: 大家看下`

但不应显示：

- `telegram:987654321: 大家看下`

#### 示例 5：同一条消息展开给多个 bot

原始消息：

```text
@risk_bot 看下这个
```

给目标 bot 的正文：

- `Alice -> you: @risk_bot 看下这个`

给其他 bot 的环境记录：

- `Alice: @risk_bot 看下这个`

#### 示例 6：bot 回复 bot

若 `总结助手` reply `风险助手`，且当前视角 bot 是 `风险助手`：

- `总结助手(bot) -> you: 我补完结论了`

若当前视角 bot 不是 `风险助手`，只是旁观记录：

- `总结助手(bot): 我补完结论了`

#### 示例 7：同一本体绑定多个 TG 账号

已知：

- 两个 TG 账号都 link 到本体 `风险助手`
- 但它们在同一群都可能出现

默认 speaker 首选都应是：

- `风险助手(bot)`

若发生冲突，再做最小消歧，例如：

- `风险助手(bot)[risk_bot_cn]`
- `风险助手(bot)[risk_helper_backup]`

## 对不同类型渠道事件的支持

### `DM`

`DM` 下，TG 适配层通常只给 gateway/core 一条消息。

默认口径：

- `DM` 是 `1 -> 1`
- 一条 Telegram 入站，通常对应一条 `dispatch`

也就是说：

- 普通 DM 文本：`dispatch`
- 普通 DM 语音/图片/位置等：如果本次策略支持并能转成 bot 可理解的事实，仍然是 `dispatch`

只有极少数情况才会是 `0`：

- 重复 update
- 协议噪声
- 根本没有可保留内容的空事件

### `Group`

`Group` 下，TG 适配层可能给 gateway/core 产出 `0..N` 条消息。

这里的 `N` 不是“群里总 bot 数”，而是：

- 这条群聊事件最终需要影响多少个 bot 视角

典型情况：

- 普通群聊发言：可能给多个 bot 各产出一条 `record`
- 明确点名某个 bot：可能给被点名 bot 产出一条 `dispatch`，同时给其他 bot 产出若干条 `record`
- 某些纯协议/重复事件：可能是 `0`

所以 `Group` 的最小理解是：

- **一条群聊事件，经 TG 适配层展开后，变成若干条 bot 视角消息**
- **每条 bot 视角消息只有 `record` / `dispatch` 两种执行语义**
- **同一条 TG 入站、同一 bot，最终最多只允许有一条 bot 视角消息交给 gateway/core**
- 哪些 bot 需要接收该群的环境 `record`，由 TG 适配层自己的群成员/订阅/可见性策略决定，gateway/core 不介入

### 群环境 `record` 的最小受众原则

这一点只冻结最小原则，不展开成完整成员策略：

- 只有处于当前群有效参与集合内的 bot，才允许接收该群环境 `record`
- "有效参与集合" 由 TG 适配层根据当前群可见性、成员关系、订阅状态、接入权限决定
- 已退群、已被移出、当前群未启用、当前群未订阅、当前群不可见的 bot，不应继续收到该群的 `record`
- TG 适配层不得为“不确定是否属于当前群有效参与集合”的 bot 猜测性地产出 `record`
- gateway/core 不参与这层筛选，也不负责事后补救或回填

人话：

- 先由 TG 适配层决定“这个 bot 现在算不算这个群的有效参与者”，只有算的才配看到该群上下文

## `record` / `dispatch` 的语义

### `record`

`record` 的含义是：

- 写入 bot 的会话/历史
- 进入 LLM 后续可见上下文
- 但这次不触发 run

这里必须强调：

- **`record` 不是“降级处理”**
- **`record` 不是“没命中就随便记一下”**
- **`record` 会影响 LLM 对群聊环境的理解，因此必须谨慎**

### `dispatch`

`dispatch` 的含义是：

- 写入 bot 的会话/历史
- 并触发本次 run

也就是说：

- `dispatch` 一定包含“可记录”
- 但 `record` 不一定包含“要立即行动”

## `mention` / `reply-to` 规则

这一轮只保留两个触发开关：

- `dispatch_on_line_start_mention`
- `dispatch_on_reply_to_bot`

二者都统一覆盖两种来源：

- 人 -> bot
- bot -> bot

### `dispatch_on_line_start_mention`

含义：

- 当消息中的 **行首 mention** 命中时，是否把该 bot 视为明确被指向，并产出 `dispatch`

“行首 mention” 的业务口径：

- 行去掉前导空白后，首个有效 token 是一个或多个连续的 bot `@mention`
- 如果行首连续出现多个 `@bot`，则这些 bot 共享其后的同一段任务文本
- 这意味着行首点名不只支持单 bot，也支持多 bot 同时点名

例子：

```text
@bot_a 帮我整理今天的结论
```

```text
@bot_a @bot_b @bot_c 请大家都处理下
```

```text
@bot_a 先补风险

@bot_b 再补行动项
```

如果开关开启：

- 每个命中的 bot 各自产出一条 `dispatch`
- 如果是连续多个行首 `@bot`，则它们共享同一段任务文本

如果开关关闭：

- 不因为行首 mention 触发 `dispatch`
- 但整条消息是否仍应 `record`，要独立判断

### `dispatch_on_reply_to_bot`

含义：

- 当消息是 **reply-to 某个 bot 的消息** 时，是否把该 bot 视为明确被指向，并产出 `dispatch`

这里同样覆盖：

- 人 reply bot
- bot reply bot

例子：

```text
[reply to bot_a] 继续
```

```text
[reply to bot_a] 已处理完，请确认
```

如果开关开启：

- reply 的目标 bot 产出 `dispatch`

如果开关关闭：

- 不因为 reply-to bot 触发 `dispatch`
- 但整条消息是否仍应 `record`，仍独立判断

## 为什么 `reply-to` 应独立成开关

`reply-to` 与 `mention` 都属于“明确指向某个 bot”的信号。

但它们的交互习惯不同：

- 有些用户/群更习惯显式 `@bot`
- 有些用户/群更习惯直接在线程里 reply

把二者拆成两个开关的好处是：

- 触发面可控
- 人 -> bot 与 bot -> bot 口径统一
- 不再强迫 bot 在已经 threaded reply 的情况下还额外重复打一遍 `@bot`

## 同一 bot 的最终产出上限

这是本方案的另一条硬约束：

- 对同一条 TG 入站、同一 bot，TG 适配层最终只能交给 gateway/core `0` 或 `1` 条消息
- 不允许同一 bot 同时出现多条 `record`
- 不允许同一 bot 同时出现多条 `dispatch`
- 不允许同一 bot 同时出现一条 `record` 和一条 `dispatch`
- 如果同一 bot 在同一条消息里命中多个有效片段，TG 适配层必须先按原文顺序合并，再决定最终是 `record` 还是 `dispatch`
- 若最终需要行动，则只交一条 `dispatch`；否则只交一条 `record`

人话：

- gateway/core 面前看到的，是每个 bot 已经收口后的最终单条结果，而不是 Telegram 原始碎片

## TG 侧去重原则

这是本方案的硬要求：

- **TG 适配层无论产出 `record` 还是 `dispatch`，都必须先经过 TG 去重**

也就是说：

- 去重不是只防 `dispatch` 重复触发
- 去重也要防 `record` 重复写入，避免污染 bot 历史上下文

TG 去重的职责边界：

- TG 适配层负责根据 Telegram 自己的事实做去重
- gateway/core 不再理解 Telegram 的去重规则细节

人话：

- 对 Telegram 来说重复的，就不要再交给 gateway/core
- 不管它最终是 `record` 还是 `dispatch`

### TG 去重至少要覆盖什么

- 重复 update
- 同一 TG 事件被重放
- 同一 bot 的同一 `record` 事实被重复产出时，只能保留一条 `record`
- 同一 bot 的同一 `dispatch` 事实被重复产出时，只能保留一条 `dispatch`
- 同一条消息里，同一个 bot 同时命中多个 `dispatch` 触发条件时，只能保留一条 `dispatch` 候选
- 具体到当前口径：同一 bot 若同时命中 `dispatch_on_line_start_mention` 和 `dispatch_on_reply_to_bot`，最终也只能 `dispatch` 一次
- 同一 bot 的同一事实若同时产出 `record` 与 `dispatch`，最终只保留 `dispatch`

## `record` 的独立判定原则

这里是本方案最需要谨慎的地方。

必须明确：

- **是否 `dispatch`**
- **是否 `record`**

是两套独立判断。

不能因为“没有触发 `dispatch`”就直接丢。

### 原则 1：`dispatch` 看“是否明确指向 bot”

也就是：

- 这条消息是否在叫某个 bot 立即行动

### 原则 2：`record` 看“这是不是值得进入 bot 群聊环境理解的事实”

也就是：

- 这条消息是否构成群聊环境中的有效上下文

### 原则 3：清洗主要用于触发判定，不能粗暴决定是否 `record`

例如：

- 引用里的 `@bot_a` 不应因为 `mention` 命中而直接触发 `dispatch`
- 代码块里的 `@bot_a` 也不应直接触发 `dispatch`

但这并不自动意味着整条消息不能 `record`。

如果这条消息整体仍然构成有价值的群聊上下文，它仍然应当 `record`。

换句话说：

- **“清洗后不触发” 不等于 “不记录”**

## 建议判定表

### 一类：应 `dispatch`

满足已启用的明确指向规则：

- 命中行首 mention，且 `dispatch_on_line_start_mention=true`
- 命中 reply-to bot，且 `dispatch_on_reply_to_bot=true`

例子：

```text
@bot_a 帮我整理今天结论
```

```text
[reply to bot_a] 继续
```

### 二类：应 `record`

只要属于 bot 理解群聊环境所需的有效事实，就应 `record`。

建议默认包括：

- 普通讨论消息
- 未触发 `dispatch` 的讨论性 mention
- 引用/转述/代码/日志，只要整体仍构成讨论上下文
- 群环境状态变化

例如以下都应倾向 `record`：

```text
今天先整理风险，晚点再看行动项
```

```text
我昨天看到 @bot_a 提过这个问题
```

```text
> @bot_a 上次说这里有问题
我觉得这次还是要重看
```

以及群环境事实：

- 某人进群
- 某人退群
- 某 bot 被拉进群
- 某 bot 被移出群
- 群标题变更
- 其他会影响 bot 理解当前群环境的状态变化

### 三类：应 `0`

只有在“既不值得 `dispatch`，也不值得 `record`”时，才应产出 `0`。

这类应尽量少。

建议只留给：

- TG 去重命中的重复事件
- 明确命中的硬拒绝事件
- 纯协议噪声
- 纯内部控件/回执事件
- 完全没有环境价值的空事件

例子：

- 同一 Telegram update 重复到达
- chat / sender 不在允许范围
- typing / read receipt
- 纯 callback ack
- 空白、无内容、无状态变化、无可保留事实的空事件

## TG 适配层判定顺序

建议顺序如下：

1. **先识别 Telegram 物理事实**
   - chat type
   - sender
   - message_id
   - reply_to
   - entities / mentions
   - thread / topic
   - system event / text / media

2. **先做 TG 去重**
   - 如果这条 Telegram 事件对 TG 来说已处理过，则直接结束

3. **判断这条事件是否构成可记录的环境事实**
   - 如果有环境价值，则准备一个或多个 `record` 候选
   - 同一 bot 的同一 `record` 事实若被多次产出，必须在 TG 侧先合并去重
   - 如果没有环境价值，则不产出 `record`

4. **判断这条事件是否命中明确指向规则**
   - 行首 mention
   - reply-to bot
   - 只看已启用开关

5. **对命中的 bot 视角，把对应候选升级为 `dispatch`**
   - 未命中的 bot 视角保持 `record`
   - 同一 bot 若同时命中多个 `dispatch` 触发条件，必须在 TG 侧先合并去重，最终只保留一次 `dispatch`
   - 同一 bot 的同一事实若此前已有 `record` 候选、此处又命中 `dispatch`，则必须收口为一条 `dispatch`，不得同时保留 `record` 与 `dispatch`

6. **把最终 `record` / `dispatch` 列表交给 gateway/core**

人话理解：

- 先判断“这条消息值不值得记住”
- 再判断“这条消息要不要叫谁行动”
- 最后只把去重后的结果交给 gateway/core

## 硬拒绝与可观测性

为了避免 silent drop / silent degrade，这里再冻结一条口径：

- TG 适配层可以在进入 `record` / `dispatch` 判定前，先执行明确的硬拒绝规则
- 一旦命中硬拒绝，直接产出 `0`，不得再继续向 gateway/core 交消息
- 硬拒绝、TG 去重命中、`dispatch` 被开关关闭拦下、`record` 与 `dispatch` 的合并收口，都必须可观测

最小可观测性要求：

- 结构化日志至少带 `event`、`reason_code`、`decision`、`policy`
- 上下文允许时，再补 `session_key`、`channel_type`、`chat_id`、`bot_id`、`message_id`、`remediation`
- 不打印敏感正文；必要时只保留短预览或哈希
- TG 适配层负责产出明确的 decision / reason_code / policy；gateway/core 负责统一承接和记录
- 若事件在交给 gateway/core 之前就被收口为 `0`，TG 适配层必须自行留下同样结构化日志；不能因为没有 handoff 就变成无日志

最小 `reason_code` 集合也应先冻结，避免各写各的：

- `tg_dedup_hit`：命中 TG 去重
- `tg_hard_reject_access`：chat / sender / bot 访问范围不允许
- `tg_hard_reject_loop`：命中自激或桥接回环防护
- `tg_hard_reject_invalid_payload`：载荷损坏，无法可靠判定 sender / target / reply_to
- `tg_record_context`：仅产出环境 `record`
- `tg_dispatch_line_start_mention`：因行首 mention 产出 `dispatch`
- `tg_dispatch_reply_to_bot`：因 reply-to bot 产出 `dispatch`
- `tg_dispatch_promoted_from_record`：原本是 `record` 候选，后续升级收口为 `dispatch`
- `tg_dispatch_blocked_by_policy`：原本命中指向信号，但被开关或策略拦下
- `tg_noise_drop`：纯噪声或空事件，最终产出 `0`

硬拒绝的典型例子：

- 当前 chat / sender 不在允许范围
- 目标 bot 不在当前群有效参与集合内
- 该事件若继续处理会形成 bot 自激或桥接回环
- 事件载荷损坏，无法可靠确定 sender / target / reply_to

## 示例

### 示例 1：DM

```text
用户：帮我总结今天结论
```

结果：

- 产出 1 条消息
- 模式：`dispatch`

### 示例 2：普通群聊讨论

```text
今天先整理风险，晚点再看行动项
```

结果：

- 对当前群有效参与集合内、需要感知群环境的 bot 产出若干条 `record`
- 不产出 `dispatch`

### 示例 3：群聊行首 mention

```text
@bot_a 帮我整理今天结论
```

当 `dispatch_on_line_start_mention=true`：

- `bot_a`：`dispatch`
- 其他 bot：是否 `record`，由环境记录规则决定

如果是行首连续多 bot：

```text
@bot_a @bot_b @bot_c 请大家都处理下
```

结果：

- `bot_a`：`dispatch`
- `bot_b`：`dispatch`
- `bot_c`：`dispatch`
- 三者共享同一段任务文本“请大家都处理下”
- 其他 bot：是否 `record`，由环境记录规则决定

### 示例 4：群聊 reply-to bot

```text
[reply to bot_a] 继续
```

当 `dispatch_on_reply_to_bot=true`：

- `bot_a`：`dispatch`
- 其他 bot：是否 `record`，由环境记录规则决定

### 示例 5：同一 bot 同时命中 reply-to 与 mention

```text
[reply to bot_a] @bot_a 请继续补风险
```

结果：

- `bot_a` 最终只产出 1 条 `dispatch`
- 不得同时再保留 1 条 `record`

### 示例 6：同一 bot 在同一帖里被点名两次

```text
@bot_a 先整理风险

@bot_a 再补行动项
```

结果建议：

- `bot_a` 最终只产出 1 条 `dispatch`
- 两段有效任务文本按原文顺序合并到同一条 bot 视角消息

### 示例 7：引用里的 mention

```text
> @bot_a 上次说这里有问题
我觉得这次还是要重看
```

结果建议：

- 不因引用里的 mention 直接触发 `dispatch`
- 但整条消息应 `record`

### 示例 8：重复 update

同一条 Telegram update 被重放两次。

结果：

- 第一次：按正常规则产出
- 第二次：TG 去重命中，产出 `0`

## 最小测试面

本方案的自动化测试应保持精简，但至少覆盖：

- `DM` 普通消息：产出 1 条 `dispatch`，且不包 TG-GST v1 transcript 头
- `Group` 普通讨论：仅对当前群有效参与集合内的 bot 产出 `record`，且正文必须是 TG-GST v1、无误加的 ` -> you`
- 群聊 speaker：命中 link identity 时优先显示本体 `display_name`
- 群聊 speaker：未命中本体时，按 `telegram_display_name` → `sender_name` → `username` → `tg-user/tg-bot-<short_id>` 回退
- 群聊 bot speaker：统一带 `(bot)`；人类 speaker 永远不带
- 同一条消息若只对目标 bot 明确指向：只有目标 bot 视角正文带 ` -> you`；其他 bot 视角不得误带
- 群聊 speaker：禁止出现 `telegram:xxxx` / `session_key` / 其他内部 key
- 同名 speaker：必须做可读消歧，且不得退回内部 key
- bot 已退群 / 被移出 / 未订阅当前群：不产出该群 `record`
- 连续行首多 bot mention：每个 bot 各 1 条 `dispatch`
- 行首 mention / reply-to 开关关闭时：不 `dispatch`，但有环境价值的消息仍可 `record`
- 同一 bot 同时命中 mention + reply-to：最终只保留 1 条 `dispatch`
- 同一 bot 在同一消息里被多次点名：最终只保留 1 条 `dispatch`，且正文按原文顺序合并
- 引用/代码块里的 mention：不触发 `dispatch`，但有效讨论仍 `record`
- 重复 update：产出 `0`
- 硬拒绝：产出 `0`，并留下 `reason_code`

## 对其他渠道的影响

这一套收敛首先是为 Telegram 服务，但它没有把 Telegram 的私有规则抬成 gateway/core 的公共概念。

因此后续其他渠道如果也需要类似模式，可以复用的是：

- TG/渠道适配层产出若干条 bot 视角消息
- 每条消息只标 `record` / `dispatch`
- gateway/core 只执行

而不需要复用的是：

- Telegram 私有的 `mention`、`reply-to`、`message_id`、thread 规则

一句话：

- **可复用的是执行壳，不是 Telegram 私有语义本身**

## 当前冻结口径

本轮先冻结为：

- TG 适配层与 gateway/core 之间只保留 `record` / `dispatch`
- TG 适配层负责 DM / Group 的 bot 视角展开
- Group 交给 gateway/core 的最终正文必须严格使用 TG-GST v1；DM 不使用 TG-GST v1
- TG adapter 负责 sender identity 匹配、speaker 渲染、`(bot)` / ` -> you` / 消歧；gateway/core 不重做
- TG 适配层负责两类触发开关：
  - `dispatch_on_line_start_mention`
  - `dispatch_on_reply_to_bot`
- TG 适配层对 `record` / `dispatch` 一律先做 TG 去重
- TG adapter 负责把群聊 speaker 写成发言人的可识别本体标识，不得把 `telegram:xxxx` 这类内部账号键直接写进 gateway 正文
- speaker 本体匹配优先走稳定 `telegram_user_id` / `chanUserId`，再退到 `username`
- 群聊 speaker 命中 identity link 后，优先显示本体 `display_name`
- 群聊 bot speaker 统一带 `(bot)`；人类 speaker 永远不带
- 只有明确指向当前 bot 的群聊 bot 视角消息，才允许在 TG-GST v1 里带 ` -> you`
- speaker 同名时必须做可读消歧，且不得退回内部 key
- 群环境 `record` 只发给当前群有效参与集合内的 bot
- `record` 与 `dispatch` 独立判断
- “清洗后不触发” 不等于 “不记录”
- 同一条 TG 入站、同一 bot，最终最多只交 1 条消息
- 同一 bot 的多段有效任务文本必须先按原文顺序合并，再决定最终单条输出
- 命中硬拒绝时直接产出 `0`
- 去重/拒绝/开关拦截/收口合并都必须可观测
- 最小 `reason_code` 集合先冻结，禁止各写各的
- `0` 只留给重复、硬拒绝和纯噪声

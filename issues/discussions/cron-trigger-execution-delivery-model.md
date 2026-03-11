# Cron 的 trigger / execution / delivery 概念模型（讨论稿）

> 讨论时间戳：2026-03-10T23:35:00+08:00
>
> 目标：把 cron 这件事压缩到最小、最稳的一套概念里，避免继续把“什么时候触发”“在哪儿运行”“结果发到哪儿”混成一团。
>
> 范围：本文只讨论 cron 的概念模型与职责边界，不展开传输重试、Web UI 具体展示细节。
>
> 口径：本文会明确区分：
> - **as-is**：当前代码/现状的问题
> - **to-be**：建议收敛后的设计口径

---

## 1. 核心判断

这份稿子最该先冻结的结论，其实只有 5 句：

1. **cron 是一个一等工具（first-class tool）**
   - 创建、查看、修改、删除、立即执行，都是这个工具的操作。
   - Web UI 和 Telegram / 聊天，只是这个工具的两个前端，不应各自长出一套 cron 逻辑。

2. **cron 必须拆成三层**
   - `trigger`：什么时候到点
   - `execution`：到点后在哪儿跑
   - `delivery`：跑完之后发给谁

3. **execution 必须统一**
   - 所有 job 一律进自己的 `cron:<job_id>` session 跑。
   - 不再把 cron execution 搞成多条岔路。

4. **delivery 要收敛**
   - 系统只负责确定“如果要发，发给谁”。
   - 这一轮到底发不发，由 LLM 本轮输出决定：
     - 有可见文本：发
     - 输出 `silence`：不发

5. **LLM 看到的工具面必须很小**
   - 后端内部可以有完整的 cron service / API。
   - 但直接暴露给 LLM 的接口必须收敛，否则 schema 太大、太费 token、还会把实现细节泄露给模型。

换句话说，cron 核心不该是“先从一句话里抽很多显示字段”，而应该是：

> **先有统一的 `cron` 工具契约，再让聊天/Telegram/Web UI 去调用它。**

但这里还要再补一句：

> **统一工具契约** 不等于 **把完整后端 JSON 结构原封不动暴露给 LLM**。

更合理的分层是：
- **Cron Core API**：给服务端 / Web UI / 存储层使用，可以完整
- **LLM-facing Cron Skill / Thin Tool**：给模型使用，必须极小化

---

## 2. 先把对象收敛：只保留 3 个核心对象

### 2.1 `CronJob`

表示“这条定时任务本身”，是持久化规格。

建议最小字段：
- `job_id`
- `name`
- `schedule`
- `task_text`
- `delivery_target`
- `enabled`

人话理解：
- 它就是“这条任务的长期档案”
- 后面每次到点，都是拿这份档案来执行

### 2.2 `CronRun`

表示“某一次真的跑了的记录”。

建议最小字段：
- `job_id`
- `trigger_time`
- `run_status`
- `delivery_status`

人话理解：
- `CronJob` 是任务档案
- `CronRun` 是每次执行回执

### 2.3 `CronSession`

表示这个 job 自己的执行上下文，会话 key 固定为：

```text
cron:<job_id>
```

它只服务于 execution，不承担创建/修改/删除入口职责。

人话理解：
- 它不是提醒渠道
- 它也不是 UI 页面
- 它只是“这个 job 自己的长期记忆”

---

## 3. 三层模型：trigger / execution / delivery 必须强行分开

### 3.1 Trigger：什么时候到点

trigger 只回答一个问题：

> 这条 job 什么时候被调度器判定为“该执行了”？

它只关心时间，不关心 LLM，不关心 Telegram，也不关心结果发给谁。

例子：
- `每天 12:00 Asia/Shanghai`
- `每 1 小时`
- `2026-03-11 09:30 Asia/Shanghai 只执行一次`

人话理解：
- trigger 只是“闹钟响了”
- 还没开始真正干活

### 3.2 Execution：到点后在哪儿跑

execution 只回答一个问题：

> 到点之后，这条 job 在什么上下文里运行？

建议这里直接收敛成唯一口径：

- 所有 job 都进入自己的 `cron:<job_id>` session
- 不再引入第二条 execution 路径
- 当前阶段不搞 `no-llm` 分流

并冻结下面几条规则：

1. **每个 job 一个独立 session**
   - 例子：吃饭提醒是 `cron:job_A`
   - 服务巡检是 `cron:job_B`
   - 两者上下文绝不串线

2. **同一个 job 不并发重入**
   - 若上一轮还没跑完，下一轮到了：
   - 直接 `skip + log`
   - 不允许同一个 job 叠两轮一起跑

3. **不同 job 可以并发**
   - `job_A` 在跑，不妨碍 `job_B` 同时跑

4. **session 持久化且复用**
   - `cron:<job_id>` 不是临时 buffer
   - 建议在 job 创建成功时就建档
   - 要落盘
   - 每次执行都复用同一个 session

5. **上下文压缩不另起一套**
   - 直接复用普通 session 的 `autocompact`
   - 不再为 cron 单独发明 compact 规则

6. **job 删除时，session 一并删除**
   - 生命周期一起管理
   - 初期不实现“删除前导出”

人话理解：
- execution 解决的是“在哪儿跑”
- 它不解决“最后发给谁”

补一条：
- **一次性任务和周期任务，不是两套 execution 机制**
- 它们只是在 `schedule` 上不同
- 一次性任务跑完后进入 `completed`
- 周期任务跑完后仍保持 `enabled/active`

### 3.3 Delivery：跑完之后发给谁

delivery 只回答一个问题：

> 这轮执行如果要对外发消息，应该发给谁？

这里建议也收敛，不做多余抽象：

1. `delivery_target` 是 `CronJob` 的持久化字段
2. 用户若未显式指定，则默认继承**创建来源**
   - 在 Telegram 里创建，就默认回到那个 Telegram chat
   - 在 Web UI 里创建，就默认回到对应的 Web 来源
3. 系统只负责记住 target
4. 本轮到底发不发，由 LLM 输出决定

即：
- 输出可见文本 -> 投递到 `delivery_target`
- 输出 `silence` -> 本轮不投递

这里**不要**再额外发明一个 `delivery_policy = always | on_error | silent` 之类的系统枚举。

因为用户真正想要的是：
- 用任务正文表达业务意图
- 由模型结合这轮上下文自己判断要不要发

而不是系统再偷偷替他做一层任务分类。

### 3.4 一个关键边界：系统不做“提醒类/巡检类/告警类”分类

这点必须明确写死。

系统不负责判断：
- 这是提醒任务
- 那是巡检任务
- 另一个是告警任务

系统只负责：
- 到点
- 拉起执行
- 带上上下文
- 记住 delivery target
- 处理本轮产物

至于这一轮该不该发消息：
- 由 LLM 基于 `task_text + trigger_time + session context` 自己判断

这样概念最小，也最不容易越写越歪。

---

## 4. `cron` 工具接口：核心接口可以完整，LLM 接口必须收敛

### 4.1 先区分两层接口

这块一定要分开看，否则会马上走向“大而全 JSON schema”：

1. **Cron Core API（后端/前端完整接口）**
   - 给 Web UI、服务端、存储层使用
   - 可以完整支持：
     - `create`
     - `list`
     - `get`
     - `update`
     - `delete`
     - `pause`
     - `resume`
     - `run_now`
     - `runs`
     - `view_session`

2. **LLM-facing Cron Skill / Thin Tool（模型侧极简接口）**
   - 给模型用
   - 必须尽量小
   - 推荐只保留高频、直接、必要的几件事：
     - `create`
     - `list`
     - `update`
     - `delete`
     - `run_now`

这里的关键点不是“接口多不多”，而是职责边界：

> **创建、修改、删除，本质上都是同一个工具的确定性操作。**

但这不代表要把后端完整结构原封不动塞给 LLM。

不应该再出现两种坏味道：
- 一套“创建字段抽取”
- 一套“删除意图抽取”
- 一套“修改意图抽取”
- 再加一套“超大 JSON schema 强塞给模型”

那样只会重新把概念搞散。

### 4.2 我更推荐：对 LLM 用 skill 或薄封装，而不是直接暴露大 schema

当前代码里的 cron tool 暴露了太多实现细节，例如：
- `payload.kind = systemEvent | agentTurn`
- `sessionTarget = main | isolated`
- `deliver / channel / to`

这类字段有两个明显问题：

1. **token 很贵**
   - schema 大
   - 说明长
   - 模型每次都得理解一堆其实不该它关心的内部实现

2. **语义泄露过深**
   - 模型被迫理解很多实现细节
   - 但这些细节未来还可能改

甚至当前代码现状里，`deliver / channel / to` 这类字段在 agent turn 路径上并没有形成完整闭环；这就更说明：

> 不该让模型去填一堆当前并不稳定、甚至未真正落地的实现字段。

所以这里的建议是：

- **后端保留完整 cron core API**
- **模型侧收敛成 skill 或薄工具**

如果系统支持 skill，我更倾向于：
- 用 **cron skill** 教模型“什么时候该 create/list/update/delete”
- 底下只调用一个很薄的 cron tool

如果暂时不做 skill，也至少要把 LLM-facing tool 缩到最小。

### 4.3 Web UI 与聊天的职责边界

#### Web UI

- 直接调用 `cron` 工具
- 用结构化表单创建/修改/删除 job
- 不依赖 LLM 做字段抽取

#### 聊天 / Telegram

- 用户说自然语言
- LLM 把意图翻译成 `cron` 工具调用
- 真正生效的仍是底层统一的 `cron` 工具

所以：
- Web UI 不是一套 cron
- Telegram 也不是一套 cron
- 它们只是不同入口

### 4.4 一个删除例子

用户在聊天里说：

```text
删掉中午吃饭提醒
```

合理流程应当是：
1. LLM 调 `cron.list`
2. 找到候选 job
3. 若唯一匹配，直接 `cron.delete(job_id)`
4. 若有多个相似项，再追问用户

这就够了。

不需要额外设计一套“删除专用字段协议”。

补充：
- 既然保留了 `CronSession` 这个概念，UI 最终至少要能从 job 详情进入 `view_session`
- 否则这个 session 会变成黑盒，不利于排障

---

## 5. LLM-facing create 到底填哪些字段，以及这些字段从哪来

这个点必须说清楚。

如果字段来源不清楚，后面就一定会出现：
- 模型乱填
- 前端乱补
- 后端乱猜

### 5.1 先定一条总原则

> **凡是运行时天然已经知道的上下文字段，不要让 LLM 再手填一遍。**

尤其是这类 opaque ID：
- Telegram `chat_id`
- 发送用的 bot/account handle
- Web UI 内部通知 target id
- `job_id`
- `cron:<job_id>` session id

这些都不该成为模型日常要自己编的字段。

这里还要特别防止一个混淆：

> **execution 的 session id** 和 **delivery 的目标标识** 不是一回事。

- `cron:<job_id>` 是执行上下文 id
- Telegram 投递目标不是 session id，而是：
  - `account_handle`
  - `chat_id`
  - `thread_id`（若有）

### 5.2 推荐的 LLM-facing create 最小字段

如果给 LLM 一个极简 create 接口，我建议只保留：

1. `when`
   - 含义：什么时候触发
   - 来源：
     - 聊天场景：由 LLM 从用户时间表达里提取
     - Web UI：由表单控件直接提供

2. `task`
   - 含义：到点时要模型做什么
   - 来源：
     - 聊天场景：由 LLM 从用户原话整理成执行文本
     - Web UI：由用户直接填写

3. `target`（可选）
   - 含义：如果这轮要发，发给谁
   - 默认值：`source`
   - 只有用户明确指定“发到别处”时才需要显式填写

4. `name`（可选）
   - 含义：人类可读标题
   - 默认做法：可省略；后端自动生成一个简短标题

除此之外，不建议把更多字段直接暴露给模型。

### 5.3 这些字段具体怎么获得

#### `when`

例子：

```text
明天中午 12 点提醒我吃饭
```

这里的 `when` 来自“明天中午 12 点”。

转换后可落为：
- `at`
- 或内部标准化后的 `schedule`

这个是聊天前端的理解工作，不是 cron 核心模型本身的职责。

#### `task`

同一句话里：

```text
明天中午 12 点提醒我吃饭
```

这里的 `task` 可以整理成：

```text
到点时提醒我去吃饭
```

也就是说：
- `task` 不是原句照抄
- 而是“到点后真正喂给执行层的任务正文”

#### `target`

这里最关键。

默认不要让 LLM 去手写 raw target id。

更合理的口径是：

- 若 `target` 省略，则表示 `source`
- `source` 由运行时调用上下文自动绑定

也就是说，模型只需要说“默认回原处”，不需要知道原处的内部 id 到底是多少。

### 5.4 Telegram 场景：`source` 到底从哪里来

如果 job 是在 Telegram 里创建的，那么运行时其实天然已经知道：

- 当前是 Telegram 来源
- 当前 bot/account 是谁
- 当前 chat id 是谁
- 如果后面支持 topic / thread，也能知道 thread id

所以默认 delivery target 应由运行时直接绑定，例如：

```text
{
  channel: "telegram",
  account_handle: "<当前接收消息的 bot account>",
  chat_id: "<当前 telegram chat id>",
  thread_id: "<可选，若存在>"
}
```

关键点是：
- 这些值来自当前 inbound update / 当前调用上下文
- 不是让 LLM 自己瞎填

### 5.5 Web UI 场景：`source` 从哪里来

如果 job 是在 Web UI 创建的，也一样。

运行时天然已经知道：
- 当前请求来自 Web UI
- 当前操作用户是谁
- 当前默认回投的 UI 目标是什么

所以默认 delivery target 应由 Web UI / 服务端上下文自动绑定，而不是让模型自己编一个“session id”。

这里最终到底落成什么内部 id，可以后面再定，但原则必须先冻结：

> **默认 target 的内部标识，应由调用上下文注入，不应由模型手填。**

### 5.6 `name`、`job_id`、`cron session id` 怎么来

这 3 个最不应该让 LLM 花 token。

#### `name`

- 可选
- 若用户没显式命名，后端自动生成简短标题即可
- 例子：`吃饭提醒（北京时间 12:00）`

#### `job_id`

- 后端生成
- 模型不负责

#### `cron session id`

- 由后端根据 `job_id` 派生：

```text
cron:<job_id>
```

- 模型不负责

### 5.7 第一阶段建议：显式 target 先收敛

为了避免工具面继续膨胀，第一阶段建议只支持两种 target 来源：

1. `source`
   - 默认值
   - 回到创建来源

2. `explicit target selected by UI / upper layer`
   - 比如 Web UI 下拉选中的目标
   - 或上层系统已经解析好的明确目标

先不要让模型在第一阶段自由拼装：
- 任意 Telegram chat id
- 任意 account handle
- 任意跨渠道地址

否则 token、复杂度、失败率都会一起上去。

### 5.8 非 create 操作的字段来源也要简单

除了 create，其他几个高频操作也不要让模型背太多字段。

#### `list`

- 通常不需要额外字段
- 最多加一个很轻的过滤条件

#### `update`

建议最小输入是：
- `job_selector`
- `patch`

其中：
- `job_selector` 先通过 `list` 命中目标
- `patch` 只包含真的要改的字段，例如：
  - 改时间 -> 只改 `when`
  - 改任务 -> 只改 `task`
  - 改投递目标 -> 只改 `target`

不要让模型每次 update 都重交整份 job。

#### `delete`

建议最小输入是：
- `job_selector`

流程是：
- 先 `list`
- 再删除命中的 job

#### `run_now`

建议最小输入是：
- `job_selector`

不需要别的附加字段。

这套做法的核心是：

> **模型只负责“选哪条、改什么”，不负责重新装配完整 job 结构。**

---

## 6. 执行阶段怎么喂给 LLM：只读已保存的 job，不再二次解释

一旦 job 创建完，后续每次到点执行时，执行层只做一件事：

> **读取已保存的 `CronJob` 规格 + 当前触发信息 + `cron:<job_id>` session 上下文，然后跑这一轮。**

执行阶段不再重新做“创建意图解析”或“字段抽取”。

### 6.1 本轮执行真正需要的输入

建议固定为：

1. `task_text`
   - 这条 job 的业务目标

2. `trigger_time`
   - 告诉模型这是哪一轮

3. `delivery_target_desc`
   - 如果这轮要发，结果会发给谁

4. `cron session context`
   - 这个 job 的历史执行上下文

### 6.2 建议的系统 prompt 外壳方向

这里不需要系统先替任务分类，但需要一层稳定外壳，至少说明：

- 你正在执行哪一条 cron job
- 当前触发时间是什么
- 结果若需要外发，会发给谁
- 若本轮无须对外发消息，请输出 `silence`
- 若本轮需要对外发消息，请直接输出要发送的可见文本

建议外壳保持短、硬、稳定，业务意图全部放在 `task_text` 里。

也就是说：
- 系统模板只负责执行协议
- job 文本只负责业务内容

而不是让系统模板自己替用户发明一堆业务字段。

---

## 7. 三个完整例子

### 7.1 例子 A：一次性提醒

用户在 Telegram 里说：

```text
明天中午 12 点提醒我吃饭
```

更合理的内部模型是：
- `trigger`：`2026-03-11 12:00 Asia/Shanghai`
- `execution`：`cron:<job_id>`
- `delivery_target`：创建来源对应的这个 Telegram chat
- `task_text`：到点时提醒我去吃饭

到点后：
1. 调度器命中
2. 在 `cron:<job_id>` session 跑一轮
3. 模型输出：`到 12:00 了，去吃饭。`
4. 系统把这条文本发回原 Telegram chat

### 7.2 例子 B：周期巡检

用户说：

```text
每小时检查一次服务状态，只有异常时再提醒我
```

更合理的内部模型是：
- `trigger`：every 1 hour
- `execution`：`cron:<job_id>`
- `delivery_target`：创建来源
- `task_text`：每小时检查服务状态；正常则 `silence`，异常则发摘要

第 1 次执行，服务正常：
- LLM 输出 `silence`
- 系统不对外发消息
- 但 run 记录必须写明：`suppressed_silence`

第 2 次执行，服务异常：
- LLM 输出异常摘要
- 系统把摘要投递到原来源
- run 记录写明：`delivered`

### 7.3 例子 C：修改 / 删除不需要另一套机制

用户说：

```text
把吃饭提醒改成每天 12:30
```

合理流程：
1. `cron.list`
2. 找到目标 job
3. `cron.update(job_id, schedule=每天 12:30)`

用户说：

```text
删掉吃饭提醒
```

合理流程：
1. `cron.list`
2. 命中唯一 job
3. `cron.delete(job_id)`
4. 连同 `cron:<job_id>` session 一并删除

这说明：
- 创建 / 修改 / 删除，本质是同一套 `cron` 工具面
- 不是三套不同的系统

---

## 8. 当前代码的 as-is 问题，用人话重述

当前最大的问题不是“cron 完全没有”，而是**概念混了**。

### 8.1 Trigger 基本有了

当前已经支持：
- `At`
- `Every`
- `Cron expr + tz`

所以“什么时候触发”这层，问题不是最大。

### 8.2 Execution 现在有两条路，语义不干净

现状里有类似：
- `sessionTarget=main + payload=systemEvent`
- `sessionTarget=isolated/named + payload=agentTurn`

这意味着：
- 到点后到底进哪儿跑，并没有统一
- execution 模型本身是分裂的

而且这些名字都太像实现细节，不像稳定产品语义。

### 8.3 Delivery 现在最乱

现状里有一些类似 `deliver/channel/to` 的字段，看起来像是“能发出去”，但执行层并没有形成一条真正稳固的投递闭环。

于是用户看到的体验就会很别扭：
- 看起来像创建了提醒
- 到点也可能真的跑了
- 但最后并没有收到 Telegram 消息

这不是一个单点 bug，而是：

> execution 和 delivery 没拆清，代码里出现了“看起来支持、实际上没闭环”的半成品状态。

---

### 8.4 目前 LLM-facing cron tool 也过重

当前代码里的 cron tool 让模型直接面对：
- `payload.kind`
- `sessionTarget`
- `deliver`
- `channel`
- `to`

这几个问题同时存在：

1. schema 大，吃 token
2. 把实现细节直接暴露给模型
3. 让模型承担了本不该承担的内部字段装配工作
4. 其中一部分字段当前甚至没有真正打通闭环

所以这不是“提示词再优化一下”能解决的问题，而是：

> **LLM-facing 接口本身就该缩。**

---

## 9. 建议冻结的设计原则

后续不管怎么实现，建议先把下面这些原则写死。

1. **cron 是一等工具，不是字段抽取流程**
2. **trigger / execution / delivery 三层必须分开**
3. **execution 统一为 `cron:<job_id>`**
4. **同 job 不重入，重入时 `skip + log`**
5. **不同 job 可以并发**
6. **delivery target 是持久化规格，不是执行时临时猜**
7. **本轮发不发由 LLM 输出决定，不引入额外 delivery policy 枚举**
8. **系统不做“提醒/巡检/告警”分类**
9. **delivery 可以沉默，但日志/状态不能沉默**
10. **删除 job 时，job 与 `cron:<job_id>` session 一并删除**

---

## 10. 可观测性口径（本稿先冻结到这个程度）

cron 至少需要明确记录下面这些 run 终态：

- `delivered`
- `suppressed_silence`
- `failed`
- `skipped_overlap`

人话理解：
- 可以不发消息
- 但不能什么都不留

否则用户根本分不清：
- 是没触发
- 是触发了但被跳过
- 还是触发了但模型决定沉默

---

## 11. 仍待后续收口的问题

这份稿子现在还剩 2 个值得后续继续收口的点：

1. **Web 来源的默认 delivery target，最终在 UI 哪个位置呈现**
   - 这个是前端显示语义问题
   - 先不在本文展开

2. **执行 prompt 外壳的最终精确文案**
   - 方向已经明确：
     - 系统模板负责执行协议
     - job 文本负责业务目标
   - 但最终文案还可以继续压缩打磨

---

## 12. 一句话总结

cron 最该先冻结的，不是“显示字段怎么抽”，而是：

> **它是一个统一工具；任务何时触发、在哪儿执行、结果发给谁，必须三件事分开讲。**

这三件事一旦拆开，整体模型就会立刻收敛很多。

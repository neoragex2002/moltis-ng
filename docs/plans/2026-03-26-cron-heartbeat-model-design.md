# Cron / Heartbeat 模型设计稿

更新日期：2026-03-30
状态：语义已定稿，代码已按主 issue 落地；后续增量修改继续回写主 issue

## 目的

本文用于保留 `cron` 与 `heartbeat` 的目标模型设计依据。

本文不是实现记录，也不是现状说明。
本文保留设计推导、目标模型与 one-cut 依据；后续代码实施与 review 以 `issues/issue-cron-system-governance-one-cut.md` 为唯一实施准绳。

## 设计原则

本文严格遵循以下原则：

- 第一性原则：先定义系统真正要解决的对象与边界，不迁就历史实现。
- 不后向兼容原则：不为历史语义保留额外兼容层、别名、fallback 或隐式猜测。
- 唯一真源原则：同一事实只允许一个 owner，不允许“会话 / 调度 / 投递”混成一个概念。
- 关键路径测试覆盖原则：后续实现必须优先覆盖核心主路径、关键边界、关键失败面。

## 一句话结论

系统只保留两类定时任务语义：

- `cron`：精确定时、一次性执行承载、无会话上下文、面向单一明确任务。
- `heartbeat`：周期性 agent 唤醒、依赖明确会话上下文、面向轻量持续关注。

两者底层可以共用一套“定时触发一次 agent run”的基础设施，
但上层产品语义、配置表面、运行合同、UI 表面必须明确分开。

## 术语表

### 1. agent

执行主体。`cron` 与 `heartbeat` 都必须明确归属于某个 `agent`。

### 2. main 会话

每个 `agent` 逻辑上固定拥有且只拥有一个 `main` 会话。

- `main` 是 `agent` 的系统合同，不是“用户先手工创建才允许存在”的可选对象。
- 即使当前还没有物理会话记录，`main` 也在逻辑上已经存在。
- 当某个运行首次显式引用 `main` 时，系统必须负责把它物化创建出来。

### 3. 显式会话

除 `main` 以外，已经存在且可稳定引用的具体会话。

这里只允许正式、持久、语义明确的 agent 会话。

不包括：

- 临时分支会话
- 内部运行 lane 会话
- 其它生命周期不稳定、不可长期绑定的对象

### 4. 运行上下文

某次定时任务运行时，agent 实际读取和推进的上下文载体。

- `cron` 默认没有会话上下文。
- `heartbeat` 必须绑定一个明确运行上下文。

### 5. 投递目标

某次运行完成后，结果发送到哪里。

投递目标与运行上下文不是同一个概念，不能混用。

## 顶层模型

### 1. 只有一套底层调度骨架

底层只需要一套最小机制：

- 到点
- 选中任务
- 启动一次 agent run
- 记录这次 run 的结果

底层不负责给 `cron` 和 `heartbeat` 硬凑成同一个产品对象，
只是提供统一的定时触发能力。

### 1.1 唯一真源与存储 owner

本文把“结构化配置”和“长文本 agent 文档”明确分开，各自只有一个 owner。

#### `cron`

- 结构化任务定义：DB
- 运行状态与 run history：DB
- 不再为 `cron` 额外引入任务级 markdown 文件
- 不再接受 file store / memory store 作为最终产品合同

#### `heartbeat`

- 结构化配置：DB
- 长文本 prompt：`agents/<agent_id>/HEARTBEAT.md`
- 运行状态：DB

#### `agent persona`

- 唯一 owner：agent 自己的身份文档体系
- `cron` 与 `heartbeat` 都只继承，不另存第二份 persona

这里必须强调：

- 不是“文件 + DB 一起拼出同一个事实”
- 而是“不同事实各有唯一 owner”
- 结构化布尔/枚举/时间配置归 DB
- 长文本 agent 指令归 agent 目录文件
- DB 不可用时，相关能力应直接失败，不做 file fallback / memory fallback

### 2. 上层只保留两种任务系统

#### `cron`

`cron` 是一个明确归属于某个 `agent` 的定时任务对象。

它的本质是：

- 到指定时间
- 执行一件明确的工作
- 这件工作必须由显式 `prompt` 定义
- 这次执行本身不依赖某条已有会话上下文
- 执行完成后，再按明确策略决定是否对外投递结果

#### `heartbeat`

`heartbeat` 是某个 `agent` 的周期性上下文唤醒机制。

它的本质是：

- 到固定节奏
- 在一个明确的会话上下文里唤醒 agent 跑一轮
- 用一段明确的 `prompt` 指定这一轮关注什么、检查什么、如何推进
- 看看当前上下文里有没有该处理的事情
- 有结果就正常落到该会话，没有结果就安静结束

## Cron 模型

### 1. 归属

- 每个 `cron` 必须显式归属于某个 `agent`
- 不存在“系统级默认 cron”
- 不存在“默认 main 会话 cron”

### 2. 执行模型

`cron` 只保留一种执行模型：

- 无会话上下文的一次性执行
- 显式 `prompt` 驱动

讲人话：

- 到点后，系统临时跑这一件事
- 这次运行不是把消息塞进某条聊天会话里
- 也不是复用某个用户会话的历史上下文继续聊

例如：

- 每天 09:00 拉一次某个外部接口，整理日报
- 每小时跑一次仓库巡检
- 每天 18:00 汇总错误日志并生成摘要

这些都属于“明确、单一、偏重型”的任务。

### 2.1 prompt 合同

每个 `cron` 都必须有自己的 `prompt`。

这个 `prompt` 不是备注，而是任务定义本身的一部分。

讲人话：

- `schedule` 决定“什么时候跑”
- `prompt` 决定“跑的时候具体做什么”

例如：

- 每天 09:00 运行
- `prompt` 是“汇总过去 24 小时 GitHub issue 变化，输出日报，只保留阻塞项和需要人工决策的项”

如果没有 `prompt`，那就只是一个空的时间触发器，不构成可执行任务。

### 2.2 schedule 合同

`cron` 的外部调度合同必须只保留面向用户的最小表达，不暴露实现味过重的内部字段。

只允许三种形状：

- `schedule = { kind: "once", at: "<ISO8601>" }`
- `schedule = { kind: "every", every: "30m" }`
- `schedule = { kind: "cron", expr: "0 9 * * *", timezone: "Asia/Shanghai" }`

这里的原则是：

- 用户表达“什么时候跑”
- 系统内部自己再转换成运行时需要的时间格式

因此外部合同不再暴露：

- `at_ms`
- `every_ms`
- `anchor_ms`
- `tz`

其中：

- `anchor_ms` 直接删除
- `tz` 改成完整的 `timezone`
- `every` 与 `heartbeat.every` 保持同一表达口径

这样做的好处是：

- `cron` 与 `heartbeat` 在“间隔”这一事实上的表达统一
- UI / RPC / tool / DB 不再围绕毫秒字段来回漂移
- 用户看到的是业务语义，不是底层实现细节

### 2.3 persona 与 model 合同

`cron` 的 `persona` 必须与所属 `agent` 保持一致，不允许单独配置第二套 `persona`。

原因很直接：

- `persona` 决定“这个 agent 是谁”
- `prompt` 决定“这次定时任务要做什么”
- `model` 决定“这次用哪一个模型来跑”

所以：

- `agent.persona` 是唯一真源
- `cron` 只允许自带任务 `prompt`
- `cron` 允许单独选择 `model`

不允许：

- 给 `cron` 单独配置另一套 `persona`
- 让同一个 `agent` 的不同 `cron` 长成不同身份

如果确实需要不同 `persona`，正确做法是新建另一个 `agent`，再把该 `cron` 归给那个 `agent`。

### 3. 结果处理

`cron` 的结果处理与执行本身分开建模。

`cron` 只保留三种结果策略：

- `silent`：执行完成，但不对外发消息
- `session`：执行完成后，把结果投递到明确指定的会话目标
- `telegram`：执行完成后，把结果投递到明确指定的 Telegram 目标

如果投递到会话，允许两种目标：

- `cron.delivery.session.target = { kind: "main" }`
- `cron.delivery.session.target = { kind: "session", sessionKey: "..." }`

如果投递到 Telegram，目标合同必须收敛为最小必要字段：

- `delivery = { kind: "telegram", target: { accountKey: "...", chatId: "..." } }`
- `delivery = { kind: "telegram", target: { accountKey: "...", chatId: "...", threadId: "..." } }`

其中：

- `accountKey`：指定使用哪个 Telegram bot 账号发送
- `chatId`：指定发往哪个 Telegram chat
- `threadId`：仅在 topic / 子线程投递时出现

不再暴露以下字段到 `cron.delivery.telegram.target` 外部合同：

- `channel_type`
- `username`
- `peer_id`
- `message_id`
- `chan_user_name`
- `bucket_key`

这些要么是重复事实，要么不是定时投递所必需的发送地址。

这里必须强调：

- `cron` 是“跑完再投递到会话”
- 不是“在会话里执行 cron”

也就是说：

- 执行阶段无会话上下文
- 投递阶段可以把产出结果发到会话

这里特别强调：

- 不再使用 `channel` 这个泛词
- 如果当前外部投递就是 Telegram，就直接叫 `telegram`
- 以后若新增别的投递面，再新增明确类型，不预留空泛总称

### 4. 严格约束

- `cron` 不以任何会话作为执行上下文
- `cron` 不读取 `main` 或其它聊天会话的历史上下文来执行
- `cron` 不存在 `session cron`
- `cron` 不通过结构化计划事件注入用户会话
- `cron` 不承担“周期性陪聊”职责

额外要求：

- 若结果投递目标是 `main`，则允许按 `main` 合同先确保并物化该会话
- 若结果投递目标是具体会话，该会话必须已经存在且可稳定引用
- 若目标会话不存在、非法、或属于内部临时对象，必须直接失败
- 不允许因为“结果投递到会话”，就把 `cron` 反向定义成“会话型执行”

### 5. 为什么删掉 session cron

因为它把两个不同事实混到了一起：

- 一个事实：这是定时执行任务
- 另一个事实：这是在某条会话里继续对话

这会导致：

- 会话 owner 和调度 owner 混乱
- 执行结果与用户上下文边界混乱
- UI、接口、日志、测试都容易分叉

所以直接删除，不保留。

## Heartbeat 模型

### 1. 归属

- 每个 `heartbeat` 必须显式归属于某个 `agent`
- `heartbeat` 不是系统全局单例
- 多 agent 系统中，每个 `agent` 都可以各自配置自己的 `heartbeat`
- 每个 `agent` 最多只允许一个 `heartbeat`

### 1.1 prompt 文件归属

`heartbeat` 的 prompt 不应再来自工作区根级 `HEARTBEAT.md`。

必须收敛为 agent 级文件：

- `agents/<agent_id>/HEARTBEAT.md`

原因：

- `heartbeat` 本来就是 agent 专属任务
- 每个 agent 都应能有自己的 heartbeat 关注规则
- 根级单文件会把多 agent 的 heartbeat 语义混在一起

因此：

- `heartbeat` 的结构化开关与调度配置归 `heartbeat` 配置 owner
- `heartbeat` 的长文本 prompt 归 `agents/<agent_id>/HEARTBEAT.md`
- 工作区根级 `HEARTBEAT.md` 退出目标模型
- 不再保留 `heartbeat.prompt` 结构化覆盖字段

### 2. 本质语义

`heartbeat` 不是“后台重型任务”，而是“带上下文的一轮周期性关注”。

讲人话：

- 系统定时把 agent 叫醒一下
- 让它在某个明确上下文里看一眼：“现在有没有该处理的事？”
- 如果没有，就什么都不说，安静结束
- 如果有，就像正常 agent turn 一样继续产出结果

### 2.1 prompt 合同

`heartbeat` 也必须有明确 `prompt`。

但它的 `prompt` 作用和 `cron` 不一样：

- `cron` 的 `prompt` 用来定义这次单一任务本身
- `heartbeat` 的 `prompt` 用来定义“基于当前会话上下文，这一轮该怎么检查、关注、推进”

讲人话：

- `cron` 更像“定时执行这张任务卡”
- `heartbeat` 更像“定时提醒 agent 按这套关注原则看一眼当前上下文”

所以两者都有 `prompt`，但语义不同，不能混成一种东西。

这里再钉死一条：

- `heartbeat` 的 prompt 文件是 `agents/<agent_id>/HEARTBEAT.md`
- 不再使用工作区根级 `HEARTBEAT.md`

### 2.2 persona 与 model 合同

`heartbeat` 的 `persona` 也必须与所属 `agent` 保持一致，不允许单独配置。

原因：

- `heartbeat` 本质上就是这个 `agent` 在某个上下文里周期性醒一轮
- 如果 `heartbeat` 可以单独改 `persona`，就等于同一个 `agent` 有两套身份源
- 这会直接破坏唯一真源

因此：

- `agent.persona` 是唯一身份源
- `heartbeat` 允许单独选择 `model`
- `heartbeat` 不允许单独选择 `persona`

推荐收敛为统一模型选择语义：

- `inherit`：若本次运行绑定了明确会话，则继承该目标会话当前模型；若目标会话没有显式模型，则回到全局默认模型
- `explicit(model_id)`：显式指定模型

不要再用空值、猜测、隐式 fallback 表达“继承模型”。

### 3. 运行上下文绑定

`heartbeat` 必须显式绑定一个运行上下文目标，且只允许以下两种：

- `main`
- 某个明确的已有会话

推荐合同形状：

- `heartbeat.sessionTarget = { kind: "main" }`
- `heartbeat.sessionTarget = { kind: "session", sessionKey: "..." }`

禁止以下语义：

- 空值代表 `main`
- 自动猜“最近活跃会话”
- 自动猜“最后一条私聊会话”
- 自动猜“最近一次打开的会话”

额外要求：

- 绑定 `main` 以外的具体会话时，该会话必须已经存在且可稳定引用
- 若目标会话不存在、非法、或属于内部临时对象，必须直接拒绝
- 只有 `main` 允许按合同自动物化创建，普通具体会话不允许自动补建

### 4. main 会话合同

`heartbeat` 必须支持显式绑定到 `main`，即使用户从未手工创建过这条会话。

系统要求如下：

- 每个 `agent` 逻辑上始终拥有一个 `main`
- 若某次运行前引用了 `main`，但该会话尚未物化存在
- 系统必须先执行“确保 main 会话存在”
- 不存在就创建
- 创建失败就直接失败并记录结构化日志

这不是 fallback，也不是兼容逻辑，而是 `main` 会话合同的一部分。

### 5. 单上下文原则

一次 `heartbeat` 运行只能服务一个运行上下文。

也就是说：

- 一个 `heartbeat` 可以绑定 `main`
- 也可以绑定某个具体会话
- 但不能在一轮运行里同时绑定多个用户会话

这点必须钉死。

### 6. 多用户场景怎么理解

如果一个 `agent` 同时和很多用户聊天：

- 绑定到 `main` 的 `heartbeat`：适合做 agent 自己的长期关注、整理待办、巡检状态
- 绑定到某个用户会话的 `heartbeat`：适合围绕这个用户的上下文做持续关注

但“一个 heartbeat 同时面向很多用户轮流陪聊”不属于 `heartbeat` 的职责。
那是另一类任务系统，不在本文范围内。

### 7. 结果落点

`heartbeat` 的结果默认属于它绑定的运行上下文。

也就是说：

- 绑定 `main`，结果就落到 `main`
- 绑定某个具体会话，结果就落到该会话
- 如果这一轮没有需要产出的消息，就安静结束，不强行塞内容

### 8. 不做的事

- 不引入“多会话 heartbeat”
- 不引入“自动广播到全部聊天对象”
- 不为了运行 heartbeat 往会话历史里硬塞结构化 JSON 事件
- 不把 heartbeat 伪装成普通 `cron job`

## One-cut 删除项

以下旧字段、旧文件、旧语义都不再进入目标模型：

### `cron`

- `payloadKind = systemEvent | agentTurn`
- `sessionTarget` 作为执行上下文字段
- `deliver/channel/to`
- `anchor_ms`
- `tz`
- file store / memory store 作为最终产品持久化合同
- job 级 `sandbox` 配置
- `Named(...)` 这类内部执行 lane 暴露

### `heartbeat`

- 工作区根级 `HEARTBEAT.md`
- `heartbeat.prompt` 结构化覆盖字段
- `heartbeat.ack_max_chars`
- `heartbeat` 独立 persona
- `heartbeat` 私有 sandbox 配置

### 通用

- `channel` 这个空泛投递总称
- 空值代表 `main`
- 自动猜最近活跃会话
- fallback 到最近会话 / 最后会话 / 任意会话
- 自动迁移 legacy 字段 / legacy 文件 / legacy 持久化形状

## 为什么 Cron 和 Heartbeat 要分开

因为它们解决的不是同一类问题。

### Cron 解决的是

- 到一个精确时间
- 做一件明确任务
- 这件事不需要依赖某条聊天上下文

### Heartbeat 解决的是

- 到一个节奏点
- 在既有上下文里看一眼是否该继续推进

如果把两者揉成一个模型，就会立刻出现：

- 是否有上下文说不清
- 结果该不该进会话说不清
- UI 到底是在配置“任务”还是“会话唤醒”说不清

所以必须分开。

## 示例

### 示例 1：日报汇总

某个 `agent` 每天 09:00 拉 GitHub issue、整理摘要，然后发到 Telegram 群。

这是 `cron`，不是 `heartbeat`。

原因：

- 它是明确任务
- 它有自己的任务 `prompt`
- 不依赖某条聊天上下文
- 执行结果要不要发，是单独的投递策略

例如它的 Telegram 投递目标可以是：

- `delivery = { kind: "telegram", target: { accountKey: "telegram:ops_bot", chatId: "-1001234567890" } }`

### 示例 1.1：把 cron 结果投到 main

某个 `agent` 每天 09:00 跑一次“汇总昨晚告警并生成处理建议”的 `cron`，
但结果不是发 Telegram，而是投递到这个 `agent` 的 `main` 会话。

这仍然是 `cron`，不是 `heartbeat`。

原因：

- 执行时仍然不读取任何会话上下文
- 只是执行完成后，把结果送到 `main`
- 如果 `main` 尚未物化，允许先按 `main` 合同创建再投递

### 示例 2：Agent 自己的周期关注

某个 `agent` 每 10 分钟看一下自己的长期上下文、待办、未处理事项。

这是绑定 `main` 的 `heartbeat`。

即使之前没人手工创建过这条 `main` 会话，也必须允许这么配置并正常运行。

它同样需要自己的 `prompt`，例如：

- “检查当前主会话里的待办、未完成承诺、最近未收口事项；若没有真正需要推进的内容，就不要输出。”

### 示例 3：围绕某个用户会话持续关注

某个 `agent` 与用户 A 有一条长期协作会话，希望每 15 分钟主动看一眼有没有该继续推进的事。

这是绑定到该会话的 `heartbeat`。

它只关注这条会话，不会顺便兼顾用户 B、C、D。

它也同样需要自己的 `prompt`，例如：

- “检查这条协作会话里是否还有未跟进事项；若暂时没有明确需要推进的动作，就保持安静。”

## 失败语义冻结

以下失败面必须按同一口径处理，不允许 silent degrade。

### 1. `heartbeat` 配置失败

- `enabled=true` 但 `agents/<agent_id>/HEARTBEAT.md` 缺失：直接拒绝
- `enabled=true` 但 `agents/<agent_id>/HEARTBEAT.md` 有效内容为空：直接拒绝
- `sessionTarget.kind="session"` 但目标会话不存在：直接拒绝
- `sessionTarget.kind="main"` 但 `main` 物化创建失败：直接失败
- `modelSelector=explicit(...)` 但模型不存在：直接拒绝

### 2. `cron` 配置失败

- `prompt` 为空：直接拒绝
- `schedule.kind="once"` 且 `at` 非法：直接拒绝
- `schedule.kind="once"` 且 `at` 已经过期：直接拒绝
- `schedule.kind="every"` 且 `every` 非法或小于等于零：直接拒绝
- `schedule.kind="cron"` 且 `expr` 非法：直接拒绝
- `schedule.kind="cron"` 且 `timezone` 非法：直接拒绝
- `modelSelector=explicit(...)` 但模型不存在：直接拒绝
- `deleteAfterRun=true` 但不是 `once` 任务：直接拒绝
- `delivery.kind="session"` 但 target 非法：直接拒绝
- `delivery.kind="telegram"` 但缺少 `accountKey` 或 `chatId`：直接拒绝

### 3. 运行时失败

- `cron` 投递到具体会话时，会话在运行前已被删除：直接失败
- `cron` 投递到 Telegram 时，`accountKey` 已不存在：直接失败
- `cron` 投递到 Telegram 时，`chatId` / `threadId` 不被 Telegram 接受：直接失败
- `heartbeat` 绑定的具体会话在运行前已被删除：直接失败
- 只要 delivery 失败，本次 run 的最终 `status` 就必须记为 `Error`，并同步写入 `lastError`；禁止“日志报错但 run/history 仍显示成功”

### 4. legacy 命中

- 命中工作区根级 `HEARTBEAT.md`：按 one-cut 直接报错并给 remediation
- 命中 `heartbeat.prompt` / `heartbeat.ack_max_chars`：按 one-cut 直接报错并给 remediation
- 命中 `payloadKind` / `deliver/channel/to` / `anchor_ms` / `tz`：按 one-cut 直接报错并给 remediation
- 命中旧 file store / memory store 持久化路径：按 one-cut 直接报错并给 remediation
- 不做自动迁移；由用户或显式运维步骤完成 remediation

### 5. 基础设施失败

- DB 不可用：相关功能直接失败，不降级到 file store / memory store
- agent 目录不可访问：相关 agent 的 `heartbeat` 直接失败

## 明确删除的旧思路

以下概念不再保留：

- `session cron`
- “把 cron 注入某条会话继续跑”
- “用结构化计划事件推进会话”
- “用最近活跃会话当 heartbeat 默认上下文”
- “heartbeat 自动服务多个用户会话”
- “系统级唯一 main 会话”

## 边缘条件

边缘条件必须在“主模型已经定死”的前提下处理，不能为了边缘覆盖反向破坏第一性原则。

### 1. 会话边界

- `main` 逻辑存在但物理记录缺失：允许按合同自动创建
- 普通具体会话缺失：不自动补建，直接失败
- 临时分支会话 / 内部 lane 会话：不允许绑定

### 2. 调度边界

- `once` 只能表示未来某一时刻；过去时间不接受
- `every` 只接受正间隔
- `cron` 表达式与 `timezone` 都必须在保存时校验

### 3. 生命周期边界

- 每个 `agent` 最多一个 `heartbeat`
- 每个 `agent` 可以有多个 `cron`
- `deleteAfterRun` 只属于一次性 `cron`
- DB 是结构化持久化唯一 owner，不接受平级文件落盘替代

### 4. 投递边界

- `telegram` 投递只认 `accountKey + chatId + threadId?`
- `session` 投递只认 `main` 或正式具体会话
- `heartbeat` 不走独立 delivery 分叉，结果默认落在其绑定会话

### 5. 文档与实现边界

- 设计稿冻结的是语义合同，不是当前代码现状
- 若实现发现与本文冲突，必须先回写主单/设计稿再改代码

## 后续实施必须满足的合同

### 1. UI

- `cron` 与 `heartbeat` 必须分成两套明确表面
- `cron` 必须有明确的 `prompt` 输入表面
- `heartbeat` 必须有明确的 `agents/<agent_id>/HEARTBEAT.md` 编辑/查看表面
- `cron` 的结果策略选择器必须明确支持：
  - `silent`
  - `session`
  - `telegram`
- 当 `cron` 选择 `session` 投递时，必须明确支持：
  - `main`
  - 具体会话
- 当 `cron` 选择 `telegram` 投递时，必须明确提供：
  - `accountKey`
  - `chatId`
  - `threadId`（仅在需要时）
- `heartbeat` 的 UI 必须体现“每个 agent 最多一个 heartbeat”
- `heartbeat` 的上下文目标选择器必须明确支持：
  - `main`
  - 具体会话
- 即使 `main` 尚未物化，UI 也必须允许选择 `main`

### 2. 运行时

- `heartbeat` 绑定 `main` 时，运行前必须确保 `main` 已物化存在
- 不得 fallback 到其它会话
- `cron` 执行路径不得依赖任何聊天会话上下文
- `cron` 选择 `session` 投递到 `main` 时，投递前必须确保 `main` 已物化存在
- `cron` 选择 `session` 投递到具体会话时，目标会话不存在就必须直接失败
- `cron` 选择 `telegram` 投递时，目标不合法或缺失就必须直接失败
- `cron` 选择 `telegram` 投递时，运行时只消费 `accountKey + chatId + threadId?` 这组最小地址字段

### 3. 可观测性

以下关键决策必须补结构化日志：

- `heartbeat` 运行前确保 `main` 会话存在
- `main` 会话自动创建成功或失败
- `heartbeat` 因目标会话不存在或非法而拒绝
- `cron` 结果按 `silent` 明确不投递
- `cron` 结果按 `telegram` 投递成功或失败
- `cron` 结果按 `session` 投递成功或失败

日志至少包含：

- `event`
- `reason_code`
- `decision`
- `policy`
- 以及上下文允许时的 `agent_id`、`session_key`、`delivery_kind`、`remediation`

### 4. 测试优先级

后续实现必须优先覆盖以下关键路径：

- `heartbeat` 在 `enabled=true` 时要求 `agents/<agent_id>/HEARTBEAT.md` 有效内容非空，否则直接拒绝
- DB 不可用时，`cron` / `heartbeat` 相关能力直接失败，不降级到其它 store
- `heartbeat` 绑定 `main`，`main` 未物化时自动创建成功
- `heartbeat` 绑定 `main`，自动创建失败时直接失败且有结构化日志
- `heartbeat` 绑定具体会话，正常在该会话上下文运行
- `heartbeat` 不会自动猜测最近活跃会话
- `cron` 不依赖任何会话上下文执行
- `cron` 的 `once/every/cron` 三种 schedule 保存校验生效
- `cron` 投递到 `main` 时会按合同创建并成功投递
- `cron` 投递到具体会话时，目标不存在会直接失败
- `cron` 投递到 Telegram 时，目标非法会直接失败
- `cron` 投递到 Telegram 时，不依赖 `username` / `peer_id` / `message_id` 等冗余字段
- `cron` 的 `silent`、`session`、`telegram` 行为边界明确

并且至少保留少量关键拒绝用例，证明：

- legacy 字段命中时直接失败
- 不存在 silent degrade
- `reason_code` 可观测

## 当前仍未冻结的只剩实现细节

以下内容还需要在实施主单里继续细化，但不影响本文语义结论：

- UI 布局与交互细节
- run history 展示字段
- `main` 会话在 UI 中的展示方式

这些都只能在不违反本文语义合同的前提下展开。

## 最终字段总表

以下字段名指的是外部 JSON / RPC / UI 合同，因此统一使用 `camelCase`。

### `heartbeat` 配置字段

- `agentId`
- `enabled`
- `every`
- `sessionTarget = { kind: "main" } | { kind: "session", sessionKey: "..." }`
- `modelSelector = inherit | explicit(modelId)`
- `activeHours.start`
- `activeHours.end`
- `activeHours.timezone`

### `heartbeat` 文件字段

- `agents/<agent_id>/HEARTBEAT.md`

这是唯一 heartbeat prompt 来源。

### `cron` 配置字段

- `jobId`
- `agentId`
- `name`
- `enabled`
- `schedule = { kind: "once", at } | { kind: "every", every } | { kind: "cron", expr, timezone }`
- `prompt`
- `modelSelector = inherit | explicit(modelId)`
- `timeoutSecs`（可选）
- `delivery = { kind: "silent" } | { kind: "session", target } | { kind: "telegram", target }`
- `deleteAfterRun`

### `cron.delivery.session.target` 字段

- `{ kind: "main" }`
- `{ kind: "session", sessionKey: "..." }`

### `cron.telegram` target 字段

- `{ accountKey: "...", chatId: "..." }`
- `{ accountKey: "...", chatId: "...", threadId: "..." }`

### 运行时状态字段

以下字段属于系统运行状态，不属于用户手填配置：

- `nextRunAt`
- `runningAt`
- `lastRunAt`
- `lastStatus`
- `lastError`
- `lastDurationMs`

### `cron` run history 字段

`runId` 必须是真实持久化字段，不能在日志里是一套 UUID、落库后再被 SQLite 自增行号替代。

- `runId`
- `jobId`
- `startedAt`
- `finishedAt`
- `status`
- `error`
- `outputPreview`
- `inputTokens`
- `outputTokens`

### `heartbeat` run history 字段

同理，`heartbeat.runId` 必须与结构化日志、会话投递记录中的 `runId` 保持同一值。

- `runId`
- `agentId`
- `startedAt`
- `finishedAt`
- `status`
- `error`
- `outputPreview`
- `inputTokens`
- `outputTokens`

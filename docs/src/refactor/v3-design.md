# V3 整体方案

本文档只定义一件事：

- 第三版整体架构方案的方向性原则与总体目标

本文档不讨论：

- 分阶段实施步骤
- 排期与迁移顺序
- 某个阶段的局部代码改造

也就是说，本文档讨论的是：

- **第三版要收敛成什么架构**

而不是：

- **第三版按什么顺序实施**

## 一句话结论

第三版的总目标是：

- 用**少量、稳定、收敛**的核心概念定义上层语义
- 用渠道适配层封装各渠道内部细节
- 用 `type` / `scope` 决定会话语义
- 用统一的事件记录格式保存稳定事实
- 用 core 的上下文管理把这些事实整理成模型输入

一句话：

- **core 定义语义并管理 agent 运行时，adapter 封装渠道细节，统一事件记录保存稳定事实，模型上下文由 core 管理**

这里还要补一条当前实施口径：

- **C 阶段的第一优先级，不是先替换保存层，而是先把 Telegram adapter / core 的职责切干净**
- **因此 C 阶段允许先保留旧 `SessionStore` / `PersistedMessage`，由 core 通过 bridge 方式读取旧记录并统一整理模型上下文**

## 总体目标

第三版想解决的不是单点命名问题，而是整条链路的责任边界问题：

- 哪些概念属于 core
- 哪些概念属于 adapter
- 哪些内容属于统一事件记录
- 哪些内容属于 core 的上下文管理
- 哪些内容属于渠道实现细节

如果这些边界不清晰，就会长期出现：

- 核心概念膨胀
- 渠道细节泄漏到 core
- transcript / prompt 文本反向污染保存层
- 不同渠道各自长出一套半独立语义

## 设计原则

### 1. 核心概念必须收敛

第三版里，core 不应一开始就维护一张膨胀的大概念表。

core 应长期冻结的，应当是**少量、稳定、跨渠道成立**的上层语义。

当前更合理的收敛方式是：

- 先冻结少量稳定概念，例如 `agent`、`type`、`scope`、`peer`
- 其中 `type` 是上层会话类型，例如 `dm`、`group`、`cron`、`heartbeat`
- `session_key`、`session_id` 是 core 管理的会话身份结果
- `sender`、`account`、`channel` 等，则由不同 `type` / `scope` 在需要时按语义引入
- `branch` 不应上升为强 core 公共概念；它更适合作为 adapter 按特定 `scope` 返回的子线标识

这里要额外强调：

- 保存层里可以出现比 core 公共概念表更丰富的结构化字段
- 但“保存时需要记录哪些事实”不等于“core 长期公开维护哪些核心概念”

一句话：

- **先冻结最少的公共语义，不把 type-specific 轴一开始就平铺成 core 总表**

### 2. 核心层与渠道层必须拆开

第三版里，core 和 adapter 的关系必须固定为：

- **core 只定义核心语义**
- **adapter 只封装渠道细节并输出统一后的结果**

这意味着：

- core 不需要知道 Telegram / Feishu / Slack 的原生字段长什么样
- core 不需要知道某个渠道内部怎么识别 account / peer / topic / thread
- core 只关心这些渠道细节最后是否被可靠地映射成 core 需要的语义

对应地：

- 渠道层可以有自己的内部概念
- 渠道层可以有自己的实现细节字段
- 渠道层可以保留本渠道协议所必需的局部状态

但这些内容应当：

- 封装在渠道层内部
- 通过统一后的结果或分桶结果暴露给 core
- 不直接泄漏成 core 长期依赖的公共概念

### 3. `type` 与 `scope` 负责会话语义

第三版的会话语义，应由：

- `type`
- `scope`

共同决定。

也就是说：

- `dm` 和 `group` 不是同一类问题
- `dm` 表示 agent 与单个外部参与者之间的 1 对 1 会话
- `group` 表示 agent 与多个外部参与者之间的共享会话
- `cron`、`heartbeat` 这类系统触发也应作为独立 `type`
- `dm_scope` 与 `group_scope` 应分别定义、分别收敛
- core 只定义同桶/异桶语义
- adapter 负责把这些语义在本渠道实现为稳定结果

### 4. `session_key` 是编码结果，不是上层语义来源

第三版里，`session_key` 的定位应明确为：

- 会话语义被实现后的编码结果

它回答的是：

- 这条消息最终应落入哪个逻辑桶

但它不应反过来成为：

- 概念定义本身
- 渠道细节泄漏的入口

### 5. 事件记录与上下文管理必须解耦

第三版里，至少要分开三件事：

- 会话身份
- 会话保存格式
- 推理上下文格式

更准确地说：

- 统一事件记录只保存稳定事实
- 上下文管理把这些事实整理成模型输入
- 在分阶段落地时，允许 core 先通过 legacy persistence bridge 读取旧会话记录

不应继续沿用：

- 直接把 adapter 拼好的 transcript / prompt 文本当作唯一事实来源

### 6. 统一事件记录只存稳定事实

第三版里，统一事件记录应收敛为：

- `session_event`

它应当是：

- 统一的
- 结构化的
- 可重放的
- 与特定渠道原生格式解耦的
- 与最终 LLM 上下文格式解耦的

可以把它理解成：

- 一条统一的会话事件记录

因此：

- 统一事件记录必须保存稳定事实
- 渠道原始数据不应直接成为统一字段
- 直接给模型看的临时拼接文本不应直接成为统一字段

### 7. 上下文管理负责组织模型输入

第三版里：

- core 的上下文管理负责把已落盘的稳定事实整理成当前 run 需要的模型输入
- 它内部可以包含不同 `type` 的上下文组织规则，以及拼装 / 压缩 / 窗口控制

也就是说：

- adapter 不负责最终给模型的文本
- core 不应依赖渠道临时拼出来的一次性 transcript
- C 阶段里，core 可以先基于旧 `PersistedMessage` 做 bridge assemble / render；这不改变“上下文归 core”的职责归属

## Core 职责范围

第三版里，core 不应被理解成“抽象层集合”，而应被理解成一组收敛、清晰的职责。

当前更合理的 core 职责范围是：

### 1. Agent 执行主链

负责：

- agent run 生命周期
- 模型调用
- 工具 / 技能 / sandbox / approval 编排
- 流式输出与 run 级状态

这里回答的是：

- 谁在思考
- 这一轮 run 如何执行
- tool / skill 如何进入这轮推理

### 2. 会话语义与会话身份

负责：

- `type`
- `scope`
- `peer`
- `session_key`
- `session_id`

这里回答的是：

- 这是什么类型的会话
- 该按什么语义同桶/异桶
- 这条消息最终落到哪个逻辑会话

### 3. 统一事件记录

负责：

- `session_event` 结构
- 持久化格式
- 可重放的事实流

这里要强调：

- 统一事件记录格式应尽量与特定渠道原生格式解耦
- 统一事件记录格式应尽量与最终 LLM 上下文格式解耦
- 保存的是稳定事实，不是 Telegram update，也不是 prompt 文本

### 4. 上下文整理

负责：

- 从已落盘的稳定事实整理模型输入
- 不同 `type` 的上下文规则
- 拼装 / 压缩 / 摘要 / 窗口控制
- 最终送入模型的 messages / context blocks

这里要强调：

- 这是 core 的内部职责
- 不再额外拆成多个并列“大层”
- adapter 不参与最终给模型的文本塑形
- 在 C 阶段里，这一职责可以先建立在 legacy persistence bridge 之上，而不要求先完成 `session_event` 持久化

### 5. 回复结果

负责：

- 这次 run 产出的回复语义结果是什么
- 是普通文本、结构化结果，还是带附件的回复
- 回复与哪条上游事实记录相关

这里不负责：

- Telegram 怎么 reply
- Telegram 怎么 chunk
- Telegram 是否先发 typing

这些属于渠道适配层。

## Telegram 适配模块职责

第三版当前先落 Telegram，但 Telegram 仍只是一个渠道适配器，不是 core 语义来源。

Telegram adapter 当前应收敛为以下职责：

### 1. Telegram 连接与生命周期

负责：

- polling / webhook 接入
- webhook 清理
- reconnect / retry / backoff
- 账号连通性与运行时可观测性

### 2. Telegram 原生对象解析

负责从 Telegram 原生 update 中提取：

- bot/account
- chat / user / sender
- message / reply / quote
- topic / thread 相关信息
- media / file / location 等原生对象

### 3. Telegram 输入整理

负责：

- 把 Telegram 原生字段整理成 core 需要的统一输入
- 产出 `peer`
- 在需要时产出 `sender`
- 在 `per_branch` 语义下能稳定识别子线（`branch`），并以 `bucket_key` / `adapter_hints` 等方式交付上层使用
- 产出 `body` / `attachments` / `reply_ref` / `source` / `adapter_hints`

但它不负责：

- 定义 `dm_scope` / `group_scope`
- 定义 `session_key` 语义
- 定义最终给模型的文本

### 4. Telegram 出站执行

负责：

- 把 core 的回复结果翻译成 Telegram API 调用
- reply_to / topic routing
- typing / media upload / chunking / parse mode
- Telegram 特定错误恢复与重试

### 5. Telegram 渠道特有策略与护栏

负责：

- mention / addressed / listen-only 等 Telegram 表面规则
- mirror / relay 等 Telegram 群策略的候选识别与协议侧护栏
- Telegram 特有 access policy
- Telegram 特有降级、跳过与 reason code 可观测性

这些策略的职责边界是：

- TG adapter 负责“协议侧能否/如何触发”的判定与护栏（候选识别、去重线索、限频、重试等）
- core 负责把“最终如何写入会话事实流/历史、如何进入上下文”收口为可审计的上层决策（transcript/history shaping）

而不应反向定义：

- core 的长期会话语义
- 统一事件记录格式
- 上下文管理格式

## Telegram 在第三版中的位置

第三版当前实施虽然优先聚焦 Telegram，但 Telegram 优先只是**实施顺序**，不是设计特权。

也就是说：

- Telegram 可以先落地
- 但 Telegram 不应反向决定第三版 core 概念
- Telegram 的实现细节仍应被封装在 Telegram adapter 内部

## 与实施路线图的关系

这份文档定义：

- 第三版整体设计目标和责任边界

而实施路线图定义：

- 第三版按什么顺序落地
- 哪些旧能力阶段性复用
- Telegram 优先的渐进步骤如何安排

## 相关文档

- `docs/src/refactor/v3-roadmap.md`
- `docs/src/concepts-and-ids.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`
- `docs/src/refactor/session-scope-overview.md`
- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

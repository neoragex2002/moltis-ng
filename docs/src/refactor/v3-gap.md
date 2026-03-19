# 当前代码现状与 V3 目标差距

本文档只定义一件事：

- 当前代码库的主体现状，以及它与第三版目标形态之间的主要差距

本文档不讨论：

- 第三版整体设计原则本身
- 第三版分阶段实施步骤
- 历史数据迁移与兼容策略

也就是说，本文档讨论的是：

- **现状是什么**
- **距离目标还差什么**

而不是：

- **目标方案本身怎么设计**
- **应该按什么顺序实施**

## 一句话结论

当前代码库仍然主要处于第二版形态：

- Telegram 相关逻辑已较多，但边界仍不清晰
- session 语义、渠道绑定、落盘格式、上下文构造仍明显耦合
- 核心层与渠道层还没有完成第三版要求的稳定拆分

一句话：

- **当前代码能跑，但主链仍是“渠道驱动 + 会话文本驱动”，离第三版的“语义驱动 + 统一事件驱动”还有明显距离**

## 当前代码的主体现状

## 1. 入站上下文模型仍是第二版混合形态

当前统一入站对象 `MsgContext` 仍同时携带：

- 渠道字段
- 会话字段
- 路由字段
- 历史遗留字段

例如当前对象里同时存在：

- `chan_type`
- `chan_account_key`
- `session_id`
- `chan_chat_key`
- `group_id`
- `guild_id`
- `team_id`

这说明当前模型还没有完成第三版要求的职责拆分。

当前形态更像是：

- 把多轮迭代中不同阶段需要的字段都塞进一个总对象

而不是：

- 按 core 语义与 adapter 细节分层建模

## 2. session 绑定仍明显由渠道 chat 驱动

当前 gateway 里的 channel 入口逻辑，会直接按：

- `channel_type`
- `chan_account_key`
- `chat_id`

查询或创建 active session。

这说明当前会话主链仍然偏向：

- 先由渠道 chat/account 维度确定 session

而不是：

- 先由 `type` / `scope` / 语义轴决定会话，再由渠道层实现这些语义

换句话说，当前 session 定位的主要驱动力仍是：

- 渠道绑定表

而不是：

- 第三版的统一会话语义层

## 3. `session_key` / `session_id` 体系仍未收敛成第三版语义

当前代码里虽然已经出现：

- `session_id`
- `chan_chat_key`
- `chan_account_key`

但它们在很多地方仍处于混合使用状态。

尤其是：

- 有些地方把 `session_id` 当成持久会话桶
- 有些地方又退回到 `chan_chat_key`
- 还有一部分旧的 `SessionKey` / `DmScope` 代码存在，但并未成为实际统一主链

这说明当前代码还没有形成第三版要求的：

- `type` / `scope` -> `session_key` -> 当前 `session_id`

这条稳定主链。

## 4. Telegram 仍在主导部分 session 语义与会话文本语义

当前 Telegram 侧不只是做“原生事件解析”。

它还直接参与决定：

- group listen-only 时写入什么文本
- mirror / relay 时写入什么文本
- group 会话文本应采用什么格式

甚至 Telegram 配置里直接存在：

- `group_session_transcript_format`

这说明当前 Telegram adapter 仍在主导一部分：

- 给模型看的会话文本语义

而不是只输出结构化事实。

这与第三版目标形态明显不同。

## 5. gateway 里仍有大量 Telegram 特化逻辑

当前 Telegram 相关逻辑并没有被完整收束在 Telegram adapter 内部。

gateway 里仍然直接处理了不少 Telegram 特化内容，例如：

- Telegram session 绑定
- Telegram label 生成
- Telegram group mirror / relay
- Telegram 出站配套逻辑

这意味着当前分层仍然是：

- adapter 一部分
- gateway 一部分
- session metadata 一部分

而不是第三版希望的：

- 渠道层封装渠道细节
- core 只处理 core 语义

## 6. 当前会话记录格式仍以 transcript / role-content 为中心

当前 `SessionStore` 落的是 JSONL；
而落盘主体 `PersistedMessage` 的中心仍是：

- `role`
- `content`
- `channel`
- tool result / assistant text 等

也就是说，当前保存层本质上仍然是：

- 面向会话文本的消息流

而不是：

- `session_event` 统一事件流

这会导致：

- 保存层与面向模型的表达仍然靠得比较近
- adapter 整出来的会话文本仍容易反向影响保存层

## 7. 当前上下文构造仍主要依赖旧会话文本主链

虽然现在代码已经有了不少 hook、tool、channel、status log 等扩展能力，
但主上下文仍主要围绕：

- 读取旧 session 历史
- 追加 user / assistant / tool result
- 再交给现有 chat run 主链

这和第三版目标里的：

- 统一事件记录层
- `dm` / `group` 的上下文整理规则
- 上下文引擎

还不是一回事。

## 8. Telegram 复杂策略与基础会话语义仍未完全分离

当前 Telegram group 的复杂能力，例如：

- relay
- mirror
- mention 策略
- topic/thread/forum 细节

仍与 session、入站改写、落盘文本、出站行为较强耦合。

这说明当前代码还没有做到第三版要求的：

- 基础会话语义一层
- 复杂渠道策略另一层

## 当前代码距离 V3 的主要差距

## 1. 缺少稳定的“核心语义层”

第三版要求：

- 先有少量稳定的 core 语义
- 再由不同 `type` / `scope` 按需引入其他语义轴

当前代码则更接近：

- 多种字段、历史概念、渠道概念并列混放

差距在于：

- 还没有形成收敛的核心概念层

## 2. 缺少独立的渠道归一化边界

第三版要求：

- adapter 负责把原生渠道事件整理成结构化归一化结果

当前代码则更接近：

- Telegram handler 做一部分
- gateway channel sink 做一部分
- gateway chat 再做一部分

差距在于：

- 渠道归一化边界还没有独立成型

## 3. 缺少统一的 session 语义主链

第三版要求：

- `type` / `scope` / 语义轴
  -> `session_key`
  -> 当前 `session_id`

当前代码则更接近：

- 渠道 account/chat 绑定
  -> active session
  -> 旧 session store

差距在于：

- 会话语义层还没有独立出来

## 4. 缺少统一事件记录层

第三版要求：

- 用 `session_event` 保存稳定事实

当前代码则更接近：

- 用会话文本消息保存运行结果与显示文本

差距在于：

- 保存层还没有从面向模型的会话文本中脱离出来

## 5. 缺少按 `type` 区分的上下文整理主链

第三版要求：

- 不同 `type` 有自己的上下文整理规则
- 上下文引擎负责 assemble / compact

当前代码则更接近：

- 在旧会话文本历史上继续增量拼接

差距在于：

- 还没有真正形成“按 `type` 整理上下文”的这一段职责

## 6. 缺少“渠道复杂策略”与“核心会话语义”的稳定隔离

第三版要求：

- relay / mirror / mention / thread 等复杂策略属于渠道实现问题
- 这些策略不应直接污染 core 公共概念与统一事件记录模型

当前代码则更接近：

- 复杂策略与会话语义、落盘文本、上下文文本互相牵连

差距在于：

- 复杂策略层还没有从基础会话主链中抽离

## 当前代码可以复用的部分

虽然距离第三版还有明显差距，但当前代码并不是要整体推倒。

当前仍有几块基础能力可以阶段性复用：

- 现有 `SessionStore`
- 现有 `PersistedMessage`
- 现有 `sessions` metadata / active session 记录
- 现有 chat run 主链
- 现有 Telegram 入站/出站协议处理

也就是说，当前代码最大的问题不是“没有东西”，而是：

- **已有能力的职责边界还没有收敛到第三版要求的形态**

## 这份现状文档的作用

这份文档的作用不是替代整体方案或实施路线图。

它的作用是单独回答两个问题：

- 当前代码主要处在什么形态
- 这些现状与第三版目标的主要差距在哪里

因此三份文档的分工应当是：

- `docs/src/refactor/v3-design.md`：讲第三版目标设计
- `docs/src/refactor/v3-gap.md`：讲当前代码现状与差距
- `docs/src/refactor/v3-roadmap.md`：讲第三版如何实施

## 相关文档

- `docs/src/refactor/v3-design.md`
- `docs/src/refactor/v3-roadmap.md`
- `docs/src/concepts-and-ids.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`
- `docs/src/refactor/session-context-layering.md`
- `docs/src/refactor/session-event-canonical.md`

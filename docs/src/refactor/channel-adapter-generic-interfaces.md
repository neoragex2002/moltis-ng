# Channel Adapter Generic Interfaces

本文档只定义一件事：

- 第三版里，渠道适配层与 core 之间，应该抽成哪些**通用接口**

本文档讨论的是：

- Rust 里如何表达这组抽象接口
- 四个边界面分别对应什么 trait
- 哪些对象应当做成跨渠道通用对象
- 哪些对象只能做成渠道私有 opaque ref
- TG 如何作为第一份实现接入

本文档不讨论：

- Telegram 具体实现细节
- 其他渠道的具体字段
- 最终版 `session_event` 全字段
- 一次性全局迁移顺序

也就是说，本文档讨论的是：

- **通用接口层**

而不是：

- **某个渠道自己的边界实现细节**

本文档与下列 Telegram 专项文档配套使用：

- `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`
- `docs/src/refactor/channel-info-exposure-boundary.md`
- `docs/src/refactor/telegram-adapter-boundary.md`

两者关系是：

- 先由 Telegram 专项边界压实真实需求
- 再从中提炼稳定的通用接口

如果两者暂时出现冲突，应先以主单 + 渠道边界专项文档为准，再回修本通用接口文档

这里再明确一条当前阶段的实施原则：

- 本文档不是当前阶段的直接施工蓝图
- 当前实现应先以 `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`、`docs/src/refactor/channel-info-exposure-boundary.md`、`docs/src/refactor/telegram-adapter-boundary.md` 为准
- 本文档更适合作为 Telegram 落地后的回看、提炼与校验材料
- C 阶段不应阻塞在“全渠道通用 trait 一次性全部落地”上
- 只要 Telegram 专项边界已经把 adapter / core 分工切清，就允许先以 TG-first 方式推进

## 一句话结论

Rust 有“抽象接口”的概念，对应的就是：

- `trait`

第三版里，渠道适配层与 core 之间，建议固定为四个边界面：

- 配置面
- 聊天面
- 控制面
- 回复面

但工程上不要做成**一个巨型 trait**，而应拆成：

- 小 trait
- 少量通用对象
- 渠道私有 opaque ref

一句话：

- **接口通用，语义通用，渠道细节不通用**

## 为什么这里应使用 Rust trait

在 Rust 里，如果想表达“这一层暴露什么能力，而不是怎么实现”，最合适的工具就是 `trait`。

在这里使用 `trait` 有三个直接好处：

- core 可以只依赖能力，不依赖某个具体渠道实现
- Telegram / Feishu / Slack 后续都可以按同一组边界实现
- 测试时可以直接用 mock adapter 替换真实渠道实现

但这里有一个重要约束：

- **不要为了抽象而抽象**

当前更合理的做法不是：

- 一开始设计一套很重的泛型体系
- 或一开始就把所有渠道的细节统一建模

而是：

- 先从 Telegram 专项边界压实真实需求
- 再回看哪些接口和对象真的足够稳定
- 其余渠道私有细节都收进 opaque ref

## 总体原则

### 1. 四个边界面固定

无论哪个渠道，和 core 的边界都应只落在这四类问题上：

- 配置怎么进入渠道适配层
- 聊天消息怎么进入 core
- 非聊天控制输入怎么进入 core
- core 产出的回复怎么回到渠道

### 2. 通用的是接口，不是所有字段

这里要明确：

- 通用的是 `trait`
- 通用的是少量上层对象

不通用的是：

- `chat_id`
- `message_id`
- `topic_id`
- `thread_id`
- `reply_to_message_id`
- relay hop / mirror key
- 各渠道自己的回包、重试、线程化细节

这些不应抬成公共字段。

### 3. 通用对象只保留稳定语义

跨渠道通用对象里，只保留 core 长期稳定需要的内容，例如：

- 聊天类型
- 进入模式
- 通用消息内容
- 逻辑路由结果

### 4. 渠道私有信息统一走 opaque ref

如果一段信息：

- 只有本渠道自己知道怎么解释
- core 只需要“保存 / 回传 / 再交还给渠道”

那它就不应展开成公共字段，而应进入：

- `MessageSourceRef`
- `ReplyTargetRef`

这样的 opaque ref 对象。

### 5. 长期的上下文分层归 core，当前 C 阶段入站最终文本由 adapter 提供

长期看，渠道适配层不应直接主导最终 LLM 可见 transcript 的分层与整理。

更合理的分工是：

- adapter 负责归一化原生事件
- adapter 负责返回路由语义
- core 负责最终的上下文分层 / compact / 长期记录结构

但当前 C 阶段 one-cut 的已冻结口径是：

- Telegram adapter 直接产出“最终可给模型看的入站文本”
- gateway/core 不再根据 Telegram 私有字段二次拼装 TG 群聊 transcript

也就是说，当前阶段并不要求先把这里落成一个统一的 `NormalizedMessage -> core renderer` 终态。

当前这一步仍可以先通过：

- `legacy persistence bridge`

来落地。

也就是说：

- 当前不要求先完成 `session_event` 持久化
- 也不要求先把所有通用 trait 都接进全系统
- 但要求先把“最终上下文由谁负责”这件事收口到 core

## 命名规则

这一组命名先冻结三条规则：

- trait 名使用**能力域名**
- 对象名使用**结果/请求/引用类型名**
- 方法名使用**动作动词**

对应到当前文档，就是：

- trait：
  - `ChannelConfig`
  - `ChannelMessageIngress`
  - `ChannelMessageRouting`
  - `ChannelControl`
  - `ChannelReplyDelivery`
- 对象：
  - `NormalizedMessage`
  - `MessageBody`
  - `MessageSourceRef`
  - `ResolvedRoute`
  - `ControlRequest`
  - `ControlResult`
  - `ReplyRequest`
  - `ReplyTargetRef`
  - `ReplyReceipt`
  - `SessionPolicy`
- 方法：
  - `apply_account_config`
  - `export_session_policy`
  - `normalize_message`
  - `resolve_route`
  - `handle_control`
  - `deliver_reply`

这里的命名意图是：

- trait 不强调 `Interface` / `Port` / `Manager`
- trait 直接表达“这一层提供哪类能力”
- 对象名一眼能看出它是：
  - 归一化结果
  - 解析结果
  - 请求
  - 回执
  - opaque 引用

## 四个边界面对应的 trait

这里说“四个边界面”，是按职责分层。

其中“聊天面”在工程实现上会拆成两个 trait：

- 入站
- 路由解析

这不代表多出第五个边界面；只是因为聊天面天然有两个方向的问题。

## 1. 配置面

建议抽象为：

```rust
#[async_trait]
pub trait ChannelConfig: Send + Sync {
    async fn apply_account_config(
        &self,
        account_key: &str,
        config: serde_json::Value,
    ) -> anyhow::Result<()>;

    async fn export_session_policy(
        &self,
        account_key: &str,
    ) -> anyhow::Result<SessionPolicy>;
}
```

### 这个接口回答什么

- 渠道配置如何应用到该渠道运行时
- core 需要的会话策略如何从渠道配置中导出

### 为什么这里不统一配置结构体

因为不同渠道的配置本来就不一样。

所以通用的应是：

- “怎么应用配置”
- “怎么导出 session policy”

而不是：

- 所有渠道共用同一个配置 struct

### 当前 TG 如何接入

TG 第一阶段完全可以继续复用：

- `TelegramAccountConfig`
- 现有 add/update/start 链路

也就是：

- 先不改配置来源
- 先不改配置存储
- 先只在代码职责上，让 TG 实现 `ChannelConfig`

## 2. 聊天面

聊天面建议拆成两个 trait：

- `ChannelMessageIngress`
- `ChannelMessageRouting`

原因很简单：

- 一部分问题是“原生消息如何归一化进入 core”
- 另一部分问题是“给定 scope 后，这条消息如何解析成 peer / sender / bucket”

这两个方向不应强行揉成一个方法。

### 2.1 `ChannelMessageIngress`

```rust
#[async_trait]
pub trait ChannelMessageIngress: Send + Sync {
    async fn normalize_message(&self) -> anyhow::Result<NormalizedMessage>;
}
```

这里的重点不是方法签名本身，而是：

- 聊天主链只接收归一化后的 `NormalizedMessage`
- 这仍是 future-facing 草案；当前 C 阶段 Telegram 落地以主单和专项边界文档里的最小 envelope 为准

### `NormalizedMessage`

> 下列结构是“后续通用接口提炼”的草案，不是当前 C 阶段 one-cut 必须逐字段落成的现网结构。

建议收敛为：

```rust
pub struct NormalizedMessage {
    pub channel_kind: ChannelKind,
    pub chat_kind: ChatKind,
    pub ingress_mode: IngressMode,
    pub body: MessageBody,
    pub source_ref: MessageSourceRef,
}
```

逐字段解释如下。

#### `channel_kind`

来源渠道，例如：

- `telegram`
- `feishu`

这是稳定的跨渠道语义，保留是合理的。

#### `chat_kind`

建议只保留：

- `dm`
- `group`

这里故意不掺：

- `cron`
- `heartbeat`

因为这里讨论的是**渠道聊天输入**，不是 core 全局所有触发类型。

#### `ingress_mode`

建议只保留：

- `dispatch`
- `record_only`

它只表达：

- 这条聊天输入是否要触发 run

它不表达：

- relay / mirror / native 这种渠道内部来历

#### `body`

建议收敛为：

```rust
pub struct MessageBody {
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub location: Option<LocationPayload>,
}
```

它回答的是：

- 这条消息的通用内容是什么

这里不应放：

- 原生 update
- file id
- mention entity
- thread/topic 原始对象
- 最终给模型看的 transcript 文本

#### `source_ref`

这是：

- **渠道私有 opaque ref**

它回答的是：

- 渠道后续如果要做 route 解析、回复路径恢复、幂等、去重、线程化，需要依赖哪些本地信息

core 对它的态度应当是：

- 可以持久化
- 可以回传给同一渠道 adapter
- 但不解析内部结构

### 2.2 `ChannelMessageRouting`

```rust
#[async_trait]
pub trait ChannelMessageRouting: Send + Sync {
    async fn resolve_route(
        &self,
        message: &NormalizedMessage,
        scope: SessionScope,
    ) -> anyhow::Result<ResolvedRoute>;
}
```

### `ResolvedRoute`

建议收敛为：

```rust
pub struct ResolvedRoute {
    pub peer: PeerRef,
    pub sender: Option<SenderRef>,
    pub bucket_key: String,
    pub addressed: bool,
}
```

#### `peer`

这是 core 语义结果。

它回答的是：

- 这条消息属于哪个逻辑对端

这里要明确：

- `peer` 不是从旧 session 反推出来的
- `peer` 不是从配置里直接写死的

而是由渠道 adapter 基于：

- 原生 source
- identity link
- 当前 `chat_kind`

解析出来的结果。

#### `sender`

这是当前消息的逻辑发言人。

允许为空。

为空的原因可能包括：

- 渠道协议边角
- 当前场景无法稳定解析 sender

这里不应为了满足某种 scope，硬在 core 里制造假 sender。

#### `bucket_key`

这是当前 `scope` 下，adapter 返回给 core 的稳定分桶结果。

它的定位应明确为：

- 同桶 / 异桶的实现结果

而不是：

- 上层概念来源

术语对齐说明：

- 在 `dm_scope` / `group_scope` 文档里，这个“黑盒分桶结果”也会被称为 `dm_subkey` / `group_subkey`
- 在工程实现里通常统一叫 `bucket_key`，表示“可持久化、可比对的具体编码值”

所以这里不应强行把：

- `account`
- `branch`
- `topic`
- `thread`

都展开成公共字段。

#### `addressed`

这是群聊语义结果。

它回答的是：

- 这条话在语义上是不是明确对 agent 说

它和 `ingress_mode` 不重复：

- `ingress_mode`：是否触发 run
- `addressed`：这条消息在群语义上是否对 agent 说

## 3. 控制面

建议抽象为：

```rust
#[async_trait]
pub trait ChannelControl: Send + Sync {
    async fn handle_control(
        &self,
        input: ControlRequest,
    ) -> anyhow::Result<ControlResult>;
}
```

这里不要求一开始就冻结完整字段表。

但职责边界应先明确：

- 控制面不属于聊天正文
- 不应混进 `ChatIngress`

典型包括：

- slash command
- inline callback
- OTP challenge / approval
- access denied feedback
- account health / runtime event

这里可以允许：

- `ControlRequest` 里保留更多 adapter-shaped 字段

因为它本来就不是会话主链对象。

## 4. 回复面

建议抽象为：

```rust
#[async_trait]
pub trait ChannelReplyDelivery: Send + Sync {
    async fn deliver_reply(
        &self,
        target: &ReplyTargetRef,
        reply: &ReplyRequest,
    ) -> anyhow::Result<ReplyReceipt>;
}
```

### `ReplyRequest`

它表达的是：

- core 想发什么

这里建议只放 core 关心的回复内容，例如：

- 文本
- 媒体
- 位置
- 流式回复

### `ReplyTargetRef`

这是另一类 opaque ref。

它表达的是：

- 往哪里回
- 用哪个本地 account 回
- 是否需要 reply_to 某条原消息
- 是否要带 thread/topic 细节

core 对它的态度也应与 `MessageSourceRef` 一致：

- 可以保存
- 可以回传
- 但不解析内部结构

## 通用对象与 opaque ref 的分层

这一层是整套方案里最关键的一点。

### 通用对象

建议长期冻结的通用对象主要是：

- `NormalizedMessage`
- `MessageBody`
- `ResolvedRoute`
- `ControlRequest`
- `ControlResult`
- `ReplyRequest`
- `ReplyReceipt`
- `SessionPolicy`

这些对象里只保留：

- 跨渠道稳定成立的语义

### opaque ref

建议长期保留为 opaque ref 的对象主要是：

- `MessageSourceRef`
- `ReplyTargetRef`

它们的职责是：

- 承载渠道私有信息
- 允许 core 持有与回传
- 不允许 core 依赖内部字段做长期语义判断

## opaque ref 在 Rust 里怎么落

这里不要一开始用一大堆复杂泛型。

当前阶段，更稳的落地方式是：

```rust
pub struct MessageSourceRef {
    pub channel_kind: ChannelKind,
    pub opaque: serde_json::Value,
}

pub struct ReplyTargetRef {
    pub channel_kind: ChannelKind,
    pub opaque: serde_json::Value,
}
```

这样做的好处是：

- object-safe
- 易于持久化
- 易于跨线程/跨组件传递
- 不要求 core 知道 TG / Feishu 私有结构

当前阶段不建议一开始就用：

- 大量 trait associated type
- 每个渠道一套不同泛型参数
- 让运行时注册系统被泛型形态绑死

## 核心职责与渠道职责

### core 应负责什么

- `scope` 语义
- `session_key` / `session_id`
- `NormalizedMessage + ResolvedRoute` -> `dm_record` / `group_record`
- 最终上下文整理与 renderer
- agent run / tool / skill / sandbox 主链

### 这里尤其要明确

群聊消息最终如何进入 LLM 上下文，应由：

- core 负责

更准确地说：

- adapter 提供 `NormalizedMessage`
- adapter 提供 `ResolvedRoute`
- core 生成 `group_record`
- core renderer 决定最终文本如何进入上下文

渠道适配层不应继续直接主导最终 transcript。

### adapter 应负责什么

- raw update / raw event 解析
- 原生媒体下载与归一化
- 原生 mention / reply / topic / thread 判别
- route 解析
- 回复投递目标恢复
- 回复投递
- 渠道私有重试、typing、threading、dedupe
- relay / mirror 这类渠道私有群策略

## TG 作为第一份实现

TG 作为第一份实现时，不需要先强改成“所有名字都通用”。

更合理的分层方式是：

- 通用接口层：本文档定义的 trait 与通用对象
- TG 实现层：`tg_*` 私有结构与实现逻辑

例如：

- TG 私有 `message_source_ref` 的内部 payload，可以继续落在 Telegram 专项文档里的 `tg_inbound.private_source`
- TG 私有 `reply_target_ref` 的内部 payload，可以继续落在 Telegram 专项文档里的 `tg_reply.private_target`

这不与通用接口冲突。

相反，这样更清楚：

- 哪些是接口层名字
- 哪些是 TG 自己的实现细节

## TG 第一阶段如何尽量复用现有代码

第一阶段建议尽量复用：

- `TelegramAccountConfig`
- 现有 channel add/update/start 流程
- `crates/telegram/src/outbound.rs`
- `crates/telegram/src/handlers.rs`
- `crates/telegram/src/bot.rs`

第一阶段不必先改：

- 配置来源
- 配置存储 schema
- 配置热更新入口

更合理的做法是：

- 先让 TG 代码按通用接口重新归类
- 再逐步把今天散在 `gateway` 里的 TG 逻辑回收

## 当前代码与通用接口的大致对应

### 现有粗接口

当前已有一批较粗的渠道接口，见：

- `crates/channels/src/plugin.rs:88`
- `crates/channels/src/plugin.rs:236`

它们今天的问题不是“没有抽象”，而是：

- chat / control / reply 仍混在一起
- `ChannelReplyTarget` 这类对象仍直接展开了很多渠道细节
- route 解析还没有单独成为一个独立接口

这里的 `ChannelReplyTarget` 引用属于 as-is 批判，不代表当前 C 阶段仍把它当正式跨层契约；当前 one-cut 目标是收敛到 `reply_target_ref`。

### 后续收敛方向

应逐步从当前粗接口，收敛到：

- `ChannelConfig`
- `ChannelMessageIngress`
- `ChannelMessageRouting`
- `ChannelControl`
- `ChannelReplyDelivery`

## 当前阶段的冻结结论

这一轮先冻结以下几点：

### 1. 用 trait 表达通用接口

Rust 里完全可以，而且应该这样做。

### 2. 四个边界面固定

- 配置面
- 聊天面
- 控制面
- 回复面

### 3. 聊天面拆成两个 trait

- 入站归一化
- 路由解析

### 4. 通用对象与 opaque ref 明确分层

通用对象只保留稳定语义；
渠道私货统一进 `MessageSourceRef` / `ReplyTargetRef`。

### 5. TG 作为第一份实现

先实现通用接口；
TG 私有细节继续保留 `tg_*` 命名，不强装通用。

### 6. C 阶段不以持久化替换为前置条件

当前 C 阶段可以先做到：

- `NormalizedMessage` / `ResolvedRoute` 语义收口
- reply / control / route 边界收口
- core context bridge 收口

而暂不要求：

- `session_event` 已经落地
- legacy persistence bridge 已经被删除

## 相关文档

- `docs/src/refactor/telegram-adapter-boundary.md`
- `docs/src/refactor/v3-design.md`
- `docs/src/refactor/v3-gap.md`
- `docs/src/refactor/v3-roadmap.md`
- `docs/src/refactor/dm-scope.md`
- `docs/src/refactor/group-scope.md`
- `docs/src/refactor/session-context-layering.md`

# 概念与标识符

本文档定义 Moltis 的核心概念，以及代表这些概念的标识符（identifier）。

目标：当你看到一个字段名/变量名时，你应该立刻知道它的语义、允许用途、以及
必须禁止的用途。

```admonish warning title="术语冻结（Authoritative）"
本文档中的名称与一行定义是权威口径。
需要新增概念时，必须先更新本文档再改代码；严禁在代码/文档/协议里偷偷引入
“再加一个 alias 先凑合”的做法。
```

```admonish danger title="禁止 alias（硬规则）"
禁止为同一概念提供多个对外名字（alias）。

例如：禁止再出现 `sessionKey` 既表示 `sessionId` 又表示 `chanChatKey`；禁止再
出现 `tool_session_key` 这种第三套命名；禁止继续扩散 `account_id` 这类历史字段。

如果兼容历史数据/旧 payload 必不可少，也只能做“输入解析兼容”，并且必须保证：
对外输出、文档示例、UI 显示只出现冻结后的名字。
```

## 核心概念（已冻结）

核心概念定义了“身份坐标”和“路由边界”。只有核心概念才允许回答如下问题：

- 对话历史存在哪？
- 这条消息来自哪个渠道的哪个 chat/thread？
- 当前说话的是哪只 bot/哪个渠道账号配置？
- 回复应该发回到哪里？

### `sessionId`

**一句话：**持久会话桶 / 存储地址。

- **用于：**消息历史存储、媒体存储路径、会话 metadata、分叉/复制（fork/branch）。
- **典型取值：**`main`、`session:<uuid>`。
- **禁止用于：**推断渠道身份（bot/chat/thread），也禁止用于任何“确定性跨域路由”。

### `chanChatKey`

**一句话：**确定性对话坐标（跨域桥）。

- **格式：**`<chanType>:<chanUserId>:<chatId>[:<threadId>]`。
- **用于：**确定性路由、绑定、可观测性，以及“同一 chat 共享边界”的可选分桶策略。
- **禁止用于：**持久存储身份（它无法表达 fork/branch 产生的多个并行会话）。

### `chanAccountKey`

**一句话：**渠道账号稳定主键。

- **格式：**`<chanType>:<chanUserId>`。
- **用于：**标识“是哪只 bot / 哪个渠道账号配置”在执行/说话。
- **禁止用于：**展示/UX 命名（展示名属于可变字段）。

```admonish important title="内部命名也要对齐（snake_case 版本）"
内部实现（Rust/DB/schema）允许使用 `snake_case`，但**概念名必须与本文档对齐**。

- Rust 代码（变量/struct 字段）优先使用 `chan_account_key`（语义 = `chanAccountKey`），避免新增
  `account_handle/account_id` 这类历史名。
- DB/schema 现存列名可能仍是 `account_handle/channel_type/session_key`（legacy 存储名）。短期允许
  保留以减少迁移 churn，但它们不得向上层协议/服务层传播；对外仍只使用冻结字段名。
- 同理：`chanChatKey`→`chan_chat_key`，`chanType`→`chan_type`，`chanReplyTarget`→`chan_reply_target`。
```

### `chanType`

**一句话：**渠道类型 / 平台标识。

- **示例：**`telegram`、`discord`。
- **用于：**组成 `chanAccountKey` 与 `chanChatKey`。

### `chanUserId`

**一句话：**渠道账号本体的稳定唯一 ID。

- **Telegram：**bot 的 `getMe.id`。
- **用于：**稳定身份；仅此而已。

### `chanUserName`

**一句话：**人类可读的渠道用户名（可变、可空）。

- **Telegram：**bot 的 `getMe.username`（展示时格式化为 `@{chanUserName}`）。
- **用于：**UI、日志、调试。
- **禁止用于：**任何 key、路由、存储、绑定。

### `chanNickname`

**一句话：**展示名/昵称（可变、可空）。

- **用于：**UI。
- **禁止用于：**任何 key、路由、存储、绑定。

### `chanReplyTarget`

**一句话：**可执行的“回信地址对象”。

`chanReplyTarget` 表示把回复发回渠道所需的最小信息集合。

- **必须包含：**`chanType`、`chanAccountKey`、`chatId`。
- **可以包含：**`messageId`（用于 threaded reply / reply-to）。
- **禁止包含（逻辑关键）：**任何展示字段（`chanUserName`、`chanNickname`）。展示字段
  可以单独用于 UI，但不能参与“回信地址”的逻辑判定。

### `chatId`

**一句话：**渠道内 chat/peer 的稳定标识。

- **用于：**定位消息发送目标。

### `messageId`

**一句话：**渠道内某条消息的标识（可选）。

- **用于：**threaded reply / reply-to。

## 对外字段命名风格（冻结）

**对外（RPC / WebSocket / Hooks / UI / 文档示例）字段名统一使用 `camelCase`。**

- **允许：**仅输出冻结字段名（例如 `sessionId`、`chanChatKey`、`chanAccountKey`）。
- **禁止：**同时输出两套命名（例如 `accountHandle` + `account_handle`；或 `sessionId` +
  `session_id`）。
- **兼容：**如需兼容历史 payload，只能在“输入解析”层做 `snake_case` 的短期兼容，并且
  必须有明确的移除截止线。

这条规则的目的就是：防止“同一概念多个名字”长期共存，导致团队心智模型无法收敛。

## 工具上下文字段（Tool Context Keys）

工具调用会收到系统注入的上下文字段（不是用户输入）。

### `_sessionId`

**一句话：**本次工具调用所属的持久会话桶。

- **用于：**读写会话历史、metadata、媒体、以及 per-session state。

### `_chanChatKey`

**一句话：**本次工具调用所属的确定性对话坐标。

- **出现条件：**当工具调用来自 channel-bound 交互时存在。
- **用于：**判断渠道来源、可选的 sandbox 分桶策略、channel 特定工具行为。

```admonish note title="工具上下文：禁止 alias"
工具上下文字段禁止引入额外 alias 名称（例如 `tool_session_key`）。
如需兼容旧 payload，只能做“输入解析兼容”，文档与对外输出必须只发冻结字段。
```

## 非核心、但独立的重要标识符

这些标识符很有用，但它们不属于“身份/路由坐标系”，不能替代核心概念。

为了避免心智模型混乱，可以把它们按“用途/生命周期”分为四类：

1) **记录 ID（record id）**：标识“某条内部记录”，不标识会话本体
2) **游标/序号（cursor/index）**：用于 UI 去重/排序诊断，通常不持久
3) **展示元信息（display/meta）**：帮助 UI 呈现，禁止参与身份/路由
4) **内部路由键（route key）**：仅用于内部路由/绑定，禁止对外

> 规则：当你需要“身份坐标”时，永远回到核心概念：`sessionId` / `chanChatKey` /
> `chanAccountKey` / `chanReplyTarget`。

另外一条经验法则：

- **核心概念回答“地址/坐标/回复目标”**（会影响路由/存储/隔离边界）
- **非核心标识回答“记录/游标/显示/追踪”**（只用于 UI/排障/策略，不得当作地址）

### `connId`

**一句话：**短生命周期的 WebSocket 连接标识。

- **生命周期：**只在 WS 连接存活期间有效，断线即失效。
- **用于：**浏览器侧工具协作（例如请求定位）、连接级默认值。
- **禁止用于：**会话身份、用户身份、路由、存储、任何持久映射。

### `runId`

**一句话：**一次 agent/LLM run 的短生命周期标识。

- **生命周期：**一次 run（一次流式响应），用于取消、诊断、错误关联。
- **用于：**tracing、错误报告、流式事件关联。
- **禁止用于：**会话身份、存储桶命名、任何需要跨 run 长期存在的映射。

### `projectId`

**一句话：**绑定在 `sessionId` 上的工程上下文选择。

- **用于：**解析项目文件/上下文、worktree 操作。
- **关系：**`sessionId -> projectId`（一个 session 可能有也可能没有 project）。
- **禁止用于：**会话身份。

### `worktreeBranch`

**一句话：**绑定在 `sessionId` 上的 git 分支/worktree 名。

- **用于：**将 session 映射到一个可用于改代码的 worktree。
- **关系：**`sessionId -> worktreeBranch`。
- **禁止用于：**身份、路由、存储。

### `sessionEntryId`

**一句话：**会话 metadata 记录自身的内部 ID（不是会话桶地址）。

- **背景：**当前 `SessionEntry` 同时存在 `id` 与 `key` 两个字段。
  - `key`（内部名）语义等价 `sessionId`（持久会话桶地址）。
  - `id` 更像是“这条 metadata 记录的 UUID”。
- **用于：**内部记录/索引/排障（若确有需要）。
- **禁止用于：**替代 `sessionId`；也不要在协议/路由/存储中作为会话身份。

### `messageIndex`

**一句话：**会话内消息序号/游标（用于去重与 UI 逻辑，不是消息身份）。

- **用于：**UI 去重、重放、历史与实时流的对齐。
- **禁止用于：**身份（不是 `messageId`，也不是 `sessionId`）。

> 直觉类比：`messageIndex` 是“第几条消息 / 第几个位置”，不是“这条消息的身份证”。

### `clientSeq`

**一句话：**客户端序列号（仅用于排序诊断/排障）。

- **用于：**在网关与 UI 之间诊断乱序/重放。
- **禁止用于：**任何持久化身份。

> 直觉类比：`clientSeq` 是“这个浏览器标签页发来的第 N 次请求序号”，页面刷新会重置。

### `chanMessageMeta`

**一句话：**渠道入站消息的“展示/提示元信息”（用于 UI 展示与提示，不是身份坐标）。

`chanMessageMeta` 表示“这条消息来自哪个渠道、谁发的、消息种类是什么、默认模型是什么”等
UI 展示友好的信息。

- **用于：**UI 展示、调试提示。
- **禁止用于：**构造 `sessionId`/`chanChatKey`/`chanAccountKey`；也禁止用于任何路由与存储。

> 直觉类比：`chanMessageMeta` 是“UI 角标/提示信息”，不是“坐标/地址”。

### `routeSessionKey`（Legacy）

**一句话：**内部路由键（历史名 `SessionKey`，与 `sessionId/chanChatKey` 无关）。

- **背景：**仓库中存在一个内部类型名叫 `SessionKey`，其格式类似
  `agent:<id>:channel:<...>:account:<...>:peer:<...>`。
- **用于：**内部“消息 → agent”路由/绑定级的 key（如果仍在使用）。
- **禁止用于：**对外协议；也不得替代 `sessionId` 或 `chanChatKey`。
- **建议：**后续实现阶段应把类型/字段名从 `SessionKey/session_key` 改为 `routeSessionKey/route_session_key`，避免与本文档冻结概念冲突。

### `peerId`

**一句话：**渠道内“消息发送者（人类用户）”的稳定标识（不是 bot）。

- **用于：**访问控制（allowlist）、按发送者分层策略（例如 tools policy）、审计日志。
- **禁止用于：**替代 `chanUserId`（bot 身份）或 `chanAccountKey`（bot 配置主键）。

> 直觉类比：`chanUserId` 是“机器人身份证”，`peerId` 是“群里某个人的身份证”。

### `operatorId`

**一句话：**Web UI / 运维操作者身份标识（非渠道 peer）。

- **用于：**权限控制、审计、passkey/WebAuthn 账号标识。
- **禁止用于：**替代 `peerId`（渠道人类发送者）或 `chanUserId`（bot 身份）。

> 直觉类比：`operatorId` 是“后台登录用户”，`peerId` 是“Telegram/Discord 里的发信人”。

### `peerUserName` / `peerDisplayName`

**一句话：**渠道内“消息发送者”的展示名（可变、可空）。

- **用于：**UI 展示、日志辅助。
- **禁止用于：**任何 key、路由、存储、绑定。

### `chatType`

**一句话：**渠道内对话类型分类（用于策略与展示，不是身份坐标）。

- **示例：**dm / group / channel。
- **用于：**访问控制与策略分层（例如群聊需要 mention gating）。
- **禁止用于：**构造 `sessionId` 或 `chanChatKey`。

### `groupId`（Legacy）

**一句话：**群/频道容器 ID（历史字段；在 Telegram 中通常等价 `chatId`）。

- **用于：**策略分层（per-group policy）与旧代码兼容。
- **禁止用于：**替代 `chatId`（新口径中应直接使用 `chatId`）。

## 非核心标识符：快速选用指南

下面这张“选型表”用于快速判断某个字段该不该用、该用在哪。

- 需要定位历史/媒体/状态存储桶 → 用 `sessionId`
- 需要定位渠道对话坐标（bot+chat+thread）→ 用 `chanChatKey`
- 需要把回复发回渠道 → 用 `chanReplyTarget`
- 需要在 UI 里把实时消息与历史对齐/去重 → 用 `messageIndex`（同时必须带 `sessionId`）
- 需要诊断浏览器侧乱序/重放 → 用 `clientSeq`（同时通常带 `connId`）
- 需要引用一条 metadata 记录本身 → 用 `sessionEntryId`（很少需要）
- 需要内部“消息 → agent”路由键 → 用 `routeSessionKey`（仅内部，禁止对外）
- 需要在策略里区分“谁发的” → 用 `peerId`（不是 bot）
- 需要按群/频道分层策略 → 用 `chatId`（不要再用 `groupId`）

## 常见结构体的口径映射（Legacy → Frozen）

这一节用于快速识别“旧结构体字段名”的真实语义，避免把旧名词继续扩散。

### `MsgContext`（legacy inbound message context）

仓库中存在一个 `MsgContext`（注释：mirrors TypeScript），它把旧术语打包在一起。
它不是核心概念的一部分，但它的字段需要明确映射到冻结概念。

- `channel` → 语义等价 `chanType`（平台类型），不应理解为“channel 对象”。
- `account_handle` → 语义等价 `chanAccountKey`（Rust 上层应优先使用 `chan_account_key`；DB 列名可能仍为 legacy）。
- `session_key` → **历史漂移字段**：必须拆分为 `sessionId` + `chanChatKey`。
- `reply_to_id` → 语义等价 `messageId`。
- `from`（PeerId）→ 语义等价 `peerId`（发送者，不是 bot）。
- `sender_name` → 语义等价 `peerDisplayName`（display-only）。
- `group_id/guild_id/team_id` → 平台/组织容器维度（非核心；策略可用，但不要当身份坐标）。

> 规则：任何新代码不得继续扩展 `MsgContext` 的旧字段；应直接使用冻结概念字段名。

### 常见 legacy type alias

`crates/common/src/types.rs` 中存在一些历史 type alias，其语义应按冻结概念理解：

- `AccountHandle` → 语义等价 `chanAccountKey`（内部应逐步改名，不再叫 handle；允许 DB 层 legacy 名继续存在一段时间）
- `ChannelId` → 语义等价 `chanType`
- `PeerId` → 语义等价 `peerId`

### `PolicyContext`（tools policy layering）

工具策略分层上下文里目前使用了 `channel/group_id/sender_id` 这样的旧字段：

- `channel` → 语义等价 `chanType`
- `group_id` → 通常语义等价 `chatId`（per-group policy 的 group 就是 chat）
- `sender_id` → 语义等价 `peerId`

建议后续实现阶段把这些字段也改成更自解释的名称（例如 `chan_type/chat_id/peer_id`）。

## 默认策略（冻结）

本节把“哪些地方用 `sessionId`，哪些地方用 `chanChatKey`”说清楚，避免后续实现
各自为政。

### Prompt Cache 分桶

**结论：prompt cache bucket key = `sessionId`。**

- 理由：`sessionId` 才是“真实上下文边界”（同一 `sessionId` 才属于同一会话桶）；在同
  一 `chanChatKey` 下 `/new` 切出来的多个会话不应共享同一个 prompt-cache bucket。
- 备注：上游 provider 的字段名/实现可能仍写作 `session_key`（历史遗留），但其语义必须
  等价于本文档定义的 `sessionId`。

### Sandbox Container Reuse Key

**结论：sandbox 容器复用边界由配置 `tools.exec.sandbox.scope_key` 冻结，默认 = `session_id`。**

- `session_id`：每个 `sessionId` 独立容器（最强隔离，默认）。
- `session_key`：同一逻辑会话桶共享容器（更强复用，但隔离更弱）。
- 严格 one-cut：`tools.exec.sandbox.scope` 属于 legacy 字段，已删除；命中必须配置校验失败（不做兼容）。

## 过载词汇（Overloaded Words）【必须避免】

以下词汇在代码库里被多个子系统复用，极易造成“同词不同义”的沟通与实现事故。

### `scope`

**规则：文档与协议中禁止裸写 `scope`，必须写全称。**

- `sandboxScopeKey`：容器复用边界（`session_id`/`session_key`；由 `tools.exec.sandbox.scope_key` 冻结）。
- `authScope`：权限范围（例如 `operator.read`/`operator.write`）。
- `throttleScope`：限流范围（login/api/ws/...）。

配置文件中不得出现 legacy `tools.exec.sandbox.scope`；sandbox 容器复用边界必须通过
`tools.exec.sandbox.scope_key` 明确表达（例如 `[tools.exec.sandbox] scope_key = "session_id"`）。

### `channel`

**规则：对外禁止使用泛化字段名 `channel`。**

必须拆成以下之一：

- 仅平台类型：用 `chanType`。
- 可执行回信地址：用 `chanReplyTarget`。
- 仅 UI 展示/提示元信息：用 `chanMessageMeta`。

### `key`

**规则：对外禁止使用泛化字段名 `key` 表示会话。**

必须使用 `sessionId`（持久会话桶）或 `chanChatKey`（渠道坐标），禁止模糊化。

### `handle`

**规则：对外与内部都避免用 `handle` 表示稳定主键。**

- 稳定主键：用 `chanAccountKey` / `chan_account_key`。
- 展示名：用 `chanUserName/chanNickname`。

### `user` / `sender`

**规则：避免使用笼统的 `user_id/sender_id` 作为长期概念名。**

必须明确“是 bot 还是人类发送者”：

- bot（渠道账号本体）→ `chanUserId` / `chanAccountKey`
- 人类发送者（peer）→ `peerId`

`sender_id` 属于 legacy 字段名：

- 当它出现在“渠道消息/群聊策略/命令触发者”语境下，语义应等价 `peerId`（内部 `peer_id`）。
- 如果未来需要表达“Web UI 登录用户/运维操作者”的身份，不得复用 `peerId`，应引入新的
  非核心概念（例如 `operatorId`），避免同名不同义。

## 示例（对外 payload）

下面示例展示“对外输出只用冻结字段名”的形态。展示字段是可选的，核心字段必须语义稳定。

```json
{
  "sessionId": "session:550e8400-e29b-41d4-a716-446655440000",
  "chanChatKey": "telegram:8576199590:-1001234567890:12",
  "chanAccountKey": "telegram:8576199590",
  "chanType": "telegram",
  "chanUserId": "8576199590",
  "chanUserName": "lovely_apple_bot",
  "chanNickname": "Apple Bot",
  "chatId": "-1001234567890",
  "messageId": "42",
  "chanReplyTarget": {
    "chanType": "telegram",
    "chanAccountKey": "telegram:8576199590",
    "chatId": "-1001234567890",
    "messageId": "42"
  }
}
```

## 关系总结（人类心智模型）

- 一个 **channel chat** 有一个稳定坐标：`chanChatKey`。
- 该 chat 默认指向一个 **当前活跃的持久会话桶**：`sessionId`。
- 同一个 `chanChatKey` 在时间维度上可以对应多个 `sessionId`（例如 `/new` 或 fork/branch），
  但任一时刻只有一个活跃 `sessionId`。
- 渠道回复使用 `chanReplyTarget`（可执行地址）。

## 常见误区（必须避免）

- 把 `chanUserName` / `chanNickname` 当作稳定标识。
- 把 `connId` 当作用户/会话标识。
- 把 `runId` 当作会话标识。
- 用一个含混的名字（例如 `sessionKey`）同时表示 `sessionId` 与 `chanChatKey`。

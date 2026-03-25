# Session Key / Bucket Key One-Cut Canonical Design

更新时间：2026-03-25

## 1. 目标与结论

本文只做一件事：

- 一刀切冻结 `bucket_key`、`session_key`、`session_id`、`run_id` 的定义、分层、命名语法和示例

本文是本轮 Session Key 专项治理的设计真源。  
后续 issue、实现、测试、review，均以本文为唯一设计事实来源。

若本文与旧 refactor 文档、旧 V3 issue、旧实现注释冲突：

- 一律以本文为准
- 旧文档与旧 issue 只作为历史背景和证据，不再与本文并列定义规则
- 当前治理主单为：
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`

本文采用以下强约束：

- 第一性原则：每个 key 只负责一个职责
- 不后向兼容：旧格式直接淘汰，不保留 alias / fallback / silent degrade
- 唯一事实来源：同一语义只允许一个权威 key
- 收敛优先：不为未来预留多余命名空间，不把复杂度外溢到系统层

**最终结论：**

1. 系统里只保留 4 个核心标识：
   - `bucket_key`
   - `session_key`
   - `session_id`
   - `run_id`
2. `session_key` 只分两大命名空间：
   - `agent:<agent_id>:<bucket_key>`
   - `system:<service_id>:<bucket_key>`
3. `session_id` 必须是 opaque 实例 id，不得再承载语义
4. `run_id` 必须是 opaque 执行 id，不得再冒充 session
5. 没有持久上下文复用需求的执行，不得再发明“execution-only session id”；这类执行只有 `run_id`
6. Telegram 适配层 `bucket_key` 必须 typed，且必须自描述；因为 Telegram 内部已经会单独存取它
7. `prompt_cache_key` 不是第 5 个 core runtime key；它只是 provider 南向缓存桶，由调用方显式决定，provider 不得自造 fallback

### 1.1 当前落地冻结（2026-03-25）

本轮实现已经按本文冻结以下关键路径：

- Web 默认主会话只允许走服务端 owner path：
  - `sessions.home`
  - 例子：首屏无本地态时，服务端返回 `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
- Web 新建会话只允许走服务端 create path：
  - `sessions.create`
  - 例子：前端点击“New Chat”后，服务端返回 `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e75`
- session-scoped consumer 缺失上下文时必须直接拒绝：
  - 例子：`chat.send` / `chat.sendSync` / `exec` / `process` / `sandbox_packages` / `spawn_agent` 缺 `_sessionId` 时直接报错
- channel `/new` 必须在原 canonical `session_key` 下创建新的 opaque session instance：
  - 例子：旧实例 `sess_current`
  - 例子：逻辑桶 `agent:zhuzhu:dm-peer-tguser.123-account-tguser.845`
  - 例子：执行 `/new` 后新实例 `sess_0195f3c5b3d27d8aa91e4439bb3c2e88`
- sandbox runtime naming 固定为 canonical 派生名：
  - 例子：`session_key = agent:zhuzhu:main`
  - 例子：`effective_sandbox_key = agent:zhuzhu:main`
  - 例子：`container_name = msb-agent-zhuzhu-main-7ab31c2d`
- Web/UI 会话展示名只允许由服务端计算：
  - 例子：`displayName = Main`
  - 例子：`displayName = TG @lovely_apple_bot · dm:123`
  - 例子：`displayName = Heartbeat`
- 用户配置层不再接受 `tools.exec.sandbox.container_prefix`
  - 例子：模板里不再出现该字段；配置校验与运行时也不再把它当可配置项

---

## 2. 为什么必须重做

当前代码库已经出现三类混层：

### 2.1 `session_id` 被当成语义 key 使用

代码现状里，下面这些都被当成 `session_id` 使用：

- `main`：`crates/gateway/src/chat.rs:1930`
- `cron:<name>`：`crates/gateway/src/server.rs:1424`
- `probe:<provider>:<model>`：`crates/gateway/src/chat.rs:640`
- `models.test:<model>`：`crates/gateway/src/chat.rs:1697`
- `provider_setup:<provider>:<model>`：`crates/gateway/src/provider_setup.rs:1760`
- `tts.generate_phrase:<context>`：`crates/gateway/src/methods.rs:2553`

这不对。  
`session_id` 的职责应该只是“当前会话实例是谁”，不应该携带“它属于哪个逻辑桶、哪个系统服务、哪个用途”。

### 2.2 Telegram `bucket_key` 被跨层当成 `session_key`

代码现状里：

- Telegram adapter 生成 `bucket_key`：`crates/telegram/src/adapter.rs:1192`
- gateway 直接把它塞进 `session_key`：`crates/gateway/src/channel_events.rs:60`
- `ChannelInboundContext.session_key` 注释实际已承认这东西更接近 “bucket key”：`crates/channels/src/plugin.rs:91`

这意味着：

- adapter 层 `bucket_key`
- 系统层 `session_key`
- 会话实例层 `session_id`

三者已经塌成一团。

### 2.3 Telegram 内部也会单独复用裸 `bucket_key`

这也是为什么 Telegram `bucket_key` 不能是“只靠外围上下文才解释得通”的半成品。

当前代码现状：

- callback 绑定表直接存裸 `bucket_key`：`crates/telegram/src/handlers.rs:67`
- callback 回来直接取回裸 `bucket_key`：`crates/telegram/src/handlers.rs:3341`
- adapter helper 直接比较 `bucket_key` 是否相等：`crates/telegram/src/adapter.rs:1106`
- 还有代码从 `bucket_key` 反解析 `sender`：`crates/telegram/src/handlers.rs:105`

所以，Telegram `bucket_key` 必须自描述，不能依赖系统层再帮它补 type。

---

## 3. 核心分层

### 3.1 `bucket_key`

**定义：**

- 某个“桶语义拥有者”产出的本地逻辑分桶键

**职责：**

- 回答“这条输入属于哪个逻辑桶”

**不负责：**

- 不负责表达会话实例
- 不负责表达执行实例
- 不负责跨全系统命名空间

**例子：**

- Agent 主会话 bucket：`main`
- Agent 手工侧聊 bucket：`chat-01jv5n5x9x4m`
- Telegram DM bucket：`dm-peer-person.neoragex2002`
- Telegram Group bucket：`group-peer-tgchat.n1001234567890-branch-topic.42`
- Cron heartbeat bucket：`heartbeat`
- Cron job bucket：`job-01jv62d6h1k7`

### 3.2 `session_key`

**定义：**

- 全系统唯一的“逻辑会话桶名”

**职责：**

- 回答“这个逻辑桶在系统里叫什么”
- 作为 session-scoped sandbox / bucket-scoped policy / active-session 命中的权威名字

**不负责：**

- 不代表当前具体会话实例

**例子：**

- `agent:zhuzhu:main`
- `agent:zhuzhu:chat-01jv5n5x9x4m`
- `agent:zhuzhu:dm-peer-person.neoragex2002`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890-sender-person.neoragex2002`
- `system:cron:heartbeat`
- `system:cron:job-01jv62d6h1k7`

### 3.3 `session_id`

**定义：**

- 某个逻辑桶当前或历史上的具体会话实例 id

**职责：**

- 只回答“这次具体会话实例是谁”

**强约束：**

- 必须 opaque
- 不得携带业务语义
- 不得再出现 `main` / `cron:*` / `session:*` / `provider_setup:*` 这类语义字符串

**建议形态：**

- `sess_<opaque>`

**例子：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
- `sess_0195f3c5b3d27d8aa91e4439bb3c2e75`

### 3.4 `run_id`

**定义：**

- 一次执行的实例 id

**职责：**

- 只回答“这一轮 run 是谁”

**强约束：**

- 必须 opaque
- 不得冒充 session
- 不得携带 provider / tool / cron / tts 这类业务语义

**建议形态：**

- `run_<opaque>`

**例子：**

- `run_0195f3c5b3d27d8aa91e4439bb3c2e90`
- `run_0195f3c5b3d27d8aa91e4439bb3c2e91`

---

## 4. 权威映射关系

系统里只允许以下权威关系：

### 4.1 `session_key -> active session_id`

含义：

- 某个逻辑桶当前激活的会话实例是谁

例子：

- `agent:zhuzhu:main -> sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890 -> sess_0195f3c5b3d27d8aa91e4439bb3c2e81`
- `system:cron:heartbeat -> sess_0195f3c5b3d27d8aa91e4439bb3c2ea2`

### 4.2 `session_id -> session metadata`

含义：

- 某个具体会话实例的元数据、历史、绑定、分支、工作区、sandbox image 等

例子：

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74 -> { label, model, project, worktree, ... }`

### 4.3 `run_id -> ephemeral run state`

含义：

- 某一轮执行过程中的临时状态

例子：

- `run_0195f3c5b3d27d8aa91e4439bb3c2e90 -> tool status / stream state / metrics`

### 4.4 明确禁止的权威关系

以下都不得再作为系统真值：

- `channel_type + account_handle + chat_id -> session_id`
- `channel_type + bucket_key -> session_id`
- “把语义型 `session_id` 当成 `session_key`”
- “把 `run_id` 当成 `session_id`”

这些都属于当前 legacy 技术债，必须在专项治理中切掉。

### 4.5 持久化与运行时合同（冻结目标）

仅有“逻辑关系正确”还不够。  
本专项还必须把持久化与运行时 API 的目标形状一起钉死，否则后续实现会继续在
`session_key` / `session_id` 之间反复返工。

#### 4.5.1 `active_sessions`

用途：

- 承载全系统唯一真值：`session_key -> active session_id`

推荐形状：

- `active_sessions(session_key PRIMARY KEY, session_id, updated_at)`

例子：

- `agent:zhuzhu:main -> sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
- `agent:zhuzhu:dm-peer-person.neoragex2002 -> sess_0195f3c5b3d27d8aa91e4439bb3c2e81`
- `system:cron:heartbeat -> sess_0195f3c5b3d27d8aa91e4439bb3c2ea2`

强约束：

- 所有 active-session 读写都只能经过这条映射
- `channel_sessions`
- `session_buckets`

以上 legacy 表/映射不得再参与主路径判定

#### 4.5.2 `sessions`

用途：

- 承载 `session_id -> session metadata`

推荐形状：

- `sessions(session_id PRIMARY KEY, session_key, label, model, project_id, ..., parent_session_id, channel_binding, updated_at, ...)`

例子：

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74 -> { session_key: "agent:zhuzhu:main", label: "Main", model: "gpt-5.2", project_id: "proj_alpha" }`
- `sess_0195f3c5b3d27d8aa91e4439bb3c2e81 -> { session_key: "agent:zhuzhu:group-peer-tgchat.n1001234567890", channel_binding: "{...}", parent_session_id: "sess_0195f3c5b3d27d8aa91e4439bb3c2e74" }`

强约束：

- `session_id` 是这张表唯一实例标识
- `session_key` 必须作为该实例所属逻辑桶显式持久化
- hooks、session management、sandbox `scope_key=session_key`、channel observability 需要从这里读取实例归属的 `session_key`
- 不允许继续保留 `key` / `id` 双 id 语义
- 不允许 `SessionEntry.key = session_id`、`SessionEntry.id = 冗余副本` 这种双写模型
- `parent_session_key` 必须硬切成 `parent_session_id`

#### 4.5.3 `session_state`

用途：

- 承载“会话实例级状态”

推荐形状：

- `session_state(session_id, namespace, key, value, updated_at)`

例子：

- `(sess_0195f3c5b3d27d8aa91e4439bb3c2e74, "calendar", "last_city") -> "Shanghai"`

强约束：

- 这是实例级状态，不是逻辑桶共享状态
- 因此它必须按 `session_id` 建模
- `session_state.session_key` 属于必须切掉的旧形状

#### 4.5.4 `session_store` / media store

用途：

- 承载会话实例历史
- 承载会话实例媒体目录
- 承载 session search 命中结果

推荐形状：

- `SessionStore(session_id)`
- `media/<session_id>/<filename>`
- `SearchResult { session_id, snippet, role, message_index }`

例子：

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74.jsonl`
- `media/sess_0195f3c5b3d27d8aa91e4439bb3c2e74/run_0195f3c5b3d27d8aa91e4439bb3c2e90.ogg`

强约束：

- `SessionStore` 只允许按 `session_id` 读写
- 文件名与媒体目录必须直接使用原始 `session_id`
- 不得再使用 `:` ↔ `_` 这类基于分隔符猜测的可逆伪编码
- `search()` / `list_keys()` / media path 返回的都必须是原始 `session_id`

原因：

- `session_id` 已经是 opaque 且文件系统安全的实例标识
- 当前 `:` ↔ `_` 方案会把 `sess_<opaque>` 误反解成 `sess:<opaque>`，直接破坏实例 id

#### 4.5.5 运行时 API 目标

运行时 owner API 也必须跟着同构收口：

- `get_session(session_id)`：按实例读取 metadata
- `create_session(session_key, ...) -> session_id`：由服务端创建实例并返回实例 id
- `update_session(session_id, ...)`：按实例更新 metadata
- `get_active_session_id(session_key)`：按逻辑桶读取当前实例
- `set_active_session_id(session_key, session_id)`：按逻辑桶写当前实例

例子：

- Telegram inbound 已经算出：
  - `bucket_key = group-peer-tgchat.n1001234567890`
  - `session_key = agent:zhuzhu:group-peer-tgchat.n1001234567890`
- 此时系统主路径只允许：
  - 先查 `get_active_session_id(session_key)`
  - 命中则得到 `sess_...`
  - 未命中则新建 `sess_...`，随后 `set_active_session_id(session_key, sess_...)`

- Web UI 用户点击“新会话”：
  - 客户端不得本地生成 `session:uuid`
  - 客户端必须请求服务端 owner create path（例如 `sessions.create`）
  - 服务端内部决定：
    - `session_key = agent:zhuzhu:chat-01jv5n5x9x4m`
  - 服务端返回：
    - `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
    - `session_key = agent:zhuzhu:chat-01jv5n5x9x4m`
  - 此后 UI/Session RPC 一律只携带 `session_id`

不允许再走：

- `get_active_session_id(channel_type, account_handle, chat_id)`
- `get_bucket_session_id(channel_type, bucket_key)`

#### 4.5.6 Web / Session 管理合同

Web/session management 是实例视角，不是逻辑桶视角。

因此：

- `sessions.list`
- `sessions.resolve`
- `sessions.preview`
- `sessions.patch`
- `sessions.reset`
- `sessions.delete`
- `sessions.fork`
- `sessions.branches`
- `sessions.search`

以上合同都必须以 `session_id` 为主标识。

强约束：

- 客户端不得再本地生成 `session_id`
- 客户端不得再把 `main`、`session:<uuid>`、`telegram:*`、`cron:*` 当作 UI 主标识规则
- `sessions.resolve` 只负责“按 `session_id` 读取实例”
- Web 新建会话必须走服务端 owner create path（例如 `sessions.create`）
- 新实例创建必须走服务端 owner create path，不得复用 `resolve` 做双语义兼容

#### 4.5.6.1 Web 默认主会话 / 启动合同

Web UI 仍然需要“默认打开哪个会话”这个能力。

但这里的默认值不能再靠客户端伪造 `"main"`。

必须改成：

- 由服务端 owner path 返回当前 agent 主桶对应的实例 `session_id`
- 该 path 只负责：
  - 定位 `agent:<current_agent_id>:main`
  - 查当前 active `session_id`
  - 若不存在，则创建一个新实例并返回

可以采用的 RPC 例子：

- `sessions.home`

必须使用该 path 的场景：

- Web 首次启动、本地没有 `session_id`
- 当前激活实例被删除后需要回落
- `clear_all` 后需要回到默认主会话
- onboarding / app root redirect 需要跳默认聊天页

**例子：**

- 当前 agent：`zhuzhu`
- 逻辑主桶：`agent:zhuzhu:main`
- `sessions.home` 返回：
  - `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e74`
  - `session_key = agent:zhuzhu:main`

客户端随后只记住：

- `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e74`

不得再记住：

- `"main"`

#### 4.5.6.2 Web 会话展示合同

UI 还需要知道“怎么显示这个会话”“允许哪些操作”。

这些都不能再靠 `session_id` 的字面去猜。

因此 session management 返回给 Web 的实例数据，必须显式携带：

- `displayName`
  - UI-ready 展示名
  - header / sidebar / search 结果统一显示它
- `sessionKind`
  - 用于 icon / 文案 / 交互分类
  - 当前冻结集合：
    - `agent`
    - `channel`
    - `system`
- `canRename`
- `canDelete`
- `canFork`
- `canClear`

其中：

- `label` 仍是实例 metadata 里的“用户标签/显式命名”
- `displayName` 是服务端计算后的 UI 展示字符串
- UI 不得再 fallback 到 `sessionId`
- UI 不得再靠 `main`、`telegram:*`、`cron:*` 前缀推断 kind 或 capability

**例子 1：Agent 主会话**

- `session_id = sess_0195f3c5...`
- `session_key = agent:zhuzhu:main`
- `label = None`
- `displayName = Main`
- `sessionKind = agent`
- `canRename = false`
- `canDelete = false`
- `canFork = true`
- `canClear = true`

**例子 2：Telegram 群会话**

- `session_id = sess_0195f3c5...`
- `session_key = agent:zhuzhu:group-peer-tgchat.n1001234567890`
- `label = TG @lovely_apple_bot · grp:-1001234567890`
- `displayName = TG @lovely_apple_bot · grp:-1001234567890`
- `sessionKind = channel`
- `canRename = false`
- `canDelete = true`
- `canFork = true`
- `canClear = false`

**例子 3：Heartbeat**

- `session_id = sess_0195f3c5...`
- `session_key = system:cron:heartbeat`
- `displayName = Heartbeat`
- `sessionKind = system`
- `canRename = false`
- `canDelete = false`
- `canFork = false`
- `canClear = false`

#### 4.5.7 JSON / 内存 helper 也必须同构

即便是测试用 JSON metadata helper、内存态索引，也必须服从同一模型：

- 会话实例索引：`session_id -> SessionEntry`
- active-session 索引：`session_key -> session_id`

不允许因为“只是测试 helper / 文件态 helper”就继续保留另一套 `key/id` 语义。

#### 4.5.8 硬切口径

- 不做自动迁移
- 不保留 alias
- 不保留兼容双写
- 命中旧持久化形状时，必须直接失败并给出明确 remediation

例子：

- 若数据库里还是 `session_state.session_key`，启动直接失败
- 若 metadata 里还是 `parent_session_key`，加载直接失败
- 若运行时仍调用 `get_bucket_session_id(channel_type, bucket_key)` 作为主路径真值，视为未完成治理
- 若启动时发现旧 `metadata.json`，不得自动导入；必须直接拒绝并要求人工处理

---

## 5. 分隔符与字符规则

为了避免层次混淆，分隔符强制分层使用：

### 5.1 `:` 只用于系统层 namespace

只出现在 `session_key` 里。

**正确例子：**

- `agent:zhuzhu:main`
- `system:cron:heartbeat`

**错误例子：**

- `dm:peer:person:neoragex2002`
- `group:account:telegram:845:peer:-100`

### 5.2 `-` 只用于 bucket grammar

只出现在 bucket 语法段之间。

**正确例子：**

- `dm-peer-person.neoragex2002`
- `group-peer-tgchat.n1001234567890-branch-topic.42`

### 5.3 `.` 只用于 atom 内部 typed value

**正确例子：**

- `person.neoragex2002`
- `tgchat.n1001234567890`
- `tguser.8344017527`
- `topic.42`

### 5.4 原则

- 所有 key 一律 lower-case ASCII
- 不允许空 segment
- 不允许在同一层混用 `:` 和 `-`
- 用户可见 display 字段（username / nickname / title）不得直接进 key

---

## 6. 原子 key（atom）定义

本节定义所有会进入 `bucket_key` 的原子值。

**Telegram 侧最小对象集：**

- 人：
  - `person.<person_id>`
- Telegram 用户 / bot 账号：
  - `tguser.<telegram_user_id>`
- Telegram chat 对象：
  - `tgchat.<chat_atom>`
- Telegram 子线：
  - `topic.<topic_id>`
  - `reply.<message_id>`

**第一性约束：**

- `peer_key` / `sender_key` / `account_key` 是槽位，不是新的对象族
- `telegram` 只是 `per_channel` grammar 里的固定字面量，不是新的 typed atom
- 除上述最小对象集外，不再新增 Telegram 专属 canonical 前缀

### 6.1 `agent_id`

**定义：**

- 配置中的 agent 标识

**来源：**

- 配置真值

**例子：**

- `zhuzhu`
- `duoduo`
- `alma`

### 6.2 `service_id`

**定义：**

- 系统服务的稳定标识

**当前冻结值：**

- `cron`

**例子：**

- `cron`

**约束：**

- 当前设计不为未来服务预铺命名空间
- 新增 `service_id` 必须单独走 issue / design / review

### 6.3 固定渠道字面量 `telegram`

**定义：**

- 只在 `dm_scope = per_channel` 这种 bucket grammar 里出现的固定渠道字面量

**第一性约束：**

- 它不是一个新的对象族
- 当前设计不引入 `channel.*` typed atom
- Telegram 口径下只冻结一个字面量：
  - `telegram`

**例子：**

- `dm-peer-person.neoragex2002-channel-telegram`

### 6.4 `conversation_key`

**定义：**

- agent 侧手工持久侧聊的逻辑桶 id

**性质：**

- 内部 opaque
- 不要求用户可读

**例子：**

- `01jv5n5x9x4m`
- `01jv5p4r8h7k`

**对应 bucket 例子：**

- `chat-01jv5n5x9x4m`

### 6.5 `job_key`

**定义：**

- cron 持久 job 的逻辑桶 id

**性质：**

- 使用 job 自身稳定 id，不使用 display name

**例子：**

- `01jv62d6h1k7`

**对应 bucket 例子：**

- `job-01jv62d6h1k7`

### 6.6 `account_key`

**定义：**

- 某个 Telegram bot 实例在 `per_account` bucket 里的账号槽位值

**规则：**

- 必须来自系统探测/接入事实
- 不使用 bot username / nickname 作为真值
- `account_key` 是槽位，不是新的对象族
- 它直接复用 canonical Telegram user atom：
  - `tguser.<telegram_user_id>`

**推荐语法：**

- `tguser.<bot_user_id>`

**例子：**

- `tguser.8344017527`
- `tguser.8576199590`
- `tguser.8704214186`

### 6.7 `person_id`

**定义：**

- 系统级 identity 主键

**唯一事实来源：**

- `PEOPLE.md` frontmatter `people[].name`

**规则：**

- 它是跨渠道 identity，不是 Telegram 原生字段
- 只有 Telegram 来件命中权威 identity link 时，才允许落成 `person.<person_id>`
- `display_name` / `telegram_user_name` / `telegram_display_name` 都不得反向冒充 `person_id`

**例子：**

- `zhuzhu`
- `duoduo`
- `neoragex2002`

**组合例子：**

- `person.zhuzhu`
- `person.neoragex2002`

### 6.8 `telegram_user_id`

**定义：**

- Telegram 用户或 bot 的原生稳定数字主键

**来源：**

- inbound sender 的 `from.id`
- managed bot 自身的 `getMe.id`
- `PEOPLE.md` identity link 中的 `telegram_user_id`

**规则：**

- 这是 Telegram 渠道内最稳定的账号事实
- identity link 匹配时，`telegram_user_id` 是高于 `telegram_user_name` 的主匹配键
- 未命中 identity link 时，canonical key 应退回 `tguser.<telegram_user_id>`
- bot account 维度也直接复用 `tguser.<telegram_user_id>`，而不是另发明第二套账号前缀

**例子：**

- `8344017527`
- `8576199590`
- `2002`

**组合例子：**

- `tguser.2002`
- `tguser.8344017527`

### 6.9 `telegram_user_name`

**定义：**

- Telegram 用户名

**规范化规则：**

- 比较前必须：
  - `trim()`
  - 去掉前导 `@`
  - 转成 lower-case ASCII

**用途：**

- identity link 的 fallback 匹配
- `@mention` 渲染
- 诊断和辅助展示

**禁止：**

- 不得直接进入任何 canonical key

**例子：**

- 原始值：`@Lovely_Apple_Bot`
- 规范化后：`lovely_apple_bot`

### 6.10 `telegram_display_name`

**定义：**

- Telegram UI 展示名

**用途：**

- transcript 展示
- UI 展示
- 辅助诊断

**禁止：**

- 不得直接进入任何 canonical key

**例子：**

- `猪猪`
- `朵朵`
- `Neo Rage`

### 6.11 `chat_atom`

**定义：**

- Telegram chat 对象的 canonical 原子值

**来源：**

- Telegram `chat.id`

**规则：**

- `chat_id >= 0`：直接用十进制
- `chat_id < 0`：编码成 `n<abs(chat_id)>`
- `chat_atom` 代表“chat 对象”，不代表“人”
- private DM 里即使 `chat.id` 与 `from.id` 数字相同，也不能把 `chat_atom` 拿来冒充 DM `peer_key`

**例子：**

- private chat `2002`
  - `chat_atom = 2002`
  - `tgchat.2002`
- supergroup / channel chat `-1001234567890`
  - `chat_atom = n1001234567890`
  - `tgchat.n1001234567890`

### 6.12 Telegram 身份 / 聊天形态与使用口径

#### 6.12.1 一个 Telegram 账号常见的 4 种形态

- identity：
  - 形式：`person.<person_id>`
  - 何时使用：只有 `PEOPLE.md` 权威 link 命中时，才用于 canonical `peer_key` / `sender_key`
- 数字账号：
  - 形式：`telegram_user_id`
  - 何时使用：作为 Telegram 原生稳定主键，用于 link 匹配、`tguser.*`
- 用户名：
  - 形式：`telegram_user_name`
  - 何时使用：fallback 匹配、`@mention`、诊断
- 显示名：
  - 形式：`telegram_display_name`
  - 何时使用：仅展示

**例子 1：系统内 bot `zhuzhu`**

- `person_id = zhuzhu`
- `telegram_user_id = 8344017527`
- `telegram_user_name = @lovely_apple_bot`
- `telegram_display_name = 猪猪`
- canonical account key：
  - `tguser.8344017527`

**例子 2：外部用户 `Neoragex2002`**

- 若 `PEOPLE.md` 已绑定：
  - canonical person：
    - `person.neoragex2002`
- 若未绑定，但 `from.id = 2002`：
  - canonical telegram user：
    - `tguser.2002`

#### 6.12.2 一个 Telegram chat 常见的 2 种形态

- private chat：
  - 原始形态：`chat.id > 0`
  - 例子：`chat.id = 2002`
- shared group / channel chat：
  - 原始形态：`chat.id < 0`
  - 例子：`chat.id = -1001234567890`

**使用规则：**

- DM：
  - canonical `peer_key` 看“对端人/用户”，不用 `tgchat.<chat_atom>`
- Group / Channel：
  - canonical `peer_key` 看“共享 chat 对象”，必须用 `tgchat.<chat_atom>`
- Group / Channel sender：
  - canonical `sender_key` 看“发言人”，因此用 `person.<person_id>` 或 `tguser.<telegram_user_id>`

**最容易混淆的例子：**

- Telegram DM 中：
  - `from.id = 2002`
  - `chat.id = 2002`
- 虽然数字一样，但语义不同：
  - `from.id = 2002` 表示“这个人/账号是谁”
  - `chat.id = 2002` 表示“这个 private chat 对象是谁”
- 因此 canonical DM `peer_key` 必须是：
  - `person.neoragex2002`
  - 或 `tguser.2002`
- 绝不能写成：
  - `tgchat.2002`

#### 6.12.3 Identity link 判定合同（硬规则）

`PEOPLE.md` 是 Telegram identity link 的唯一事实来源。

**索引构建规则：**

- `telegram_user_id` 索引：
  - 只接受唯一值
  - 同一个 `telegram_user_id` 若出现在 2 个及以上 `person_id` 上，配置直接判为无效
  - `moltis config check` 必须失败
  - Telegram identity link 启动必须失败
- `telegram_user_name` 索引：
  - 先做规范化：
    - `trim()`
    - 去掉前导 `@`
    - lower-case ASCII
  - 只接受唯一值
  - 同一个规范化 username 若出现在 2 个及以上 `person_id` 上，配置直接判为无效
  - `moltis config check` 必须失败
  - Telegram identity link 启动必须失败

**入站判定顺序：**

1. 若事件携带 `telegram_user_id`
   - 只按 `telegram_user_id` 判定 canonical identity
   - 命中：
     - `person.<person_id>`
   - 未命中：
     - `tguser.<telegram_user_id>`
2. 只有在事件**缺失** `telegram_user_id` 时，才允许退回 `telegram_user_name` 做唯一匹配
   - 命中：
     - `person.<person_id>`
   - 未命中：
     - 不得凭 username 伪造 `tguser.*`
     - canonical actor 视为缺失

**canonical actor 缺失时的硬处理：**

- DM：
  - `peer_key` 无法生成
  - 该入站不得继续走需要 canonical peer 的主路径
  - 不得伪造 `tguser.<normalized_username>`
- Group / Channel：
  - `sender_key` 视为缺失
  - 只允许按既定 group scope 降级规则继续
  - 不得伪造 `sender_key`

**冲突规则：**

- 若事件同时携带 `telegram_user_id` 和 `telegram_user_name`
- 且 `telegram_user_id` 命中了 A，username 规范化后却命中了 B
- 则：
  - canonical 结果必须仍然选择 `telegram_user_id -> A`
  - 不得因为 username 把 canonical actor 改写到 B
  - 必须打结构化冲突日志

**例子 1：稳定 id 命中**

- 入站：
  - `telegram_user_id = 2002`
  - `telegram_user_name = @neo_rage`
- `PEOPLE.md`：
  - `neoragex2002.telegram_user_id = 2002`
- 结果：
  - `person.neoragex2002`

**例子 2：id 未命中，不得拿 username 顶替**

- 入站：
  - `telegram_user_id = 2002`
  - `telegram_user_name = @neo_rage`
- `PEOPLE.md`：
  - 只有 `telegram_user_name = @neo_rage`
  - 但没有 `telegram_user_id = 2002`
- 结果：
  - `tguser.2002`
- 说明：
  - 因为稳定 id 已经出现，就只能按稳定 id 判

**例子 3：只有 username 时才允许 fallback**

- 入站：
  - `telegram_user_id = None`
  - `telegram_user_name = @neo_rage`
- `PEOPLE.md`：
  - `neoragex2002.telegram_user_name = @neo_rage`
- 结果：
  - `person.neoragex2002`

**例子 4：配置重复，直接判无效**

- `PEOPLE.md`：
  - `person.a.telegram_user_id = 2002`
  - `person.b.telegram_user_id = 2002`
- 结果：
  - identity link 配置无效
  - 不得 silent pick first

### 6.13 `peer_key`

**定义：**

- 当前消息所面对的逻辑对端

**规则：**

- Telegram DM：
  - 有权威 identity link：
    - `person.<person_id>`
  - 无 identity link：
    - `tguser.<telegram_user_id>`
  - 禁止：
    - `tgchat.<chat_atom>`
- Telegram Group / Channel：
  - 一律使用共享 chat：
    - `tgchat.<chat_atom>`

**例子：**

- `person.neoragex2002`
- `tguser.2002`
- `tgchat.n1001234567890`

**语义例子：**

- DM 给 `Neoragex2002`，有 identity link：
  - `peer_key = person.neoragex2002`
- DM 给 Telegram 用户 `2002`，无 identity link：
  - `peer_key = tguser.2002`
- Group `-1001234567890`：
  - `peer_key = tgchat.n1001234567890`

### 6.14 `sender_key`

**定义：**

- 群消息发言人的逻辑标识

**规则：**

- 有 identity link：
  - `person.<person_id>`
- 无 identity link：
  - `tguser.<telegram_user_id>`
- 不得使用：
  - `tgchat.<chat_atom>`

**例子：**

- `person.neoragex2002`
- `tguser.1001`

### 6.15 `branch_key`

**定义：**

- Telegram 群内子线判别结果

**当前冻结类型：**

- forum topic
- reply-root branch

**推荐语法：**

- topic：`topic.<topic_id>`
- reply-root：`reply.<message_id>`

**生成优先级（硬规则）：**

1. 若当前事件存在 Telegram forum `thread_id`
   - `branch_key = topic.<thread_id>`
2. 否则，若当前事件存在 `reply_to_message_id`
   - `branch_key = reply.<reply_to_message_id>`
3. 否则
   - `branch_key = None`

**强约束：**

- `topic` 优先级永远高于 `reply`
- 不做 reply 链递归追根
- 不从历史消息、binding、缓存里反推一个“真正 root”
- 只使用当前 Telegram 事件已经携带的 branch 事实
- 当 `branch_key = None` 且 scope 需要 branch 时，只允许按既定降级规则退化，不得伪造 branch

**例子：**

- `topic.42`
- `reply.98765`

**例子 1：topic 与 reply 同时存在**

- 入站：
  - `thread_id = 42`
  - `reply_to_message_id = 98765`
- 结果：
  - `branch_key = topic.42`
- 说明：
  - forum topic 是更强的结构事实，reply anchor 被忽略

**例子 2：只有 reply**

- 入站：
  - `thread_id = None`
  - `reply_to_message_id = 98765`
- 结果：
  - `branch_key = reply.98765`

**例子 3：两者都没有**

- 入站：
  - `thread_id = None`
  - `reply_to_message_id = None`
- 结果：
  - `branch_key = None`

### 6.16 带符号整数的编码规则

为了保证 `-` 只保留给 bucket grammar，原子值内部不得直接带负号。

**规则：**

- 非负整数：直接写十进制
- 负整数：前缀 `n`，再写绝对值

**例子：**

- Telegram chat id `-1001234567890`
  - 原子值：`n1001234567890`
  - 组合后：`tgchat.n1001234567890`
- Telegram private chat id `2002`
  - 原子值：`2002`
  - 组合后：`tgchat.2002`
- Telegram user id `2002`
  - 原子值：`2002`
  - 组合后：`tguser.2002`

---

## 7. `session_key` 规范

### 7.1 总语法

```text
session_key = agent_session_key | system_session_key

agent_session_key  = "agent:"  agent_id   ":" agent_bucket_key
system_session_key = "system:" service_id ":" system_bucket_key
```

### 7.2 `agent` 命名空间

`agent` 命名空间承载“某个 agent 的长期会话桶”。

#### 7.2.1 主会话

**bucket：**

- `main`

**session_key 例子：**

- `agent:zhuzhu:main`

#### 7.2.2 手工持久侧聊

**bucket：**

- `chat-<conversation_key>`

**session_key 例子：**

- `agent:zhuzhu:chat-01jv5n5x9x4m`

#### 7.2.3 Telegram adapter 产出的 bucket

**session_key 例子：**

- `agent:zhuzhu:dm-main`
- `agent:zhuzhu:dm-peer-person.neoragex2002`
- `agent:zhuzhu:dm-peer-person.neoragex2002-channel-telegram`
- `agent:zhuzhu:dm-peer-person.neoragex2002-account-tguser.8344017527`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890-sender-person.neoragex2002`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890-branch-topic.42`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`

### 7.3 `system` 命名空间

`system` 命名空间承载“系统服务自身需要长期复用上下文的会话桶”。

#### 7.3.1 当前冻结的 `service_id`

当前只冻结：

- `cron`

#### 7.3.2 Cron heartbeat

**bucket：**

- `heartbeat`

**session_key 例子：**

- `system:cron:heartbeat`

#### 7.3.3 Cron 持久 job

**bucket：**

- `job-<job_key>`

**session_key 例子：**

- `system:cron:job-01jv62d6h1k7`

### 7.4 为什么 `session_key` 不再额外写 channel 前缀

因为 channel 是否参与同桶判定，应该由 `bucket_key` 决定，而不是由系统层先写死。

**例子：**

- 同一个人跨 Telegram / Feishu，如果 `dm_scope = per_peer`，应共桶
- 如果系统层强行写成 `agent:zhuzhu:telegram:...`，那这个能力会被系统层提前砍死

所以正确做法是：

- `per_peer`：`agent:zhuzhu:dm-peer-person.neoragex2002`
- `per_channel`：`agent:zhuzhu:dm-peer-person.neoragex2002-channel-telegram`

channel 只在 scope 需要它的时候才进入 key。

---

## 8. Telegram `bucket_key` 规范

### 8.1 总语法

```text
tg_bucket_key = dm_bucket_key | group_bucket_key
```

### 8.2 DM 全部形式

**DM 总口径：**

- DM bucket 的 `peer_key` 看“对端人/用户”，不看 private chat 对象
- 即使 Telegram 原始事件里 `from.id = chat.id = 2002`，canonical DM peer 仍然只能是：
  - `person.<person_id>`
  - 或 `tguser.<telegram_user_id>`
- 绝不能写成：
  - `tgchat.2002`

#### 8.2.1 `dm_scope = main`

**bucket：**

- `dm-main`

**含义：**

- 同一 agent 下所有 DM 共桶

**例子：**

- `dm-main`
- 对 `Neoragex2002` 的 DM：`dm-main`
- 对另一个陌生用户 `tguser.3003` 的 DM：仍然是 `dm-main`

#### 8.2.2 `dm_scope = per_peer`

**bucket：**

- `dm-peer-<peer_key>`

**含义：**

- 同一逻辑对端共桶

**例子：**

- linked person：
  - `dm-peer-person.neoragex2002`
- unresolved tg user：
  - `dm-peer-tguser.2002`

#### 8.2.3 `dm_scope = per_channel`

**bucket：**

- `dm-peer-<peer_key>-channel-telegram`

**含义：**

- 同一逻辑对端在不同渠道不共桶

**Telegram 例子：**

- `dm-peer-person.neoragex2002-channel-telegram`

#### 8.2.4 `dm_scope = per_account`

**bucket：**

- `dm-peer-<peer_key>-account-<account_key>`

**含义：**

- 同一逻辑对端在不同接入账号不共桶

**例子：**

- `dm-peer-person.neoragex2002-account-tguser.8344017527`
- `dm-peer-person.neoragex2002-account-tguser.8576199590`

**说明：**

- `account_key` 天然已经带渠道边界
- 因此 `per_account` 不再重复写 `channel-telegram`

### 8.3 Group 全部形式

#### 8.3.1 `group_scope = group`

**bucket：**

- `group-peer-<peer_key>`

**含义：**

- 同一共享群对象共桶

**例子：**

- `group-peer-tgchat.n1001234567890`

#### 8.3.2 `group_scope = per_sender`

**bucket：**

- `group-peer-<peer_key>-sender-<sender_key>`

**含义：**

- 同群按发言人拆桶

**例子：**

- `group-peer-tgchat.n1001234567890-sender-person.neoragex2002`
- `group-peer-tgchat.n1001234567890-sender-tguser.1001`

**降级：**

- `sender` 缺失时，降级为：
  - `group-peer-<peer_key>`

**降级例子：**

- 原计划：
  - `group-peer-tgchat.n1001234567890-sender-tguser.1001`
- 但 sender 缺失：
  - `group-peer-tgchat.n1001234567890`

#### 8.3.3 `group_scope = per_branch`

**bucket：**

- `group-peer-<peer_key>-branch-<branch_key>`

**含义：**

- 同群按子线拆桶

**例子：**

- `group-peer-tgchat.n1001234567890-branch-topic.42`
- `group-peer-tgchat.n1001234567890-branch-reply.98765`

**降级：**

- `branch` 缺失时，降级为：
  - `group-peer-<peer_key>`

#### 8.3.4 `group_scope = per_branch_sender`

**bucket：**

- `group-peer-<peer_key>-branch-<branch_key>-sender-<sender_key>`

**含义：**

- 同群按“子线 + 发言人”共同拆桶

**例子：**

- `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`
- `group-peer-tgchat.n1001234567890-branch-reply.98765-sender-tguser.1001`

**降级规则：**

- 缺 `branch`、有 `sender`
  - `group-peer-<peer_key>-sender-<sender_key>`
- 有 `branch`、缺 `sender`
  - `group-peer-<peer_key>-branch-<branch_key>`
- 两者都缺
  - `group-peer-<peer_key>`

**降级例子 1：**

- 目标：
  - `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`
- 但 `branch` 缺失：
  - `group-peer-tgchat.n1001234567890-sender-person.neoragex2002`

**降级例子 2：**

- 目标：
  - `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`
- 但 `sender` 缺失：
  - `group-peer-tgchat.n1001234567890-branch-topic.42`

### 8.4 为什么 Group 默认不带 `account`

这是本轮最关键的第一性修正之一。

`group_scope = group` 的同桶判定语义是：

- 同一 agent
- 同一共享群对象 `peer`

而不是：

- 同一 agent
- 同一 bot account
- 同一群

**正确例子：**

- `group-peer-tgchat.n1001234567890`

**必须淘汰的旧思路：**

- `group:account:telegram:8344017527:peer:-1001234567890`

因为它把不属于 `group` 语义轴的 `account` 硬烤进了默认 bucket。

### 8.5 为什么 Telegram `bucket_key` 必须保留 type

因为 Telegram 内部已经会单独存取和复用裸 `bucket_key`。

**现状例子：**

- callback 键盘消息发出时保存 `bucket_key`
- callback 点击回来时直接用这个 `bucket_key` 恢复原桶

如果此时 `bucket_key` 没有 `dm-` / `group-` 前缀，就会出现：

- Telegram 内部只能靠外围上下文猜它属于 DM 还是 Group

这是不合格的。

所以 Telegram `bucket_key` 必须 typed。

---

## 9. Agent / System / Transient 三类场景示例

### 9.1 Agent 主会话

用户在 Web UI 打开 `zhuzhu` 的主会话。

**bucket_key：**

- `main`

**session_key：**

- `agent:zhuzhu:main`

**session_id：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74`

**run_id：**

- `run_0195f3c5b3d27d8aa91e4439bb3c2e90`

### 9.2 Agent Telegram DM：`per_peer`

`zhuzhu` 在 Telegram 上收到 `Neoragex2002` 的私信，且 identity link 已命中。

**原始 Telegram 事实：**

- `from.id = 2002`
- `chat.id = 2002`
- `chat.kind = private`

**解释：**

- 这里 `from.id` 和 `chat.id` 虽然数值相同，但语义不同
- canonical DM `peer_key` 仍然按“人/用户”建模，不按 `tgchat.2002` 建模

**peer_key：**

- `person.neoragex2002`

**bucket_key：**

- `dm-peer-person.neoragex2002`

**session_key：**

- `agent:zhuzhu:dm-peer-person.neoragex2002`

**session_id：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e81`

### 9.3 Agent Telegram Group：`per_branch_sender`

`zhuzhu` 在群 `-1001234567890` 的 topic `42` 里收到 `Neoragex2002` 的发言。

**原始 Telegram 事实：**

- `chat.id = -1001234567890`
- `thread_id = 42`
- `from.id = 2002`

**peer_key：**

- `tgchat.n1001234567890`

**branch_key：**

- `topic.42`

**sender_key：**

- `person.neoragex2002`

**bucket_key：**

- `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`

**session_key：**

- `agent:zhuzhu:group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`

**session_id：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e95`

### 9.4 System Cron Heartbeat

系统 heartbeat 任务需要长期复用自己的上下文。

**bucket_key：**

- `heartbeat`

**session_key：**

- `system:cron:heartbeat`

**session_id：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2ea2`

### 9.5 System Cron 普通持久 Job

job id 为 `01jv62d6h1k7` 的 cron 任务需要长期保留上下文。

**bucket_key：**

- `job-01jv62d6h1k7`

**session_key：**

- `system:cron:job-01jv62d6h1k7`

**session_id：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2eb0`

### 9.6 Transient 执行：Provider Setup Probe

provider setup 探针只是一轮探测，不需要长期上下文。

**正确做法：**

- `session_key = None`
- `session_id = None`
- `run_id = run_0195f3c5b3d27d8aa91e4439bb3c2ef1`

**错误做法：**

- `session_id = provider_setup:openai:gpt-5.2`

### 9.7 Transient 执行：Model Capability Probe / Stream Test

模型可用性探针、模型连通性测试也都只是一轮探测，不需要长期上下文。

**正确做法：**

- `session_key = None`
- `session_id = None`
- `run_id = run_0195f3c5b3d27d8aa91e4439bb3c2ef2`

**错误做法：**

- `session_id = probe:openai:gpt-5.2`
- `session_id = models.test:gpt-5.2`

### 9.8 Transient 执行：TTS Phrase Probe

TTS phrase 生成探针只是一轮探测，不需要长期上下文。

**正确做法：**

- `session_key = None`
- `session_id = None`
- `run_id = run_0195f3c5b3d27d8aa91e4439bb3c2ef2`

**错误做法：**

- `session_id = tts.generate_phrase:voice`

---

## 10. 为什么删除 “execution-only session id”

本轮设计明确删除这个概念。

原因很简单：

1. 它会把“执行实例”伪装成“会话实例”
2. 它会重新把业务语义塞进 `session_id`
3. 它会制造第三套伪命名空间，进一步加剧混乱

正确分工是：

- 需要长期上下文复用：给 `session_key`
- 需要会话实例：给 `session_id`
- 只是一次执行：给 `run_id`

**例子对比：**

### 错误

- `session_id = provider_setup:openai:gpt-5.2`
- `session_id = probe:openai:gpt-5.2`
- `session_id = models.test:gpt-5.2`
- `session_id = cron:heartbeat`
- `session_id = main`

### 正确

- `session_key = system:cron:heartbeat`
- `session_id = sess_0195f3c5...`
- `run_id = run_0195f3c5...`

或者对于 transient probe：

- `session_key = None`
- `session_id = None`
- `run_id = run_0195f3c5...`

---

## 11. Sandbox / Prompt Cache / Binding 口径

### 11.1 `scope_key=session_key`

适用于要按逻辑桶复用环境的场景。

**例子：**

- `agent:zhuzhu:dm-peer-person.neoragex2002`
- `system:cron:heartbeat`

这意味着：

- 同一个逻辑桶，即使滚动出多个 `session_id`，也可继续复用同一个 sandbox

### 11.2 `scope_key=session_id`

适用于要按具体会话实例隔离环境的场景。

**例子：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e81`

这意味着：

- 同一逻辑桶如果切出新的会话实例，sandbox 也会切开

#### 11.2.1 sandbox runtime artifact naming

sandbox 运行时还会派生出：

- container name
- `.sandbox_views/<...>` 目录名
- 其他 backend-specific runtime artifact name

这里要明确：

- sandbox 的事实源不是这些 artifact name
- sandbox 的事实源只有 `effective_sandbox_key`

其中：

- 当 `scope_key=session_key` 时：
  - `effective_sandbox_key = session_key`
- 当 `scope_key=session_id` 时：
  - `effective_sandbox_key = session_id`

runtime artifact name 必须满足：

- 由完整 `effective_sandbox_key` 稳定派生
- collision-safe
- backend / filesystem safe
- 不得使用简单 `sanitize` / 字符替换作为真名

因为：

- `agent:zhuzhu:dm-peer-person.a/b`
- `agent:zhuzhu:dm-peer-person.a:b`

这类不同 key，简单 sanitize 后可能会撞成同一个 runtime name。

**推荐形态：**

- `sandbox_runtime_name = msb-<readable-slice>-<short-hash>`

其中：

- `msb`
  - 固定前缀
  - 含义：`moltis sandbox`
- `readable-slice`
  - 给人看的短语义片段
  - 只用于辅助识别，不承担真值职责
- `short-hash`
  - 基于完整 `effective_sandbox_key` 的稳定短哈希
  - 用于防撞

`readable-slice` 的推荐裁剪规则：

- `agent:<agent_id>:main`
  - `agent-<agent_id>-main`
- `agent:<agent_id>:chat-<id>`
  - `agent-<agent_id>-chat`
- `agent:<agent_id>:dm-*`
  - `agent-<agent_id>-dm`
- `agent:<agent_id>:group-peer-tgchat.n<chat_id>...`
  - `agent-<agent_id>-group-<chat_id>`
- `system:cron:heartbeat`
  - `system-cron-heartbeat`
- `system:cron:job-<job_id>`
  - `system-cron-job`

**例子：**

- `effective_sandbox_key = agent:zhuzhu:dm-peer-person.neoragex2002`
- `sandbox_runtime_name = msb-agent-zhuzhu-dm-4f2a91c0`
- `container_name = msb-agent-zhuzhu-dm-4f2a91c0`
- `public_data_view = .sandbox_views/msb-agent-zhuzhu-dm-4f2a91c0`

- `effective_sandbox_key = agent:zhuzhu:main`
- `sandbox_runtime_name = msb-agent-zhuzhu-main-7ab31c2d`
- `container_name = msb-agent-zhuzhu-main-7ab31c2d`

- `effective_sandbox_key = agent:zhuzhu:group-peer-tgchat.n1001234567890`
- `sandbox_runtime_name = msb-agent-zhuzhu-group-1001234567890-a83c1d92`
- `container_name = msb-agent-zhuzhu-group-1001234567890-a83c1d92`

- `effective_sandbox_key = system:cron:heartbeat`
- `sandbox_runtime_name = msb-system-cron-heartbeat-d91a7c44`
- `container_name = msb-system-cron-heartbeat-d91a7c44`

**强约束：**

- `msb` 是固定字面量，不是用户可配置项
- 旧配置 `tools.exec.sandbox.container_prefix` 必须直接 reject
- debug / observability / UI 若展示 sandbox 信息，必须同时能看到：
  - `effectiveSandboxKey`
  - `containerName`
- `containerName` 只是运行时派生名，不得回流为 session truth
- `containerName` 可以直观，但其可读片段只做辅助识别；真正复用、命中、判等一律只看完整 `effective_sandbox_key`

#### 11.2.2 `non-main` 的判定口径

`non-main` 是逻辑桶语义，不是实例 id 语义。

因此：

- 不得再用 `session_id == "main"` 判定

必须改成：

- Agent 主桶：
  - `session_key = agent:<agent_id>:main`
  - 这是 `main`
- 其他 agent 桶：
  - 都是 `non-main`
- `system:*`
  - 都是 `non-main`

**例子：**

- `session_key = agent:zhuzhu:main`
  - `non-main = false`
- `session_key = agent:zhuzhu:chat-01jv5n5x9x4m`
  - `non-main = true`
- `session_key = agent:zhuzhu:group-peer-tgchat.n1001234567890`
  - `non-main = true`
- `session_key = system:cron:heartbeat`
  - `non-main = true`

若某条路径启用了 `non-main` 策略，却拿不到判定所需的 canonical `session_key`：

- 必须直接 reject 或显式报错
- 不得退回去猜 `session_id`

### 11.3 `prompt_cache_key`

`prompt_cache_key` 是 provider 南向 prompt cache 的桶名。

它不是：

- 不是 `session_key`
- 不是 `session_id`
- 不是 `run_id`
- 不是 active-session truth

它只回答一件事：

- 这次南向 LLM 请求，应当与哪一类请求共用 provider prompt cache

**生成 owner：**

- 必须由调用方决定并传入
- provider 只能消费，不得自行猜测或补默认值

**默认规则：**

1. Agent 会话请求
   - 默认取 `session_id`
2. 稳定 system lane 请求
   - 默认取稳定 `session_key`
3. 非 Agent 且没有 canonical session、但业务上仍想吃 prompt cache
   - 必须由调用方显式提供稳定 `prompt_cache_key`
4. 若调用方既没有 canonical 默认值，也不想显式提供
   - 直接省略 `prompt_cache_key`

**例子 1：Agent 会话**

- `session_id = sess_0195f3c5b3d27d8aa91e4439bb3c2e81`
- `prompt_cache_key = sess_0195f3c5b3d27d8aa91e4439bb3c2e81`

**例子 2：System heartbeat**

- `session_key = system:cron:heartbeat`
- `prompt_cache_key = system:cron:heartbeat`

**例子 3：System 持久 cron job**

- `session_key = system:cron:job-01jv62d6h1k7`
- `prompt_cache_key = system:cron:job-01jv62d6h1k7`

**例子 4：非 Agent transient，但调用方显式给缓存桶**

- `run_id = run_0195f3c5b3d27d8aa91e4439bb3c2ef1`
- `prompt_cache_key = provider_setup:openai:gpt-5.2`

**强约束：**

- provider 不得生成 `moltis:*:no-session`
- `prompt_cache_key` 不得回流为 `session_key` / `session_id`
- `prompt_cache_key` 不参与 active-session、sandbox、routing、history truth

### 11.4 Transient run

transient run 没有 `session_key` / `session_id`。

因此：

- 不得靠伪造 `session_id` 来骗过 sandbox
- provider 不得伪造 `prompt_cache_key`
- 若 transient 调用确实需要 southbound prompt cache，必须由调用方显式提供稳定 `prompt_cache_key`
- 若某个 transient 工作真的需要长期复用环境，它就不再是 transient，必须被提升为真实 `system` 会话

**例子：**

- provider setup probe：不应拥有 `system:provider_setup:*`
- 若未来某个“系统级长期 provider audit”真的需要复用上下文，必须单独定义新的 `service_id`，而不是偷用 probe run

### 11.5 Telegram adapter 内部局部缓存

Telegram callback/message binding 这类局部 helper，可以继续存储 Telegram 自己的 `bucket_key`。

但要注意：

- 这只是 adapter 内部局部辅助真值
- 不是全系统 session 真值

全系统真值仍然是：

- `session_key`
- `session_key -> active session_id`

### 11.6 Key 消费矩阵（硬规则）

以下矩阵用于冻结“谁该看哪个 key”。

#### 11.6.1 只能消费 `bucket_key` 的位置

- Telegram adapter 内部 callback / binding / route helper
- Telegram adapter 内部 bucket 相等性比较

**强约束：**

- `bucket_key` 只允许留在适配层局部语义里
- 不得把 `bucket_key` 直接冒充系统层 `session_key`

#### 11.6.2 只能消费 `session_key` 的位置

- `session_key -> active session_id` 的权威映射
- 逻辑桶级 sandbox（仅当 `scope_key=session_key`）
- bucket-scoped policy / routing 命中
- 其他“明确声明为逻辑桶共享状态”的组件

**强约束：**

- 若某个组件消费的是“逻辑桶共享状态”，它必须显式使用 `session_key`
- 当前仓库内的 `SessionStateStore` 不属于这里；它的语义已经冻结为“会话实例级状态”，必须走 `session_id`
- 不得把 `channel_type + bucket_key` 当 `session_key` 的替代真值

#### 11.6.3 只能消费 `prompt_cache_key` 的位置

- provider 南向 prompt cache
- provider prompt cache debug / observability

**强约束：**

- `prompt_cache_key` 不是 core runtime key
- Agent 会话默认取 `session_id`
- 稳定 system lane 默认取 `session_key`
- 非 Agent 无 canonical session 时，若想吃 cache，必须由调用方显式提供稳定 bucket
- 若调用方未提供，也没有默认 derivation，则直接省略
- provider 不得生成 `moltis:*:no-session` 或其他 fallback bucket

#### 11.6.4 只能消费 `session_id` 的位置

- `LlmRequestContext.session_id`
- 会话实例历史 / metadata / label / model / project
- worktree 绑定
- branching parent / child 关系
- `session_state` 工具与其持久化存储（语义是“当前会话实例状态”）
- silent memory flush / compaction helper 注入给工具的 `_sessionId`
- 实例级 sandbox（仅当 `scope_key=session_id`）

**强约束：**

- 这些位置消费的是“具体会话实例”
- 不得改用 `session_key`
- 若现有字段、变量、列名写成 `session_key`，但实际承载的是实例语义，必须硬切修正
- 对应旧持久化形状（如 `parent_session_key`、`session_state.session_key`）必须直接 reject
- 不得为旧字段名保留 serde alias、SQL fallback、silent ignore
- 若调用方没有 `session_id`，这些消费者必须显式 reject 或保持缺失
- 不得把缺失的 `session_id` 默认补成 `main`

#### 11.6.5 只能消费 `run_id` 的位置

- transient probe
- stream state
- run-scoped metrics / tracing

**强约束：**

- transient probe 不得再伪造 `session_id`
- 不得发明 `system:provider_setup:*`、`system:tts:*` 这种伪长期会话

---

## 12. 硬切 legacy 拒绝口径

以下格式在专项治理后必须直接拒绝：

### 12.1 旧 `session_key`

**拒绝例子：**

- `main`
- `cron:heartbeat`
- `session:abc`
- `telegram:845:group:-1001234567890`
- 直接把 Telegram raw bucket 当 `session_key`

**正确例子：**

- `agent:zhuzhu:main`
- `system:cron:heartbeat`
- `agent:zhuzhu:group-peer-tgchat.n1001234567890`

### 12.2 旧 `session_id`

**拒绝例子：**

- `main`
- `cron:heartbeat`
- `probe:openai:gpt-5.2`
- `models.test:gpt-5.2`
- `provider_setup:openai:gpt-5.2`
- `tts.generate_phrase:voice`
- `session:550e8400-e29b-41d4-a716-446655440000`

**正确例子：**

- `sess_0195f3c5b3d27d8aa91e4439bb3c2e74`

### 12.3 旧 Telegram bucket

**拒绝例子：**

- `dm:main`
- `dm:account:telegram:8344017527:peer:foo`
- `group:account:telegram:8344017527:peer:-1001234567890`
- `group:account:telegram:8344017527:peer:-1001234567890:branch:42:sender:1001`

**正确例子：**

- `dm-main`
- `dm-peer-person.neoragex2002-account-tguser.8344017527`
- `group-peer-tgchat.n1001234567890`
- `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`

### 12.4 结构化拒绝 / 冲突日志合同（硬规则）

命中 strict one-cut 拒绝或 identity 冲突时，必须留下结构化日志。

**必带字段：**

- `event`
- `reason_code`
- `decision`
- `policy`

**上下文字段：**

- 视场景补充：
  - `agent_id`
  - `session_key`
  - `bucket_key`
  - `telegram_user_id`
  - `telegram_user_name`
  - `chat_id`
  - `chat_type`
  - `group_scope`
- 禁止打印完整消息正文、token、secret

**固定 policy：**

- Session / bucket one-cut：
  - `session_key_bucket_key_one_cut_v1`
- Telegram identity link：
  - `telegram_identity_link_one_cut_v1`
- Telegram group route degrade：
  - `telegram_group_scope_one_cut_v1`

**固定日志事件与 reason_code：**

- 旧 `session_key` 被拒绝：
  - `event = "canonical_key.reject"`
  - `reason_code = "legacy_session_key_shape"`
  - `decision = "reject"`
- 旧 `session_id` 被拒绝：
  - `event = "canonical_key.reject"`
  - `reason_code = "legacy_session_id_shape"`
  - `decision = "reject"`
- 旧 Telegram bucket 被拒绝：
  - `event = "canonical_key.reject"`
  - `reason_code = "legacy_tg_bucket_shape"`
  - `decision = "reject"`
- `PEOPLE.md` 中 `telegram_user_id` 重复：
  - `event = "telegram.identity_link.reject"`
  - `reason_code = "identity_link_duplicate_user_id"`
  - `decision = "reject"`
- `PEOPLE.md` 中规范化 username 重复：
  - `event = "telegram.identity_link.reject"`
  - `reason_code = "identity_link_duplicate_user_name"`
  - `decision = "reject"`
- 同一入站事件里 user_id 与 username 指向不同 `person_id`：
  - `event = "telegram.identity_link.conflict"`
  - `reason_code = "identity_link_username_conflicts_with_user_id"`
  - `decision = "prefer_user_id"`
- group scope 需要 sender 但 sender 缺失：
  - `event = "telegram.route.degrade"`
  - `reason_code = "sender_missing"`
  - `decision = "degrade"`
- group scope 需要 branch 但 branch 缺失：
  - `event = "telegram.route.degrade"`
  - `reason_code = "branch_missing"`
  - `decision = "degrade"`

**强约束：**

- 不得 silent reject
- 不得一会儿写 `legacy_bucket`，一会儿写 `bucket_legacy`
- 上述 `reason_code` 在本专项内冻结，实施不得自由发挥别名

---

## 13. 最小测试面（设计冻结后的验收）

本设计落地后，至少要覆盖下面这组关键测试。

### 13.1 Key 语法测试

- 允许：
  - `agent:zhuzhu:main`
  - `agent:zhuzhu:dm-peer-person.neoragex2002`
  - `system:cron:heartbeat`
- 拒绝：
  - `main`
  - `cron:heartbeat`
  - `dm:main`

### 13.2 Telegram bucket 生成测试

- `dm_scope = main`
  - 输入：任意 peer
  - 输出：`dm-main`
- DM private chat 不得冒充 peer
  - 输入：
    - `from.id = 2002`
    - `chat.id = 2002`
    - identity link 命中 `person.neoragex2002`
  - 输出：
    - `dm-peer-person.neoragex2002`
  - 禁止输出：
    - `dm-peer-tgchat.2002`
- DM unresolved user
  - 输入：
    - `from.id = 2002`
    - `chat.id = 2002`
    - 无 identity link
  - 输出：
    - `dm-peer-tguser.2002`
- `dm_scope = per_peer`
  - 输入：`peer_key = person.neoragex2002`
  - 输出：`dm-peer-person.neoragex2002`
- `group_scope = per_branch_sender`
  - 输入：
    - `peer_key = tgchat.n1001234567890`
    - `branch_key = topic.42`
    - `sender_key = person.neoragex2002`
  - 输出：
    - `group-peer-tgchat.n1001234567890-branch-topic.42-sender-person.neoragex2002`
- Group shared chat 编码
  - 输入：
    - `chat.id = -1001234567890`
  - 输出：
    - `peer_key = tgchat.n1001234567890`

### 13.3 Identity link 判定测试

- `telegram_user_id` 唯一命中
  - 输入：
    - `telegram_user_id = 2002`
    - `telegram_user_name = @neo_rage`
  - 输出：
    - `person.neoragex2002`
- `telegram_user_id` 未命中时不得退回 username 顶替
  - 输入：
    - `telegram_user_id = 2002`
    - `telegram_user_name = @neo_rage`
    - `PEOPLE.md` 只配置 username link
  - 输出：
    - `tguser.2002`
- `telegram_user_id` 缺失时才允许 username fallback
  - 输入：
    - `telegram_user_id = None`
    - `telegram_user_name = @neo_rage`
  - 输出：
    - `person.neoragex2002`
- `PEOPLE.md` 重复 `telegram_user_id`
  - 结果：
    - 配置检查失败
    - 启动失败
    - `reason_code = identity_link_duplicate_user_id`
- `PEOPLE.md` 重复规范化 username
  - 结果：
    - 配置检查失败
    - 启动失败
    - `reason_code = identity_link_duplicate_user_name`
- 无 `telegram_user_id` 且 username 也未命中
  - DM：
    - `peer_key` 生成失败
    - 不得伪造 `tguser.<username>`
  - Group：
    - `sender_key` 视为缺失
- 同一事件里 user_id 与 username 指向不同 person
  - 结果：
    - canonical actor 仍按 user_id
    - `decision = prefer_user_id`
    - `reason_code = identity_link_username_conflicts_with_user_id`

### 13.4 Branch 判定测试

- topic 与 reply 同时存在
  - 输入：
    - `thread_id = 42`
    - `reply_to_message_id = 98765`
  - 输出：
    - `branch_key = topic.42`
- 只有 reply
  - 输入：
    - `thread_id = None`
    - `reply_to_message_id = 98765`
  - 输出：
    - `branch_key = reply.98765`
- 两者都缺失
  - 输入：
    - `thread_id = None`
    - `reply_to_message_id = None`
  - 输出：
    - `branch_key = None`
- `group_scope = per_branch_sender` 且 branch 缺失
  - 输出：
    - 按既定 bucket 降级
    - `reason_code = branch_missing`

### 13.5 Transient run 测试

- provider setup probe
  - 必须：
    - 有 `run_id`
    - 无 `session_key`
    - 无 `session_id`
- tts phrase probe
  - 必须：
    - 有 `run_id`
    - 无 `session_key`
    - 无 `session_id`

### 13.6 Session truth 与拒绝日志测试

- 全系统 active-session 命中必须基于完整 `session_key`
- 不得再基于 `channel_type + bucket_key`
- 不得再基于语义型 `session_id`
- legacy `session_key` 被拒绝时：
  - `event = canonical_key.reject`
  - `reason_code = legacy_session_key_shape`
- legacy `session_id` 被拒绝时：
  - `event = canonical_key.reject`
  - `reason_code = legacy_session_id_shape`
- legacy Telegram bucket 被拒绝时：
  - `event = canonical_key.reject`
  - `reason_code = legacy_tg_bucket_shape`

---

## 14. 最终冻结摘要

如果只记 6 条，记下面这 6 条：

1. `bucket_key` 是本地分桶键；`session_key` 是全局逻辑桶名；`session_id` 是会话实例 id；`run_id` 是执行实例 id
2. `session_key` 只允许两大命名空间：
   - `agent:<agent_id>:<bucket_key>`
   - `system:<service_id>:<bucket_key>`
3. `session_id` 与 `run_id` 必须 opaque，不得再带业务语义
4. transient 执行没有 “execution-only session id”；只有 `run_id`
5. Telegram `bucket_key` 必须 typed，且必须 self-describing
6. 旧 `main` / `cron:*` / `session:*` / `provider_setup:*` / `tts.generate_phrase:*` / `dm:...` / `group:...` 形态全部一刀切淘汰

# Issue: Terminology / Concept Convergence（概念与术语收敛：两域分离、核心键冻结、呈现口径统一）

## 实施现状（Status）【增量更新主入口】
- Status: TODO（记录在案，尚未系统推进）
- Priority: P2（长期质量/可维护性问题；会放大后续需求与排障成本）
- Components: gateway / sessions / channels / telegram / ui-debug / docs

---

## 背景（Background）
当前仓库中，“Telegram 域概念”和 “Moltis 域概念”在代码/日志/UI/issue 文档里混用，且出现同名字段跨层复用（尤其 `session_key`/`account_id`/`chat`/`scope`），导致：
- 讨论需求必须先解释名词，沟通与实现成本高；
- 同一条链路的日志很难读懂、很难串起来排障；
- 一旦引入多 agent / 多 loop / admin 工具，这种混乱会指数级放大（安全边界也会变脆弱）。

本 Issue 的目标不是“解释更多”，而是**冻结一套极简、准确、可执行的术语体系**，并把它落实到可观测性与命名规范里。

---

## 需求（Requirements）【必须满足】
1) **两域严格隔离**：Telegram 域术语与 Moltis 域术语不得混用；同名词不允许跨域复用语义。
2) **核心概念极简且冻结**：参与路由/分桶/落盘/鉴权的核心字段必须最少、语义固定、全仓统一。
3) **跨域桥只保留一个主键**：从 Telegram 的 chat/thread 唯一定位到 Moltis 的“会话桶”，必须通过同一个核心键（避免到处隐式推导）。
4) **核心 vs 呈现严格区分**：`@username`、display name、chat title、container name 等只允许用于“呈现”，不得用于分桶/鉴权/存储主键。
5) **呈现字段也要收敛**：默认日志/UI 只展示 2–3 个稳定字段，其余只在“展开详情”里提供。

---

## 目标（Goals）
- 冻结一套 Glossary（核心对象 + 核心键 + 呈现口径），供所有后续 issue/实现复用。
- 让日志/UI/debug 面板能“一眼回答”：
  - 我到底是哪只 bot（配置别名）在处理？
  - 哪个 Telegram chat/thread 的消息？
  - 对应 Moltis 的哪个会话桶？
  - 工具执行用了哪个 sandbox（scope + effective key）？
- 用增量迁移的方式消除现有 `session_key` 的语义漂移与同名字段复用。

## 非目标（Non-goals）
- 不要求一次性大重构（允许分 Phase 推进）。
- 不在本 Issue 内引入多 agent 常驻架构（本单是其前置基础设施：术语与可观测性收敛）。

---

## 核心术语与键（Core Vocabulary, Frozen）
> 只要涉及“分桶/路由/鉴权/落盘”，只能使用本节术语；其余词汇只能作为呈现层别名。

### A) Telegram 域（平台定义）
- `user_id`：发送者/成员 id（人和 bot 在 Telegram 都是 user）
- `chat_id`：对话容器 id（DM/群/频道统一都叫 chat）
- `thread_id`：Topic/Thread id（可选；仅 supergroup topics）
- `message_id`：消息 id

### B) Moltis 域（系统定义）
#### 1) `account_id`（保留此名，但冻结语义）
`account_id` = Moltis 配置里“这只 Telegram bot 实例”的别名（例如 `fluffy`/`lovely`）。

- 它**不是** Telegram 的 account 概念；
- 语义：选择哪套 token/连接参数、以及在同一群里区分多只 bot；
- 约束：必须稳定、可读、可作为 key 片段；不依赖 Telegram `@username`。

#### 2) `session_key`（唯一跨域桥；建议对用户保持这个名字）
`session_key = telegram:<account_id>:<chat_id>[:<thread_id>]`

示例：
- DM：`telegram:fluffy:8454363355`（chat_id 可能是正数）
- 群：`telegram:fluffy:-5288040422`
- topics：`telegram:fluffy:-5288040422:12`

**硬规则**
- `session_key` 只允许表示“账号实例 + chat/thread 的会话桶”；
- 任意工具上下文（如 `_session_key`）、sandbox key 派生、channel ingest/reply routing 必须以 `session_key` 为主轴；
- `session_key` 不得拼 `@username`，不得拼 display name/title。

#### 3) 内部持久会话（为消歧：建议用 `session_id` / `conversation_id`）
仓库现状里存在 `session:<uuid>` 这种“持久会话 key”，用于承载历史、compaction、metadata 等。

为了彻底止住歧义：对外呈现与跨域桥固定为 `session_key`；而 `session:<uuid>` 在文档/UI/日志里应标注为 `session_id`（或 `conversation_id`），并尽量只在 debug/详情里出现。

---

## 呈现口径（Presentation Contract, Frozen）
> 呈现字段要“少且稳定”，并与核心键明确解耦。

### 默认呈现（日志/UI 默认只显示 3 项）
1) `actor`：`<account_id>[/@handle?]`
2) `where`：`chat:<chat_id>[#<thread_id>] <title?>`
3) `session`：`<session_key>`

### 详情呈现（展开后可见，但不得参与逻辑）
- `bot_handle`（Telegram `@username`，可空/可变）
- `sender_display_name`、`chat_title`
- `bot_user_id`、`sender_user_id`
- `message_id`、entities 解析结果
- `container_name`/hash 等

---

## Sandbox / Tools 相关口径（最小必要）
> 仅定义必须出现在可观测性里的两项，避免再引入新的“工作台/环境”名词漂移。

- `sandbox_scope`：配置值 `tools.exec.sandbox.scope=session|chat|bot|global`
- `effective_sandbox_key`：由 `sandbox_scope + session_key` 派生的复用键（必须可在 debug/详情里反查）

---

## 方案（Proposed Solution / Migration Plan）
### Phase 0：文档冻结
- [ ] 本 Issue 的 Glossary 与 Presentation Contract 作为“团队共识基线”冻结；
- [ ] 在后续 Telegram / sessions / sandbox / multi-agent 相关 issue 里引用本术语表。

### Phase 1：可观测性收敛（先让人看得懂）
- [ ] 日志统一输出：默认 3 项（actor/where/session），并提供展开详情字段；
- [ ] `/context` 与 debug panel 统一显示：
  - `session_key`（核心桥）
  - `account_id`（配置别名）
  - `chat_id/thread_id`（Telegram 定位）
  - `sandbox_scope/effective_sandbox_key`（工具环境定位）
- [ ] 任何输出避免裸词 `session_key` 漂移：如果出现 `session:<uuid>`，必须标注为 `session_id`（或 `conversation_id`）。

### Phase 2：代码命名与类型消歧（逐步、可回滚）
- [ ] 将 `default_channel_session_key()` 等函数注释/命名逐步收敛为“生成 `session_key`（跨域桥）”；
- [ ] 在跨模块边界把 `session_key` 拆成两个明确字段（或新类型）：
  - `session_key`（跨域桥，确定性）
  - `session_id`（内部持久会话，opaque）
- [ ] 把 `bot_username` 的语义固定为 `bot_handle`（presentation only），禁止在核心逻辑里做 key。

---

## 验收标准（Acceptance Criteria）
- [ ] 术语：本文件成为后续设计/实现的引用基线（至少被关键 issue 引用）。
- [ ] 可观测性：同一条链路里能稳定对齐 `account_id + chat_id/thread_id + session_key`，无需猜。
- [ ] 消歧：用户与调试输出中不再出现“无法判断语义”的 `session_key`；内部 `session:<uuid>` 一律标注为 `session_id`/`conversation_id`。

## 交叉引用（Cross References）
- `issues/done/issue-telegram-group-mention-gating-not-working.md`
- `issues/done/issue-telegram-context-debug-parity.md`
- `issues/done/issue-chat-debug-panel-llm-session-sandbox-compaction.md`

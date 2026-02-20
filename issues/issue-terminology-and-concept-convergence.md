# Issue: Terminology / Concept Convergence（命名与概念收敛不一致导致理解与实现成本过高）

## 实施现状（Status）【增量更新主入口】
- Status: TODO（记录在案，暂缓处理）
- Priority: P2（长期质量/可维护性问题；会放大后续需求与排障成本）
- Components: gateway / sessions / channels / telegram / ui-debug / docs

---

## 背景（Background）
当前代码库中存在多套“会话/作用域/身份/渠道”相关概念，且在不同模块中使用相同或相近的术语（例如 `session_key`、`account_id`、`group_id`、`channel`、`scope`）表达不同语义，导致：
- 新功能设计难以对齐口径（容易“各做各的”）
- 日志与 debug 信息难以解读（同名字段含义不一致）
- 排障时容易把“平台投递问题”与“本地门禁/会话绑定问题”混在一起

该问题已在 Telegram mention、session routing、prompt cache key 分桶、/context debug 等议题中反复出现。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一个词在不同层含义不同：
   - `session_key` 既被用于“渠道 chat 的确定性键”（如 `telegram:<account_id>:<chat_id>`），又被用于“持久会话 key”（如 `session:<uuid>`）。
2) 不同词却指同一件事或强相关对象：
   - Telegram：`group_id`/`chat_id` 在某些地方混用，容易误导“group == group_id”的语义边界。
3) 身份相关字段命名不收敛：
   - `account_id`（配置中的频道账号标识） vs `bot_username`（Telegram `@handle`）经常在日志/UI 中混用，导致用户误解“我到底在跟谁说话”。
4) `scope` / `channel` / `group` / `dm` 的层级关系缺乏统一词汇表与图示。

### 影响（Impact）
- 设计沟通成本高：需要大量“先解释名词”才能讨论需求。
- 实现容易偏航：不同模块可能各自做“合理但不一致”的假设。
- Debug 难：同一条链路里出现多个 `session_key`，但无法快速判断“这是 chat scope 还是 conversation session”。

## 概念与口径（Glossary & Semantics）【建议冻结为统一词汇表】
> 本节是本 Issue 的核心交付之一：冻结术语，后续所有 issue/doc/debug/UI 统一使用。

### 1) Channel / Connector（渠道）
例如 `telegram` / `web` / `discord`。用于描述消息来源。

### 2) Channel Account（渠道账号）
例如 Telegram 中的一套 bot token 实例。在配置中通常以 `account_id` 作为标识（建议术语：`channel_account_id`）。

### 3) Chat（对话入口）
渠道内的一个 chat（Telegram `chat_id` / Web 的连接或房间等）。同一个 chat 通常对应“群级上下文”。

### 4) Chat Scope Key（建议新增统一术语）
稳定标识“某渠道账号 + 某 chat”的键，建议口径：
`<channel_type>:<channel_account_id>:<chat_id>`
示例：`telegram:fluffy:-5288040422`

### 5) Conversation Session（持久会话）
真正承载 LLM history/compaction/sandbox override/tool `_session_key` 的持久会话，示例：
`session:<uuid>`

### 6) Active Session Binding（chat → active session 映射）
`ChatScopeKey -> ConversationSessionKey` 的映射关系（例如 `channel_sessions` 表的含义）。

## 现状证据（Evidence in Code）
- ChatScopeKey（确定性键）存在：
  - `crates/gateway/src/channel_events.rs`：`default_channel_session_key()` 返回 `"{channel}:{account_id}:{chat_id}"`
- Active session binding 表存在：
  - `crates/sessions/src/metadata.rs`：`channel_sessions` 表、`get_active_session()`、`set_active_session()`
- Prompt runtime context 里出现 `session_key` 字段，但缺乏“这是 ChatScope 还是 ConversationSession”的明确标注：
  - `crates/agents/src/prompt.rs`：`PromptHostRuntimeContext.session_key`
- Telegram 侧同时存在 `account_id`（配置标识）与 `bot_username`（Telegram handle）：
  - `crates/telegram/src/bot.rs`：`get_me().username`
  - `crates/telegram/src/config.rs`：以 `account_id` 启动账号

## 目标（Goals）
- 统一术语表（Glossary）并在 docs/issues 中冻结。
- UI debug / 日志 / /context 输出对齐术语：明确区分 ChatScopeKey 与 ConversationSessionKey。
- 逐步重命名/重构：避免同名字段跨层复用导致歧义（不要求一次性大改）。

## 非目标（Non-goals）
- 不在本 issue 内改变实际路由/会话行为（先收敛口径与可观测性）。
- 不在本 issue 内做大规模破坏性重构（以小步可回滚为原则）。

## 方案（Proposed Solution）
### Phase 0（文档与口径收敛）
- 新增/完善统一 Glossary（本单已给出建议版本）。
- 在关键 issue 模板、/context debug 文档中引用该 Glossary。

### Phase 1（可观测性收敛：先让人看得懂）
- 日志/Debug 面板同时展示：
  - `chat_scope_key`（稳定输入来源键）
  - `conversation_session_key`（持久会话 key）
  - `channel_account_id`（配置标识）+ `bot_handle`（如 Telegram `@username`）
- 在输出中避免使用裸词 `session_key`，必须带前缀或标签。

### Phase 2（代码命名收敛：逐步消歧）
- 将 `default_channel_session_key()` 等函数命名/注释改为 `chat_scope_key` 语义。
- 将 `SessionKey`/`session_key` 在需要时拆分为两个不同类型（或至少不同字段名）。
- 将 `account_id` 在跨模块边界时改名为 `channel_account_id`（避免与“用户 account / OAuth account”混淆）。

## 验收标准（Acceptance Criteria）
- [ ] 文档层：统一 Glossary 落地并被模板引用。
- [ ] Debug/日志：同一条链路里不再出现“无法判断语义”的 `session_key` 字段；至少展示清晰标签。
- [ ] 代码层：关键边界（gateway<->channels<->sessions）术语一致，减少同名复用。

## 交叉引用（Cross References）
- `issues/done/issue-telegram-group-mention-gating-not-working.md`（多 bot 群聊时“收到/处理/留痕”三层口径）
- `issues/done/issue-telegram-context-debug-parity.md`（/context debug 口径与字段命名）
- `issues/done/issue-chat-debug-panel-llm-session-sandbox-compaction.md`（debug 面板字段口径收敛）

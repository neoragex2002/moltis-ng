# Issue: 术语与概念收敛（核心身份坐标冻结 / 禁止 alias）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Owners: <可选>
- Components: gateway / agents / tools / sessions / channels / telegram / web-ui / docs / issues
- Affected providers/models: openai-responses（prompt cache）

**已实现（既有实现，需按新口径复核）**
- 生成/解析确定性渠道坐标（旧名 `session_key`）：`crates/common/src/identity.rs:41`
- Telegram 入站维护 chat → active 持久会话桶映射：`crates/gateway/src/channel_events.rs:895`
- `session_state` 工具优先使用持久会话桶（旧键 `_session_id`）：`crates/tools/src/session_state.rs:76`

**已覆盖测试（如有）**
- `prefers_session_id_when_present`：`crates/tools/src/session_state.rs:187`

**已知差异/后续优化（非阻塞，但会持续制造回归）**
- Web UI 仍以 `sessionKey` 作为路由/媒体寻址主键：`crates/gateway/src/assets/js/page-chat.js:1090`
- WebSocket payload 仍做 `sessionId || sessionKey` 兜底：`crates/gateway/src/assets/js/websocket.js:292`
- tools 上下文仍注入 `_session_id/_session_key/_conn_id` 且存在 `tool_session_key`：`crates/gateway/src/chat.rs:2000`

---

## 背景（Background）
- 场景：Web UI / Telegram 入站 → gateway 选择会话桶 → LLM/Tools 执行 → 结果回传与持久化。
- 约束：必须支持同一渠道 chat 下 `/new` 切换多个持久会话桶（fork/branch 同理）；必须支持 sandbox 复用边界（默认 chat）。
- Out of scope：引入新的对外 alias（即便只是“临时兼容”）。

现状问题不是“字段太多”，而是“字段名与语义不一一对应”：
- 同一个词（例如 `sessionKey/session_key`）在不同层代表不同概念；
- 兼容策略长期采用“多字段/多别名双写”，导致永远无法收敛；
- 文档与 issue 描述长期滞后，反复把旧口径抄回实现。

本 issue 的目标：
- **以 `docs/src/concepts-and-ids.md` 为唯一权威口径**；
- **在协议层实现“所见即所得”的字段语义**；
- **禁止 alias 扩散**；
- **一次性清理干净（硬切换）**：不为存量数据迁移/兼容性留后门。

---

## 实施前置条件（Preconditions）【必须具备】

本 issue 采取“一步到位、硬切换”的策略，因此需要明确实施条件，避免半切换导致更大混乱。

- [ ] 允许清空并重建存量数据（例如 `moltis.db`、`sessions/` 数据目录）。
- [ ] 能保证 Web UI 与 gateway 同步升级（同一发布/同一二进制/同一环境）。
- [ ] 不需要兼容第三方/外部客户端（RPC/WS/hook 脚本）在升级窗口内继续使用旧字段名。
- [ ] 有一个可用于验证的环境（本地或 CI），至少能跑 `cargo check` 与关键单测。

若任一前置条件不满足，则本 issue 不应推进（否则会出现“双口径长期并存”的隐患）。

---

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 权威口径：`docs/src/concepts-and-ids.md`。
>
> 本章节只允许记录“旧词汇 → 新词汇”的迁移映射（Legacy mapping）。
> **正文与对外契约只使用主称呼**。

### 主称呼（Frozen）

- **`sessionId`**（主称呼）：持久会话桶 / 存储地址。
  - Why：历史/媒体/metadata 的唯一寻址边界；fork/branch 必须产生新 `sessionId`。
  - Not：不能用于推断渠道身份（bot/chat/thread）。
  - Source/Method：authoritative（系统生成并持久化）。

- **`chanChatKey`**（主称呼）：确定性对话坐标（跨域桥）。
  - 定义：`<chanType>:<chanUserId>:<chatId>[:<threadId>]`
  - Why：路由/绑定/可观测；同时是 sandbox 默认复用边界（scope=chat）。
  - Not：不是持久桶（不能表达 `/new`/fork 产生的多个并行会话）。
  - Source/Method：configured + effective（由渠道事件字段确定性构造）。

- **`chanAccountKey`**（主称呼）：渠道账号稳定主键。
  - 定义：`<chanType>:<chanUserId>`
  - Why：标识“是哪只 bot/哪个渠道账号配置”。
  - Not：不是展示字段。
  - Source/Method：authoritative（由平台稳定 id 推导）。

- **`chanReplyTarget`**（主称呼）：可执行的回信地址对象。
  - 必须包含：`chanType`、`chanAccountKey`、`chatId`
  - 可包含：`messageId`
  - Not：展示字段（`chanUserName/chanNickname`）不得影响逻辑。

### Legacy mapping（仅记录，不得在对外契约继续使用）

- `sessionKey` / `session_key`：历史漂移词（禁止继续输出）。
  - 当值形如 `session:<uuid>` 或 `main` → 语义等价 `sessionId`
  - 当值形如 `<chanType>:<chanUserId>:<chatId>...` → 语义等价 `chanChatKey`
- `account_handle` / `account_id` / `accountHandle`：语义等价 `chanAccountKey`（且内部命名也必须改为 `chan_account_key`，禁止继续使用 `account_handle/account_id`）
- `channel_type` / `ChannelType`：语义等价 `chanType`
- `channel_binding`：语义应收敛为 `chanReplyTarget`（存储形态可以是 JSON 字符串，但对外概念必须是对象）
- `bot_handle`：语义等价 `chanUserName`（display-only，不得参与 key/路由/绑定/存储）
- 工具上下文旧键：`_session_id/_session_key/_conn_id` → 目标：`_sessionId/_chanChatKey/_connId`

---

## 决策冻结（Decisions）【本 issue 内不得再改口径】

- 对外契约字段名：统一 `camelCase`。
- 内部实现（Rust/DB）：默认 `snake_case`。
  - 内部稳定主键命名必须对齐核心概念：`chan_account_key` 禁止继续使用 `account_handle/account_id`。
  - `chan_chat_key/chan_type/chan_reply_target` 同理（snake_case 版本）。
- prompt cache bucket key：固定使用 `sessionId`。
- sandbox scope 默认值：固定为 `chat`（按 `chanChatKey` 复用边界）。
- 禁止：对外出现 `sessionKey/session_key`；禁止任何新增 alias；禁止双写输出。
- tools context：一步到位切换到 `_sessionId/_chanChatKey/_connId`（不保留 `tool_session_key`）。

---

## 需求与目标（Requirements & Goals）

### 功能目标（Functional）

- [ ] 对外契约（RPC/WS/Hooks/UI/Docs）只输出冻结字段名：
  - `sessionId`（必填）
  - `chanChatKey`（channel 场景必填）
  - `chanAccountKey`（涉及渠道账号配置的场景必填）
  - `chanType`、`chatId`、`messageId`（按需）
  - `chanReplyTarget`（涉及回信能力必填）
- [ ] 移除对外 `sessionKey` 一词（避免继续歧义扩散）。
- [ ] prompt cache bucket key 固定为 `sessionId`。
- [ ] sandbox scope 默认值固定为 `chat`（按 `chanChatKey` 复用边界）。

### 非功能目标（Non-functional）

- 正确性口径（必须/不得）：
  - 必须：字段名与语义一一对应（看到 `sessionId` 就是持久桶；看到 `chanChatKey` 就是渠道坐标）。
  - 不得：对外 payload 双写 camelCase + snake_case。
  - 不得：引入新的 alias（包括工具上下文、docs 示例、issue 文字）。
- 兼容性（硬规则）：**不做兼容、不做迁移**。
  - 允许的唯一“兼容动作”：在本次切换落地前，先把所有对外消费者（Web UI、hooks 脚本、工具链）改到新字段。
  - 禁止：保留旧字段名输入解析、保留旧字段名输出、保留 alias 并行存在。
- 可观测性：日志/Debug 面板字段必须与冻结概念一致。

### 命名风格与最小改动原则（Implementation Discipline）

- 内部实现（Rust 变量/struct 字段/DB 列名）优先使用 `snake_case`（Rust 生态默认 + 存量最多），避免无意义重命名。
- 对外契约（RPC/WS/Hooks/UI/Docs）统一使用 `camelCase`（与冻结概念同名），并且只输出主称呼。
- 如果某个内部命名会持续诱发语义误读（例如把 `sessionId` 叫成 `key`），允许一次性重命名并接受 breaking change（本 issue 不考虑存量数据）。

---

## 问题陈述（Problem Statement）

### 现象（Symptoms）

1) UI/Docs/WS/RPC 仍使用 `sessionKey/session_key` 这类历史漂移词。
2) tools 上下文与 hooks 仍使用旧键名（`_session_id/_session_key/session_key`），使开发者把它们当作“权威名词”。
3) DB/metadata 内部字段名 `key`/`parent_session_key` 等语义不直观，容易误读为跨域桥。

### 影响（Impact）

- 用户体验：字段名无法自解释，解释成本高。
- 可靠性：共享/隔离边界误用风险高（prompt-cache、sandbox、state scope）。
- 排障成本：同一链路日志难串联。

---

## 现状核查与证据（As-is / Evidence）【不可省略】

- 权威口径：`docs/src/concepts-and-ids.md:1`
- UI 使用 `sessionKey`：`crates/gateway/src/assets/js/page-chat.js:1090`
- WS 兜底 `sessionId || sessionKey`：`crates/gateway/src/assets/js/websocket.js:292`
- tools 上下文注入旧键：`crates/gateway/src/chat.rs:4695`
- runner hooks 从 `_session_key` 抽取：`crates/agents/src/runner.rs:718`
- metadata 使用 `SessionEntry.key`：`crates/sessions/src/metadata.rs:15`
- sandbox scope 默认值目前为 `session`（需改成 `chat`）：`crates/tools/src/sandbox.rs:347`

---

## 根因分析（Root Cause）

- A. 历史上为了快速贯通 channel ↔ session ↔ tools，把“持久桶地址”和“渠道坐标”混进同名词（`sessionKey/session_key`）。
- B. 兼容策略长期采用“多别名双写”，没有任何一个版本真正只输出主称呼。
- C. 文档/issue 没有被当作协议的一部分同步更新，导致旧口径持续回流。

---

## 期望行为（Desired Behavior / Spec）【尽量冻结】

- 必须：
  - 对外只输出 `sessionId`（持久桶）与 `chanChatKey`（渠道坐标）。
  - prompt cache bucket key 必须等价 `sessionId`。
  - sandbox scope 默认必须等于 `chat`（按 `chanChatKey` 复用边界）。
- 不得：
  - 不得输出 `sessionKey/session_key/account_id/account_handle/channel_type/channel_binding` 作为对外字段名。
  - 不得新增任何 alias。
- 应当：
  - DB 内部列名可以暂不迁移，但对外 JSON/协议字段名必须按冻结概念映射（例如 `parentSessionId`）。

---

## 方案（Proposed Solution）

### 最终方案（Chosen Approach）

#### 行为规范（Normative Rules）

- 对外字段风格冻结为 `camelCase`（RPC/WS/Hooks/UI/Docs）。
- 兼容性：不做输入解析兼容、不做双写；一次性切换。
- prompt cache bucket key = `sessionId`。
- sandbox 默认 scope = `chat`。

#### 接口与数据结构（Contracts）

- RPC/WS：
  - `sessionId`：必填
  - `chanChatKey`：channel 场景必填
  - `chanReplyTarget`：需要回信能力时必填
- Tools context：
  - `_sessionId`：必填
  - `_chanChatKey`：channel 场景必填
  - `_connId`：可选
- sessions metadata：
  - 对外字段名以冻结概念为准；内部 `key` 语义等价 `sessionId`。

---

## 验收标准（Acceptance Criteria）【不可省略】

- [ ] Web UI 不再使用 `sessionKey` 作为核心字段（路由、媒体 URL、localStorage）。
- [ ] RPC/WS/Hooks 对外输出只剩冻结字段（不双写、不旧名）。
- [ ] tools 上下文只使用 `_sessionId/_chanChatKey/_connId`（不出现 `tool_session_key`）。
- [ ] prompt cache bucket key 按 `sessionId` 生效。
- [ ] sandbox 默认 scope=chat 生效。
- [ ] 全仓 grep 结果满足（至少对外层满足）：
  - 不再出现对外 `"sessionKey"` 字段输出
  - 不再出现对外 `"session_key"` 字段输出
  - 不再出现 `tool_session_key`
  - 不再出现 `account_handle` / `account_id`（内部实现也必须改为 `chan_account_key`）
  - 不再出现 `MsgContext.session_key`（必须拆分为 `sessionId` + `chanChatKey`）

## 测试计划（Test Plan）【不可省略】

### Unit

- [ ] provider prompt cache：bucket key 使用 `sessionId` 语义（改名可后做，但语义必须对齐）：`crates/agents/src/providers/openai_responses.rs:548`
- [ ] tools session_state：scope 仅按 `sessionId`：`crates/tools/src/session_state.rs:76`

### Integration

- [ ] Telegram：同一 `chanChatKey` 下 `/new` 后 `sessionId` 切换；sandbox 复用边界按 `chanChatKey`。

### UI E2E（Playwright，如适用）

- [ ] session 切换、媒体播放、通知跳转都以 `sessionId` 寻址。

---

## 发布与回滚（Rollout & Rollback）

- 发布策略：一次性切换（big bang），所有对外消费者同步升级。
  - Web UI、WS/RPC、tools context、hooks docs 与样例必须在同一次变更里对齐。
  - 本次切换默认假设允许清空存量数据（例如重建 `moltis.db` 与 `sessions/` 数据目录）。
- 回滚策略：如果必须回滚，回滚到“上一套完整口径”的版本；不要在同一版本里同时存在两套字段名。

## 实施拆分（Implementation Outline）

- Stream A（Docs P0）：
  - `docs/src/session-branching.md`
  - `docs/src/session-state.md`
  - `docs/src/hooks.md`
  - `docs/src/mobile-pwa.md`
- Stream B（Web UI）：
  - 替换路由/localStorage/媒体寻址：`crates/gateway/src/assets/js/page-chat.js`
  - 去掉 `sessionId || sessionKey` 兜底：`crates/gateway/src/assets/js/websocket.js`
- Stream C（WS/RPC）：
  - payload 只输出冻结字段；移除 `sessionKey`。
- Stream D（Tools/Agents）：
  - tools 上下文改为 `_sessionId/_chanChatKey/_connId`；runner hooks 对齐。
- Stream E（DB/metadata 映射）：
  - 对外字段名与冻结概念一致；内部列名暂不迁移也可。
- Stream F（Common types & hooks）：
  - 收敛 `MsgContext`（禁止继续扩展旧字段名 `channel/account_handle/session_key/...`，按冻结概念拆分与更名）。
  - 收敛 hooks payload（`HookPayload.session_key` → `sessionId`，并在需要时显式增加 `chanChatKey`）。
  - 目标：彻底切断“公共协议类型把旧术语扩散到各 crate”的回流路径。

### 推荐执行顺序（确保一次性切换不翻车）

1) 先修文档与 issue（Docs P0 + issue drift）：保证“权威口径不会回流”。

2) 先修“旧术语扩散源头”（Common types & hooks）：
   - 优先处理 `MsgContext` 与 `HookPayload` 等公共协议类型，避免它们继续把
     `channel/account_handle/session_key` 旧口径传播到各 crate。
   - 这是一次性切换（big bang）的关键：不先切断源头，后续任何清理都会被回流污染。

3) 再修 gateway 的 WS/RPC 输出：对外只输出新字段名（冻结的 `camelCase`）。

4) 同步修 Web UI：移除 `sessionKey` 心智与 fallback（例如 `sessionId || sessionKey`），
   路由/媒体/localStorage 全部只认 `sessionId`。

5) 同步修 tools context + agents hooks：只注入/读取 `_sessionId/_chanChatKey/_connId`。

6) 最后修 DB/metadata 对外映射与命名（必要时一次性改内部列名；本 issue 允许清库重建）。

7) 最后做硬验收：
   - 全仓 grep：不得再出现 `sessionKey/session_key/tool_session_key/account_handle/...`
   - 编译与测试：至少 `cargo check` + 关键单测
   - UI 链路：切换会话、媒体播放、通知跳转、hooks 触发

任何顺序导致“对外同时存在两套字段名”，都应视为失败并立即回滚。

---

## 交叉引用（Cross References）

- Authoritative glossary：`docs/src/concepts-and-ids.md`
- Drift hotspots：`docs/src/session-branching.md`、`docs/src/session-state.md`、`docs/src/hooks.md`、`docs/src/mobile-pwa.md`
- Related issues：
  - `issues/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md`
  - `issues/issue-spawn-agent-session-key-model-selection-timeout-and-errors.md`

## 未决问题（Open Questions）

本 issue 采取“一步到位”策略，未决问题在此关闭为决策：

- [x] sessions metadata / DB 内部的 `key`/`parent_session_key`：本次允许一次性改名到语义自解释（不考虑存量）。
- [x] tools context：本次切换到 `_sessionId/_chanChatKey/_connId`，不保留 alias。

## Close Checklist（关单清单）【不可省略】

- [ ] 对外字段名与语义已按 `docs/src/concepts-and-ids.md` 收敛（不双写、不 alias）
- [ ] prompt-cache bucket 与 sandbox 默认策略已按冻结口径生效
- [ ] docs + issues 已同步（避免旧口径回流）
- [ ] UI E2E 或手工验收覆盖已补齐
- [ ] 回滚策略明确（不允许在同一版本里双口径并存）

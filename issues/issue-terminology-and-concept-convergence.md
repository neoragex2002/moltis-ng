# Issue: Terminology / Concept Convergence（概念与术语收敛：两域分离、核心键冻结、呈现口径统一）

## 实施现状（Status）【增量更新主入口】
- Status: IN-PROGRESS
- Priority: P2
- Components: gateway / sessions / channels / telegram / ui-debug / docs

**已实现（2026-02-25）**
- 引入确定性 `session_key` 格式化/解析工具：`crates/common/src/identity.rs:41`
- Channel ingest 同时维护 `session_id`（持久会话）与 `session_key`（跨域桥）：`crates/gateway/src/channel_events.rs:100`
- `run_with_tools` 增加 `tool_session_key`（用于工具上下文、sandbox key）：`crates/gateway/src/chat.rs:4270`
- `run_with_tools` 工具上下文同时注入 `_session_id`（持久会话 id）与 `_session_key`（跨域桥）：`crates/gateway/src/chat.rs:4695`
- tools 侧对持久会话相关操作优先使用 `_session_id`（避免 channel session 使用 UUID 时读错历史/metadata）：`crates/tools/src/branch_session.rs:59`、`crates/tools/src/session_state.rs:79`、`crates/tools/src/location.rs:375`
- gateway 测试覆盖 tools 上下文 `_session_id` 注入：`crates/gateway/src/chat.rs:8583`
- 修复 sandbox/router 与 channel session_id 的键不一致：gateway 在写入/加载 sandbox overrides 以及 session delete cleanup 时，对 channel session 使用 `channel_binding` 派生的确定性 key 与 SandboxRouter 交互：`crates/gateway/src/session.rs:140`、`crates/gateway/src/server.rs:1739`

**已覆盖测试（如有）**
- `run_with_tools_passes_session_key_via_llm_request_context`：`crates/gateway/src/chat.rs:8411`
- `run_with_tools_injects_session_id_into_tool_calls`：`crates/gateway/src/chat.rs:8583`
- `sandbox_session_key_for_channel_binding_parses_legacy_account_id`：`crates/gateway/src/session.rs:797`

**已知差异/后续优化（非阻塞）**
- Web UI/WS payload 仍需逐步清理旧字段别名（例如兼容 `account_handle`/`accountHandle` 的过渡期）。

---

## 背景（Background）
- 场景：Telegram 入站消息 → gateway 生成会话桶 → LLM/Tools 执行 → 结果回传 Telegram + Web UI。
- 约束：术语必须能支撑日志排障、UI debug、持久化 schema、以及 sandbox 复用键。
- Out of scope：一次性全仓大重构（允许分阶段迁移）。

当前仓库里长期存在“Telegram 域概念”和“Moltis 域概念”混用，且出现同名字段跨层复用（尤其 `session_key` / `account_id` / `scope`）。这会让：
- 需求讨论必须先解释名词；
- 日志与 UI 很难把同一链路串起来；
- 多 bot、多 session、多 sandbox scope 的组合复杂度指数级上升。

---

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`account_handle`**（主称呼）：Channel 账号实例的稳定主键（对 Telegram：`telegram:<chan_user_id>`）
  - Why：跨进程/跨重启必须稳定；可作为存储 key 片段；不依赖可变的 `@username`。
  - Not：不是“配置别名”，不是 `@username`。
  - Source/Method：authoritative（由 `getMe` 的 `chan_user_id` 推导并固化）。
  - Aliases：`account_id`（历史名，禁止继续扩散）。

- **`bot_handle`**（主称呼）：平台展示用 handle（对 Telegram：`@username`，可空、可变）
  - Why：便于人读（日志/UI），但不能参与分桶/路由/鉴权/落盘。
  - Not：不得出现在任何核心 key 生成逻辑中。
  - Source/Method：authoritative（来自 Telegram `getMe`）。
  - Aliases：`bot_username`（历史名）。

- **`session_key`**（主称呼）：跨域桥（确定性会话桶键），用于把「某只 bot + 某个 chat/thread」映射到一个逻辑会话桶
  - 定义：`<channel>:<chan_user_id>:<chat_id>[:<thread_id>]`
  - Why：稳定、可预测、可在日志/UI 直接对齐 Telegram 定位。
  - Not：不是持久会话 id；不得包含 display name/title/`@username`。
  - Source/Method：configured+effective（由输入字段确定性生成，见 `format_session_key`）。
  - Aliases：旧 `session_key`（曾漂移为 `session:<uuid>`，此用法必须消失）。

- **`session_id`**（主称呼）：内部持久会话 id（opaque），用于承载历史、compaction、metadata 等
  - 定义：`session:<uuid>`（示例）
  - Why：允许会话迁移/切换而不破坏历史落盘结构；避免把“持久 id”误当成“跨域桥”。
  - Not：不应作为 sandbox 复用键，不应作为跨域路由主轴。
  - Source/Method：authoritative（由系统生成并落库）。

---

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 两域严格隔离：Telegram 域术语与 Moltis 域术语不得混用。
- [ ] 跨域桥只保留一个主键：`session_key` 必须是唯一跨域桥。
- [ ] 核心 vs 呈现严格区分：`bot_handle`、title、display name 只能用于呈现。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：sandbox/tools 复用键以 `session_key` 为主轴（可观测、可复现）。
  - 不得：任何地方用 `bot_handle`/chat title 拼 key。
- 兼容性：允许短期兼容旧 WS payload 字段名，但核心存储/路由必须统一。
- 可观测性：默认日志/UI 只展示少且稳定的字段（见后续 Spec）。

---

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同名字段在不同层含义不同（例如 `session_key` 同时指“确定性桶键”和“持久会话 id”）。
2) UI/日志里无法一眼判断“哪个 bot + 哪个 chat/thread + 哪个持久会话”。

### 影响（Impact）
- 用户体验：debug 信息不稳定，解释成本高。
- 可靠性：错误路由/错误复用 sandbox 的风险上升。
- 排障成本：同一链路的日志难以串联。

---

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/common/src/identity.rs:41`：`format_session_key` 固化 `session_key` 结构
  - `crates/gateway/src/channel_events.rs:118`：广播 payload 同时包含 `sessionId` + `sessionKey`
  - `crates/gateway/src/chat.rs:2000`：`chat.send` 支持 `_session_id` 与 `_session_key` 分离
  - `crates/gateway/src/chat.rs:4695`：工具上下文 `_session_key` 使用 `tool_session_key`，并同时注入 `_session_id`
- 当前测试覆盖：
  - 已有：`crates/gateway/src/chat.rs:8411`
  - 缺口：UI E2E 未覆盖（需手工验证 WS payload 渲染）。

---

## 根因分析（Root Cause）
- A. 历史上为快速实现跨域路由，把“确定性桶键”和“持久会话 id”都塞进了 `session_key`。
- B. `account_id` 既被当作“配置别名”，又被当作“Telegram bot 身份”，导致字段名误导。

---

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `session_key` 只表示确定性桶键：`<channel>:<chan_user_id>:<chat_id>[:<thread_id>]`
  - `session_id` 只表示内部持久会话 id：`session:<uuid>`
  - Tools/sandbox 的复用键必须基于 `session_key`（并可在 debug/详情里反查）。
- 不得：
  - 不得用 `bot_handle`、chat title、display name 参与 key。
- 应当：
  - WS/UI 默认显示：`account_handle`（或更友好的 label）、`chat_id/thread_id`、`session_key`；`session_id` 仅在详情显示。

---

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- Channel ingest：对每个 (account_handle, chat_id) 维护一个“当前 active session_id”，但跨域桥始终使用 `session_key`。
- Chat service：
  - `session_key`（历史落盘/metadata 的 key）允许为 `session_id`
  - `tool_session_key` 专用于 tools/sandbox `_session_key`（跨域桥）
  - tools 侧如需读取/写入持久会话（历史/metadata/分叉等），应优先使用 `_session_id`

#### 接口与数据结构（Contracts）
- WS payload：同时发 `sessionId` 与 `sessionKey`（过渡期允许旧字段 fallback，但新逻辑以这两个为准）。
- 存储：
  - channel_sessions 记录 (channel_type, account_handle, chat_id) → active `session_id`
  - sessions 表以 `session_id` 为 key 存历史/metadata；`session_key` 仅用于确定性路由与工具上下文。

---

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 同一条链路能稳定对齐：`account_handle + chat_id/thread_id + session_key + session_id(详情)`。
- [ ] `session_key` 在日志/UI 中不再出现语义漂移（不得再代表 `session:<uuid>`）。
- [ ] gateway + sessions + telegram 编译与测试通过（至少 workspace 编译 + gateway 单测）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `run_with_tools_passes_session_key_via_llm_request_context`：`crates/gateway/src/chat.rs:8411`

### Integration
- [ ] 手工：Telegram 入站消息 → Web UI 实时显示，确认 WS payload 使用 `sessionId` 取历史/媒体路径，`sessionKey` 用于工具上下文与可观测性。

---

## 发布与回滚（Rollout & Rollback）
- 发布策略：无 feature flag（属于命名/口径与 schema 迁移，按迁移脚本推进）。
- 回滚策略：保留 WS payload 的旧字段兼容一段时间；数据库迁移回滚需要显式降级脚本（暂不提供）。

## 实施拆分（Implementation Outline）
- Step 1: 引入 `identity::format_session_key` 并替换旧拼接
- Step 2: 引入 `session_id` 与 `session_key` 并行传递（WS/UI/tools）
- Step 3: 清理旧字段名与旧语义（逐步）
- 受影响文件：
  - `crates/common/src/identity.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/chat.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-chat-debug-panel-llm-session-sandbox-compaction.md`
  - `issues/issue-spawn-agent-session-key-model-selection-timeout-and-errors.md`

## 未决问题（Open Questions）
- Q1: 是否需要引入“人可读 bot alias”（仅呈现）并在 UI/日志默认展示？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

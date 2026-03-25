# Issue: Session Web UI 一刀切专项治理（naming / routing / switch race / stream binding）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P0
- Updated: 2026-03-25
- Checklist discipline: 每次增量更新除补“已实现 / 已覆盖测试”外，必须同步勾选正文里对应的 checklist；禁止出现文首已完成、正文 TODO 未更新的漂移
- Owners: TBD
- Components: gateway/ui/sessions/chat/websocket/router
- Affected providers/models: all（会话 UI 与流式绑定为上层通用路径）

**已实现（如有，必须逐条写日期）**
- 无；当前为专项治理前的 inventory / review / 开工筹备单。

**已覆盖测试（如有）**
- main/home/create 的基础返回形状已覆盖：`crates/gateway/src/session.rs:1178`
- `sessions.switch` 服务端“resolve 失败不污染 active session”已覆盖：`crates/gateway/src/methods.rs:5741`

**已知差异/后续优化（非阻塞）**
- 当前 issue 先冻结问题、边界、口径与测试面；暂不直接修改代码。
- 本单不接受“先补一个前端小判断先跑起来”的散点修补；必须按会话实例真源统一治理。

---

## 背景（Background）
- 场景：Web chat 页当前在 `new session`、列表展示、切换、clear、delete、流式回复、首屏恢复、搜索结果展示等链路上，出现了同类问题：同一个 UI 会话实例被多套状态和多种字段名同时解释。
- 约束：
  - 必须遵循第一性原则：会话实例只允许一个 authoritative owner。
  - 必须遵循唯一事实来源原则：UI、RPC、WS、流式状态不能各用一套 session 身份口径。
  - 必须遵循不后向兼容原则：不能继续同时容忍 `sessionId` / `sessionKey` 在 UI 实例归属上混用。
  - 必须优先治理关键路径：`new -> switch -> send -> stream -> clear/delete -> startup restore -> search`。
- Out of scope：
  - Telegram bucket/session 设计本身
  - session persistence schema 重构
  - 非 Web UI 渠道（Telegram / cron / hooks）独立行为规则

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **会话实例**（主称呼）：Web UI 当前正在查看、切换、发送、清空、删除的单个 session 对象。
  - Why：这是本单唯一要治理的 UI owner。
  - Not：不是逻辑桶名，不是流式 run，不是 sidebar 上某个纯展示文案。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`sessionId`

- **会话展示名**（主称呼）：服务端返回给 UI 的人类可读名称，用于 sidebar / header / search label。
  - Why：它决定用户是否能区分不同会话实例。
  - Not：不是 session owner；也不是前端自行从 `session_key` 推导的临时别名。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`displayName`

- **会话切换事务**（主称呼）：一次从旧会话切到新会话的完整异步过程。
  - Why：它必须是可序列化、可丢弃 stale 回包的事务，而不是“先改本地 active，再赌 RPC 回来顺序正确”。
  - Not：不是单纯一次 `sessions.switch` RPC。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：switch flow / switch request

- **聊天事件归属键**（主称呼）：WS `chat` 事件里用于把 delta/final/error/tool 结果绑定到正确会话实例的字段。
  - Why：流式与终态消息不能跟当前 active session 猜测绑定。
  - Not：不是任意能“差不多代表会话”的字段。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：event session owner

- **sidebar 集合操作**（主称呼）：作用于“全部会话集合”的按钮和 RPC。
  - Why：必须与“当前会话操作”明确区分。
  - Not：不是 header 里的 current-session action。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：clear all / bulk delete

- **authoritative**：来自服务端真实实例 id / 真实返回字段的权威值。
- **effective**：前后端状态机合并后的生效行为；若与 authoritative 冲突，必须以 authoritative 修正。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 收敛 Web 会话 UI 的唯一 owner：所有当前会话相关操作统一以 authoritative `sessionId` 驱动。
- [ ] 修复 sidebar / header / search / startup / switch / streaming / clear / delete 之间的会话归属错乱。
- [ ] 明确区分“当前会话操作”和“全部会话集合操作”，消除误导性文案和错误动作绑定。
- [ ] 修复当前流式回复在切换会话时串到别的会话上的关键路径问题。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须保证两个不同 session instance 不会因为 fallback display 名或流式状态混用而在 UI 层不可区分。
  - 必须保证 stale `switchSession()` 回包不会覆盖当前已切走的新会话页面。
  - 必须保证 WS `chat` 事件在前端无需猜测 active session 就能知道归属。
  - 不得继续让 UI mutating RPC 仅依赖“当前连接 active session”隐式定位目标。
  - 不得继续把 `sessionKey` 当作 Web UI 实例 owner 使用。
- 兼容性：本单按 one-cut 治理当前 Web UI 行为，不保留旧混用语义。
- 可观测性：需要补齐 stale switch 丢弃、缺失事件 owner、非法 UI action 目标等结构化日志 / debug 证据。
- 安全与隐私：日志不得打印完整消息正文；只记录 session/routing 诊断字段。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 连续点两次 `new session`，sidebar 里两个新会话都显示成 `Chat`，用户无法区分实例。
2) sidebar 里的 `Clear` 按钮看起来像“清当前会话”，实际会删掉所有可删除会话。
3) 在会话 A 回复过程中切到会话 B，回复可能继续画到 B 的页面里。
4) 快速切换会话时，旧 `sessions.switch` 回包可能覆盖新会话的 DOM 和历史。
5) 搜索结果、启动恢复、删除后的跳转等边缘路径，和主会话 contract 也没有完全统一。

### 影响（Impact）
- 用户体验：会话实例不可辨认、按钮语义误导、流式消息串页，属于核心交互破坏。
- 可靠性：当前 UI 的 active session、WS 事件、流式 DOM、服务端实例 owner 没有同一真源。
- 排障成本：同一个问题会在 naming、routing、switch、streaming、search、startup 多条链路反复出现。

### 复现步骤（Reproduction）
1. 打开 chat 页，连续点击两次 `new session`。
2. 观察 sidebar：会看到两个不同实例都显示为 `Chat`。
3. 在会话 A 发送一条会触发流式回复的消息；回复尚未完成时切到会话 B。
4. 观察期望 vs 实际：
   - 期望：A 的后续 delta/final 只留在 A；B 只显示 B 自己的内容。
   - 实际：A 的回复可能被渲染到 B 当前页面，或旧 switch 回包把 B 页面覆盖成 A 历史。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/session.rs:139`：fallback display 名从服务端生成，但把所有 `chat-*` scratch session 一律压成 `"Chat"`。
  - `crates/gateway/src/session.rs:336`：`sessions.create` 默认总是创建 `agent:default:chat-<opaque>` scratch session。
  - `crates/gateway/src/assets/index.html:210`：sidebar 按钮文案仅写 `Clear`，没有表达它是集合操作。
  - `crates/gateway/src/assets/js/sessions.js:220`：上述按钮实际调用 `sessions.clear_all`，会删除全部可删除 agent session。
  - `crates/gateway/src/assets/js/sessions.js:80`：当前会话 clear 未显式传 `_sessionId`，仅靠连接 active session。
  - `crates/gateway/src/assets/js/page-chat.js:887` + `crates/gateway/src/assets/js/helpers.js:21`：`chat.send` 也未显式传 `_sessionId`，只发送当前 params 原样 RPC。
  - `crates/gateway/src/assets/js/sessions.js:548`：`switchSession()` 在 RPC 返回前就先本地切 active session 并清空 DOM。
  - `crates/gateway/src/assets/js/sessions.js:572`：`switchSession()` 回包后未校验是否已 stale，仍直接渲染历史和恢复状态。
  - `crates/gateway/src/assets/js/websocket.js:578`：WS chat 事件前端使用 `p.sessionId || activeSessionId` 猜测事件归属。
  - `crates/gateway/src/chat.rs:190`：`ChatFinalBroadcast` 结构体字段名是 `session_key`，序列化后会变成 `sessionKey`，不是前端当前要求的 `sessionId`。
  - `crates/gateway/src/chat.rs:223`：`ChatErrorBroadcast` 同样使用 `session_key`。
  - `crates/gateway/src/chat.rs:5399`：`final` 事件通过 `ChatFinalBroadcast` 发出；与 `delta` 手工 JSON 的 `sessionId` 字段口径不一致。
  - `crates/gateway/src/chat.rs:4565`：`error` 事件通过 `ChatErrorBroadcast` 发出；同样口径不一致。
  - `crates/gateway/src/assets/js/websocket.js:308`：前端流式 DOM 使用全局 `S.streamEl` / `S.streamText`，不是按 session/run 隔离。
  - `crates/gateway/src/assets/js/websocket.js:381`：`final` 也继续消费全局流式状态，串线后污染当前页面。
  - `crates/gateway/src/assets/js/app.js:43`：根路由恢复逻辑在 bootstrap 前就尝试从空的 `sessionStore` 恢复上次 session。
  - `crates/gateway/src/assets/js/page-chat.js:1093`：chat 页初始化也在 bootstrap 前依赖 `sessionStore.getById(storedSessionId)`，失败时会降到空，再走 home fallback。
  - `crates/gateway/src/assets/js/session-search.js:60`：搜索结果 UI 读取 `hit.label`，而不是服务端返回的 `displayName`。
  - `crates/gateway/src/session.rs:858`：服务端搜索返回的是 `displayName` 字段。
  - `crates/gateway/src/assets/js/components/session-header.js:87`：delete 回调除了 uncommitted changes 特例外，没有校验一般失败就直接切走 next session。
- 当前测试覆盖：
  - 已有：`crates/gateway/src/session.rs:1178` 仅覆盖 home/create 返回形状；`crates/gateway/src/methods.rs:5741` 仅覆盖服务端 `sessions.switch` 失败时不污染连接 active state。
  - 缺口：没有覆盖 `new -> switch -> stream -> final` 的前端竞态；没有覆盖 WS event owner contract；没有覆盖 `Clear`/`Clear All` 语义；没有覆盖 startup restore 与 search label。

## 根因分析（Root Cause）
- A. 服务端展示口径和 UI 实例口径没有分层：scratch session 的 fallback display 名被压扁成常量 `"Chat"`。
- B. sidebar 集合操作与当前会话操作没有明确边界，文案和 RPC 绑定错位。
- C. 前端 `switchSession()` 把“准备切换”和“切换成功”混成同一个本地状态更新，没有 transaction / generation guard。
- D. WS chat event contract 漂移：部分事件发 `sessionId`，部分事件发 `sessionKey`，前端因此退回到 active session 猜测。
- E. 前端流式渲染状态是全局单例，不是按 session instance / run instance 绑定。
- F. 搜索、首屏恢复、删除回调等旁路链路没有跟主 contract 一起收口，形成系统性技术债。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - Web UI 当前会话的 authoritative owner 必须是 `sessionId`，且必须由服务端提供。
  - 所有 UI mutating RPC（至少 `chat.send`、`chat.clear`）必须显式携带目标 `sessionId` / `_sessionId`，不得只靠连接 active state。
  - 所有 WS `chat` 事件必须携带前端可直接消费的 authoritative `sessionId`。
  - `switchSession()` 必须具备 stale response 丢弃能力；旧请求回包不得覆盖新会话页面。
  - 流式 DOM / thinking / tool / voice pending 等 transient state 必须按 session instance（必要时再按 run）隔离。
  - scratch session 的服务端 fallback 展示名必须可区分，不能把所有 `chat-*` 折叠成同一个 `"Chat"`。
  - sidebar 集合操作必须在文案和交互上明确表达为 `Clear All` / `Delete All Sessions` 之类的集合语义。
- 不得：
  - 不得继续使用 `p.sessionId || activeSessionId` 作为 chat event 归属推断主路径。
  - 不得继续把 `sessionKey` 当作 Web UI 实例 owner 在 payload 里混发。
  - 不得在 delete / switch 失败时继续做 optimistic 跳转。
  - 不得让 startup restore 在 session 列表尚未可用时偷偷降到 `home/main`，覆盖用户上次会话意图。
- 应当：
  - UI 只消费服务端 `displayName` / `sessionKind` / capability flags；前端不再自行猜测名称或权限。
  - stale switch / missing event session owner / wrong action target 应当有结构化 debug 证据。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：以“Web session instance one-cut contract”做专项治理，一次收口 naming、RPC target、WS owner、switch transaction、transient state、sidebar action semantics。
- 优点：
  - 根因层统一，不会修一个点又从旁路重新串回来。
  - 可以一次性建立测试矩阵，覆盖关键用户路径。
  - 符合唯一事实来源原则。
- 风险/缺点：
  - 变更点横跨 `session.rs`、`chat.rs`、前端 chat/session/websocket/router，多文件联动。

#### 方案 2（备选）
- 核心思路：分别补丁修 display 名、clear 按钮、switch 竞态、WS payload。
- 优点：
  - 起手快。
- 风险/缺点：
  - 极易再次口径漂移，属于散点补丁，不符合本单治理目标。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（唯一 owner）：Web UI 当前会话只认 authoritative `sessionId`。
- 规则 2（服务端展示真源）：`displayName` 完全服务端 owner；前端禁止自行从 `sessionKey` 退化推导 scratch 名称。
- 规则 3（显式目标）：UI 发出的当前会话 mutating RPC 必须显式带目标 `_sessionId`。
- 规则 4（切换事务化）：每次 switch 必须有 request generation / token；回包若不再对应当前 intent，直接丢弃。
- 规则 5（事件 owner 一致）：WS `chat` 事件只允许一种实例 owner 字段口径；前端不再 fallback 到当前 active session。
- 规则 6（transient state 局部化）：stream / thinking / voice / lastToolOutput 至少按 `sessionId` 隔离，必要时按 `runId` 细化。
- 规则 7（动作语义清晰）：当前会话操作与集合操作在按钮文案、确认文案、RPC 方法上必须可一眼区分。

#### 接口与数据结构（Contracts）
- API/RPC：
  - `chat.send`：显式带 `_sessionId`
  - `chat.clear`：显式带 `_sessionId`
  - `sessions.switch`：返回值只用于对应那次切换事务，不得无条件覆盖当前 UI
- WS：
  - `chat.delta` / `chat.final` / `chat.error` / `chat.tool_*` / `chat.session_cleared` 统一携带 authoritative `sessionId`
  - 若需要调试展示 `sessionKey`，必须作为非 owner 的 debug-only 字段单独携带
- UI/展示：
  - sidebar / header / search 全部消费 `displayName`
  - scratch session 的 fallback 名称必须能区分不同实例
  - sidebar 集合操作文案必须显式为集合语义

#### 失败模式与降级（Failure modes & Degrade）
- stale switch response：直接丢弃，不更新 DOM、不更新 active session 派生状态。
- WS chat 事件缺失 authoritative `sessionId`：前端不得猜当前 active；必须直接拒收并打可观测日志。
- current-session mutating RPC 缺失 `_sessionId`：前端应视为 contract violation；后端也应硬拒绝 UI 主路径缺参。
- delete / clear / search / startup restore 任一路径失败：不得偷偷切到别的 session 或偷偷回落 `main` 覆盖用户意图。

#### 安全与隐私（Security/Privacy）
- 日志只记录 `sessionId`、`runId`、request generation、reason_code。
- 禁止把完整消息正文写进 stale-switch / wrong-owner 诊断日志。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] sidebar 中不同 scratch session 在未重命名时仍可被人类区分，不再统一显示为 `Chat`
- [ ] sidebar 集合按钮文案与行为一致，不再把“全部删除”伪装成“当前清空”
- [ ] `switchSession()` stale 回包不会覆盖当前新会话页面
- [ ] 流式回复中途切会话时，旧会话 delta/final/error 不会串到新会话页面
- [ ] `chat.send` / `chat.clear` 当前会话主路径都显式携带 `_sessionId`
- [ ] WS `chat` 关键事件统一携带 authoritative `sessionId`
- [ ] 首屏 `/` 恢复与 chat 页初始化不会在 bootstrap 前错误覆盖上次 session
- [ ] 搜索结果显示使用服务端 `displayName`，不再错误退化到 `sessionId`
- [ ] delete 失败不会继续 optimistic 跳转到 next session

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/gateway/src/session.rs`：scratch session fallback `displayName` 不再统一塌缩为 `"Chat"`
- [ ] `crates/gateway/src/chat.rs`：`final` / `error` / `delta` 等 chat WS payload owner 字段统一为 `sessionId`
- [ ] `crates/gateway/src/chat.rs`：UI 主路径缺 `_sessionId` 时直接拒绝（至少 `chat.send` / `chat.clear`）

### Integration
- [ ] `crates/gateway/src/methods.rs` / `crates/gateway/src/chat.rs`：`sessions.switch` + chat run 并发场景下，旧会话回包不覆盖新会话实例
- [ ] `crates/gateway/src/chat.rs`：clear / delete / queued / final / error 等事件都绑定正确 `sessionId`

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/session-ui-switch-stream.spec.js`：A 会话回复中切到 B，不串消息
- [ ] `crates/gateway/ui/e2e/specs/session-ui-new-and-clear.spec.js`：连续新建两个 scratch session，sidebar 可区分；sidebar 集合清理只在用户确认后删除全部
- [ ] `crates/gateway/ui/e2e/specs/session-ui-startup-restore.spec.js`：刷新 `/` 后正确恢复上次 session，而不是静默掉回 home
- [ ] `crates/gateway/ui/e2e/specs/session-ui-search-label.spec.js`：search dropdown 显示 `displayName`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：当前仓库对前端会话竞态类问题缺少现成 UI E2E 骨架与事件注入辅助。
- 手工验证步骤：
  1. 新建两个 scratch session，确认 sidebar 名称可区分
  2. 在 A 发流式消息，回复期间切到 B，确认 A 的后续内容不进入 B
  3. 点击 sidebar 集合操作按钮，确认文案和行为都是“全部删除”
  4. 刷新 `/`，确认恢复到上次 session，而不是静默回到 `Main`

## 发布与回滚（Rollout & Rollback）
- 发布策略：作为 Web UI / gateway contract one-cut 一次发布，不做双口径并存。
- 回滚策略：整组回滚到治理前 commit；不做新旧 contract 双跑。
- 上线观测：重点看 stale switch dropped、missing chat event session owner、ui action missing explicit session id、delete failed but navigation suppressed 等 reason_code。

## 实施拆分（Implementation Outline）
- Step 1: 冻结 Web session instance contract：`sessionId`、`displayName`、sidebar action semantics
- Step 2: 修复 chat WS payload owner contract，统一 `sessionId`
- Step 3: 把前端 current-session mutating RPC 改成显式 `_sessionId`
- Step 4: 把 `switchSession()` 改成 transaction / generation-based stale discard
- Step 5: 把 stream/thinking/voice/tool transient state 改为按 session instance 隔离
- Step 6: 修复 startup restore、search label、delete failure 等外围路径
- Step 7: 增补 gateway tests + UI E2E + 手工验收脚本
- 受影响文件：
  - `crates/gateway/src/session.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/assets/index.html`
  - `crates/gateway/src/assets/js/sessions.js`
  - `crates/gateway/src/assets/js/websocket.js`
  - `crates/gateway/src/assets/js/page-chat.js`
  - `crates/gateway/src/assets/js/components/session-header.js`
  - `crates/gateway/src/assets/js/components/session-list.js`
  - `crates/gateway/src/assets/js/session-search.js`
  - `crates/gateway/src/assets/js/app.js`
  - `crates/gateway/src/assets/js/stores/session-store.js`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
  - `issues/issue-session-page-cron-session-delete-entry-missing.md`
  - `issues/issue-onboarding-websocket-readiness-race.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- Q1: scratch session 的默认 fallback 展示名要不要直接带短实例片段（例如 `Chat · ab12`），还是改成完全服务端生成的递增命名？
- Q2: 前端 transient state 是只按 `sessionId` 隔离，还是直接进一步按 `runId` 隔离，避免同会话重入时再出第二类串线？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative owner 已统一到 `sessionId`
- [ ] 已补齐关键路径自动化测试（或记录缺口 + 手工验收）
- [ ] 文案 / 交互 / debug 口径已同步更新
- [ ] 不再存在 `sessionId` / `sessionKey` 混用 owner 的 UI 主路径
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

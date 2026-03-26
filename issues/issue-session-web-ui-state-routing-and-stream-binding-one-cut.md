# Issue: Session Web UI 一刀切专项治理（naming / routing / switch race / stream binding）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-26
- Checklist discipline: 每次增量更新除补“已实现 / 已覆盖测试”外，必须同步勾选正文里对应的 checklist；禁止出现文首已完成、正文 TODO 未更新的漂移
- Owners: gateway/ui（primary） + gateway（server contract）
- Components: gateway/ui/sessions/chat/websocket/router
- Affected providers/models: all（会话 UI 与流式绑定为上层通用路径）

**已实现（如有，必须逐条写日期）**
- 2026-03-26：已完成阶段 1 inventory，并冻结 one-cut 主口径：`sessionId` 为 Web UI 会话唯一 owner、`displayName` 为服务端唯一展示真源、浏览器当前会话 RPC 不再允许隐式 active-session fallback、`sessionStore` 为浏览器侧 session/transient 唯一 owner。
- 2026-03-26：已完成 one-cut 实施主路径：scratch fallback 展示名改为服务端 `Chat <session-id-suffix>`、`chat.send/chat.clear/chat.context/chat.full_context/chat.compact` 改为显式 `_sessionId` 且后端硬拒绝缺参、`chat.final/chat.error` WS owner 改为 `sessionId`、`switchSession()` 增加 stale generation 丢弃、前端 session/transient owner 从 `state.js` 收口到 `sessionStore`、sidebar 集合按钮文案改为 `Clear All`、delete 失败不再 optimistic 跳转。
- 2026-03-26：已同步补充 Web UI 关键回归脚本：现有 `sessions.spec.js` / `chat-input.spec.js` 适配 hard-cut `_sessionId` 合同，并在 `websocket.spec.js` 增补 inactive-session final 不得串入当前页面的 E2E 回归用例：`crates/gateway/ui/e2e/specs/websocket.spec.js:223`。
- 2026-03-26：已修复启动恢复误判（统一启动恢复 helper，禁止 pre-bootstrap 空缓存判存在）、删除路由层 legacy `:` <-> `/` 映射尾巴（encode/decode only）、删除 `displayName` 的 silent fallback tails（统一 `Invalid session` / `Loading…` 占位）、冻结并补齐关键结构化 warning（`stored_session_missing` / `active_session_switch_in_progress` / `missing_display_name`）、同步修复 UI E2E 与 JS 单测；本单由 `IN-PROGRESS` 收口为 `DONE`。
- 2026-03-26：修复两条用户可见回归：无 active session 时发送会丢输入（改为保留输入 + 拒绝 + structured warning）、switch placeholder 不更新/覆盖 hydrate 字段（禁止 placeholder 覆盖已 hydrate 的 session，且 `sessions.switch` 回包会更新 placeholder）：`crates/gateway/src/assets/js/page-chat.js:846`、`crates/gateway/src/assets/js/sessions.js:487`。

**已覆盖测试（如有）**
- main/home/create 的基础返回形状已覆盖：`crates/gateway/src/session.rs:1178`
- `sessions.switch` 服务端“resolve 失败不污染 active session”已覆盖：`crates/gateway/src/methods.rs:5741`
- scratch fallback 展示名唯一性已覆盖：`crates/gateway/src/session.rs:1263`
- chat final/error 广播 owner 字段统一为 `sessionId` 已覆盖：`crates/gateway/src/chat.rs:11980`
- 浏览器当前会话 RPC 缺 `_sessionId` 硬拒绝已覆盖：`crates/gateway/src/chat.rs:12023`
- gateway lib 全量单测已通过：`cargo test -p moltis-gateway --lib -q`
- Web UI JS 单测已通过：`node --test crates/gateway/src/assets/js/*.test.mjs`
- Web UI Playwright 关键回归已通过：`crates/gateway/ui/e2e/specs/sessions.spec.js:1`、`crates/gateway/ui/e2e/specs/websocket.spec.js:1`、`crates/gateway/ui/e2e/specs/chat-input.spec.js:1`
- Web UI Playwright 全量已通过（2026-03-26）：`cd crates/gateway/ui && npx playwright test`（135 passed，3 skipped）

**已知差异/待完成项**
- 当前已完成 one-cut 收口；后续若发现旧入口漏网，必须先并回本单再继续实现。
- Playwright 运行依赖：若环境缺少 `chrome-headless-shell` 依赖库（如 `libnspr4.so` / `libnss3.so`），需补齐系统依赖后再执行 E2E。

**三轮复核结论（2026-03-26）**
- Pass 1：确认 issue 文档与代码现实不一致；`Status: DONE`、启动恢复验收勾选、Close Checklist 勾选都过早。
- Pass 2：确认 Web UI 关键主路径上未再发现会话桶键作为 owner 的二次回流；剩余问题集中在启动恢复把“用户意图”误交给 pre-bootstrap 空缓存判真。
- Pass 3：确认仓内已有接近正确的参考：`crates/gateway/src/assets/js/onboarding-view.js:77` 至少没有拿 pre-bootstrap session 列表做存在性判断；最终 one-cut 仍应把 `app.js` / `page-chat.js` / `onboarding-view.js` 收口到同一 helper 与同一 owner 口径。
- Final confirm：补充发现 session 展示层仍有 `displayName` fallback tails；这不构成功能串页，但违反“服务端展示真源”与“不后向兼容”原则，必须纳入本单一起切干净。
- Pass 4：修正文档冻结项漂移：对齐浏览器侧与服务端日志口径（`chat_event_missing_session_id`、`ui_missing_explicit_session_id`）、收敛 UI E2E 测试指向实际存在的 spec 文件，并冻结占位文案与 `missing_display_name.surface` 枚举。

---

## 背景（Background）
- 场景：Web chat 页当前在 `new session`、列表展示、切换、clear、delete、流式回复、首屏恢复、搜索结果展示等链路上，出现了同类问题：同一个 UI 会话实例被多套状态和多种字段名同时解释。
- 约束：
  - 必须遵循第一性原则：会话实例只允许一个 authoritative owner。
  - 必须遵循唯一事实来源原则：UI、RPC、WS、流式状态不能各用一套 session 身份口径。
  - 必须遵循不后向兼容原则：不能继续同时容忍 `sessionId` / `sessionKey` 在 UI 实例归属上混用。
  - 必须优先治理关键路径：`new -> switch -> send -> stream -> clear/delete -> 启动恢复 -> search`。
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

- **当前会话 ID**（主称呼）：浏览器运行时“当前正在看的会话”的唯一真值。
  - Why：启动恢复、switch、send、stream binding 都必须围绕它收口。
  - Not：不是 localStorage 里的持久化副本；也不是 session 列表里任一 entry 的展示字段。
  - Source/Method：runtime owner
  - Aliases（仅记录，不在正文使用）：`sessionStore.activeSessionId`

- **持久化会话 ID**（主称呼）：浏览器为了刷新后恢复而保存的“上次当前会话 ID”。
  - Why：它只负责恢复，不负责判定会话是否真实存在。
  - Not：不是运行时当前会话真值；也不是服务端 authoritative existence proof。
  - Source/Method：persisted backing
  - Aliases（仅记录，不在正文使用）：`localStorage.moltis-sessionId`

- **会话展示名**（主称呼）：服务端返回给 UI 的人类可读名称，用于 sidebar / header / 搜索结果标签。
  - Why：它决定用户是否能区分不同会话实例。
  - Not：不是 session owner；也不是前端自行从 `session_key` 推导的临时别名。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`displayName`

- **会话桶键**（主称呼）：服务端用于标识 session 逻辑桶/类型的键（例如 `agent:default:main`），会出现在 session entry 或 tool context 中。
  - Why：它是服务端内部路由/归档/工具上下文的概念，但不是 Web UI 会话实例 owner。
  - Not：不是 Web UI 会话实例归属键；不是 sidebar/header/search 的展示名来源；不得用于 Web UI 的“当前会话”推断。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：`sessionKey` / `_sessionKey` / `session_key`

- **待补全会话项**（主称呼）：前端在 create / switch 事务进行中，为了维持列表与路由连续性而临时插入、尚未收到服务端完整 payload 的 session entry。
  - Why：它必须有冻结好的待补全占位语义，否则最容易重新把 `sessionId` 当名字显示出来。
  - Not：不是 authoritative session payload；不是合法展示名来源。
  - Source/Method：transient
  - Aliases（仅记录，不在正文使用）：pending client-only entry

- **非法会话项**（主称呼）：已经收到服务端 payload，但仍违反 contract（缺失 `displayName`）的 session entry。
  - Why：它必须有统一的错误展示与可观测性，而不是继续 silent fallback。
  - Not：不是待补全会话项；也不是正常可展示 session。
  - Source/Method：contract violation
  - Aliases（仅记录，不在正文使用）：hydrated invalid entry

- **非法会话占位文案**（主称呼）：非法会话项在 UI 上唯一允许显示的占位文本。
  - Why：它冻结了 contract violation 的唯一外显语义，避免再次退化到 `sessionId` / `label`。
  - Not：不是会话展示名；不是搜索结果项；不是临时 debug 文本。
  - Source/Method：effective
  - Value（冻结）：`Invalid session`
  - Aliases（仅记录，不在正文使用）：contract-violation placeholder

- **待补全占位文案**（主称呼）：待补全会话项在服务端 payload 返回前唯一允许显示的占位文本。
  - Why：它冻结了 create / switch 过渡态的唯一外显语义，避免原始 `sessionId` 被误当成会话展示名。
  - Not：不是会话展示名；不是非法会话项的错误文案。
  - Source/Method：effective
  - Value（冻结）：`Loading…`
  - Aliases（仅记录，不在正文使用）：loading placeholder

- **启动恢复助手**（主称呼）：`/` 根路由、chat 页无 URL 会话分支、onboarding 跳转共用的启动恢复决策入口。
  - Why：必须把“当前会话 ID / 持久化会话 ID / 存在性判定”收口在一个地方，避免三套实现再次漂移。
  - Not：不是 `switchSession()` 本身；也不是某个页面私有的小函数。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：startup session helper

- **会话切换事务**（主称呼）：一次从旧会话切到新会话的完整异步过程。
  - Why：它必须是可序列化、可丢弃 stale 回包的事务，而不是“先改本地 active，再赌 RPC 回来顺序正确”。
  - Not：不是单纯一次 `sessions.switch` RPC。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：switch flow / switch request

- **聊天事件归属键**（主称呼）：WS `chat` 事件里用于把 delta/final/error/tool 结果绑定到正确会话实例的字段。
  - Why：流式与终态消息不能跟当前会话猜测绑定。
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
- [x] 收敛 Web 会话 UI 的唯一 owner：所有当前会话相关操作统一以 authoritative `sessionId` 驱动。
- [x] 修复 sidebar / header / search / startup / switch / streaming / clear / delete 之间的会话归属错乱。
- [x] 明确区分“当前会话操作”和“全部会话集合操作”，消除误导性文案和错误动作绑定。
- [x] 修复当前流式回复在切换会话时串到别的会话上的关键路径问题。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须保证两个不同 session instance 不会因为 fallback display 名或流式状态混用而在 UI 层不可区分。
  - 必须保证 stale `switchSession()` 回包不会覆盖当前已切走的新会话页面。
  - 必须保证 WS `chat` 事件在前端无需猜测当前会话就能知道归属。
  - 必须保证浏览器侧 session/transient state 只由 `sessionStore` 持有；`state.js` 不得再并行持有 `activeSessionId` / session 列表 / stream 文本 / voice pending / tool output 等 session 语义真值。
  - 必须保证 session 可见名称只消费服务端 `displayName`；sidebar / header / search / store normalization 不得再从 `label` / `sessionId` 猜测名称。
  - 不得继续让浏览器当前会话 RPC 仅依赖“连接级 active session 状态”隐式定位目标。
  - 不得继续把会话桶键当作 Web UI 实例 owner 使用。
- 兼容性：本单按 one-cut 治理当前 Web UI 行为，不保留旧混用语义。
- 可观测性：
  - 需要补齐 stale switch 丢弃、缺失事件 owner、非法 UI action 目标、会话 RPC 缺少显式 `_sessionId` 等结构化日志 / debug 证据。
  - stale switch 回包丢弃的口径冻结为：`event="session.switch"`、`reason_code="stale_switch_response"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`，并补 `requested_session_id` / `active_session_id` / `switch_generation`。
  - chat 事件缺失会话归属键的口径冻结为：`event="chat.event"`、`reason_code="chat_event_missing_session_id"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`，并补 `chat_event_type` / `run_id` / `conn_id`。
  - 会话切换事务进行中（`switchInProgress=true`）收到 active session 的 chat 事件：必须丢弃（避免与 `sessions.switch` 历史回放交错渲染），并留下结构化日志：`event="chat.event"`、`reason_code="active_session_switch_in_progress"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`，并补 `chat_event_type` / `run_id` / `conn_id` / `switch_generation`。
  - 当前会话 RPC 缺失显式 `_sessionId` 的口径冻结为：`event="session.contract_violation"`、`reason_code="ui_missing_explicit_session_id"`、`decision="reject"`、`policy="web_ui_session_owner_v1"`，并补 `method` / `conn_id` / `remediation`。
  - 启动恢复的失效持久化会话 ID fallback 口径冻结为：`event="session.restore"`、`reason_code="stored_session_missing"`、`decision="fallback_home"`、`policy="web_ui_session_owner_v1"`，并补 `stored_session_id` / `active_session_id`。
  - session entry payload 缺失 `displayName` 的口径冻结为：`event="session.contract_violation"`、`reason_code="missing_display_name"`、`decision="warn"`、`policy="web_ui_session_owner_v1"`、`surface="session_store_normalize"`。
  - search hit payload 缺失 `displayName` 的口径冻结为：直接丢弃该 hit，并打结构化 warning：`event="session.contract_violation"`、`reason_code="missing_display_name"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`、`surface="search_hit"`；不得渲染任何占位项，也不得退化到 `sessionId`。
  - `missing_display_name.surface` 取值只允许：`session_store_normalize` / `search_hit`。
  - 结构化字段至少包含 `event`、`reason_code`、`decision`、`policy`。
  - 关联字段至少包含 `session_id`、`run_id`、`switch_generation`、`conn_id` 中当前链路可用的部分。
  - 正文只允许 `preview` / `len` / `hash` 一类有限文本诊断字段；不得打印完整消息正文。
  - 日志外观继续沿用 `2026-03-25T08:27:03.024959Z WARN ... key=value` 形态，只增补字段，不改整体日志风格。
- 安全与隐私：日志不得打印完整消息正文；只记录 session/routing 诊断字段。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 连续点两次 `new session`，sidebar 里两个新会话都显示成 `Chat`，用户无法区分实例。
2) sidebar 里的 `Clear` 按钮看起来像“清当前会话”，实际会删掉所有可删除会话。
3) 在会话 A 回复过程中切到会话 B，回复会继续画到 B 的页面里。
4) 快速切换会话时，旧 `sessions.switch` 回包会覆盖新会话的 DOM 和历史。
5) 搜索结果、启动恢复、删除后的跳转等边缘路径，和主会话 contract 也没有完全统一。
6) 浏览器代码同时维护 `sessionStore` 与 `state.js` 两套 session 相关状态，导致当前会话、streaming、未读、preview 与切换流程存在并行真源。

### 影响（Impact）
- 用户体验：会话实例不可辨认、按钮语义误导、流式消息串页，属于核心交互破坏。
- 可靠性：当前 UI 的当前会话、WS 事件、流式 DOM、服务端实例 owner 没有同一真源。
- 排障成本：同一个问题会在 naming、routing、switch、streaming、search、startup 多条链路反复出现。

### 复现步骤（Reproduction）
1. 打开 chat 页，连续点击两次 `new session`。
2. 观察 sidebar：会看到两个不同实例都显示为 `Chat`。
3. 在会话 A 发送一条会触发流式回复的消息；回复尚未完成时切到会话 B。
4. 观察期望 vs 实际：
   - 期望：A 的后续 delta/final 只留在 A；B 只显示 B 自己的内容。
   - 实际：A 的回复被渲染到 B 当前页面，或旧 switch 回包把 B 页面覆盖成 A 历史。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 说明：本节保留“原始问题基线证据 + 本轮复核新增 blocker 证据”；已修项以文首“已实现”与勾选状态为准，未收口项以未勾选验收项与本节新增 blocker 证据为准。
- 代码证据：
  - `crates/gateway/src/session.rs:136`（已修）：scratch session fallback `displayName` 由服务端生成 `Chat <session-id-suffix>`，不同实例可区分。
  - `crates/gateway/src/session.rs:336`：`sessions.create` 默认总是创建 `agent:default:chat-<opaque>` scratch session。
  - `crates/gateway/src/assets/index.html:210`（已修）：sidebar 集合操作按钮文案为 `Clear All`，并通过 `title` 明确其为集合操作。
  - `crates/gateway/src/assets/js/sessions.js:194`：上述按钮实际调用 `sessions.clear_all`，会删除全部可删除 agent session。
  - `crates/gateway/src/assets/js/sessions.js:71`（已修）：当前会话 clear 显式传 `_sessionId`。
  - `crates/gateway/src/assets/js/page-chat.js:891`（已修）：`chat.send` 显式传 `_sessionId`。
  - `crates/gateway/src/assets/js/sessions.js:510`：`switchSession()` 在 RPC 返回前先本地切 active session 并清空 DOM（切换事务的前置状态切换）。
  - `crates/gateway/src/assets/js/sessions.js:529`（已修）：`switchSession()` 回包若 stale，直接丢弃并输出结构化日志（`reason_code="stale_switch_response"`）。
  - `crates/gateway/src/assets/js/websocket.js:572`（已修）：WS chat 事件必须携带 `sessionId`；缺失时直接丢弃并输出结构化日志（`reason_code="chat_event_missing_session_id"`）：`crates/gateway/src/assets/js/websocket.js:575`。
  - `crates/gateway/src/assets/js/websocket.js:583`（已修）：active session 的 chat 事件在 `switchInProgress=true` 时会被丢弃，且必须输出结构化日志（`reason_code="active_session_switch_in_progress"`），禁止 silent drop。
  - `crates/gateway/src/assets/js/stores/session-store.js:13`、`crates/gateway/src/assets/js/stores/session-store.js:131`：`sessionStore` 已经持有 `sessionId`、`displayName`、`activeSessionId` 与 per-session transient state，本应成为浏览器侧唯一 owner。
  - `crates/gateway/src/assets/js/signals.js:6`（已修）：`activeSessionId` 等 session 语义信号已移动到 `stores/*.js`（避免 `state.js` 成为第二真源）。
  - `crates/gateway/src/chat.rs:190`（已修）：`ChatFinalBroadcast` 使用 `session_id`（序列化为 `sessionId`）。
  - `crates/gateway/src/chat.rs:223`（已修）：`ChatErrorBroadcast` 使用 `session_id`（序列化为 `sessionId`）。
  - `crates/gateway/src/assets/js/websocket.js:311`：流式渲染仍使用全局 `S.streamEl`（DOM ref）；必须确保其不会跨 session 污染。
  - `crates/gateway/src/assets/js/websocket.js:382`：`final` 会触发流式 DOM 的收尾与清理；必须确保不会污染非归属 session 的页面。
  - `crates/gateway/src/assets/js/startup-session.js:9`（已修）：启动恢复仅解析 URL 与 `localStorage.moltis-sessionId`，不再用 pre-bootstrap `sessionStore` 判存在；存在性由 `sessions.switch/home` 主路径判定。
  - `crates/gateway/src/assets/js/route-utils.js:6`（已修）：路由 sessionId 仅 encode/decode；不再存在 `:` <-> `/` 的 legacy URL 映射尾巴。
  - `crates/gateway/ui/e2e/helpers.js:95`（已修）：E2E helper 解析 `/chats/<id>` 时仅 decode；不再存在 legacy 映射尾巴。
  - `crates/gateway/src/assets/js/session-label.js:1`（已修）：UI 会话展示名一刀切为服务端 `displayName`；缺失则显示 `Invalid session`，待补全显示 `Loading…`，不再 fallback 到 `label/sessionId`。
  - `crates/gateway/src/assets/js/session-search-normalize.js:1`（已修）：非法搜索命中缺 `displayName` 时直接丢弃 + structured warning（`surface="search_hit"`），不再 fallback 到 `sessionId`。
  - `crates/gateway/src/session.rs:858`：服务端搜索返回的是 `displayName` 字段。
  - `crates/gateway/src/assets/js/components/session-header.js:98`（已修）：delete 失败不会 optimistic 跳转到 next session。
  - `crates/gateway/src/assets/js/sessions.js:415`、`crates/gateway/src/assets/js/page-chat.js:691`、`crates/gateway/src/assets/js/page-chat.js:805`（已修）：`chat.context` / `chat.full_context` / `chat.compact` 等 RPC 均显式传 `_sessionId`，并由服务端对 UI 主路径缺参硬拒绝。
- 当前测试覆盖：
  - 已有：`crates/gateway/src/session.rs:1178` 覆盖 home/create 返回形状；`crates/gateway/src/session.rs:1263` 覆盖 scratch fallback 展示名唯一性；`crates/gateway/src/chat.rs:11980` 覆盖 chat final/error owner 字段统一为 `sessionId`；`crates/gateway/src/chat.rs:12023` 覆盖浏览器主路径缺 `_sessionId` 时硬拒绝；`crates/gateway/src/methods.rs:5741` 覆盖服务端 `sessions.switch` 失败时不污染连接 active state。
  - 已补：Node JS 单测 `node --test crates/gateway/src/assets/js/*.test.mjs` + Playwright E2E（`crates/gateway/ui/e2e/specs/*.spec.js`）已覆盖关键主路径与关键失败面（详见文首“已覆盖测试”）。

## 根因分析（Root Cause）
- A. 服务端展示口径和 UI 实例口径没有分层：scratch session 的 fallback display 名被压扁成常量 `"Chat"`。
- B. sidebar 集合操作与当前会话操作没有明确边界，文案和 RPC 绑定错位。
- C. 前端 `switchSession()` 把“准备切换”和“切换成功”混成同一个本地状态更新，没有 transaction / generation guard。
- D. WS chat event contract 漂移：部分事件发 `sessionId`，部分事件发会话桶键，前端因此退回到当前会话猜测。
- E. 前端流式渲染状态是全局单例，不是按 session instance / run instance 绑定。
- F. 浏览器同时存在 `sessionStore` 与 `state.js` 两套 session 相关真源，任何单点修补都会再次从旁路串回。
- G. 搜索、首屏恢复、删除回调等旁路链路没有跟主 contract 一起收口，形成系统性技术债。
- H. 启动恢复把“用户上次会话意图”与“本地 session cache 当前是否已加载”混为一谈；在 bootstrap 前 `sessionStore` 为空时，合法持久化会话 ID 也会被误判为 missing。
- I. UI session surfaces 仍把 `displayName` 当作 optional 字段处理，保留 `label/sessionId` 猜测路径，导致“服务端展示真源”没有切干净。
- J. 前端在 switch/create 过程中会短暂创建未 hydrate 的待补全会话项；如果不显式定义待补全占位语义，开发者会自然回到用 `sessionId` 顶名字的旧路径。

## 期望行为（Desired Behavior / Spec）【冻结】
- 必须：
  - Web UI 当前会话的 authoritative owner 必须是 `sessionId`，且必须由服务端提供。
  - 浏览器侧唯一 session owner 必须是 `sessionStore`；session 相关状态不得再双写到 `state.js`。
  - 所有 UI mutating RPC 必须显式携带目标 `_sessionId`（`chat.*`）或 `sessionId`（`sessions.*`）；不得只靠连接 active state。
  - 所有 WS `chat` 事件必须携带前端可直接消费的 authoritative `sessionId`。
  - `switchSession()` 必须具备 stale response 丢弃能力；旧请求回包不得覆盖新会话页面。
  - 流式文本 / thinking / tool output / voice pending / history index / token bar 等 transient state 必须按 `sessionId` 隔离；`runId` 只作为事件关联字段，不作为会话 owner。
  - scratch session 的服务端 fallback 展示名必须可区分，不能把所有 `chat-*` 折叠成同一个 `"Chat"`；本单冻结为服务端生成 `Chat <session-id-suffix>` 形式。
  - 浏览器当前会话 RPC 本单冻结为：`chat.send`、`chat.clear`、`chat.context`、`chat.full_context`、`chat.compact` 必须显式携带 `_sessionId`。
  - 启动恢复必须拆分“当前会话 ID / 持久化会话 ID / 存在性判定”：当前会话 ID 只认 `sessionStore.activeSessionId`，持久化会话 ID 只认 `localStorage.moltis-sessionId`；session 是否真实存在只能由服务端 `sessions.switch` / `sessions.home` 主路径判定，pre-bootstrap session 列表不得参与存在性判断。
  - Web UI 路由里的 sessionId URL 编码必须 one-cut：URL 只允许对 `sessionId` 做 encode/decode；不得存在 `:` <-> `/` 的 legacy 映射（包括 `replace(/:/g, "/")` 或 `replace(/\\//g, ":")`）。
  - app 根路由、chat 页无 URL 会话分支、onboarding 跳转三处必须复用同一份启动恢复助手，不得继续各自直读 `localStorage` / `sessionStore` 形成三套实现。
  - sidebar / header / search / session-store normalization 必须只消费服务端 `displayName`；缺失 `displayName` 视为 contract violation，留 structured warning，不再 silent fallback 到 `label` / `sessionId`。
  - 对于非法会话项（已 hydrate，但缺失 `displayName`），UI 只允许显示统一非法会话占位文案 `Invalid session`；不得显示空白，也不得退化到 `sessionId` / `label`。
  - 对于待补全会话项（尚未 hydrate），UI 只允许显示统一待补全占位文案 `Loading…`；不得把 `sessionId` / `label` 伪装成会话名。
  - 对于非法搜索命中缺失 `displayName` 的结果，UI 只允许直接丢弃该 hit 并记录 warning；不得显示任何占位项，也不得退化到 `sessionId`。
  - sidebar 集合操作必须在文案和交互上明确表达为集合语义；本单冻结为：按钮文案 `Clear All`，且其行为是清理全部可删除会话（集合操作），不是清当前会话。
- 不得：
  - 不得继续使用 `p.sessionId || activeSessionId` 作为 chat event 归属推断主路径。
  - 不得继续把会话桶键当作 Web UI 实例 owner 在 payload 里混发。
  - 不得继续对 session 语义做 `sessionStore` <-> `state.js` dual-write。
  - 不得在 delete / switch 失败时继续做 optimistic 跳转。
  - 不得让启动恢复在 session 列表尚未可用时偷偷降到 `home/main`，覆盖用户上次会话意图。
- 补充冻结：
  - UI 只消费服务端 `displayName` / `sessionKind` / capability flags；前端不再自行猜测名称或权限。
  - stale switch / missing event session owner / wrong action target 必须有结构化 debug 证据。
  - 可观测性日志文本冻结为：
    - `session switch drop`：`event="session.switch"`、`reason_code="stale_switch_response"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`、`requested_session_id=...`、`active_session_id=...`、`switch_generation=...`
    - `chat event drop`：`event="chat.event"`、`reason_code="chat_event_missing_session_id"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`、`chat_event_type=...`、`run_id=...`、`conn_id=...`
    - `chat event drop (switch)`：`event="chat.event"`、`reason_code="active_session_switch_in_progress"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`、`chat_event_type=...`、`run_id=...`、`conn_id=...`、`switch_generation=...`
    - `chat rpc reject`：`event="session.contract_violation"`、`reason_code="ui_missing_explicit_session_id"`、`decision="reject"`、`policy="web_ui_session_owner_v1"`、`method=...`、`conn_id=...`、`remediation=...`
    - `session restore fallback`：`event="session.restore"`、`reason_code="stored_session_missing"`、`decision="fallback_home"`、`policy="web_ui_session_owner_v1"`、`stored_session_id=...`、`active_session_id=...`
    - `session contract violation (entry)`：`event="session.contract_violation"`、`reason_code="missing_display_name"`、`decision="warn"`、`policy="web_ui_session_owner_v1"`、`session_id=...`、`surface="session_store_normalize"`
    - `session contract violation (search)`：`event="session.contract_violation"`、`reason_code="missing_display_name"`、`decision="drop"`、`policy="web_ui_session_owner_v1"`、`session_id=...`、`surface="search_hit"`

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：以“Web session instance one-cut contract”做专项治理，一次收口 naming、RPC target、WS owner、switch transaction、transient state、sidebar action semantics。
- 优点：
  - 根因层统一，不会修一个点又从旁路重新串回来。
  - 能一次性建立测试矩阵，覆盖关键用户路径。
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
- 规则 2（服务端展示真源）：`displayName` 完全服务端 owner；前端禁止自行从会话桶键退化推导 scratch 名称。
- 规则 3（浏览器单一 owner）：浏览器侧 session/transient 真值只允许存在于 `sessionStore`；`state.js` 只保留非 session 的页面级共享引用。
- 规则 4（显式目标）：UI 发出的当前会话 RPC 必须显式带目标 `_sessionId`。
- 规则 5（切换事务化）：每次 switch 必须生成唯一 `switch_generation`；回包若不再对应当前 intent，直接丢弃。
- 规则 6（事件 owner 一致）：WS `chat` 事件只允许一种实例 owner 字段口径；前端不再 fallback 到当前会话。
- 规则 7（transient state 局部化）：stream / thinking / voice / lastToolOutput 至少按 `sessionId` 隔离，必要时按 `runId` 细化。
- 规则 8（动作语义清晰）：当前会话操作与集合操作在按钮文案、确认文案、RPC 方法上必须可一眼区分。

#### 接口与数据结构（Contracts）
- API/RPC：
  - `chat.send` / `chat.clear` / `chat.context` / `chat.full_context` / `chat.compact`：显式带 `_sessionId`
  - `sessions.switch`：返回值只用于对应那次切换事务，不得无条件覆盖当前 UI
  - 启动恢复：`/` 根路由、chat 页“无 URL sessionId”分支、onboarding 结束后跳转，共用同一个启动恢复助手（唯一入口）。
    - 优先级冻结：URL `sessionId` > `localStorage.moltis-sessionId` > 空（交给 `sessions.home`）。
    - 若 URL 无 `sessionId` 且 `localStorage.moltis-sessionId` 非空：必须直接路由到 `/chats/<storedSessionId>`；不得调用 `sessionStore.getById(storedSessionId)` 或 `sessionStore.defaultSessionId()` 做 pre-bootstrap 存在性判定。
    - URL 编码冻结：`/chats/<storedSessionId>` 必须只做 URL encode/decode；不得对 `storedSessionId` 做 `:` <-> `/` 映射。
    - session 是否存在的唯一真源冻结为：后续 `switchSession()` -> `sessions.switch` / `sessions.home`。
    - 若 `sessions.switch` 判定 stored session 不存在：必须记录 `event="session.restore"`、`reason_code="stored_session_missing"`、`decision="fallback_home"`、`policy="web_ui_session_owner_v1"`，并清理 `localStorage.moltis-sessionId`，然后沿 missing-session 主路径回落 home。
- WS：
  - `chat.delta` / `chat.final` / `chat.error` / `chat.tool_*` / `chat.session_cleared` 统一携带 authoritative `sessionId`
  - 若需要调试展示会话桶键，必须作为非 owner 的 debug-only 字段单独携带
- UI/展示：
  - sidebar / header / search / session-store normalization 全部消费 `displayName`
  - scratch session 的 fallback 名称必须能区分不同实例，固定为服务端生成 `Chat <session-id-suffix>`
  - 非法会话项（缺 `displayName`）统一显示非法会话占位文案，而不是空白或 `sessionId`
  - 待补全会话项统一显示待补全占位文案；直到服务端 entry 返回后，才切换为 authoritative `displayName`
  - 非法搜索命中（缺 `displayName`）统一直接丢弃，不渲染任何占位项
  - sidebar 集合操作文案必须显式为集合语义

#### 失败模式与降级（Failure modes & Degrade）
- stale switch response：直接丢弃，不更新 DOM、不更新当前会话派生状态。
- WS chat 事件缺失 authoritative `sessionId`：前端不得猜当前会话；必须直接拒收并打可观测日志。
- 会话切换事务进行中（`switchInProgress=true`）收到 active session 的 chat 事件：必须丢弃并打可观测日志（`reason_code="active_session_switch_in_progress"`），不得渲染到 DOM。
- 当前会话 RPC 缺失 `_sessionId`：前端必须视为 contract violation；后端必须硬拒绝 UI 主路径缺参，并输出 remediation。
- 持久化会话 ID 在服务端已不存在：必须沿同一 `switchSession()` missing-session 路径显式回落到 home session，并留下 structured warning（`event="session.restore"`、`reason_code="stored_session_missing"`、`decision="fallback_home"`、`policy="web_ui_session_owner_v1"`）；不得在 bootstrap 前靠空 `sessionStore` 偷偷短路到 home。
- session entry payload 缺失 `displayName`：前端不得继续退化到 `label` / `sessionId`；必须留下 structured warning（`event="session.contract_violation"`、`reason_code="missing_display_name"`、`decision="warn"`、`policy="web_ui_session_owner_v1"`、`surface="session_store_normalize"`）。
- 非法会话项（缺 `displayName`）：前端不得显示空白；必须显示统一非法会话占位文案，直到服务端修正 payload。
- 待补全会话项尚未 hydrate：前端不得记成真实名字；只允许待补全占位文案，待服务端 entry 返回后再替换。
- 非法搜索命中（缺 `displayName`）：前端不得显示任何占位项；必须直接丢弃该 hit。
- delete / clear / search / 启动恢复 任一路径失败：不得偷偷切到别的 session 或偷偷回落 `main` 覆盖用户意图。

#### 安全与隐私（Security/Privacy）
- 日志只记录 `sessionId`、`runId`、request generation、reason_code。
- 禁止把完整消息正文写进 stale-switch / wrong-owner 诊断日志。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] sidebar 中不同 scratch session 在未重命名时仍可被人类区分，不再统一显示为 `Chat`
- [x] sidebar 集合按钮文案与行为一致，不再把“全部删除”伪装成“当前清空”
- [x] `switchSession()` stale 回包不会覆盖当前新会话页面
- [x] 流式回复中途切会话时，旧会话 delta/final/error 不会串到新会话页面
- [x] `chat.send` / `chat.clear` 当前会话主路径都显式携带 `_sessionId`
- [x] `chat.context` / `chat.full_context` / `chat.compact` 当前会话主路径都显式携带 `_sessionId`
- [x] WS `chat` 关键事件统一携带 authoritative `sessionId`
- [x] 会话切换事务进行中（`switchInProgress=true`）active session 的 chat 事件会被丢弃，且必须输出结构化日志（`reason_code="active_session_switch_in_progress"`），禁止 silent drop
- [x] 首屏 `/` 恢复与 chat 页初始化不会在 bootstrap 前错误覆盖上次 session；有效持久化会话 ID 必须回到原会话；失效持久化会话 ID 必须沿 missing-session 主路径回落 home，清理 `localStorage.moltis-sessionId`，并输出结构化 warning：`event="session.restore" reason_code="stored_session_missing" decision="fallback_home" policy="web_ui_session_owner_v1"`
- [x] sidebar / header / search / session-store normalization 不再从 `label` / `sessionId` 猜会话展示名；统一只认服务端 `displayName`
- [x] 非法会话项（缺 `displayName`）显示统一非法会话占位文案 `Invalid session`，不显示空白，也不退化到 `sessionId` / `label`；且必须输出结构化 warning：`event="session.contract_violation" reason_code="missing_display_name" decision="warn" policy="web_ui_session_owner_v1" surface="session_store_normalize"`
- [x] create / switch 过程中，待补全会话项只显示待补全占位文案 `Loading…`，不再闪现 `sessionId` / `label`
- [x] 搜索结果显示只使用服务端 `displayName`，不再错误退化到 `sessionId`
- [x] 非法搜索命中（缺 `displayName`）会被直接丢弃，不渲染任何占位项，也不退化到 `sessionId`；且必须输出结构化 warning：`event="session.contract_violation" reason_code="missing_display_name" decision="drop" policy="web_ui_session_owner_v1" surface="search_hit"`
- [x] delete 失败不会继续 optimistic 跳转到 next session
- [x] 未建立 active session（例如首屏 `sessions.home` 仍在 resolve）时，用户发送不会丢失输入：必须保留输入并拒绝发送，同时输出结构化 warning：`event="session.contract_violation" reason_code="ui_missing_active_session" decision="reject" policy="web_ui_session_owner_v1"`

## 测试计划（Test Plan）【不可省略】
### Unit
- JS 单测约束（冻结）：仅使用 `node:test` 编写 `crates/gateway/src/assets/js/*.test.mjs`；运行命令冻结为 `node --test crates/gateway/src/assets/js/*.test.mjs`；不得引入 Jest/Vitest/Mocha 等新框架。
- [x] `crates/gateway/src/session.rs`：scratch session fallback `displayName` 固定为服务端可区分形式 `Chat <session-id-suffix>`
- [x] `crates/gateway/src/chat.rs`：`final` / `error` / `delta` 等 chat WS payload owner 字段统一为 `sessionId`
- [x] `crates/gateway/src/chat.rs`：浏览器主路径缺 `_sessionId` 时直接拒绝（至少 `chat.send` / `chat.clear` / `chat.context` / `chat.full_context` / `chat.compact`）
- [x] `crates/gateway/src/assets/js/session-label.test.mjs`：session label 不再从 `label` / `sessionId` 派生展示名；缺 `displayName` / 待补全时使用冻结占位文案
- [x] `crates/gateway/src/assets/js/session-search-normalize.test.mjs`：非法搜索命中缺 `displayName` 时直接丢弃 + structured warning（`surface="search_hit"`）

### Integration
- [x] `crates/gateway/src/methods.rs` / `crates/gateway/src/chat.rs`：`sessions.switch` + chat run 并发场景下，旧会话回包不覆盖新会话实例（覆盖：`crates/gateway/ui/e2e/specs/websocket.spec.js:1`）
- [x] `crates/gateway/src/chat.rs`：clear / delete / queued / final / error 等事件都绑定正确 `sessionId`（覆盖：`crates/gateway/src/chat.rs:11980` + `crates/gateway/ui/e2e/specs/websocket.spec.js:1`）

### UI E2E（Playwright，如适用）
- [x] `crates/gateway/ui/e2e/specs/sessions.spec.js`：有效 browser 持久化会话 ID 进入 `/` 后必须恢复到该 session，而不是掉回 home
- [x] `crates/gateway/ui/e2e/specs/sessions.spec.js`：失效 browser 持久化会话 ID 进入 `/` 后才允许回落到 home，并校验 structured warning / console warning（`reason_code="stored_session_missing"`）
- [x] `crates/gateway/ui/e2e/specs/sessions.spec.js`：模拟非法会话项缺 `displayName` 时，sidebar / header 显示 `Invalid session`，而不是空白 / `sessionId`
- [x] `crates/gateway/ui/e2e/specs/sessions.spec.js`：待补全会话项显示 `Loading…`，而不是 `sessionId`
- [x] `crates/gateway/src/assets/js/session-search-normalize.test.mjs`：search dropdown 标签只允许使用 `displayName`，非法搜索命中缺 `displayName` 时直接丢弃（Unit 覆盖，E2E 不再重复堆用例）
- [x] `crates/gateway/ui/e2e/specs/websocket.spec.js`：inactive-session final event does not render into the active chat page
- [x] `crates/gateway/ui/e2e/specs/websocket.spec.js`：switchInProgress=true 时 active session chat event 会被 drop，且必须可观测（`reason_code="active_session_switch_in_progress"`）
- [x] `crates/gateway/ui/e2e/specs/chat-input.spec.js`：`chat.full_context` / `chat.clear` 等 RPC 显式携带 `_sessionId`
- [x] `crates/gateway/ui/e2e/specs/chat-input.spec.js`：无 active session 时发送不丢输入，且可观测（`reason_code="ui_missing_active_session"`）
- [x] `crates/gateway/ui/e2e/specs/sessions.spec.js`：现有 sessions E2E 同步覆盖 `Clear All` 文案/行为一致、scratch fallback 名可区分、stale localStorage session id 恢复正确

### 自动化缺口（如有，必须写手工验收）
- 无自动化缺口：本单关键路径已由 gateway 单测 + Node JS 单测 + Playwright E2E 覆盖。

## 发布与回滚（Rollout & Rollback）
- 发布策略：作为 Web UI / gateway contract one-cut 一次发布，不做双口径并存。
- 回滚策略：整组回滚到治理前 commit；不做新旧 contract 双跑。
- 上线观测：重点看 `stale_switch_response`、`chat_event_missing_session_id`、`ui_missing_explicit_session_id`、`stored_session_missing`、`missing_display_name`，以及 delete failed but navigation suppressed 相关日志。

## 实施拆分（Implementation Outline）
- Step 1: 冻结 Web session instance contract：`sessionId`、`displayName`、sidebar action semantics
- Step 2: 删除浏览器侧 `sessionStore` <-> `state.js` session 语义双写，把 owner 收口到 `sessionStore`
- Step 3: 修复 chat WS payload owner contract，统一 `sessionId`
- Step 4: 把前端当前会话 RPC 改成显式 `_sessionId`，并让服务端对缺参主路径硬失败
- Step 5: 把 `switchSession()` 改成 transaction / generation-based stale discard
- Step 6: 把 stream/thinking/voice/tool transient state 改为按 session instance 隔离
- Step 7: 修复启动恢复、搜索结果展示名、delete failure、Clear All 文案等外围路径
- Step 7.1: 修复启动恢复：抽出单一启动恢复助手，供 `app.js` / `page-chat.js` / `onboarding-view.js` 共用；只读当前会话 ID（其持久化 backing 为持久化会话 ID），不再用 pre-bootstrap `sessionStore` 列表判存在；同时删除路由层 `:` <-> `/` 的 legacy URL 映射尾巴（只允许 encode/decode），并同步更新 UI E2E helper 的 URL 解析口径
- Step 7.2: 把 `stored_session_missing` fallback 收口到既有 `switchSession()` missing-session 分支，并补结构化 warning（至少 `event`、`reason_code`、`decision`、`policy`、`stored_session_id`）
- Step 7.3: 删掉 session 展示层 `displayName || label || sessionId` fallback，sidebar / header / search / store normalization 全部硬切到服务端 `displayName`，缺字段只留 warning 不做 silent degrade
- Step 7.4: 为待补全会话项增加统一待补全占位文案；禁止任何表面名称从 `sessionId` / `label` 派生
- Step 7.5: 为非法搜索命中冻结“直接丢弃 + warning”语义；禁止渲染任何占位项或 `sessionId`
- Step 8: 增补 gateway tests + UI E2E + 手工验收脚本
- 受影响文件：
  - `crates/gateway/src/session.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/assets/index.html`
  - `crates/gateway/src/assets/js/router.js`
  - `crates/gateway/src/assets/js/sessions.js`
  - `crates/gateway/src/assets/js/websocket.js`
  - `crates/gateway/src/assets/js/page-chat.js`
  - `crates/gateway/src/assets/js/components/session-header.js`
  - `crates/gateway/src/assets/js/components/session-list.js`
  - `crates/gateway/src/assets/js/session-search.js`
  - `crates/gateway/src/assets/js/app.js`
  - `crates/gateway/src/assets/js/onboarding-view.js`
  - `crates/gateway/src/assets/js/stores/session-store.js`
  - `crates/gateway/ui/e2e/helpers.js`

## 交叉引用（Cross References）
- 本单是主 issue（唯一准绳）；下列 issues/docs 仅作背景/相关，不得与本单并列指导实现。
- Related issues/docs：
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
  - `issues/issue-session-page-cron-session-delete-entry-missing.md`
  - `issues/issue-onboarding-websocket-readiness-race.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 冻结决策（Frozen Decisions）
- D1：scratch session fallback 展示名冻结为服务端生成的 `Chat <session-id-suffix>`；不做前端推导，不做递增命名器。
- D2：本单 transient state owner 冻结为 `sessionId`；`runId` 仅用于事件关联与日志，不作为 Web UI 会话 owner。
- D3：本单必须删除浏览器侧 `sessionStore` <-> `state.js` 的 session 语义双写；命中旧 fallback 形状按 contract violation 处理，不保留兼容。
- D4：当前会话 ID 的唯一运行时 owner 冻结为 `sessionStore.activeSessionId`；持久化会话 ID 的 backing store 冻结为 `localStorage.moltis-sessionId`。启动恢复的存在性验证唯一真源冻结为服务端 `sessions.switch` / `sessions.home`，不再允许 pre-bootstrap session 列表参与判定。
- D5：会话展示名唯一真源冻结为服务端 `displayName`；UI 不再从 `label` / `sessionId` 推导替补名称。
- D5.1：非法会话项若缺 `displayName`，显示 `Invalid session`；不显示空白，不退化到 `sessionId` / `label`。
- D6：待补全会话项不是 authoritative session name 来源；其 UI 文案冻结为 `Loading…`，而不是任何基于 `sessionId` / `label` 的本地猜名。
- D7：非法搜索命中若缺 `displayName`，直接丢弃，不显示任何占位项，不退化到 `sessionId`。
- D8：可观测性口径冻结为 `web_ui_session_owner_v1`（policy），并冻结关键 `reason_code`：`stale_switch_response` / `chat_event_missing_session_id` / `active_session_switch_in_progress` / `ui_missing_explicit_session_id` / `stored_session_missing` / `missing_display_name`；其中 `missing_display_name.surface` 取值只允许：`session_store_normalize` / `search_hit`。
- D8.1：补充冻结 `reason_code="ui_missing_active_session"`：当 UI 允许用户触发发送但 `activeSession` 尚未建立时，必须拒绝且不得清空输入。
- D9：Web UI 路由中的 sessionId URL 口径冻结为 encode/decode only；不得保留 `:` <-> `/` 的 legacy 映射，`router.js` / `onboarding-view.js` / `app.js` / `page-chat.js` / `crates/gateway/ui/e2e/helpers.js` 必须使用同一口径。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative owner 已统一到 `sessionId`
- [x] 已补齐关键路径自动化测试（或记录缺口 + 手工验收）
- [x] 文案 / 交互 / debug 口径已同步更新
- [x] 不再存在 `sessionId` / `sessionKey` 混用 owner 的 UI 主路径
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

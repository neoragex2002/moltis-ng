# SUPERSEDED BY `docs/plans/2026-03-26-cron-heartbeat-model-design.md`
#
# 2026-03-27 决策更新：
# - `cron` 目标模型已 hard-cut 收敛为“无会话上下文执行 + 结果投递”。
# - 旧的 `cron execution session 暴露到 generic Session UI` 讨论建立在已废弃的旧模型上。
# - 后续实施以 `issues/issue-cron-system-governance-one-cut.md` 为实施主单，以设计稿为当前语义准绳。

# Issue: cron 执行会话错误泄露到 Session UI，导致删除语义错位（cron / sessions / ui）

## 实施现状（Status）【增量更新主入口】
- Status: SUPERSEDED（不再推进；以新准绳与新主单为准）
- Priority: P1
- Updated: 2026-03-27
- Owners: TBD
- Components: gateway / ui / sessions / cron
- Affected providers/models: N/A

**已实现（如有，写日期）**
- Session 页普通会话已有删除 RPC 与 UI 入口：`crates/gateway/src/assets/js/components/session-header.js:83`
- Cron Jobs 页面已有 job 级删除入口，直接调用 `cron.remove`：`crates/gateway/src/assets/js/page-crons.js:477`
- 网关已注册 `sessions.delete` 与 `cron.remove` 两套删除 RPC：`crates/gateway/src/methods.rs:1520`、`crates/gateway/src/methods.rs:1855`

**已覆盖测试（如有）**
- `main` 与普通会话的 header action 可见性已有 E2E：`crates/gateway/ui/e2e/specs/sessions.spec.js:103`

**已知差异/后续优化（非阻塞）**
- N/A：本单已 superseded。旧证据保留仅用于追溯，不再作为实施依据。

---

## 背景（Background）
- 场景：用户在 Web UI 里能看到 `cron:*` 会话，并能进入 `/chats/cron/...`，但 header 没有 Delete；表面像“少了一个按钮”，本质上是 cron execution artifact 被错误暴露到了 generic Session UI。
- 约束：
  - `cron` 已经有自己的 CRUD 入口 `/crons` + `cron.*` RPC；generic Session UI 不应再长出第二套 cron 生命周期管理。
  - `enabled=false` 已经承担“停用 cron job”语义，删除不应与停用混淆。
  - 当前 issue 必须优先修正数据流与 ownership，而不是给错误 owner 补一个 Delete 按钮。
  - 本单按 hard switch 处理：不做旧 cron execution session 形态的向后兼容，不做自动迁移。
- Out of scope：
  - 不顺手重构 cron 的 trigger / delivery 语义
  - 不新增 GraphQL 或新的 REST delete 路径
  - 不在本单补 `/crons -> transcript` 的专用查看器

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **cron 执行会话**（主称呼）：`sessionId` 以 `cron:` 开头、承载 cron agent-turn 执行上下文与 transcript 的内部会话对象。
  - Why：这是当前错误泄露到 generic Session UI 的对象。
  - Not：不等于 cron job 规格本身；不等于 `/crons` 页面应直接管理的用户对象；不适用于 `systemEvent -> main` 路径。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：cron transcript / cron execution session

- **cron job**（主称呼）：持久化在 cron store 中、由调度器管理的任务规格对象。
  - Why：这是 cron 域唯一应承担 CRUD / enable / disable / delete 生命周期管理的实体。
  - Not：不等于某次执行产生的聊天历史；不等于 session metadata entry。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：scheduled job / cron config

- **generic Session UI**（主称呼）：`/chats` 路由、session sidebar、session search、chat header 这一整套普通聊天会话界面。
  - Why：这是当前把 cron execution artifact 暴露出来的错误 owner。
  - Not：不等于 `/crons` 页面；不等于 cron domain 的调度与存储层。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：chat session surface / chats UI

- **job 硬删**（主称呼）：从 cron store 中移除 job，并同步移除其执行上下文。
  - Why：这是 cron 域唯一正确的删除语义。
  - Not：不等于 `enabled=false`；不等于仅清空某次聊天历史。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：cron remove / delete reminder

- **job 停用**（主称呼）：保留 job 规格与历史，仅把 `enabled` 设为 `false`。
  - Why：现有模型已具备该语义，适合“先停用，不删除”。
  - Not：不等于删除；不等于归档。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：disable / pause

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] generic Session UI 不再暴露 cron 执行会话为普通聊天对象。
- [ ] cron 场景的删除入口只保留在 cron domain（`/crons` + `cron.remove`），不再让 `/chats` 承担 cron 删除 owner。
- [ ] 未来的 cron agent-turn 执行会话必须能被 cron job 单值拥有和清理。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把 cron 生命周期 owner 收敛到 cron domain，generic Session UI 只负责普通聊天会话。
  - 必须避免用户在 `/chats` 页面误以为自己能管理 cron job。
  - generic Session UI 对 cron 的判定规则必须收敛到单一来源，不保留多处冗余特判。
  - 必须优先复用现有 cron / session 服务边界，不新增第三套删除抽象。
  - 不得把“删除”与“停用”混为一谈。
  - 不得引入 archive/soft-delete 新语义作为本单首选方案。
  - 不得为了兼容旧 `cron:<name>` / `cron:<uuid>` 再加 alias / fallback / legacy guard。
- 兼容性：本单采用 hard switch；现有普通 session 删除与 cron 列表删除能力保持不变，但旧 cron execution session 形态不再属于受支持 UI 合同。
  - 历史上已落盘的 `cron:<name>` / `cron:<uuid>` artifact 不做自动迁移；若未来需要物理清理，另开 maintenance issue。
- 可观测性：
  - job 删除后，必须能从 cron 通知事件或日志判断哪个 job 被删、哪个 execution session 被清理。
  - generic Session UI 拒绝 `cron:*` 时，必须是显式 redirect / error，不得 silent degrade。
  - 对 `/chats/cron:*` redirect、cron artifact 清理失败等策略性分支，必须补结构化日志并带 `reason_code`。
- 安全与隐私：删除链路日志不得打印完整对话正文或敏感 token。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 用户可以在 session sidebar、session search 或直接 `/chats/cron:...` 进入 cron 执行会话。
2. 进入后 header 没有 Delete，表现为“像普通 session，但又不是完整 session owner”。
3. `/crons` 页面已经有 cron job 删除入口，说明当前问题不是“没能力”，而是同一个 cron artifact 被两个 UI surface 以不一致方式暴露。

### 影响（Impact）
- 用户体验：
  - 用户在 generic Session UI 里能看到 cron transcript，却拿不到完整生命周期动作，行为不一致。
  - 用户很难判断应该去 `/chats` 还是 `/crons` 管理 cron。
- 可靠性：
  - 如果直接给 `SessionHeader` 补 Delete，极易变成“删 transcript 但 job 继续跑”的假闭环。
- 排障成本：
  - 表面像“少个按钮”，实际是 owner 分裂、路由泄露和 execution id 不稳定三件事叠在一起。

### 复现步骤（Reproduction）
1. 让系统存在一个可见的 `cron:*` 会话。
2. 打开 `/chats/<cron-session-id>`。
3. 观察 header action。
4. 期望 vs 实际：
   - 期望：cron 只在自己的 owner surface 里被管理，生命周期动作一致。
   - 实际：`cron:*` 会话被 generic Session UI 暴露，但 generic Session UI 又故意不提供完整生命周期动作。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/assets/js/components/session-list.js:51`：sidebar 会把 `cron:*` 渲染成普通可点击 session item，只是换图标
  - `crates/gateway/src/assets/js/components/session-header.js:36`：`isCron = currentKey.startsWith("cron:")`
  - `crates/gateway/src/assets/js/components/session-header.js:154`：Delete 按钮条件是 `!(isMain || isCron)`，前端显式把 cron 会话排除在删除入口之外
  - `crates/gateway/src/assets/js/sessions.js:193`：Clear All 只统计普通可删 session，明确跳过 `cron:*`
  - `crates/gateway/src/assets/js/app.js:43`：根路由默认直接读取 `localStorage.moltis-sessionId` 并跳去 `/chats/<sessionId>`，没有排除 `cron:*`
  - `crates/gateway/src/assets/js/page-chat.js:1089`：chat route 对 URL 传入的 `sessionId` 直接 `switchSession(sessionId)`，没有把 `cron:*` 视作 unsupported owner
  - `crates/gateway/src/assets/js/session-search.js:26`：generic session search 直接调用 `sessions.search` 并可导航到任意命中的 `sessionId`
  - `crates/gateway/src/assets/js/page-crons.js:477`：Cron Jobs 表格中的 Delete 调用 `sendRpc("cron.remove", { id: job.id })`
  - `crates/gateway/src/methods.rs:1520`：网关已提供 `sessions.delete`
  - `crates/gateway/src/methods.rs:1855`：网关已提供 `cron.remove`
  - `crates/gateway/src/session.rs:440`：`LiveSessionService::delete` 实际做的是普通 session 硬删；若被拿来删 cron，只会删 transcript，不会删 job
  - `crates/gateway/src/cron.rs:71`：`LiveCronService::remove` 只删除 cron job，本身不处理 execution session 清理
  - `crates/cron/src/service.rs:104`：`AgentTurnRequest` 当前不携带 `job_id`
  - `crates/gateway/src/server.rs:1412`：gateway 生成 cron execution session id 时仍依赖 `session_target` 分支，`Named(name)` 走 `cron:{name}`，`Isolated` 走随机 `cron:{uuid}`
  - `crates/gateway/src/server.rs:1483`：cron job 删除后目前只广播 `cron.job.removed`，没有级联 session 清理
  - `crates/cron/src/service.rs:580`：`delete_after_run` 会在 cron core 内部直接调用 `self.remove(&job.id)`，说明级联清理不能只挂在外部 RPC handler 上
- 文档/设计证据：
  - `docs/src/session-branching.md:15`：文档已承认 generic Session UI 对 cron session 有专门特判，说明它不是普通 chat session
  - `issues/discussions/cron-trigger-execution-delivery-model.md:29`：讨论稿明确 execution 应统一为 `cron:<job_id>`
  - `issues/discussions/cron-trigger-execution-delivery-model.md:340`：若保留 `CronSession`，UI 也应由 cron 详情页去 `view_session`，而不是 generic `/chats` surface
  - `issues/discussions/cron-trigger-execution-delivery-model.md:778`：讨论稿冻结规则 10：删除 job 时，job 与 `cron:<job_id>` session 一并删除
- 当前测试覆盖：
  - 已有：`crates/gateway/ui/e2e/specs/sessions.spec.js:103` 覆盖了 main/普通 session 的 header action 可见性
  - 缺口：
    - 没有 generic Session UI 过滤/拒绝 cron session 的自动化测试
    - 没有根路由 / 直接 cron chat route redirect 的自动化测试
    - 没有 `cron.remove` 级联删除 deterministic execution session 的自动化测试

## 根因分析（Root Cause）
- A. generic Session UI 暴露了不属于自己 owner 的 cron execution artifact
  - sidebar / search / route 都把 `cron:*` 当成普通 session surface；
  - 但 header / clear-all 又在行为上承认它不是普通 session。
- B. cron 删除 owner 已经存在于 cron domain，但 generic Session UI 仍在旁路暴露同一个实体
  - `cron.remove` 删除的是 job；
  - `sessions.delete` 删除的是普通 transcript；
  - 一旦在 `/chats` 给 cron 补 Delete，就会制造 owner 冲突。
- C. execution session id 当前不稳定，导致 cron domain 也拿不到单值可清理的 execution artifact
  - `Named(name)` 使用 `cron:{name}`；
  - `Isolated` 使用随机 `cron:{uuid}`；
  - `AgentTurnRequest` 里甚至没有 `job_id`。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - cron job 必须是 cron 生命周期的唯一 owner；generic Session UI 不再直接管理 cron execution artifact。
  - generic Session UI 不得继续列出、搜索、默认跳转到 `cron:*` 会话。
  - agent-turn 型 cron execution 对未来运行必须统一为 deterministic `cron:<job_id>`。
  - 删除 cron job 时，必须同时清理对应 deterministic execution session artifact。
  - generic Session UI 在 hard switch 后不得继续保留死掉的 cron 专属分支。
- 不得：
  - 不得在 `SessionHeader` 上给 cron 补一个伪 Delete，然后把错 owner 固化下来。
  - 不得把“删除 transcript”伪装成“删除 cron job”。
  - 不得引入 archive/soft-delete 新语义或 legacy alias/fallback 来绕开当前闭环问题。
- 应当：
  - 应当让 `/crons` 成为 cron 的唯一用户管理面。
  - 应当让 direct `/chats/cron:*` 访问显式重定向到 `/crons`，而不是继续进入 generic chat surface。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 修正 ownership，而不是给错误 owner 补按钮：
    1. generic Session UI 停止暴露 cron execution artifact；
    2. cron domain 保留唯一删除入口 `cron.remove`；
    3. 对未来 agent-turn 执行统一使用 `cron:<job_id>`；
    4. job 删除时在 gateway orchestration 层级联清理 deterministic execution session。
- 优点：
  - owner 单一，generic chat 与 cron management 不再重叠。
  - 删除语义与 execution artifact 生命周期都能在 cron domain 内闭环。
  - 与已有讨论稿里“`cron:<job_id>` + job delete 级联删 session”一致。
- 风险/缺点：
  - 需要 hard switch 掉当前不稳定的 cron execution session id 生成方式。
  - 需要前后端同时收紧 generic Session UI 的入口。

#### 方案 2（备选，不推荐）
- 核心思路：
  - 仅在 Session header 放开 cron 会话的 `sessions.delete`，只删 transcript，不动 cron job。
- 优点：
  - 改动最小，纯 UI + 现有 session delete 即可闭环。
- 风险/缺点：
  - 用户会以为“删掉了这个 cron”，但调度任务还在跑。
  - 会把错误 owner 固化进 generic Session UI。
  - 继续保留 execution artifact 泄露、root route 泄露与 search 泄露。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：cron 的用户管理 owner 只保留在 cron domain；generic Session UI 不再承担 cron artifact 的展示与生命周期动作。
- 规则 2：`sessions.list`、`sessions.search`、`sessions.resolve` 不再把 `cron:*` 当成 generic session 合同的一部分。
- 规则 3：根路由和 direct `/chats/cron:*` 访问必须显式跳回 `/crons`，避免 generic chat surface 继续消费 cron artifact。
- 规则 4：future agent-turn cron execution 一律使用 `cron:<job_id>`；`job_id` 必须从 cron core authoritative 地传到 gateway 执行层。
- 规则 5：`cron.remove` 是 cron 场景唯一删除入口；删除成功后，gateway 必须在 `server.rs:on_cron_notify` 的 `cron.job.removed` 收口点级联清理 `cron:<job_id>` 的 session artifact。
- 规则 6：本单不做 legacy execution session 迁移；旧 `cron:<name>` / `cron:<uuid>` 形态停止受支持，不再暴露给 generic UI。
- 规则 7：job 停用与 job 删除继续分层：停用用 `enabled=false`，删除用 `cron.remove`。
- 规则 8：hard switch 完成后，generic Session UI 里遗留的 `isCron` / cron icon / clear-all skip 等分支必须删除，不保留防御式重复逻辑。

#### 接口与数据结构（Contracts）
- API/RPC：
  - 继续以 `cron.remove` 作为 cron 删除主入口。
  - `sessions.delete` 保持普通 session 硬删，不承接 cron job 删除语义。
  - `sessions.list` / `sessions.search` / `sessions.resolve` 对 `cron:*` 改为 unsupported。
- 执行 contract：
  - `AgentTurnRequest` 增加 `job_id`（或等价 authoritative 标识），禁止 gateway 继续从 `session_target` 猜 execution session id。
  - agent-turn execution session id 统一为 `cron:<job_id>`。
- 删除收口点：
  - cron execution session 的级联清理统一挂在 `server.rs:on_cron_notify` 对 `cron.job.removed` 的处理上。
  - 不允许在 `LiveCronService::remove`、`SessionHeader`、`page-crons.js` 各自再写半套清理逻辑。
- 存储/字段兼容：
  - 现有 session `archived` 字段不作为本单方案。
  - 现有 cron store 继续保留硬删实现，停用仍由 `enabled` 表示。
  - 不做旧 execution session 形态自动迁移，不做 alias 读取。
- UI 展示（如适用）：
  - generic Session UI 不再展示 cron 会话；因此也不再讨论 Session header 上的 cron Delete。
  - `/crons` 页面 Delete 行为保持不变，继续作为唯一删除入口。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若用户通过旧 localStorage / 旧链接命中 `/chats/cron:*`，前端必须直接跳回 `/crons`，不得继续以 generic session 打开。
  - 若 `cron.remove` 成功但 deterministic session 清理失败，必须留有可定位日志与 remediation。
- 结构化日志：
  - `/chats/cron:*` redirect 至少记录 `event`、`reason_code`、`decision`、`policy`。
  - cron session cleanup failed 至少记录 `event`、`reason_code`、`decision`、`policy`、`job_id`、`session_id`、`remediation`。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - job 删除成功后，必须删除其 deterministic cron execution session 历史与 metadata。
  - 停用路径必须保留 job 与历史，不得误删。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 日志只记录 job id / session id / decision，不打印完整对话内容。
- 禁止打印字段清单：
  - 完整 transcript 正文
  - token / channel 凭据

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] generic Session UI 的 sidebar、search、默认落地页不再暴露 `cron:*`。
- [ ] 直接访问 `/chats/cron:*` 不再进入 generic chat surface，而是显式回到 `/crons`。
- [ ] `/crons` 继续是 cron 唯一删除入口；不在 `SessionHeader` 上新增 cron Delete。
- [ ] future agent-turn cron execution 统一使用 `cron:<job_id>`。
- [ ] 删除 cron job 后，对应 deterministic execution session transcript 被同步清理。
- [ ] `/chats/cron:*` redirect 与 cron cleanup failure 具备带 `reason_code` 的结构化日志。
- [ ] `enabled=false` 的停用语义保持不变，不被误替换为删除。
- [ ] 普通 session 的 `sessions.delete` / `sessions.list` / `sessions.search` 行为不回归。
- [ ] generic Session UI 内原有 cron 专属死分支被删除，不残留第二份规则实现。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/gateway/src/session.rs`：覆盖 `sessions.list/search/resolve` 对 `cron:*` 的排除或拒绝。
- [ ] `crates/gateway/src/server.rs`：覆盖 `job_id -> cron:<job_id>` 的 deterministic execution session 生成与 cron route redirect 边界。
- [ ] `crates/gateway/src/server.rs`：覆盖 `cron.job.removed` 后的 deterministic session 清理，包括 `delete_after_run` 触发的内部 remove 路径。
- [ ] `crates/gateway/src/server.rs`：覆盖 `/chats/cron:*` redirect 与 cleanup failure 的 `reason_code` 可观测性。

### Integration
- [ ] 创建 agent-turn cron job -> 执行产生 `cron:<job_id>` -> 删除 job -> 断言 job 与 deterministic session 都被移除。

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/sessions.spec.js`：补 cron session 不出现在 sidebar/search/root landing、direct `/chats/cron:*` redirect 的回归。
- [ ] `crates/gateway/ui/e2e/specs/cron.spec.js`：补 `/crons` 删除后 deterministic transcript 同步清理的回归。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若当前 E2E harness 难以稳定构造 agent-turn 型 cron job，需先补最小测试桩。
- 手工验证步骤：
  1. 构造一个 agent-turn 型 cron job，并确认执行后产生 `cron:<job_id>` transcript。
  2. 打开 `/`、session search、direct `/chats/cron:<job_id>`，确认 generic Session UI 不再暴露它。
  3. 在 `/crons` 删除该 job，确认 transcript 与 job 一并消失。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认 hard switch，不增加兼容开关。
- 回滚策略：回滚 generic Session UI 的 cron 过滤/redirect、deterministic execution session 命名与 cron 删除级联；保留现有 `/crons` 删除能力。
- 上线观测：
  - `cron.job.removed`
  - cron session cleanup success/failure 日志
  - `/chats/cron:*` redirect 命中情况
  - generic Session UI 是否仍出现 `cron:*`

## 实施拆分（Implementation Outline）
- Step 1: 从 generic Session UI 收紧 cron 泄露入口（list/search/root/direct route）。
- Step 1a: 删除 generic Session UI 里围绕 `cron:*` 的死分支，避免规则双写。
- Step 2: 给 agent-turn cron execution 引入 `job_id` authoritative 透传，并收敛到 `cron:<job_id>`。
- Step 3: 在 cron 删除主路径上级联清理 deterministic execution session。
- 受影响文件：
  - `crates/gateway/src/assets/js/app.js`
  - `crates/gateway/src/assets/js/page-chat.js`
  - `crates/gateway/src/assets/js/components/session-header.js`
  - `crates/gateway/src/assets/js/components/session-list.js`
  - `crates/gateway/src/assets/js/sessions.js`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/session.rs`
  - `crates/cron/src/service.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-cron-system-governance-one-cut.md`
  - `issues/discussions/cron-trigger-execution-delivery-model.md`
  - `docs/src/session-branching.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- 本单范围内无未决问题。
- 未来若要在 `/crons` 提供 transcript 查看入口，单开 issue；不回到 generic Session UI 兜底。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

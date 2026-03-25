# Issue: V3 one-cut 严格模式：移除 legacy fallback/alias，遇到非 V3 口径直接报错强告警

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-22
- Owners: TBD
- Components: config/tools/sessions/channels/telegram/docs/issues
- Affected providers/models: all

**已实现（如有，写日期）**
- 2026-03-22：完成一次“严格 one-cut”审计，定位所有 fallback/alias/legacy parse 的代码点，并整理为本 issue（证据见 As-is）。
- 2026-03-22：配置、sessions、telegram、gateway、runtime/UI helper 已全部按 strict one-cut 收口：旧 `people/` 文档读取、`tools.exec.sandbox.scope` alias、`scope_key=session_key` 的回退、legacy `channel_binding` 解析、legacy schema 自动 rename、缺失 `bucket_key` 的宽松复用，以及 gateway/UI `persona` helper 尾巴均已移除或改为显式拒绝。
- 2026-03-22：额外收掉两处同类 runtime tail：`resolve_telegram_session_id()` 不再合成 legacy session id，`resolve_channel_bridge_session()` 不再为空 `session_key` 生成替代 bucket；两者都改为结构化拒绝。
- 2026-03-22：补齐最后一个启动期 schema compat tail：`crates/sessions/src/lib.rs` 不再自动将 `channel_sessions.session_key/account_id` rename 到 V3 列名，而是 fail-fast 并记录 `legacy_schema_rejected`。

**已覆盖测试（如有）**
- `crates/config/src/loader.rs`：legacy `people/` 命中时拒绝读取。
- `crates/config/src/validate.rs`：legacy `tools.exec.sandbox.scope` 为硬错误。
- `crates/tools/src/sandbox.rs`：`scope_key=session_key` 且缺 `_sessionKey` 时硬失败。
- `crates/telegram/src/adapter.rs`：legacy binding shape 被拒绝，缺失 `bucket_key` 不再兼容。
- `crates/sessions/src/metadata.rs`：legacy `account_id` 绑定不再命中查询。
- `crates/gateway/src/channel_events.rs`、`crates/gateway/src/chat.rs`、`crates/gateway/src/lib.rs`：legacy binding / missing bucket route / missing session key / gateway legacy schema 全部覆盖定向回归。
- `crates/sessions/src/lib.rs`：`channel_sessions.session_key/account_id` legacy 列名在启动期被显式拒绝，不再自动 rename。
- `crates/gateway/src/assets/js/persona-utils.test.mjs`：runtime/UI helper 已收敛到 agent 命名。

**已知差异/后续优化（非阻塞）**
- 历史/归档 issue 文档中仍会出现 `people/` 等旧术语；本次已同步活动主单与补缺单口径，冻结历史单据暂不重写。

---

---
- 场景：V3 one-cut 推进过程中，为修复升级回归引入了若干 fallback/alias/legacy parse。现在明确要求严格 one-cut：任何不符合 V3 口径的输入/配置/落盘一律拒绝（error/强告警），不再做任何兼容读取或隐式降级。
- 约束：
  - 不做前向兼容与数据迁徙（legacy 数据/配置不再尝试自动修复）。
  - 任何“策略/护栏/限制”导致行为变化，必须有结构化日志（带 `reason_code`），且不打印敏感信息。
- Out of scope：
  - 不做自动迁移工具（例如自动复制 `people/` 到 `agents/`）。
  - 不承诺对存量 legacy 会话/配置可继续工作。

- 场景：对 V3 one-cut 严格模式做补充审查时，发现当前 issue 已覆盖 agent docs / sandbox / Telegram binding 的主要兼容逻辑，但仍漏掉了启动时自动 schema rename、bucket 级宽松匹配、gateway 侧 legacy 测试夹具，以及 runtime/UI 的 `persona` 命名尾巴。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`one_cut_strict`**（主称呼）：V3 严格 one-cut 模式。
  - What：不允许 fallback/alias/legacy parse；遇到 legacy 输入直接拒绝。
  - Why：避免“半切/暗兼容”导致口径漂移与排障歧义。
  - Not：不是“升级不断”的版本策略。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：strict mode / no-compat

- **`legacy_input`**（主称呼）：任何 V2/迁移期字段、旧目录、旧 JSON shape。
  - Why：必须被显式拒绝并可观测。
  - Not：不是可接受的“容错输入”。
  - Source/Method：authoritative

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 移除/禁止所有运行时 fallback 读取（例如 agent docs 的 `people/` 回退读取）。
- [x] 移除/禁止所有配置 alias（例如 `tools.exec.sandbox.scope` 作为 `scope_key` 别名）。
- [x] 移除/禁止所有 legacy parse（例如 Telegram `channel_binding` 旧 JSON shape）。
- [x] 移除/禁止所有启动期自动 schema 迁移与宽松匹配（例如 `account_id -> account_handle` 自动 rename、缺失 `bucket_key` 也视为兼容）。
- [x] 清理 one-cut 口径下残留的 `persona` runtime/UI 命名尾巴，避免 issue 落地后仍保留旧概念 helper。
- [x] 对所有 legacy 输入做到“显式拒绝 + 强告警/错误 + 可定位 reason_code”。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：legacy 输入不得改变系统行为（不得“继续工作/继续读取/继续映射”）。
  - 必须：拒绝时必须可观测（结构化日志 + `reason_code`；配置校验以 error 形式返回）。
  - 不得：任何隐式降级（例如“未知值回退默认”“缺字段用别的字段凑”）。
- 兼容性：明确“不兼容”。
- 可观测性：仅在命中 legacy 输入并被拒绝时输出（避免噪声），必要时做去重。
- 安全与隐私：日志不得包含 token/正文/完整敏感字段。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) V3 口径要求 one-cut，但代码中仍存在 fallback/alias/legacy parse，导致“看似 V3，实际在吃旧输入”。
2) 这些兼容逻辑会让配置/数据的错误长期潜伏，直到某些边界条件触发（例如 sandbox scope 选择、web/main 会话缺 `_sessionKey`）。

### 影响（Impact）
- 用户体验：升级后某些 legacy 配置/数据可能“看起来还能跑”，但口径不一致，后续行为不可预测。
- 可靠性：跨层契约变成“可选/可回退”，导致核心路径在错误输入下继续推进。
- 排障成本：fallback/alias 让 root cause 难以定位，且容易出现“环境不同结果不同”。

### 复现步骤（Reproduction）
1. 在 data_dir 保留旧 `people/<agent_id>/...`，删除 `agents/<agent_id>/...`。
2. 在配置里设置 legacy `tools.exec.sandbox.scope = "chat"`。
3. 触发非 channel 的 web/main session 执行一个需要 sandbox 的 tool，并缺少 `_sessionKey`。
4. 期望 vs 实际：严格 one-cut 期望直接拒绝并提示迁移；但当前实现会发生 fallback/alias/回退。

## 修复前现状核查与证据（Pre-fix Evidence）【不可省略】
> 本节为 2026-03-22 审计时的修复前快照；修复后行为以“实施现状”和测试为准。
- Agent docs legacy fallback：
  - `crates/config/src/loader.rs:579`：`reason_code = "legacy_people_dir_fallback"`
  - `crates/config/src/loader.rs:586`：`fn resolve_agent_doc_path(...)`（存在回退读取逻辑）

- Sandbox legacy alias（配置）：
  - `crates/config/src/schema.rs:1258`：schema 明确保留 legacy `tools.exec.sandbox.scope`
  - `crates/config/src/schema.rs:1313`：`legacy_sandbox_scope_to_scope_key(...)`（legacy 映射函数）
  - `crates/config/src/validate.rs:1024`：校验注释写明 legacy scope “still maps during migration”
  - `crates/config/src/validate.rs:1041`：`tools.exec.sandbox.scope is deprecated; use ...scope_key`

- Sandbox legacy alias / runtime fallback：
  - `crates/tools/src/sandbox.rs:601`：`using deprecated tools.exec.sandbox.scope compatibility alias`
  - `crates/tools/src/sandbox.rs:622`：`unknown ...scope_key; falling back to session_id`
  - `crates/tools/src/sandbox.rs:2445`：`reason_code = "missing_session_key_fallback_to_session_id"`

- Telegram `channel_binding` legacy parse：
  - `crates/telegram/src/adapter.rs:258`：`struct LegacyTelegramChannelBinding`（旧 JSON shape）
  - `crates/telegram/src/adapter.rs:273`：`telegram_reply_target_from_binding(...)`（兼容解析入口）
  - `crates/telegram/src/adapter.rs:287`：legacy `account_handle/account_id` 兼容分支
  - `crates/telegram/src/adapter.rs:631`：测试名 `binding_helpers_accept_legacy_account_handle_shape`（证明当前接受 legacy）

- Sessions metadata legacy binding 兼容查询：
  - `crates/sessions/src/metadata.rs:646`：注释说明 legacy `account_id`
  - `crates/sessions/src/metadata.rs:652`：SQL LIKE pattern 包含 `account_id`
  - `crates/sessions/src/metadata.rs:673`：再次说明 legacy `account_id`

- Gateway 启动期自动 schema 迁移：
  - `crates/gateway/src/lib.rs:79`：`run_migrations(...)` 在启动时检查旧列名
  - `crates/gateway/src/lib.rs:88`：自动执行 `ALTER TABLE channels RENAME COLUMN account_id TO account_handle`
  - `crates/gateway/src/lib.rs:101`：自动执行 `ALTER TABLE message_log RENAME COLUMN account_id TO account_handle`

- Telegram bucket 级宽松匹配：
  - `crates/telegram/src/adapter.rs:376`：`telegram_binding_is_compatible_for_bucket(...)`
  - `crates/telegram/src/adapter.rs:392`：`info.bucket_key.as_deref().is_none_or(...)`，缺失 `bucket_key` 也会被视为兼容

- Gateway 侧 legacy 测试夹具：
  - `crates/gateway/src/channel_events.rs:2241`：测试 `resolve_channel_session_id_reuses_active_session_with_legacy_binding_blob_shape`
  - `crates/gateway/src/chat.rs:13938`：legacy binding fixture
  - `crates/gateway/src/chat.rs:14190`：legacy binding fixture

- Runtime/UI 的 `persona` 命名尾巴：
  - `crates/gateway/src/personas.rs:15`：`is_valid_persona_id(...)`
  - `crates/gateway/src/assets/js/persona-utils.js:1`：`isPersonaListLoaded(...)`
  - `crates/gateway/src/assets/js/persona-utils.js:5`：`isPersonaMissing(...)`

## 根因分析（Root Cause）
- A. 为修复升级回归（旧目录/旧字段/旧 JSON shape）引入了兼容逻辑。
- B. 兼容逻辑没有被 gated（例如 strict 模式开关），也没有形成“只报错不继续”的硬约束。
- C. 因为兼容路径在多数场景下静默成功，导致“V3 口径”与“运行时真实行为”发生偏离。
- D. 初始盘点主要盯住 fallback/alias/legacy parse，本轮补审发现同性质问题还存在于启动期 schema 自动 rename、bucket 宽松匹配、legacy 测试夹具和旧命名 helper。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：任何 legacy 输入都被拒绝（error/强告警），不得 fallback/alias/降级继续运行。
- 必须：拒绝必须可观测：
  - 配置层：`validate` 返回 Error（category 建议 `breaking-change`）。
  - 运行时：结构化日志带 `reason_code`，且不包含敏感内容。
- 不得：
  - 不得读取 `people/` 目录内容来“继续工作”。
  - 不得将 `tools.exec.sandbox.scope` 作为 `scope_key` 别名映射。
  - 不得在 `scope_key=session_key` 缺 `_sessionKey` 时回退到 `session_id`。
  - 不得解析 Telegram legacy `channel_binding` JSON shape。
  - 不得在启动时自动把 legacy schema 列名 rename 为 V3 列名。
  - 不得在 `bucket_key` 缺失时把旧 binding 视为“仍兼容当前 bucket”。
  - 不得保留会误导实现/评审的 gateway legacy 测试夹具与 `persona` 命名 helper。
- 应当：错误文案清晰给出 remediation（迁移到 `agents/`、改用 `scope_key`、修复 binding shape 等）。
- 应当：`scope_key=session_key` 在缺少 `_sessionKey` 时直接硬失败，不引入 session-type override 或回退分桶策略。
- 应当：legacy `channel_binding` 一旦命中，统一视为无效绑定；不复用、不 fork、不继续推导替代 session。
- 应当：gateway 启动发现 legacy schema 时直接拒绝启动，不采用 partial disable。

## 方案（Proposed Solution）
### 方案 1（推荐）：严格 one-cut（默认生效）
- 核心思路：删除所有兼容逻辑；必要时保留“识别 legacy 并报错”的最小解析，以便给出可读错误（但不得继续推进）。

#### 行为规范（Normative Rules）
- 规则 1（Agent docs）：`agents/<agent_id>/...` 不存在时，即便 `people/<agent_id>/...` 存在也不得读取；必须强告警并返回空/错误。
- 规则 2（Sandbox config）：出现 `tools.exec.sandbox.scope` 必须是 Error（不允许 warning/映射）；`scope_key` 只能是 `session_id|session_key`，未知值必须 Error。
- 规则 3（Sandbox runtime）：`scope_key=session_key` 时 `_sessionKey` 必须存在且非空；否则 Error。
- 规则 4（Telegram binding）：`channel_binding` 只接受 V3 形状；任何 legacy shape 直接判定不支持。
- 规则 5（Sessions metadata）：移除 legacy `account_id` 的兼容查询；只以 V3 形状查找。
- 规则 6（Schema migration）：启动链路不得自动 rename legacy 列名；发现旧 schema 必须显式失败并提示人工迁移。
- 规则 7（Bucket compatibility）：Telegram binding 的 `bucket_key` 必须存在且与当前 bucket 严格相等；缺失或不等都必须拒绝复用。
- 规则 8（Naming tails）：runtime/UI/helper 中残留的 `persona` 命名必须收敛到 `agent`，或在同一 issue 中明确删除。
- 规则 9（Session-key contract）：当 `tools.exec.sandbox.scope_key=session_key` 时，所有进入 sandbox 的调用都必须携带 `_sessionKey`；缺失即失败，`reason_code = "missing_session_key_for_scope_key_session_key"`。
- 规则 10（Legacy binding reject）：legacy `channel_binding` 被识别后，gateway 必须将其视为 invalid binding，返回显式错误并记录 `reason_code = "legacy_channel_binding_rejected"`；不得新建替代 session。
- 规则 11（Legacy schema reject）：启动链路发现 legacy schema 时必须 fail fast，记录 `reason_code = "legacy_schema_rejected"`，并指出具体 table/column。

#### 接口与数据结构（Contracts）
- 配置：
  - 仅允许 `tools.exec.sandbox.scope_key=session_id|session_key`。
  - 禁止 `tools.exec.sandbox.scope`。
- Tool context：
  - 当配置选择 `scope_key=session_key` 时，tool 调用必须提供 `_sessionKey`。

#### 失败模式与降级（Failure modes & Degrade）
- legacy 配置/数据被拒绝：
  - 配置：启动/校验失败（明确字段与修复方式）。
  - 运行时：工具执行失败（明确缺失字段），并记录 `reason_code`。
  - channel 绑定：依赖 legacy binding 的 session reuse / reply / location / typing / outbound 直接失败，不做 fork 或隐式替代。

#### 安全与隐私（Security/Privacy）
- 日志仅记录：字段名、路径、session_id（如需）、reason_code；不得打印正文或 token。

### 方案 2（备选）：严格模式开关（不建议）
- 思路：增加 `one_cut_strict=true` 开关，兼容逻辑默认保留。
- 缺点：会让团队持续背负双口径与测试分叉成本，且容易线上“忘记开”。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `people/` legacy 文档不再被读取；命中时强告警并拒绝（含 `reason_code`）。
- [x] `tools.exec.sandbox.scope` 出现即为 Error（不再映射为 `scope_key`）。
- [x] `scope_key=session_key` 且缺 `_sessionKey` 时返回 Error（不再回退 `session_id`）。
- [x] Telegram `channel_binding` legacy JSON shape 被拒绝（不再 parse）。
- [x] legacy `channel_binding` 命中后不会触发 session fork，也不会创建替代 session；而是显式报错并记录 `legacy_channel_binding_rejected`。
- [x] Sessions metadata 不再对 legacy `account_id` 做 SQL 匹配。
- [x] gateway 启动时不再自动执行 `account_id -> account_handle` 列 rename；旧 schema 明确失败。
- [x] Telegram binding 缺失 `bucket_key` 时不再被视为兼容当前 bucket。
- [x] gateway 侧 legacy binding 测试夹具被删除或改写为“明确拒绝 legacy”。
- [x] `persona` runtime/UI helper 尾巴已清理，或至少不再作为有效 V3 命名出现在实现层。
- [x] 文档与 issue 口径同步为 strict one-cut（不再描述 fallback/alias）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] loader：legacy `people/` 存在但 `agents/` 缺失时必须拒绝（新增/修改测试）：`crates/config/src/loader.rs`
- [x] validate：legacy `tools.exec.sandbox.scope` 必须 Error（新增/修改测试）：`crates/config/src/validate.rs`
- [x] sandbox runtime：`scope_key=session_key` 且缺 `_sessionKey` 必须 Error（新增/修改测试）：`crates/tools/src/sandbox.rs`
- [x] telegram adapter：legacy binding shape 输入必须返回 None/错误（删除/改写现有测试）：`crates/telegram/src/adapter.rs:631`
- [x] sessions metadata：移除 legacy `account_id` 匹配后更新回归（新增/修改测试）：`crates/sessions/src/metadata.rs`
- [x] gateway migrations：旧 `account_id` 列存在时必须显式失败，而不是自动 rename（新增/修改测试）：`crates/gateway/src/lib.rs`
- [x] sessions migrations：旧 `channel_sessions.session_key/account_id` 列存在时必须显式失败，而不是自动 rename（新增测试）：`crates/sessions/src/lib.rs`
- [x] telegram bucket compatibility：缺失 `bucket_key` 的 binding 不得复用当前 bucket（新增/修改测试）：`crates/telegram/src/adapter.rs`
- [x] gateway legacy fixtures：`channel_events` / `chat` 中 legacy binding 测试改为“拒绝 legacy”断言：`crates/gateway/src/channel_events.rs`、`crates/gateway/src/chat.rs`
- [x] naming tails：`persona` helper 改名/删除后的回归：`crates/gateway/src/personas.rs`、`crates/gateway/src/assets/js/persona-utils.js`

### Integration
- [x] 基础回归：严格 one-cut 下，旧配置/旧数据应在启动或首次触发点明确失败（记录 reason_code）。
- [x] channel 回归：legacy `channel_binding` 存在时，inbound reuse / web-originated reply / location / typing 均显式失败，不发生 silent fork。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：某些 TG 真实联调场景难以自动化（依赖外部 bot/chat）。
- 手工验证步骤：
  1. 准备旧 `people/` 文档与 legacy `tools.exec.sandbox.scope` 配置。
  2. 启动/校验应直接失败并指向修复方式。
  3. 准备 legacy `channel_binding` 的 session metadata；触发 web-originated reply，应明确报“不支持 legacy binding”。

## 发布与回滚（Rollout & Rollback）
- 发布策略：breaking change（严格 one-cut）；发布说明必须明确“不会自动迁移”，包括 `channel_sessions` legacy 列名也不再自动修复。
- 回滚策略：回滚到上一版本二进制与配置（如需继续跑 legacy 数据）。
- 上线观测：收敛 reason_code 并重点关注：
  - `legacy_people_dir_rejected`
  - 配置校验错误：`tools.exec.sandbox.scope`（无 runtime `reason_code`）
  - `missing_session_key_for_scope_key_session_key`
  - `legacy_channel_binding_rejected`
  - `legacy_schema_rejected`

## 实施拆分（Implementation Outline）
- Step 1：移除 agent docs 的 `people/` 回退读取，改为拒绝 + 强告警（`crates/config/src/loader.rs`）。
- Step 2：配置层：删除 legacy scope 映射（保留“检测并报错”）；更新模板/文档/测试（`crates/config/src/schema.rs`、`crates/config/src/validate.rs`、`crates/config/src/template.rs`、`docs/src/*`）。
- Step 3：tools sandbox：去掉所有 fallback/alias 分支；缺字段/未知值直接 Error（`crates/tools/src/sandbox.rs`）。
- Step 4：telegram adapter：移除“接受 legacy shape 并继续推进”的解析路径；仅保留用于识别后拒绝的最小 legacy shape 检测，并由调用方补齐 reason_code（`crates/telegram/src/adapter.rs`，可能联动 gateway）。
- Step 5：sessions metadata：移除 legacy `account_id` LIKE 匹配；更新查询与回归测试（`crates/sessions/src/metadata.rs`）。
- Step 6：启动链路 schema reject：移除 legacy 列自动 rename；`gateway` 与 `sessions` 命中旧列名时统一显式失败（`crates/gateway/src/lib.rs`、`crates/sessions/src/lib.rs`）。
- Step 7：Telegram bucket 兼容：`bucket_key` 改为严格匹配，不再接受缺失值（`crates/telegram/src/adapter.rs`，可能联动 `crates/gateway/src/channel_events.rs`）。
- Step 8：清理 gateway 侧 legacy fixture 与 `persona` 命名 helper（`crates/gateway/src/channel_events.rs`、`crates/gateway/src/chat.rs`、`crates/gateway/src/personas.rs`、`crates/gateway/src/assets/js/persona-utils.js`）。
- Step 9：同步活动 issue 文档口径（one-cut 主单/补缺单）与配置文档（`issues/*`、`docs/src/*`）。
- 受影响文件：
  - `crates/config/src/loader.rs`
  - `crates/config/src/schema.rs`
  - `crates/config/src/validate.rs`
  - `crates/tools/src/sandbox.rs`
  - `crates/telegram/src/adapter.rs`
  - `crates/sessions/src/metadata.rs`
  - `crates/gateway/src/lib.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/personas.rs`
  - `crates/gateway/src/assets/js/persona-utils.js`
  - `docs/src/configuration.md`
  - `docs/src/config-reset-and-recovery.md`
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
  - `docs/src/refactor/session-key-bucket-key-one-cut.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
  - `docs/src/refactor/session-key-bucket-key-one-cut.md`
- Related commits/PRs：
  - TBD

## 冻结决议（Frozen Decisions）
- D1：`tools.exec.sandbox.scope_key=session_key` 是硬契约；缺少 `_sessionKey` 时直接失败，不引入 session-type override，也不回退到 `session_id`。
- D2：legacy `channel_binding` 一律视为 invalid binding；gateway 不复用、不 fork、不继续推进依赖该 binding 的能力，统一返回显式错误并记录 `legacy_channel_binding_rejected`。
- D3：启动链路发现 legacy schema 时直接拒绝启动，不采用 partial disable，也不自动 rename；统一记录 `legacy_schema_rejected`。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

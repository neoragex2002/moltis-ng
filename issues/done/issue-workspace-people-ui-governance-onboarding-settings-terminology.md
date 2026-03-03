# Issue: Workspace/People 相关 UI 治理（Onboarding / Settings / Terminology）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Owners: luy
- Components: gateway / onboarding / ui / config
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-03：Settings Sidebar 移除 `Identity`，新增 `Contacts`，Type4 入口收敛到 `User / People / Contacts`：`crates/gateway/src/assets/js/page-settings.js:42`
- 2026-03-03：People（私有）管理落地（`people/<name>/*` 的 CRUD；default 不可删；统一保存 RPC）：`crates/gateway/src/assets/js/page-settings.js:331`、`crates/gateway/src/person.rs:166`、`crates/gateway/src/methods.rs:1341`
- 2026-03-03：Contacts（公共）编辑入口落地（`PEOPLE.md` frontmatter + body 可编辑；emoji/creature 只读对齐）：`crates/gateway/src/assets/js/page-settings.js:675`、`crates/gateway/src/people.rs:213`
- 2026-03-03：Channels 绑定口径收敛：UI 文案改为 “Agent (optional)” + 指向 `people/<name>/`，并改为 selector（值仍落到存量 `persona_id` 字段）：`crates/gateway/src/assets/js/page-channels.js:31`、`crates/gateway/src/assets/js/page-channels.js:393`
- 2026-03-03：Onboarding 文案收敛：Step label/summary 不再出现 “Identity”，统一使用 “Agent” / “Owner & Agent”：`crates/gateway/src/assets/js/onboarding-view.js:35`
- 2026-03-03：停止首次 seed 生成 workspace root 的 `AGENTS.md`/`TOOLS.md`（避免与 `people/default/*` 造成双 SOT 误解）：`crates/gateway/src/server.rs:5124`

**已覆盖测试（如有）**
- `workspace.user.update` 支持 `body`（保留/更新 body）：`crates/gateway/src/user.rs:122`、`crates/gateway/src/user.rs:139`
- `workspace.people.updateEntry` 支持 `body`（保留/更新 body）：`crates/gateway/src/people.rs:317`、`crates/gateway/src/people.rs:352`
- `workspace.people.sync` 保留 `PEOPLE.md` body 且不触碰未知键/用户字段：`crates/config/src/loader.rs:1914`
- `workspace.person.*` 私有目录 CRUD：`crates/gateway/src/person.rs:359`、`crates/gateway/src/person.rs:367`、`crates/gateway/src/person.rs:382`、`crates/gateway/src/person.rs:412`

**已知差异/后续优化（非阻塞）**
- 存量兼容：Channels/DB 字段仍为 `persona_id`（仅作为存储兼容；UI 已不再展示 “persona” 概念）：`crates/gateway/src/assets/js/page-channels.js:87`
- 自动化缺口：当前无 UI e2e；本单以 unit tests + 手工验收为主（见 Test Plan）

---

## 背景（Background）
- 场景：
  - 初次安装：用户在 Onboarding “Set up your agent” 页面填写 Owner 与默认 agent 信息。
  - 日常使用：用户在 Settings 页面配置 Type4（USER/PEOPLE/people/<name>）相关数据，并在 Channels 页面给 bot 绑定 “persona_id”。
- 约束：
  - Type4 现有 SOT 已确定：
    - 公共：`USER.md`、`PEOPLE.md`（YAML frontmatter 字段 + Markdown 正文；UI 可编辑字段与正文）
    - 私有：`people/<name>/{IDENTITY.md,SOUL.md,TOOLS.md,AGENTS.md}`（agent 自身文件；UI 可编辑 `IDENTITY.md` 字段与正文；`SOUL/TOOLS/AGENTS` 正文可编辑）
  - 备注：此前 Type4 治理曾采用 “frontmatter 字段可编辑、正文由人手工维护” 的更保守策略；本单明确升级为 **WebUI 允许编辑正文**（同时要求不做自动重排/格式化）。
  - 兼容性：Channels/DB 仍存在 `persona_id` 字段，但 UI/文案应收敛到 “people/<name> / agent name” 口径。
- Out of scope（本单不做，或仅记录口径）：
  - Skill / Hook 配置治理（另单）
  - Heartbeat / Cron 数据治理（另单；仅保证 UI 不再把它们混入 Type4 叙事）
  - 后端内部大规模重命名（如 `chat.rs` 里的 `PromptPersona` 全量改名）——本单优先 UI/协议口径收敛，后端内部名可后续渐进式治理

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **People（私有 agent 配置）**（主称呼）：指 `people/<name>/` 目录及其 4 个文件，用于构建该 agent 自身 prompt/行为。
  - Why：这是 agent 自我定义与工具/路由规则的唯一可写入口（除正文手工编辑外），也是 default agent 的配置承载。
  - Not：不是公共通信录；不用于“让其他 agent 了解我”（隐私边界：默认不对其它 agent 暴露）。
  - Source/Method：configured（文件）
  - Aliases（仅记录，不在正文使用）：persona / profiles / identity（旧口径）

- **Contacts（公共通信录）**（主称呼）：指 `PEOPLE.md`（公共、全体可读），用于让 agent 认识“系统里有哪些 agent”以及对外联系信息（如 Telegram）。
  - Why：这是跨 agent 的“最小公开信息集”，也承接 UI 的可见/可编辑部分。
  - Not：不包含私有 prompt/行为细节；不包含任何凭证/内部实现信息。
  - Source/Method：configured（文件）+ effective（emoji/creature 由系统对齐）
  - Aliases（仅记录，不在正文使用）：roster / directory / people list

- **Default Agent（默认 agent）**（主称呼）：指 `people/default/` 对应的 agent，用于 main 会话、spawn agent、Web UI 会话等默认行为拼装。
  - Why：它是系统默认人格/提示词的落点；Onboarding 与 Settings 应清晰围绕它配置，而不制造第二套 “Identity” SOT。
  - Not：不是 `moltis.toml` 的 `[identity]`（已退场）。
  - Source/Method：configured（文件）
  - Aliases（仅记录，不在正文使用）：main persona / default persona

- **persona_id（存量字段）**（主称呼）：渠道配置/数据库里的绑定字段（例如 Telegram bot 绑定哪个 agent）。
  - Why：存量兼容；短期保留字段名，长期可考虑迁移/别名化。
  - Not：不应在 UI/文案中对用户呈现为 “persona” 概念；UI 展示应改为 “agent name (people/<name>)”。
  - Source/Method：configured（DB/config）
  - Aliases（仅记录，不在正文使用）：people_name / agent_name

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 初次安装引导（Onboarding）口径收敛：只配置 Default Agent + USER（不制造独立 “Identity” 概念）
- [x] Settings 栏目治理：
  - [x] 移除 Identity 栏
  - [x] User 栏保留（字段可改、正文可改）
  - [x] People 栏：提供对 `people/<name>/` 的管理（包含 default），支持创建/删除（default 不可删）、以及 4 个文件的 CRUD（见 Spec）
  - [x] Contacts 栏：呈现 `PEOPLE.md` 公共通信录内容，并清晰区分“可编辑字段 vs 只读字段”
- [x] UI 侧描述与概念名词收敛：全站 UI 文案不再出现 “persona”（至少：Settings、Onboarding、Channels、以及相关 util）
- [x] 初次安装生成的初始配置文件治理（最小要求）：
  - [x] `moltis.toml` 模板/默认配置不再引导用户在 TOML 中配置 identity/user（已退场）
  - [x] 初次启动/引导完成时，Type4 文件的生成/seed 行为清晰、可预测（不产生“哪个文件生效”的误解）
  - [x] workspace root 的 `AGENTS.md`/`TOOLS.md` 视为 Deprecated，并停止首次 seed 生成（避免与 `people/default/*` 造成双 SOT 误解）

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：UI 允许编辑 `USER.md`、`PEOPLE.md`、`people/<name>/IDENTITY.md` 的 YAML frontmatter 字段与 Markdown 正文（body）
  - 必须：UI 保存时不得“自动重排/重格式化” Markdown 正文（除非用户明确修改正文内容）
  - 必须：任何“sync/对齐/修复”动作必须保留 Markdown 正文（body）原样不变
  - 必须：Contacts 的 `name/emoji/creature` 为只读展示（由系统从 `people/<name>/IDENTITY.md` 对齐/同步）
  - 不得：UI/协议引入新的第二 SOT（例如再造 `moltis.toml [identity]` 或单独 Identity 页面写入另一套存储）
  - 不得：Contacts/People 传播凭证/内部实现信息
- 接口收敛（必须）：
  - 必须：RPC/接口尽量复用与扩展现有方法，禁止为同一资源的“局部更新”无限拆分成多个小接口（例如为 `PEOPLE.md` 再新增多套 updateXxx RPC）
  - 必须：UI 交互避免琐碎的多次 RPC（例如一次保存拆成多次请求）；优先一次请求原子保存，降低中间态与实现复杂度
- UI 质量（必须）：
  - 必须：沿用现有 Settings/Channels 的组件与视觉规范（按钮/输入框/表格/Modal/间距/颜色变量），避免“半成品”风格
  - 必须：兼容并正确呈现现有所有 color theme（如 light/dark）；不得硬编码颜色值，优先使用现有 CSS variables/tokens
  - 必须：新栏目提供完整的空状态/加载态/错误态/保存反馈（禁用按钮、toast/提示），并保持布局可滚动、响应式不崩
  - 必须：UI 文案以英文为主（不做全站中英混搭）；必要解释用短句 + tooltip/secondary text，不在主标签里夹中文
  - 必须：所有用户可见字符串使用统一概念（People / Contacts / Default agent），不得出现 “persona” 术语
  - 应当：信息架构优先简单（dropdown + 垂直分区/表单），避免复杂表格/多栏布局导致观感“像半成品”
- 兼容性：
  - 保留 `persona_id` 存量字段（短期）；UI 仅做“展示名/文案/字段提示”收敛，不强制一次性迁移 DB/字段名
- 遗留字段处理口径（本单冻结）：
  - DB/配置仍使用 `persona_id`（存量）承载绑定值
  - UI 一律展示为 “Agent (optional)”（并通过 secondary text 指向 `people/<name>/`），同时隐藏 “persona” 术语
  - 若后续需要重命名/迁移字段（例如 `agentName`），必须另开 issue 做 DB migration 与回滚策略
- 可观测性：
  - Onboarding/Settings 关键操作失败需返回明确错误信息（不含敏感内容）
- 安全与隐私：
  - People（私有）与 Contacts（公共）在 UI 上要有清晰边界提示，避免用户误把私有 prompt 当公共信息发布

## 问题陈述（Problem Statement）
### 现象（Symptoms）
（历史问题，已修复）
1) Settings 曾暴露 “Identity” 栏并默认打开，导致 Type4 概念混乱。
2) Channels 曾暴露 “Persona ID” 术语并要求自由输入，用户不知道填什么。
3) Onboarding 曾使用 “Set up your identity”，实际是在配置 default+owner，但 UI 概念不清晰。

### 影响（Impact）
- 用户体验：概念混乱、学习成本高、容易误操作（把私有 prompt 当公共信息/或者不知道哪处生效）
- 可靠性：配置入口分散导致维护困难，出现“改了但没生效”的主观故障
- 排障成本：日志/页面/代码同时出现 people/persona/identity 多套术语，沟通与定位成本高

### 复现步骤（Reproduction）
（历史问题，已修复；现状见文档顶部 “实施现状”。）
1. Settings：验证 Sidebar 仅包含 `User / People / Contacts`
2. Channels：新增/编辑 Telegram bot 时，绑定字段为 “Agent (optional)” 且为 selector
3. People：创建/克隆/删除（default 不可删），并能保存 `IDENTITY/SOUL/TOOLS/AGENTS`
4. Contacts：可编辑公共字段与 `PEOPLE.md` body；emoji/creature 只读且可 sync

## 现状核查与证据（As-is / Evidence）【不可省略】
- UI 证据：
  - Settings Sidebar 已移除 Identity，并新增 Contacts：`crates/gateway/src/assets/js/page-settings.js:42`
  - Settings People（私有 `people/<name>/`）管理：`crates/gateway/src/assets/js/page-settings.js:331`
  - Settings Contacts（公共 `PEOPLE.md`）编辑：`crates/gateway/src/assets/js/page-settings.js:675`
  - Channels 绑定 agent selector（UI “Agent (optional)”）：`crates/gateway/src/assets/js/page-channels.js:393`
  - Onboarding step labels/summary 不再出现 “Identity”：`crates/gateway/src/assets/js/onboarding-view.js:35`
- RPC/协议证据：
  - `workspace.person.*` 注册（list/get/save/delete）：`crates/gateway/src/methods.rs:1341`
  - `workspace.user.update` / `workspace.people.updateEntry` 支持 `body`：`crates/gateway/src/user.rs:63`、`crates/gateway/src/people.rs:213`
- Seed 证据：
  - workspace root seed 不再写 `AGENTS.md`/`TOOLS.md`：`crates/gateway/src/server.rs:5124`
- 当前测试覆盖：
  - 私有 People RPC：`crates/gateway/src/person.rs:359`
  - USER/PEOPLE body 更新与保留：`crates/gateway/src/user.rs:122`、`crates/gateway/src/people.rs:317`
  - PEOPLE sync 保留 body 与其它字段：`crates/config/src/loader.rs:1914`

## 根因分析（Root Cause）
- A. 历史上存在 “persona” 概念，UI/Channels/内部结构长期使用 persona_id 作为绑定键
- B. Type4 SOT 已切换到 people/<name> + PEOPLE.md/USER.md，但 UI 信息架构未同步收敛
- C. Onboarding/Settings 为了快速配置，沿用 `agent.identity.*`，造成“Identity”概念在 UI 层残留并误导用户

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - UI 不再出现独立 “Identity” 栏；默认 agent 配置全部进入 People（私有）与 Contacts（公共）两处
  - Onboarding 仍能完成：配置 USER（公共）+ default（私有/公共展示名），但 UI 文案与字段指向必须明确且英文为主
  - Contacts（PEOPLE.md）字段边界必须明确：
    - 只读：`name`、`emoji`、`creature`（它们从 `people/<name>/IDENTITY.md` 同步而来；UI 需要标注来源路径）
    - 可编辑：`displayName`、`telegramUserId`、`telegramUserName`、`telegramDisplayName`
    - 正文：`PEOPLE.md` body 必须可编辑（作为公共备注/说明）
  - People（私有）必须包含 default，并提供 `people/<name>/` 管理：
    - `<name>` 必须为稳定目录名（ASCII 小写 + 数字 + `_`/`-`，长度 ≤ 64；与 `moltis_config::is_valid_person_name` 一致）
    - `IDENTITY.md`：允许编辑 YAML frontmatter（字段：`emoji/creature/vibe`；`name` 强制写回 `<name>`）与 Markdown 正文（body）
    - `SOUL.md`：正文可编辑（允许 UI 写入）
    - `TOOLS.md`：正文可编辑（允许 UI 写入）
    - `AGENTS.md`：正文可编辑（允许 UI 写入）
  - User（`USER.md`）：
    - YAML frontmatter 字段可编辑
    - Markdown 正文（body）必须可编辑
- 不得：
  - 不得引入新的 persona/identity 存储 SOT
  - 不得在 UI 保存时覆盖/重排 Markdown 正文（除非用户明确修改正文内容）
- 应当：
  - Channels 页面把 “Persona ID” 改为 “Agent (optional)”（并通过 secondary text 指向 `people/<name>/`）：
    - v1：使用 selector（下拉选择器），选项来自 `people/<name>/` 目录列表；若存量配置含未知值，应展示 “Missing: <value>” 并允许保留/修改
    - v2（可选）：暂不做 “custom value” 输入（保持 selector-only）

## UI 文案与样式规范（UI Copy & Visual Spec）
> 本节用于冻结对用户展示的“主文案”，避免实现时临时发挥导致概念漂移。

### Onboarding（初次安装引导）
- Step 标题（推荐）：
  - 从：`Set up your identity`
  - 到：`Set up your agent`
- 结构（推荐两块卡片/分区）：
  - `Owner`（写入 `USER.md` frontmatter）
    - 字段：`Owner name`（原 “Your name”）
  - `Default agent`（写入 `people/default/IDENTITY.md` + `PEOPLE.md` displayName）
    - 字段：`Display name`（替代 “Agent name”）
    - 字段：`Emoji`、`Creature`、`Vibe`（如保留）
    - secondary text（英文，简短）：说明这些用于默认 agent，并可在 Settings → People/Contacts 后续调整

### Settings（栏目）
- Sidebar：
  - 移除：`Identity`
  - 保留：`User`（Owner profile; public）
  - 调整：`People`（Private agent configuration）
  - 新增/调整：`Contacts`（Public directory）
- User 页面（最小要求）：
  - 表单字段：编辑 `USER.md` 的 frontmatter 字段（如 name/timezone/location）
  - `USER.md` body：正文可编辑 textarea
  - 保存按钮（位置冻结）：
    - 页面 header 右侧放置 `Save`（保存字段与正文），仅在有变更时启用；保存成功显示短暂 `Saved`
- People 页面（推荐布局，简洁版）：
  - 顶部：`Agent` dropdown（来源：`people/<name>/` 目录；必须包含 `default`，并标记 `Default` badge）
  - 顶部操作按钮（靠右；沿用现有按钮样式/间距）：
    - `New`：创建 `people/<name>/`
    - `Clone`：复制当前 agent 的 4 文件到新 `<name>`（允许以 default 为源）
    - `Delete`：删除当前 agent（禁用 default；并在 tooltip 明确 “Default agent cannot be deleted”）
  - 下方：垂直分区依次呈现（不做复杂 tabs/多栏）：
    - `Identity`：编辑 frontmatter 字段（emoji/creature/vibe）与正文（body）；secondary text 显示 `people/<name>/IDENTITY.md`
    - `Soul`：正文可编辑；secondary text 显示 `people/<name>/SOUL.md`
    - `Tools`：正文可编辑；secondary text 显示 `people/<name>/TOOLS.md`
    - `Agents`：正文可编辑；secondary text 显示 `people/<name>/AGENTS.md`
  - 保存按钮（必须明确）：
    - 每个分区都有独立的 `Save` 按钮（置于分区 header 右侧），仅在内容变更后启用；保存成功后显示短暂 `Saved` 状态
    - `New/Clone/Delete` 与分区 `Save` 解耦；删除需要二次确认 modal（default 时禁用）
  - RPC（必须收敛）：
    - 分区 `Save` 一律调用 `workspace.person.save`（只提交该分区的变更字段，避免琐碎接口）
    - `New`：调用 `workspace.person.save({ name })`（不存在则 seed 创建）
    - `Clone`：调用 `workspace.person.get(sourceName)` 后再调用 `workspace.person.save({ name: destName, identityPatch, identityBody, soul, tools, agents })`
    - `Delete`：调用 `workspace.person.delete({ name })`
  - 必须有：empty state、loading state、error state、saved feedback
- Contacts 页面（推荐布局，简洁版）：
  - 顶部：`Agent` dropdown（来源：`PEOPLE.md` 的 `people[]`）
  - 顶部操作按钮：
    - `Sync emoji/creature`（沿用现有行为）
  - 下方：垂直字段表单（不使用复杂表格）：
    - 只读展示：`name` / `emoji` / `creature`（灰色、不可输入）
    - 可编辑字段：`displayName` / `telegramUserId` / `telegramUserName` / `telegramDisplayName`
  - PEOPLE.md body：正文可编辑 textarea（公共备注/说明）
    - 注意：该正文为全局内容，不随上方 dropdown 切换而变化
  - 保存按钮（必须明确）：
    - 页面 header 右侧放置 `Save`（保存当前 entry 的 frontmatter 字段与 PEOPLE.md 正文），仅在有变更时启用
    - header 右侧按钮组顺序：`Sync emoji/creature`（secondary）→ `Save`（primary）
    - `Sync emoji/creature` 为独立按钮（执行后应直接落盘并显示 “Synced” feedback；不得覆盖正文）
    - RPC：`Save` 应通过一次 `workspace.people.updateEntry` 同时提交 `{ patch, body }`（避免多次 RPC 造成中间态/复杂度膨胀）

  - 异常态（必须明确）：
    - PEOPLE.md 条目对应的 `people/<name>/` 目录不存在：展示 `Missing` badge（不自动删除条目）
    - `people/<name>/` 目录存在但 PEOPLE.md 缺条目：在 People 页面顶部提示（不自动写入，除非用户显式操作）

### Channels（bot 绑定）
- 字段标签：
  - 从：`Persona ID (optional)`
  - 到：`Agent (optional)`
- 输入控件（v1）：
  - 使用 selector（下拉选择器），选项来自 `people/<name>/` 目录
  - 若当前存量值不在目录中，显示 `Missing: <value>`（warning 风格），并允许用户显式改成有效项或保留原值
- placeholder（示例）：
  - 从：`e.g. default / ops / research`
  - 到：`Select an agent (e.g. default)`

## 初始配置文件治理（Initial Config / Seeds）
### `moltis.toml`（首次运行生成的默认模板）
- 必须：模板中不得出现已退场的 `[identity]`、`[user]` 等段落（SOT 已迁移到 workspace data files）
- 当前模板段落（供核对，避免回归）：`crates/config/src/template.rs:1`
  - 顶层 sections（非详尽，仅列主段落）：`[server] [auth] [tls] [providers] [chat] [tools] [skills] [mcp] [metrics] [heartbeat] [failover] [voice.*] [tailscale] [memory] [channels]`

### Workspace data dir（`~/.moltis/`）首次启动的 seed 行为
- 现状证据：
  - `people/default/*` 与 `PEOPLE.md` 对齐：`crates/gateway/src/server.rs:911`
  - workspace root seed：`BOOT.md`、`HEARTBEAT.md`（不再 seed `AGENTS.md`/`TOOLS.md`）：`crates/gateway/src/server.rs:5124`
- 治理要求（本单最小目标）：
  - Settings 的 Type4 相关 UI（User/People/Contacts）不得引用或编辑 workspace root 的 `AGENTS.md`/`TOOLS.md`，避免与 `people/default/*` 形成“看似双 SOT”的误解
  - workspace root 的 `AGENTS.md`/`TOOLS.md` 在产品口径上视为 **Deprecated**（不再作为任何 agent prompt 的组成部分；仅保留兼容/历史文件）
  - 首次 seed 行为应停止生成 workspace root 的 `AGENTS.md`/`TOOLS.md`（保留 `BOOT.md`/`HEARTBEAT.md` 的既有逻辑不变）

## 配置文件示例（Examples）
> 示例只展示“最小必须字段 + 常见字段”。YAML frontmatter 部分允许系统格式化/对齐；Markdown 正文允许在 WebUI 中编辑。

### 公共（所有 agent 可读）
#### `~/.moltis/USER.md`（Owner 档案；YAML frontmatter + body）
```md
---
name: Alice
timezone: America/Los_Angeles
---

# USER.md

Notes about the owner.
```

#### `~/.moltis/PEOPLE.md`（公共通信录；YAML frontmatter + body）
```md
---
schema_version: 1
people:
  - name: default
    display_name: Default
    emoji: 🤖          # read-only in UI; synced from people/<name>/IDENTITY.md
    creature: Assistant # read-only in UI; synced from people/<name>/IDENTITY.md
    telegram_user_id: "123456789"
    telegram_user_name: "@my_bot"
    telegram_display_name: "My Bot"
---

# PEOPLE.md

Public directory notes.
```

### 私有（仅该 agent 自身可读；用于拼装 prompt）
#### `~/.moltis/people/default/IDENTITY.md`（YAML frontmatter + body）
```md
---
name: default  # forced to match directory name
emoji: 🤖
creature: Assistant
vibe: Direct, clear, efficient
---

# IDENTITY.md

Longer self-definition for this agent.
```

#### `~/.moltis/people/default/SOUL.md`（body only）
```md
You are concise, direct, and helpful.
```

#### `~/.moltis/people/default/TOOLS.md`（body only）
```md
- Allowed tools:
  - exec (sandboxed)
```

#### `~/.moltis/people/default/AGENTS.md`（body only）
```md
# AGENTS.md

Notes about sub-agents/spawn behavior.
```

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：UI 信息架构重排 + 新增 People 私有目录 RPC
- 核心思路：
  - Settings：Identity 退场；User 保留；People=私有目录管理；Contacts=公共通信录
  - Onboarding：保留 `agent.identity.update`（兼容），但只作为“配置 default + USER”实现；UI 文案/字段名收敛
  - Channels：文案/提示收敛到 people/<name>，字段名仍保持 persona_id（兼容）
- 优点：最贴近用户心智模型（私有 vs 公共）；不需要一次性迁移 DB/字段名；落地路径清晰
- 风险/缺点：需要新增 RPC + UI 组件（People 私有目录 CRUD）

#### 方案 2（备选）：保留 Identity 但弱化/隐藏
- 核心思路：Identity 仍存在但默认不展示，或仅保留显示
- 风险：仍保留概念分裂（用户一旦发现 Identity，会继续困惑）

### 最终方案（Chosen Approach）
选择方案 1。

#### 行为规范（Normative Rules）
- 规则 1：Onboarding 只配置 default + USER；UI 文案不得再使用 persona/identity 作为用户侧主概念
- 规则 2：People（私有）编辑 `IDENTITY.md` 时允许同时 patch frontmatter 与 body；并强制 frontmatter 的 `name` 字段写回 `<name>`（稳定目录名），避免历史遗留不一致
- 规则 3：Contacts 从 `PEOPLE.md` 读取；其中 `name/emoji/creature` 为只读展示（由 `people/<name>/IDENTITY.md` 同步）；可编辑字段只包含对外展示名与对外联系信息

#### 接口与数据结构（Contracts）
- 现有 RPC（保留/复用）：
  - `workspace.user.get/update`
  - `workspace.people.get/updateEntry/sync`
  - `agent.identity.get/update`（Onboarding 兼容入口；Settings 不再使用）
- 现有 RPC 语义补齐（本单需要扩展/对齐的行为）：
  - `workspace.user.update`：在现有 `patch` 基础上，增加可选 `body`（允许 UI 编辑 `USER.md` 正文）
  - `workspace.people.updateEntry`：在现有 entry patch 基础上，增加可选 `body`（允许 UI 编辑 `PEOPLE.md` 正文；一次 RPC 原子保存 entry+body，避免接口过度细分）
  - `workspace.people.sync`：必须保留 `PEOPLE.md` 正文（body）原样不变，且不得触碰用户可编辑字段（displayName/telegram*）
- 新增 RPC（本单新增，供 People 私有目录管理；必须收敛）：
  - `workspace.person.list`：列出 `people/<name>/`
  - `workspace.person.get`：读取 `IDENTITY/SOUL/TOOLS/AGENTS`
  - `workspace.person.save`：统一保存入口（写入 `people/<name>/` 的 4 文件；尽量一次请求提交变更，避免 updateXxx 过度细分）
  - `workspace.person.delete`：删除（禁止删除 default）
- UI：
  - Settings sidebar：`User`、`People`、`Contacts`（Identity 删除）
  - Channels：把 “Persona ID” 改为 “Agent (optional)”（secondary text 指向 `people/<name>/`），并使用 selector（选项来自 `workspace.person.list`；存量未知值显示 Missing）

**RPC 字段约定（必须遵守）**
- 外部 JSON/RPC 字段使用 `camelCase`（与仓库约定一致）
- Rust 内部结构使用 `snake_case`（serde 显式映射）

**建议的最小请求/响应形状（供实现对齐）**
- `workspace.user.update` params: `{ patch, body? }` → 返回 `workspace.user.get` 形状（包含 `body`）
- `workspace.people.updateEntry` params: `{ name, patch, body? }` → 返回 `workspace.people.get` 形状（包含 `body`）
- `workspace.person.list` → `{ people: [{ name, isDefault }] }`
- `workspace.person.get` params: `{ name }` → `{ name, identity: { name, emoji, creature, vibe, body }, soul, tools, agents }`
- `workspace.person.save` params（推荐）：
  - `{ name, identityPatch?, identityBody?, soul?, tools?, agents? }` → 返回最新 `workspace.person.get` payload
  - 约束：`identityPatch.name` 必须忽略，后端强制写回 `<name>`；`identityBody?` 缺省时必须保留原正文不变
  - 创建：当 `people/<name>/` 不存在时，`workspace.person.save({ name })` 必须自动 seed 创建（等价于 “New”）
  - 克隆：UI 可通过 `workspace.person.get(source)` + `workspace.person.save({ name: dest, ...payload })` 实现，不新增专用 clone RPC
- `workspace.person.delete` params: `{ name }` → `{ ok: true }`

#### 失败模式与降级（Failure modes & Degrade）
- People 私有目录不存在/损坏：
  - list/get 返回可诊断错误；UI 显示告警但不崩溃
- YAML frontmatter 解析失败：
  - 涉及 frontmatter 的页面必须 fail-fast，提示用户手工修复（不允许在解析失败时“盲写”覆盖文件）
 
#### 实现注意事项（Implementation Notes）
- 必须：所有“sync/对齐/修复”逻辑只能改动明确声明为系统维护的字段（例如 Contacts 的 `name/emoji/creature`），不得重写 body，不得覆盖用户可编辑字段（displayName/telegram*），不得丢弃未知键
- 应当：文件写回尽量采用“读 → 局部 patch → atomic write if changed”的方式，避免每次保存都整文件重写造成无意义 diff 与竞态风险

#### 安全与隐私（Security/Privacy）
- UI 说明必须明确：
  - People：仅“自己”使用的私有 prompt/规则（谨慎填写，避免复制敏感信息到公共）
  - Contacts：公共最小信息集

## 验收标准（Acceptance Criteria）【不可省略】
- [x] Settings 不再出现 Identity 栏；默认进入 User 或 People
- [x] People 栏可管理 `people/<name>/`（含 default），并支持 4 文件 CRUD（按 Spec 权限/只读规则）
- [x] Contacts 栏展示 `PEOPLE.md`，只读/可编辑字段边界清晰，且支持编辑并保存 `PEOPLE.md` 正文（body）
- [x] Channels 页面不再出现 “persona” 文案；字段口径改为 people/<name>，并使用 selector（能处理 Missing 存量值）
- [x] UI 侧字符串/文案中不再出现 “persona”（至少覆盖 Settings/Onboarding/Channels）
- [x] 初次安装生成的默认配置/种子文件不会引导用户配置已退场字段（identity/user in TOML）
- [x] UI 体验不降级：样式与现有 Settings/Channels 一致，具备空状态/加载态/错误态/保存反馈，不出现明显未完成布局
- [x] UI 文案英文为主且不混搭；关键字段命名与提示按 “UI 文案与样式规范” 执行

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] People 私有目录 RPC：list/get/save/delete（default 不可删、仅 patch 字段时保留正文不变、提供 body 时能正确写入）：`crates/gateway/src/person.rs:359`
- [x] `workspace.user.update` 支持 body：仅 patch 字段时保留正文不变；提供 body 时能正确写入：`crates/gateway/src/user.rs:122`
- [x] `workspace.people.updateEntry` 支持 body：仅 patch entry 字段时保留正文不变；提供 body 时能正确写入；`workspace.people.sync` 不覆盖正文：`crates/gateway/src/people.rs:317`、`crates/config/src/loader.rs:1914`
- [ ] （可选）新增更严格的 UI 文案断言（避免未来回归引入 “persona” 文案；注意避免误伤 `persona_id` 存量字段名）

### Integration
- [ ] Onboarding：填写后能正确写入 `USER.md`、`PEOPLE.md`、`people/default/IDENTITY.md` 并完成对齐

### UI E2E（Playwright，如适用）
- [ ] （如当前仓库未引入 e2e，则记录缺口并手工验收）

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：UI E2E 未纳入当前测试体系（如属实）
- 手工验证步骤：
  1) 全新数据目录启动 → 走 Onboarding → 验证 USER/PEOPLE/default identity 写入
  2) Settings：People 创建一个新 `<name>` → 编辑 SOUL/TOOLS/AGENTS → 验证落盘
  3) Contacts：编辑 displayName/telegram 字段 + PEOPLE.md 正文 → 验证 frontmatter 与正文均更新且不发生非预期重排
  4) Channels：绑定 agent name（旧字段 persona_id）→ 运行会话验证生效

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认开启（纯 UI/RPC 改动），旧 RPC 保留以兼容旧客户端
- 回滚策略：保留 `agent.identity.*` 与旧 Contacts/People 的 RPC；若新 People 私有目录 UI 出问题，可暂时隐藏入口并退回旧 Settings（需保留旧实现一段时间或 feature flag）
- 上线观测：关注 RPC 错误日志（frontmatter parse error / file io），以及 UI 操作失败率

## 实施拆分（Implementation Outline）
- Step 1: Onboarding 文案/字段名收敛（明确 default + USER；不再向用户强调 “identity” 概念）
- Step 2: Settings sidebar 移除 Identity；新增/调整 People 与 Contacts 栏
- Step 3: 新增 People 私有目录管理 RPC（workspace.person.*）
- Step 4: UI 侧 “persona” 术语收敛（Channels + utils + 相关提示文案）
- Step 5: 默认配置模板/初始 seed 行为核查与最小治理（确保不出现已退场 identity/user TOML 引导；Type4 种子文件路径清晰）
  - 包含：停止首次 seed 生成 workspace root 的 `AGENTS.md`/`TOOLS.md`（Deprecated）
- Step 6: 补齐单测/必要的手工验收脚本说明
- 受影响文件（预估）：
  - `crates/gateway/src/assets/js/page-settings.js`
  - `crates/gateway/src/assets/js/onboarding-view.js`
  - `crates/gateway/src/assets/js/page-channels.js`
  - `crates/gateway/src/assets/js/persona-utils.js`（改名/替换）
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/<new_person_api>.rs`（新增）
  - `crates/config/src/template.rs`（若需最小模板治理）

## 交叉引用（Cross References）
- Related issues/docs：
  - Type4 Single SOT DONE：`issues/done/issue-workspace-persona-frontmatter-fields-only-single-sot.md:1`
  - Terminology convergence（历史）：`issues/done/issue-terminology-and-concept-convergence.md:1`
- Related commits/PRs：<TBD>
- External refs（可选）：N/A

## 未决问题（Open Questions）
- None

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（本单无相关口径新增/变更）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

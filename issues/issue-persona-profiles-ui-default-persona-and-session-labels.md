# Issue: Persona Profiles UI 管理 + `personas/default` 口径收敛 + Channels/Session 信息显性化

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P2（体验/可控性增强；不阻断核心对话）
- Components: gateway / config / sessions metadata / Web UI / channels(telegram)
- Affected providers/models: all（主要是 UI/配置/会话元数据；与 provider 无强耦合）

**已实现（相关能力，作为本单前置条件）**
- 支持按 Telegram bot 绑定 `persona_id`，并从 `~/.moltis/personas/<persona_id>/` 加载 persona 文件：`crates/config/src/loader.rs:345`、`crates/gateway/src/chat.rs:4265`
- Channels UI 已支持为 Telegram bot 设置 `persona_id`（但仅是“绑定”，不是“编辑 persona profile”）：`crates/gateway/src/assets/js/page-channels.js:318`

**Review Notes（2026-02-24）**
- 本单无明显逻辑矛盾；属于“管理/可观测性补齐”类体验工作。
- “默认 persona 迁移到 `personas/default`”会牵扯现有 Settings/Onboarding 的 identity/soul 编辑 RPC：当前 UI 明确写入 workspace root（`IDENTITY.md`/`SOUL.md`），若读取口径改为 `personas/default/*`，必须同步调整写入端，否则会出现“UI 改了但不生效”的错觉（见证据）。
- Telegram session label 目前用户侧不可重命名（UI 禁止 rename channel sessions），因此要解决重复 `Telegram 1`，必须走“服务端生成/修正 label”的路径（见证据）。

**已知痛点（本单要解决）**
1) UI 没有 persona profile（`persona_id` 对应的 `IDENTITY/SOUL/TOOLS/AGENTS`）编辑器
2) 全局 persona profile 仍散落在 `~/.moltis/{IDENTITY,SOUL,TOOLS,AGENTS}.md`，希望口径收敛到 `~/.moltis/personas/default/`
3) Channels 列表/详情里，`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id` 信息展示不够显性
4) Session 列表标题（label）混乱：存在大量重复的 `Telegram 1`，无法快速分辨不同 bot / chat

---

## 背景（Background）
在落地“按 Telegram bot identity 绑定 persona_id”之后，Moltis 已具备多 persona 的运行基础，但在“管理与可观测性”上仍有明显缺口：
- 只能在 UI 里把 bot 绑定到一个 `persona_id`，但 persona 内容必须手改文件，缺乏“可发现/可编辑/可校验”的管理入口。
- 默认 persona 的存储口径分裂：既有 `~/.moltis/*.md` 又有 `~/.moltis/personas/<id>/*.md`，长期会导致排障困难与配置漂移。
- Channels/Session 两个视图缺少“关键身份字段”的显性展示，导致多 bot 场景下难以确认：当前 bot 是谁、绑定了哪个 persona、当前会话属于哪个 bot/chat。

Out of scope（本单不做）：
- 不做复杂的富文本/版本控制/多人协作编辑器（先最小可用）
- 不做兼容/迁移保底（你已明确“不在乎迁移与兼容，可清空旧数据”）

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **persona_id**：persona profile 的 ID（目录名），例如 `default` / `ops`
- **data_dir**：Moltis 数据目录，默认是 `~/.moltis`（可通过 CLI/env 覆盖）。
- **Persona Profile**：`{data_dir}/personas/<persona_id>/{IDENTITY,SOUL,TOOLS,AGENTS}.md`
- **默认 Persona**：`persona_id=default`；建议将其作为“唯一全局默认口径”（即 `{data_dir}/personas/default/*`）
- **Telegram identity fields**：
  - `account_id`：稳定句柄（本项目现状：`telegram:<chan_user_id>`）
  - `chan_user_id`：`getMe.id`（数字，稳定）
  - `chan_user_name`：`getMe.username`（不带 `@`，可变）
  - `chan_nickname`：显示名（可变）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] Persona Profiles UI：
  - [ ] UI 可列出本地可用 persona（扫描 `~/.moltis/personas/*`）
  - [ ] UI 可创建/删除/编辑 persona（至少编辑 4 个文件：`IDENTITY/SOUL/TOOLS/AGENTS`）
  - [ ] UI 能提示 persona_id 合法性（仅允许 `[A-Za-z0-9_-]{1,64}`，与 loader 校验一致）
- [ ] 默认 persona 口径收敛：
  - [ ] 将“全局 persona profile”口径收敛为 `~/.moltis/personas/default/`
  - [ ] 系统读取默认 persona 时应优先读 `personas/default/*`（可选择直接废弃 root `~/.moltis/*.md`，或短期 fallback）
- [ ] Channels 信息显性化：
  - [ ] 在 Channels UI 中显性展示：`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`
  - [ ] 提供“复制”按钮（至少对 `account_id`、`chan_user_id`、`session_key`）
- [ ] Session 标题（label）改造：
  - [ ] Telegram channel session 的 label 必须包含可区分信息（至少包含 bot 身份 + chat_id）
  - [ ] 避免不同 bot 的首个会话都叫 `Telegram 1` 这类重复标签

### 非功能目标（Non-functional）
- 可观测性：UI 一眼能确认“这个会话属于哪个 bot/chat、绑定哪个 persona”
- 安全与隐私：persona 编辑只允许操作 `~/.moltis/personas/` 下文件；避免路径穿越；避免在 UI/日志中泄露 token 等 secrets
- 可测试性：核心格式化/选择逻辑有单元测试；UI 至少有基本 smoke（可选 E2E）

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) UI 只能绑定 `persona_id`，无法在 UI 编辑 persona 内容（只能手改文件）。
2) 默认 persona 的文件来源分裂（root vs personas/default），使用者难判断“哪份生效”。
3) Channels UI 信息太隐蔽：添加 bot 后难以快速确认它的 `account_id/chan_user_*` 与 persona 绑定。
4) Session 列表 label 重复：多 bot 场景下出现多个 `Telegram 1`，难以选择/排障。

### 影响（Impact）
- 用户体验：多 bot/多用途下“我在跟谁说话/它是谁”不清晰
- 排障成本：无法从 UI 快速确认绑定与生效口径
- 配置可维护性：长期容易出现 persona 文件漂移与误编辑

### 复现步骤（Reproduction）
1. 连接两个 Telegram bots（两个不同 token），并分别绑定不同 `persona_id`（在 Channels 页面）。
2. 观察 Channels 列表：难以快速确认每个 bot 的 `chan_user_*` 与绑定 persona（信息不显性）。
3. 让两个 bots 分别在各自 chat 里触发新会话（各发一条消息）。
4. 观察 Session 列表：出现多个 `Telegram 1`（跨 bot 重复），且 Telegram sessions 不可在 UI 重命名，难以区分。
5. 在 Settings/Onboarding 修改 identity/soul 后，若默认 persona 口径改为 `personas/default/*` 而写入仍落在 root，会出现“改了不生效”的错觉。

## 现状核查与证据（As-is / Evidence）【不可省略】
- persona 绑定入口存在，但仅是输入框：`crates/gateway/src/assets/js/page-channels.js:318`（Add modal）、`crates/gateway/src/assets/js/page-channels.js:436`（Edit modal）
- Onboarding 的 Connect Telegram step 不支持设置 `persona_id`（仅 token/DM allowlist）：`crates/gateway/src/assets/js/onboarding-view.js:1959`
- persona profile 文件加载路径当前是“personas/<id> + root fallback”的组合：`crates/config/src/loader.rs:345`
- Channels 列表卡片目前不展示 `chan_user_*` 与 persona_id：`crates/gateway/src/assets/js/page-channels.js:60`
- Telegram session label 目前由服务端写入 `Telegram {n}`（对每个 account 独立计数，因此跨 bot 会重复）：`crates/gateway/src/chat.rs:6456`、`crates/gateway/src/channel_events.rs:155`
- SessionList/SessionHeader 直接展示 `session.label || session.key`：`crates/gateway/src/assets/js/components/session-list.js:165`、`crates/gateway/src/assets/js/components/session-header.js:31`
- Telegram sessions 在 UI 被视作 channel sessions，禁止 rename（因此只能靠服务端生成可读 label）：`crates/gateway/src/assets/js/components/session-header.js:37`
- Settings UI 的 identity/soul 目前明确写入 workspace root（`IDENTITY.md`/`SOUL.md`），与“默认 persona 收敛到 `personas/default`”存在写入端联动风险：`crates/gateway/src/assets/js/page-settings.js:408`、`crates/gateway/src/assets/js/page-settings.js:457`
- 对应保存端实现写入 root 文件：`crates/config/src/loader.rs:503`（`save_soul`）、`crates/config/src/loader.rs:522`（`save_identity`）

## 根因分析（Root Cause）
- A) Persona 是“文件体系”，但缺少 UI 管理层（CRUD/校验/预览/回滚）。
- B) 默认 persona 没有“单一 canonical 位置”，导致“生效来源”不直观。
- C) Channel identity fields 虽然已存/可获取，但 UI 未做显性展示与复制能力。
- D) Telegram session label 生成策略缺乏“bot identity + chat identity”的信息量，导致重复且不可区分。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - Persona profile 能在 UI 中被发现、编辑、创建（最小 4 文件 + identity frontmatter）
  - `persona_id=default` 有清晰的单一来源（优先 `personas/default/*`）
  - Channels UI 显性展示关键 identity 与 persona 绑定
  - Telegram session label 唯一可分辨（至少 bot + chat）
- 不得：
  - UI 编辑器允许写入 `~/.moltis/personas/` 以外的任意路径
  - UI/日志泄露 Telegram token
- 应当：
  - label 生成规则稳定、可预测（便于搜索与脚本化）

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（推荐）：保持“文件即事实”，在 UI 上加最小管理层 + 口径收敛
- Persona Profiles UI：
  - 新增 Settings → Personas（或侧栏单独入口）
  - 列表：`personas/*`；操作：Create/Clone/Delete/Edit
  - Edit：四个 Markdown 文本域（`IDENTITY/SOUL/TOOLS/AGENTS`），另提供 Identity 的结构化编辑（name/emoji/creature/vibe）同步写入 YAML frontmatter
- 默认 persona：
  - 读取顺序：优先 `personas/default/*`，其次（可选）fallback 到 root `~/.moltis/*.md`
  - 可配置：提供“一键迁移/复制 root → personas/default”（你不要求兼容也可直接不做迁移，只做新口径）
- Channels UI：
  - channel 卡片增加一个“Details”折叠区：显示上述 5 个字段 + copy
- Sessions label：
  - 新 label 规范建议（示例）：`TG @{chan_user_name} · {chat_id}` 或 `TG {chan_user_id} · {chat_id}`
  - 若 `account_handle`（@username）可用，优先展示；否则回退 `account_id`
  - label 写入点：创建绑定/ensure_channel_bound_session 时生成一次并 upsert

优点：实现量可控，符合“可删旧数据”的前提；不会引入 DB schema 大改。  
风险：UI 文本编辑容易写坏，需要基础校验/预览；需要约束写路径。

#### 方案 2：persona profile 入库（SQLite），UI 管理一等化
- Persona 作为结构化实体（含版本、审计、回滚），加载时从 DB 拼 prompt
优点：更强的管理与回滚；缺点：实现复杂、侵入性强、与现有文件体系割裂。

### 最终方案（Chosen Approach）
- 采用方案 1（文件为真值 + UI 最小管理层）

#### 接口与数据结构（Contracts）
- RPC（建议新增）：
  - `personas.list`：列出 persona_id 与文件存在性
  - `personas.get`：读取 persona 的 4 文件（及 identity frontmatter）
  - `personas.save`：保存 persona 的 4 文件（强制写入 `personas/<id>/...`）
  - `personas.delete`：删除 persona（禁止删除 `default`，或需二次确认）
- 现有 `channels.add/update`：继续用 `config.persona_id` 绑定

#### 失败模式与降级（Failure modes & Degrade）
- persona 文件缺失：UI 显示缺失；加载时回退到 root 或空（按你最终口径冻结）
- persona 保存失败：返回明确错误（权限/非法 id/写入失败）
- label 改造的存量 session：允许维持旧 label；提供一次性“重命名 channel sessions”工具（可选）

#### 安全与隐私（Security/Privacy）
- personas RPC 只允许读写 `~/.moltis/personas/` 下固定文件名集合
- UI 不显示 token；编辑器不应触达 token

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] UI 能创建/编辑 `personas/default`，并能绑定到 Telegram bot
- [ ] 默认 persona 生效口径清晰（优先 `personas/default`；root 是否 fallback 需冻结）
- [ ] Channels 页面可一眼看到并复制：`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`
- [ ] Telegram sessions 不再出现多 bot 重复的 `Telegram 1`（label 可分辨）

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] persona_id 合法性校验与路径约束（与 loader 一致）
- [ ] Telegram session label 生成：给定 `account_id/handle/chat_id` 断言格式稳定且包含 bot+chat

### Integration
- [ ] `channels.add` 后 `channels.status` 返回 payload 中包含上述字段，UI 能渲染（可用 snapshot test 或最小契约测试）

### UI E2E（如适用）
- [ ] 创建 persona → 绑定到 bot → 发消息 → 在请求 dump 中出现 `# Persona: <id>`（若有抓包/trace 面板）

## 发布与回滚（Rollout & Rollback）
- 发布策略：先做“只读 Personas 列表 + 编辑 default”，再扩展 CRUD；可加实验开关
- 回滚策略：保留 root fallback（如你愿意）可作为临时回滚；或直接删功能但保留既有文件

## 实施拆分（Implementation Outline）
- Step 1: Personas RPC（list/get/save/delete）+ 严格路径约束
- Step 2: Settings UI 增加 Personas 页面（最小编辑器 + identity frontmatter 编辑）
- Step 3: config loader 默认 persona 优先读 `personas/default`
- Step 4: Channels UI 增强展示（details + copy）
- Step 5: Telegram session label 规则升级（新会话写入；存量可选批量修复）

## 交叉引用（Cross References）
- 已实现的“按 bot 绑定 persona_id + OpenAI developer preamble”issue：`issues/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md`
- 早期设计备档（多 persona / per-session profiles）：`issues/issue-named-personas-and-per-session-agent-profiles.md`

## 未决问题（Open Questions）
- Q1：默认 persona 是否彻底废弃 root `~/.moltis/*.md`？
  - A) 彻底废弃（更干净，但可能需要一次性迁移）
  - B) 短期 fallback（更稳妥，但口径会更复杂）
- Q2：是否允许删除非 default persona？删除后绑定到该 persona 的 bot 如何降级？
- Q3：Session label 改造是否需要对历史 session 做批量重命名，还是只影响新创建的 channel session？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（默认 persona/label/展示口径一致）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 安全隐私检查通过（路径穿越/敏感字段不泄露）
- [ ] 回滚策略明确

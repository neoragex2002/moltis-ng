# Issue: Persona Profiles UI 管理 + `personas/default` 口径收敛 + Channels/Session 信息显性化

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-24）
- Priority: P2（体验/可控性增强；不阻断核心对话）
- Components: gateway / config / sessions metadata / Web UI / channels(telegram)
- Affected providers/models: all（主要是 UI/配置/会话元数据；与 provider 无强耦合）

**已实现（2026-02-24）**
- 默认 persona canonical 路径收敛到 `<data_dir>/personas/default/*`（无 root fallback）：`crates/config/src/loader.rs:266`
- Settings → General：新增 `Personas` 与 `Owner`（替换原 Identity 导航入口）：`crates/gateway/src/assets/js/page-settings.js:69`
- Personas CRUD（list/get/save/delete/clone）+ `default` 不可删除 + persona_id 校验：`crates/gateway/src/personas.rs:47`、`crates/gateway/src/methods.rs:1253`
- Owner（`USER.md`）全局编辑 RPC + UI：`crates/gateway/src/owner.rs:3`、`crates/gateway/src/assets/js/page-settings.js:847`
- `PEOPLE.md` 自动生成（channels.add/update/remove 后 best-effort；不含 secrets）：`crates/gateway/src/people.rs:46`、`crates/gateway/src/channel.rs:214`
- Channels：显性化展示并可复制 `account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`，并提示 persona 缺失降级：`crates/gateway/src/assets/js/page-channels.js:144`
- Telegram session label（新会话）：`crates/gateway/src/session_labels.rs:13`、`crates/gateway/src/chat.rs:6453`
- Prompt：`PEOPLE.md` 引用改为 data_dir 口径（不再硬编码 `~/.moltis/PEOPLE.md`）：`crates/agents/src/prompt.rs:123`
- Review fix：Personas UI frontmatter 解析修正（避免结构化字段读取为空导致保存时覆写 frontmatter）：`crates/gateway/src/assets/js/identity-frontmatter.js:22`
- Review fix：Channels UI persona 缺失提示仅在 persona 列表可靠加载后展示（避免加载中/失败时的误报）：`crates/gateway/src/assets/js/page-channels.js:86`
- Review fix：忽略本地 prompt-dump artifacts，避免误提交：`.gitignore:14`

**已覆盖测试（2026-02-24）**
- default persona canonical 路径：`crates/config/src/loader.rs:1316`
- Personas CRUD + default 不可删：`crates/gateway/src/personas.rs:169`
- Owner（USER.md）读写：`crates/gateway/src/owner.rs:26`
- PEOPLE.md 生成不泄露 token：`crates/gateway/src/people.rs:61`
- Telegram session label：`crates/gateway/src/channel_events.rs:1864`、`crates/gateway/src/chat.rs:9884`
- Onboarding 写入落点更新（`personas/default/*`）：`crates/onboarding/src/service.rs:347`
- Prompt 引用 `/moltis/data/PEOPLE.md`：`crates/agents/src/prompt.rs:34`
- Personas frontmatter 解析/写回回归测试（Node）：`crates/gateway/src/assets/js/identity-frontmatter.test.mjs:10`
- Channels persona 缺失提示逻辑回归测试（Node）：`crates/gateway/src/assets/js/persona-utils.test.mjs:6`

**已知差异/后续优化（非阻塞）**
- Personas 编辑器当前是“纯文本为主”（textarea），无 YAML/Markdown 校验与预览。
- `default` persona 的 seeded 文本是最小模板（后续可按需收敛为更稳定、更一致的默认内容）。
- `crates/gateway/src/server.rs` 仍会 seed `<data_dir>/AGENTS.md` 与 `<data_dir>/TOOLS.md`（历史遗留；当前默认 persona 不再读取该路径）。

**已冻结口径**
- Q1（default persona canonical 来源）：选择 A）彻底废弃 root `~/.moltis/*.md`，以 `{data_dir}/personas/default/*` 为唯一真值（不做 fallback）。
- Q2（Telegram session label 存量处理）：选择“只影响新会话”；不考虑/不保留既有存量 sessions（可直接清空旧配置与数据库后重新接入）。
- Q3（全局文件范围）：`USER.md` / `PEOPLE.md` 全局一份；不做 per-persona/per-bot 拆分（已确认）。
- Q4（默认 persona ID 与删除策略）：默认 persona id 固定为 `default`（不使用 `main` 等别名）；`default` 不能被删除。
- Q5（persona 删除降级）：允许删除非 `default` persona；若某 bot/session 绑定的 persona 缺失，则运行时自动降级到 `default`（并在 UI 给出提示）。

**已知痛点（本单要解决）**
1) UI 没有 persona profile（`persona_id` 对应的 `IDENTITY/SOUL/TOOLS/AGENTS`）编辑器
2) 全局 persona profile 仍散落在 `{data_dir}/{IDENTITY,SOUL,TOOLS,AGENTS}.md`，希望口径收敛到 `{data_dir}/personas/default/`
3) Channels 列表/详情里，`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id` 信息展示不够显性
4) Session 列表标题（label）混乱：存在大量重复的 `Telegram 1`，无法快速分辨不同 bot / chat

---

## 背景（Background）
在落地“按 Telegram bot identity 绑定 persona_id”之后，Moltis 已具备多 persona 的运行基础，但在“管理与可观测性”上仍有明显缺口：
- 只能在 UI 里把 bot 绑定到一个 `persona_id`，但 persona 内容必须手改文件，缺乏“可发现/可编辑/可校验”的管理入口。
- 默认 persona 的存储口径分裂：既有 `{data_dir}/*.md` 又有 `{data_dir}/personas/<id>/*.md`，长期会导致排障困难与配置漂移。
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
- [x] Persona Profiles UI：
  - [x] Settings → General 下新增 `Personas` 栏目，替换掉现有 `Identity` 栏目（UI 入口层级不变，只换内容/名称）。
  - [x] `Personas` 页面布局：沿用现有 Identity 页面风格（简单、少层级）。
  - [x] 支持 list/create/delete/clone：
    - [x] list：下拉框选择 persona
    - [x] create：输入 persona_id，创建目录与默认文件
    - [x] delete：删除 persona（默认禁止删 `default`，或需二次确认）
    - [x] clone：从当前 persona 复制到新 persona_id
  - [x] 编辑区域随下拉 persona 切换而变化（无需复杂多标签 UI）。
  - [x] 保存对象：`{data_dir}/personas/<id>/{IDENTITY,SOUL,TOOLS,AGENTS}.md`
  - [x] persona_id 校验：仅允许 `[A-Za-z0-9_-]{1,64}`（与 loader 一致）
- [x] 默认 persona 口径收敛：
  - [x] 将“全局 persona profile”口径收敛为 `{data_dir}/personas/default/`
  - [x] 系统读取/写入默认 persona 时只使用 `personas/default/*`（不做 root fallback；已冻结）
- [x] Channels 信息显性化：
  - [x] 在 Channels UI 中显性展示：`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`
  - [x] 提供“复制”按钮（至少对 `account_id`、`chan_user_id`）
- [x] Session 标题（label）改造：
  - [x] Telegram channel session 的 label 必须包含可区分信息（至少包含 bot 身份 + chat_id）
  - [x] 避免不同 bot 的首个会话都叫 `Telegram 1` 这类重复标签
- [x] Owner（USER.md）拆分：
  - [x] 将现有 Identity 页面中的 user 字段拆出，在 General 下新增 `Owner` 栏目
  - [x] `Owner` 页面专门编辑 `{data_dir}/USER.md`（全局一份；已冻结）
- [x] PEOPLE.md 自动生成：
  - [x] `{data_dir}/PEOPLE.md` 全局一份（已冻结）
  - [x] 自动根据已配置 channels/bots 生成（不包含 secrets，如 token）
  - [x] 生成触发：channels.add/update/remove 时 best-effort 刷新（无需兼容旧数据）

### 非功能目标（Non-functional）
- 可观测性：UI 一眼能确认“这个会话属于哪个 bot/chat、绑定哪个 persona”
- 安全与隐私：persona 编辑只允许操作 `{data_dir}/personas/` 下文件；避免路径穿越；避免在 UI/日志中泄露 token 等 secrets
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
  - `persona_id=default` 有清晰的单一来源：仅 `{data_dir}/personas/default/*`（无 root fallback）
  - Channels UI 显性展示关键 identity 与 persona 绑定
  - Telegram session label 唯一可分辨（至少 bot + chat）
- 不得：
  - UI 编辑器允许写入 `{data_dir}/personas/` 以外的任意路径
  - UI/日志泄露 Telegram token
- 应当：
  - label 生成规则稳定、可预测（便于搜索与脚本化）

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（推荐）：保持“文件即事实”，在 UI 上加最小管理层 + 口径收敛
- Persona Profiles UI：
  - 新增 Settings → General → Personas（替换原 Identity）
  - 列表：`personas/*`；操作：Create/Clone/Delete/Edit
  - Edit：四个 Markdown 文本域（`IDENTITY/SOUL/TOOLS/AGENTS`），另提供 Identity 的结构化编辑（name/emoji/creature/vibe）同步写入 YAML frontmatter
- Owner UI（USER.md）：
  - 新增 Settings → General → Owner（从原 Identity 拆出）
  - 仅编辑 `{data_dir}/USER.md`（全局一份）
- 默认 persona：
  - canonical：仅使用 `{data_dir}/personas/default/*`（不做 root fallback；不做兼容迁移；按需直接废弃旧数据）
- Channels UI：
  - channel 卡片增加一个“Details”折叠区：显示上述 5 个字段 + copy
- PEOPLE.md：
  - 在 channels.add/update/remove 后 best-effort 自动生成 `{data_dir}/PEOPLE.md`（全局一份；不含 secrets）
- Sessions label：
  - 新 label 规范（已采纳）：
    - group chat（`chat_id < 0`）：`TG @{chan_user_name} · grp:{chat_id}`
    - dm（`chat_id > 0`）：`TG @{chan_user_name} · dm:{chat_id}`
    - fallback：缺失 `chan_user_name` 时用 `chan_user_id` 替代（`TG {chan_user_id} · grp|dm:{chat_id}`）
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
- persona 缺失/被删除：
  - 运行时自动降级到 `default`（不回退到 root；按冻结口径）
  - UI 提示“missing → default”（避免静默漂移）
- `default` persona 文件缺失：
  - best-effort 自动创建 `{data_dir}/personas/default/` 与 4 个文件（允许使用默认内容/空模板）
  - 若创建失败：在 UI 与日志给出明确错误（权限/磁盘写入失败）
- persona 保存失败：返回明确错误（权限/非法 id/写入失败）
- label 改造的存量 session：不处理（你允许直接清空旧配置/DB 后重新接入）

#### 安全与隐私（Security/Privacy）
- personas RPC 只允许读写 `{data_dir}/personas/` 下固定文件名集合
- UI 不显示 token；编辑器不应触达 token

## 验收标准（Acceptance Criteria）【不可省略】
- [x] UI 能 list/create/delete/clone persona，并可编辑 `personas/default`
- [x] 默认 persona 生效口径清晰：仅 `{data_dir}/personas/default/*`（无 root fallback）
- [x] `Owner` 页面可编辑 `{data_dir}/USER.md`（且不影响 persona 管理；USER.md 仍全局一份）
- [x] `{data_dir}/PEOPLE.md` 能自动生成（channels 变更后刷新；不包含 secrets）
- [x] Channels 页面可一眼看到并复制：`account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`
- [x] 新创建的 Telegram sessions 不再出现多 bot 重复的 `Telegram 1`（label 可分辨：含 bot + dm/grp + chat_id）

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] persona_id 合法性校验与路径约束：`crates/gateway/src/personas.rs:169`
- [x] personas CRUD（create/clone/delete；禁止删除 `default`）：`crates/gateway/src/personas.rs:180`
- [x] 默认 persona canonical 路径（`<data_dir>/personas/default/*`）：`crates/config/src/loader.rs:1316`
- [x] Telegram session label 生成：`crates/gateway/src/chat.rs:9884`
- [x] PEOPLE.md 生成且不泄露 token：`crates/gateway/src/people.rs:61`

### Integration
- [x] `channels.add` 后 `channels.status` 返回 payload 中包含上述字段，UI 能渲染（可用 snapshot test 或最小契约测试）
- [x] Settings：Personas/Owner 页面基本交互（保存后 reload 生效）

### UI E2E（如适用）
- [x] 创建 persona → 绑定到 bot → 发消息 → 在请求 dump 中出现 `# Persona: <id>`（若有抓包/trace 面板）

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：当前未引入 Web UI 的 Playwright E2E；本单以 unit 覆盖为主。
- 手工验收步骤（建议在清空存量数据后执行；你允许直接删除旧 <data_dir>/DB）：
  1. 可选：清空 `data_dir`（默认 `~/.moltis`；也可能由 `MOLTIS_DATA_DIR` 指向）。
     - UI 配置项：如需自定义 data_dir，可在启动前设置环境变量 `MOLTIS_DATA_DIR=/path/to/data_dir`（或使用 CLI 参数/配置中的 data_dir 覆盖方式，按你的部署习惯）。
  2. 启动：`moltis gateway`，在日志中找到 “listening on …” 的 URL 并打开 Web UI。
  3. Settings → General → Personas：
     - 选择 `default`，修改 frontmatter（name/emoji/creature/vibe）与 4 个文本域，点击 Save。
     - 若 `IDENTITY.md` 已存在 frontmatter：结构化字段应自动回填；直接 Save 不应清空原有 frontmatter。
     - Create：输入 `ops`（示例），确认 clone 自 `default` 成功，dropdown 出现 `ops`。
     - Clone：将 `ops` clone 为 `ops2`，验证内容一致。
     - Delete：删除 `ops2` 成功；尝试删除 `default` 应被拒绝。
     - 文件落点：确认对应文件写入 `<data_dir>/personas/<persona_id>/{IDENTITY,SOUL,TOOLS,AGENTS}.md`。
  4. Settings → General → Owner：
     - 编辑 USER.md 保存并 Reload，确认内容持久化。
     - 文件落点：确认写入 `<data_dir>/USER.md`。
  5. Channels：
     - 新增 Telegram bot（token + allowlist 等），确认卡片出现 Details 并可复制：
       `account_id/chan_user_id/chan_user_name/chan_nickname/persona_id`。
     - 设置 `persona_id=ops` 并保存；若 `ops` 不存在，应显示 “(missing → default)” 提示。
     - 在 `personas.list` 未加载成功/为空时，不应出现短暂的 “(missing → default)” 误报。
     - 交互：Details 的 Copy 按钮点击后应出现 toast（copied / Copy failed）。
  6. PEOPLE.md：
     - 在 data_dir 下检查 `PEOPLE.md` 被生成/刷新，且不包含 Telegram token。
     - 交互：对 bot 的 add/update/remove 后刷新文件内容应同步变化（best-effort；允许失败时仅在日志 warn）。
  7. Sessions（Telegram）：
     - 与 bot 发起一个新的 DM 会话、以及一个新的 group/supergroup 会话；
     - Sessions 列表中，新会话 label 应为 `TG @<bot_username> · dm:<chat_id>` / `TG @<bot_username> · grp:<chat_id>`（缺 username 时回退为 `TG <chan_user_id> · ...`），不再出现大量重复的 `Telegram 1`。
  8. 本地安全（防误提交）：
     - 在仓库根目录执行 `git status --porcelain`，不应看到 prompt-dump artifacts（例如 `issues/background.md` / `issues/Codex*Prompt*Dump*.md`）被纳入改动（已在 `.gitignore` 忽略）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：按步骤落地（先 canonical default + 写入端，再 UI/RPC，再 session label/channels 展示）
- 回滚策略：git revert（你不要求兼容/迁移；允许清空旧数据）

## 实施拆分（Implementation Outline）
- Step 1: 冻结 `default` canonical 路径（读/写都指向 `{data_dir}/personas/default/*`；废弃 root）
- Step 2: 增加 Personas/Owner 所需 RPC + 保存端（含 `AGENTS/TOOLS/PEOPLE` 的受控写入）
- Step 3: Settings UI：General 下 `Personas` 替换 `Identity`；新增 `Owner`（USER.md）
- Step 4: 自动生成 `{data_dir}/PEOPLE.md`（channels 变更后 best-effort 刷新；不含 secrets）
- Step 5: Channels UI 显性化展示（details + copy）
- Step 6: Telegram session label 规则升级（仅新会话；写入时包含 bot + dm/grp + chat_id）

## 交叉引用（Cross References）
- 已实现的“按 bot 绑定 persona_id + OpenAI developer preamble”issue：`issues/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md`
- 早期设计备档（多 persona / per-session profiles）：已废弃（原文件 `issues/issue-named-personas-and-per-session-agent-profiles.md` 已删除；实现以本单与 `issues/done/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md` 为准）

## 未决问题（Open Questions）
None (frozen for implementation).

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（默认 persona/label/展示口径一致）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 安全隐私检查通过（路径穿越/敏感字段不泄露）
- [x] 回滚策略明确

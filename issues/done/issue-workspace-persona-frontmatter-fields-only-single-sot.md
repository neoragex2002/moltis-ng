# Issue: Workspace/People 配置治理：公共 USER/PEOPLE + 私有 people/<name> + 字段/正文分离 + 单一 SOT（workspace / people / config）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Owners: luy
- Components: config/loader, tools/sandbox, onboarding, gateway/methods+server+ui, agents/prompt
- Affected providers/models: <N/A>

**已实现（2026-03-03）**
- 私有目录统一为 `people/<name>/...`，并在启动时 seed `people/default`：`crates/config/src/loader.rs:302`、`crates/gateway/src/server.rs:913`
- `PEOPLE.md` seed + 启动时同步修复（仅对齐 `emoji/creature`，正文保持不变）：`crates/config/src/loader.rs:351`
- `moltis.toml` persona/资料字段退场（`[identity]` / `[user]` / `identity.soul`）：`crates/config/src/schema.rs:182`
- Gateway RPC：`workspace.user.get/update`（字段级更新、正文只读）：`crates/gateway/src/methods.rs:1285`、`crates/gateway/src/user.rs:39`
- Gateway RPC：`workspace.people.get/updateEntry/sync`（`emoji/creature` 只读 + 自动对齐）：`crates/gateway/src/methods.rs:1307`、`crates/gateway/src/people.rs:213`
- UI：新增 Settings `User`/`People` 字段表单 + 正文只读；移除 raw 编辑入口：`crates/gateway/src/assets/js/page-settings.js:499`
- Sandbox（bind mount）：容器内仅暴露公共 `USER.md`/`PEOPLE.md`（`.sandbox_views/<sandbox_key>`）：`crates/tools/src/sandbox.rs:340`
- Prompt 口径更新：`PEOPLE.md` 不再是“系统整文件生成禁改”，而是“字段可维护 + emoji/creature 自动对齐 + 正文手工”：`crates/agents/src/prompt.rs:60`

**已覆盖测试（如有）**
- PEOPLE 同步：保留正文 + 不覆盖其它字段：`crates/config/src/loader.rs:1914`
- PEOPLE 同步：identity 清空 emoji 时自动移除：`crates/config/src/loader.rs:1977`
- USER 字段更新：保留正文：`crates/gateway/src/user.rs:100`
- PEOPLE 字段更新：保留正文：`crates/gateway/src/people.rs:306`
- Sandbox public view：仅复制 USER/PEOPLE：`crates/tools/src/sandbox.rs:2940`

**已知差异/后续优化（非阻塞）**
- Sandbox 隐私边界当前仅对 `data_mount_type=bind` 生效；`volume` 挂载无法做文件级裁剪（需后续另议）。
- Settings/Identity 页面仍提供 owner name 的快捷编辑（会写 `USER.md` 字段）；属于重复入口但不破坏 SOT。
- `spawn_agent` 仍支持显式 `persona_id`（对应 `people/<name>`）；若要做到“任何 agent 均无法通过工具间接读取其它 agent 私有文件”，需另行引入更严格的 tool policy/审批机制。

---

## 背景（Background）
### 全局分类（决策背景）
用户将配置/数据文件分 4 类（本 issue 仅治理第 4 类）：
1) **系统配置**：唯一官方格式为 `moltis.toml`（本 issue 不改动系统配置格式/解析）
2) **heartbeat + cron**（本 issue 不涉及；`HEARTBEAT.md` 归入第 2 类）
3) **skills/hooks**（本 issue 不涉及）
4) **workspace/people（重点）**：持久化资料/人格/通信录，供 prompt/UI 使用

### 本 issue 的核心治理目标（用户明确）
- `USER.md` / `PEOPLE.md` / `people/<name>/IDENTITY.md` 统一采用：**YAML frontmatter 字段 + Markdown 正文**。
- 代码必须提供**前端字段修改能力**（UI 编辑 frontmatter）。
- 当 UI/后端更新字段时，**不得修改正文**（正文只能手工编辑；禁止 UI 保存正文）。
- 长期口径：同一信息必须收敛到**单一 SOT**（Single Source of Truth），避免多处来源 merge。
  - 特别是：`moltis.toml [identity]` 不应继续作为 identity 的来源/写入目标；默认 agent 的 identity 应以 **`people/default`** 为准（而不是“toml + 文件 overlay”）。
  - 决策：`moltis.toml` 中所有 persona/资料相关字段必须**退场删除**（避免双 SOT）：
    - `[identity]`（`name/emoji/creature/vibe`）
    - `[user]`（`name/timezone/location` 等 user profile）
    - `identity.soul`（当前为“挂名字段”，应一并移除）

Out of scope：
- Hook/Skill 的 frontmatter+正文机制暂不动。
- heartbeat/cron 的存储与治理暂不动。
- 不在本 issue 内讨论“是否移除 YAML frontmatter 机制本身”（本 issue 的前提是保留 YAML 字段 + 正文的双段结构，但要治理写入边界与 SOT）。

---

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 正文统一用“字段/正文”，不使用“formatter”。

- **字段（frontmatter）**（主称呼）：文件开头 `--- ... ---` 内的 YAML key/value，仅用于结构化读写（UI/RPC 可更新）。
  - Not：不包含正文提示词/长文本规则。
  - Source/Method：authoritative（以文件为准）

- **正文（body）**（主称呼）：frontmatter 之后的 Markdown 内容，仅供人类手工编辑与 prompt 注入（只读，不可由 UI/RPC 写入）。
  - Source/Method：authoritative（以文件为准）

- **单一 SOT**（主称呼）：同一语义字段（如 `identity.name`）只允许由一个来源决定；其它来源只能做一次性迁移/兼容读取，不能形成长期 overlay。

---

## 配置治理方案（Type 4 Spec / 用户决策版）【供审阅：文件/格式/字段/用途/可见性】
> 本节是最终 Spec（尽量冻结）。后续变更优先补实现与测试，不频繁改 Spec。

### 总体模型（公共 vs 私有）
- 公共文件（所有 agent 均可读）：
  - `<data_dir>/USER.md`：系统主人/主要管理员信息（告诉各 agent “我的主人是谁”）。
  - `<data_dir>/PEOPLE.md`：通信录（告诉 agent “系统里除了自己还有谁” + 对外联系信息）。
- 私有目录（仅该 agent 自己可读）：
  - `<data_dir>/people/<name>/**`：该 agent 的自我定义（用于自己拼 prompt），属于隐私边界；其它 agent 不应读取。

### 目录结构（目标态）
- `<data_dir>/USER.md`（公共；字段 + 正文）
- `<data_dir>/PEOPLE.md`（公共；字段 + 正文）
- `<data_dir>/people/<name>/`（私有；每个 agent 一个目录）
  - `IDENTITY.md`（私有；字段 + 正文）
  - `SOUL.md`（私有；正文）
  - `TOOLS.md`（私有；正文）
  - `AGENTS.md`（私有；正文）

**目录结构示例（最小可运行）**
```text
<data_dir>/
  USER.md
  PEOPLE.md
  people/
    default/
      IDENTITY.md
      SOUL.md
      TOOLS.md
      AGENTS.md
```

### `<name>`：目录名/稳定键（仅英文/ASCII）
> 用户决策：`<name>` 仅英文名（ASCII），作为目录名与稳定键；中文/花名只作为展示名（`display_name`），不得作为目录名。

- 规则（实现应当保持简单清晰；建议直接做 hard reject 而非复杂 normalize）：
  - `name` 必须匹配：`^[a-z0-9][a-z0-9_-]{0,63}$`（全小写，长度 ≤ 64）
  - 必须拒绝：空、`.`、`..`、包含 `/` 或 `\\`、控制字符/换行/NUL
  - UI/接口侧建议：对输入做 `trim()`，并在校验失败时给出明确错误信息（例如 “name 仅允许 a-z0-9_- 且必须小写”）。
- 一致性硬规则（必须）：
  - 目录名：`people/<name>/...`
  - `people/<name>/IDENTITY.md` 的字段：`name: <name>`
  - `PEOPLE.md` 的字段：`people[].name == <name>`

### 默认 agent：`people/default`（必须存在）
- 决策：必须保留 `people/default/`，用于系统默认 agent：
  - spawn agent 的默认人格
  - main 会话机器人、web ui 会话机器人等默认会话的 prompt 拼装
- 规范：
  - 启动时必须确保 `people/default/` 与 4 个文件存在（seed）。
  - 任何未显式指定 `name`/agent 的地方，必须以 `default` 为准。

### 文件格式与字段规范（逐文件）
#### 1) `USER.md`（公共；维持现状）
- 路径：`<data_dir>/USER.md`
- 格式：YAML frontmatter（字段）+ Markdown（正文）
- 字段（现有解析/写回的受管字段；字段更新不得改正文）：
  - `name`：系统主人/主要管理员展示名
  - `timezone`：时区（IANA，如 `Asia/Shanghai`）
  - `latitude` / `longitude`：地理坐标（可选）
  - `location_place`：地点文字描述（可选）
  - `location_updated_at`：更新时间（Unix 秒，可选）
- 正文用途：自然语言补充说明（手工维护；字段保存不得触碰正文）
  - 示例：
    - 字段：
      - `name: luy`
      - `timezone: Asia/Shanghai`
    - 正文：说明“主人是谁/沟通偏好/工作时间”等（手工维护）

**`USER.md` 示例（字段 + 正文）**
```md
---
name: Alice
timezone: Asia/Shanghai
location_place: 上海
location_updated_at: 1710000000
latitude: 31.2304
longitude: 121.4737
---

# USER.md

这里是正文（手工写）。UI/RPC 更新字段时不得改动这段正文。
```

#### 2) `PEOPLE.md`（公共；通信录；仅允许列“系统内 agent”）
- 路径：`<data_dir>/PEOPLE.md`
- 格式：YAML frontmatter（字段）+ Markdown（正文）
- 定位（必须写死）：
  - 仅用于：告诉 agent “本系统内有哪些 agent”以及对外联系/识别信息（通信录）。
  - 严禁：任何内部实现信息（token/secret、内部路由键、存储主键、内部配置参数等）。
- 条目约束（必须）：
  - `PEOPLE.md` 只允许列出存在 `people/<name>/` 的系统内 agent。
  - `people[].name` 必须唯一（不允许重复）；若检测到重复条目，应告警并跳过后续重复项的同步（避免不确定覆盖）。
  - `people[].name` 必须符合 `<name>` 校验规则；不符合则告警并跳过该条目的同步（不自动删除条目）。
  - 若 `PEOPLE.md` 存在条目但 `people/<name>/` 目录不存在：**告警，但不删除条目**（按用户决策）。
  - 若 `people/<name>/` 目录存在但未出现在 `PEOPLE.md`：应告警（避免“目录存在但通信录不可发现”的静默缺陷）；不自动写入新增条目（避免越权/意外曝光）。
- 字段（建议固定为 v1；字段更新不得改正文）：
  - `schema_version`（int，必填）：固定为 `1`
  - `people`（list，必填）：
    - `people[].name`（string，必填）：系统内稳定键（对应 `people/<name>/`）
    - `people[].display_name`（string，可选）：对外展示名（可中文）
    - `people[].emoji`（string，可选）：对外识别符号
    - `people[].creature`（string，可选）：对外识别补充
    - `people[].telegram_user_id`（number|string，可选）：Telegram user/bot id（对外识别用）
    - `people[].telegram_user_name`（string，可选）：建议不带 `@` 的用户名
    - `people[].telegram_display_name`（string，可选）：平台展示昵称
- 字段写入规则（SOT + 自动对齐；必须）：
  - `people[].emoji` 与 `people[].creature` 的 SOT 在 `people/<name>/IDENTITY.md`：
    - `PEOPLE.md` 中对应字段必须自动对齐（修复回写）。
    - UI 必须只读展示（置灰）；不得手工编辑。
  - `people[].display_name` 的 SOT 在 `PEOPLE.md`（公共通信录字段）：
    - 允许手工编辑（UI 不置灰）。
    - 为避免多 SOT，`people/<name>/IDENTITY.md` 不再承载 `display_name` 字段；展示名只在 `PEOPLE.md` 维护。
- 正文用途：自然语言补充说明（手工维护；字段保存不得触碰正文）
  - 示例（仅示意字段形态；`emoji/creature` 由系统对齐写入）：
    - `schema_version: 1`
    - `people: [{ name: default, display_name: 默认, emoji: 🤖, creature: 助手, telegram_user_name: my_bot }]`

**`PEOPLE.md` 示例（字段 + 正文）**
```md
---
schema_version: 1
people:
  - name: default
    display_name: 默认助手
    emoji: "🤖"
    creature: 助手
    telegram_user_id: "123456789"
    telegram_user_name: my_bot
    telegram_display_name: Moltis Bot
---

# PEOPLE.md

这里是正文（手工写）。例如：备注系统里有哪些 agent、他们擅长什么、如何联系等。

说明：`emoji/creature` 会由系统从 `people/<name>/IDENTITY.md` 自动对齐；同步时可能会规范化 frontmatter 的排版（不保证保留 YAML 注释/格式）。
```

#### 3) `people/<name>/IDENTITY.md`（私有；自我定义；用于自己拼 prompt）
- 路径：`<data_dir>/people/<name>/IDENTITY.md`
- 格式：YAML frontmatter（字段）+ Markdown（正文）
- 字段（v1）：
  - `name`（string，必填）：必须等于目录 `<name>`
  - `emoji`（string，可选）
  - `creature`（string，可选）
  - `vibe`（string，可选）
  - 注：展示名（`display_name`）对外/对 UI 的来源为 `PEOPLE.md`（公共），避免在私有 IDENTITY 再引入第二个 SOT。
- 正文用途：更长的自我说明（私有；手工维护；字段保存不得触碰正文）
  - 示例：
    - 字段：
      - `name: default`
      - `emoji: 🤖`
      - `creature: 助手`
      - `vibe: 直接、清晰、效率优先`
    - 正文：更完整的自我定义（仅自己可读）

**`people/default/IDENTITY.md` 示例（字段 + 正文）**
```md
---
name: default
emoji: "🤖"
creature: 助手
vibe: 直接、清晰、效率优先
---

这里是正文（手工写）：更长的身份设定、长期行为约束、风格偏好等。
```

#### 4) `people/<name>/SOUL.md` / `TOOLS.md` / `AGENTS.md`（私有；正文-only）
- 路径：
  - `<data_dir>/people/<name>/SOUL.md`
  - `<data_dir>/people/<name>/TOOLS.md`
  - `<data_dir>/people/<name>/AGENTS.md`
- 格式：纯 Markdown（无字段）
- 用途：
  - `SOUL.md`：人格/价值观/边界（给自己拼 prompt）
  - `TOOLS.md`：工具使用偏好/约束（给自己拼 prompt）
  - `AGENTS.md`：协作/分工说明（私有）

**`people/default/SOUL.md` 示例（正文-only）**
```md
# SOUL.md

价值观、边界、原则……（手工写）
```

**`people/default/TOOLS.md` 示例（正文-only）**
```md
# TOOLS.md

- exec：优先小步验证；不重复回显 stdout/stderr
- web_fetch：需要最新信息时再用
```

**`people/default/AGENTS.md` 示例（正文-only）**
```md
# AGENTS.md

与其它 agent 的协作方式、分工边界……（手工写）
```

### 一致性机制（用户决策：启动校验修复 + identity 变更同步）
- 启动时：
  - 必须执行一次 `PEOPLE.md` ↔ `people/<name>/IDENTITY.md` 的一致性校验与修复回写（同步 `emoji/creature`；仅更新字段，不改正文）。
- 每次通过程序修改 `people/<name>/IDENTITY.md` 字段后：
  - 必须触发一次同步：将该 agent 的 `emoji/creature` 回写到 `PEOPLE.md`。
- 同步实现要求（避免低估复杂度；必须写清）：
  - `PEOPLE.md` 的 frontmatter 含嵌套结构（`people: [ ... ]`），不能复用仅支持“按行删 key”的 `update_markdown_yaml_frontmatter()` 作为合并器，否则容易误删/覆盖用户维护字段。
  - 推荐实现：读取 `PEOPLE.md` → 解析 YAML frontmatter 为结构化对象（v1 schema）→ 按 `people[].name` 与 `people/<name>/IDENTITY.md` 建立映射 → 仅更新每个条目的 `emoji/creature` → 重新序列化写回 frontmatter。
  - 写回必须满足：
    - `PEOPLE.md` 正文 byte-for-byte 不变（仅修改 frontmatter）。
    - 不得覆盖 `people[]` 条目的其它字段（例如 `display_name`、`telegram_*` 等）。
    - 顺序：保持 `people[]` 原有顺序不变（仅就地更新字段），避免无意义 diff 与 UI 抖动。
    - 原子性：建议使用“临时文件 + rename”原子写入，避免读者观察到部分写入内容。
    - 高效：若同步前后无变化，应跳过写回（避免无意义 IO、mtime 抖动与日志噪声）。
    - 可预期：同步过程允许对 frontmatter 做规范化输出（可能丢失 YAML 注释/手工排版）；需要说明性文本请放在正文中。
- 失败与降级：
  - 任一 agent 的 `IDENTITY.md` 缺失/不可解析：告警并跳过该条目，不阻塞启动。
  - `PEOPLE.md` 不存在：可以 seed 最小模板（含 `schema_version: 1` 与 `people: []`），正文可为空。

---

## 与现状实现的冲突点（需在实现阶段消解）
- 当前实现仍使用 `personas/<persona_id>/...` 目录与相关校验（ASCII persona_id），需要一步到位改为 `people/<name>/...`（按用户决策，不考虑迁移兼容）。
- 当前 `{data_dir}/PEOPLE.md` 由 gateway 根据 channels 自动生成并整文件覆盖写回，且包含内部键（`chanAccountKey` 等），与本 Spec 冲突：
  - 目标态：`PEOPLE.md` 是公共通信录（字段 + 正文），不得整文件覆盖，不得写入内部键。
  - channel 变更不应再重写 `PEOPLE.md`（或应改为仅更新通信录中对外字段的建议值，且必须遵守“字段不改正文”的边界）。

---

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] `USER.md`：UI/RPC 只能更新 YAML 字段，必须保留正文原样不变。
- [x] `PEOPLE.md`：UI/RPC 只能更新 YAML 字段，必须保留正文原样不变。
- [x] `people/<name>/IDENTITY.md`：UI/RPC 只能更新 YAML 字段，必须保留正文原样不变。
- [x] 移除/关闭 UI 对上述文件“raw 正文写入”的入口（或改成只读预览 + 手工编辑提示）。
- [x] 公共/私有边界收敛：
  - 公共：仅 `USER.md` 与 `PEOPLE.md` 注入所有 agent 的 prompt。
  - 私有：仅当前 agent 的 `people/<name>/**` 可用于该 agent 的 prompt 拼装；不得读取其它 agent 的 `people/<other>/**`。
- [x] 隐私边界 enforcement（必须落地为“机制”，不只是一句约定）：
  - Prompt 拼装层面：严格按 “公共 + 自己私有目录” 读取文件。
  - 工具/沙盒层面（如 exec/file-read 能直接读磁盘）：必须确保运行时不可访问其它 agent 的 `people/<other>/**`。
    - 落地机制（当前实现）：当 `data_mount_type=bind` 时，sandbox 仅挂载公共视图（`.sandbox_views/<sandbox_key>`，只包含 `USER.md`/`PEOPLE.md`），不暴露任何 `people/<name>/...` 私有目录。
    - 限制（仍需单独治理）：当 sandbox backend 为 `none`（无容器运行时）或 `data_mount_type=volume` 时，无法对宿主机路径做同等隔离。
- [x] 默认 agent 收敛：默认 prompt 拼装必须以 `people/default/**` 为准（spawn/main/web ui 默认会话等）。
- [x] PEOPLE 一致性机制落地：
  - 启动时必须校验并修复回写 `PEOPLE.md`（同步 `emoji/creature`）。
  - 每次通过程序修改 `people/<name>/IDENTITY.md` 字段后，必须同步一次 `PEOPLE.md`。
- [x] identity/user 的来源收敛：默认 agent identity 以 `people/default/IDENTITY.md` 字段为准；user profile 以 `USER.md` 字段为准；不得再出现“toml + 文件 overlay”的双 SOT 语义。
- [x] `moltis.toml` persona/资料字段退场删除：移除 `[identity]`、`[user]` 以及 `identity.soul`（不再读取、不再写入、不再在 template/validate/schema 中出现）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：字段更新是幂等的、可预测的；不会重排/覆盖正文。
  - 不得：任何 UI 保存动作导致正文内容变化（包括插入模板、自动补标题、自动格式化等）。
  - 不得：同一字段存在多个长期生效来源（例如 `moltis.toml [identity]` 与 `people/default/IDENTITY.md` 同时生效）。
- 兼容性（用户决策：一步到位，不做迁移兼容）：
  - 不考虑 `personas/<persona_id>/...` 旧目录的自动迁移；旧数据可由用户自行重新生成。
  - 不提供“旧路径兼容读取/overlay”；避免引入第二套 SOT。
- 可观测性：
  - UI/日志应能定位字段 SOT（例如 “identity source: people/default/IDENTITY.md frontmatter”）。

---

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) **正文可被 UI 改写**：
   - Personas 页面提供 `IDENTITY.md (raw)` 编辑框，保存时会把整段文本写回：`crates/gateway/src/assets/js/page-settings.js:738`、`crates/gateway/src/assets/js/page-settings.js:649`
   - `personas.save` RPC 当前接受 `identity/soul/tools/agents` 原文并落盘：`crates/gateway/src/methods.rs:1320`、`crates/gateway/src/personas.rs:128`
2) **identity 存在多 SOT / overlay**：
   - gateway prompt persona 组装：先取 `moltis.toml` identity，再用 `IDENTITY.md` 覆盖：`crates/gateway/src/chat.rs:811`
   - spawn_agent 同样 overlay：`crates/tools/src/spawn_agent.rs:42`
3) **PEOPLE.md 当前是系统整文件重写**（与“正文只能手工编辑”冲突）：
   - `regenerate_people_md()` 直接重写 `PEOPLE.md`：`crates/gateway/src/people.rs:46`
   - channel add/update/remove 会触发重写：`crates/gateway/src/channel.rs:214`

### 影响（Impact）
- 用户体验：字段与正文混在同一编辑面板/同一保存动作里，极易误改正文；且规则不显性。
- 可靠性：多 SOT 导致字段表现不一致（UI/branding/session 命名/prompt 看到的 identity 可能来自不同来源），排障困难。
- 治理成本：结构化字段应用散落（UI、prompt、spawn_agent、CLI 等），长期维护痛苦。

---

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/config/src/loader.rs:708`：`IDENTITY.md` 仅解析 `name/emoji/creature/vibe` 字段。
  - `crates/config/src/loader.rs:604`：frontmatter 更新逻辑（保留正文与未管理字段）。
  - `crates/gateway/src/assets/js/page-settings.js:738`：UI 存在 raw 编辑 `IDENTITY.md` 的入口。
  - `crates/gateway/src/methods.rs:1320`：`personas.save` 接收并落盘 raw 内容（identity/soul/tools/agents）。
  - `crates/gateway/src/chat.rs:811`：identity overlay（toml → file_identity）。
  - `crates/tools/src/spawn_agent.rs:42`：spawn_agent identity overlay（toml → file_identity）。
  - `crates/gateway/src/people.rs:46`：`PEOPLE.md` 目前是系统生成并整文件重写。
  - `crates/config/src/template.rs:55`：`moltis.toml` 仍提供 `[identity]`（name/emoji/creature/vibe）。
  - `crates/config/src/template.rs:68`：`moltis.toml` 仍提供 `[user]`（name/timezone）。
  - `crates/config/src/validate.rs:2294`：`identity.soul` 被显式允许为“非 unknown-field”（但 schema 中并无该字段，属于挂名字段）。
- Prompt 口径证据：
  - Prompt 引导会要求读取 `/moltis/data/USER.md` 与 `/moltis/data/PEOPLE.md`：`crates/agents/src/prompt.rs:92`

---

## 根因分析（Root Cause）
- A. 写入边界未收敛：
  - `personas.save` 等接口以“整文件字符串”作为写入单位，天然会改正文。
- B. SOT 未冻结：
  - identity/user 同时存在 config 与 workspace 文件两条写入链路（而且会互相覆盖），形成长期 overlay。
- C. PEOPLE.md 定位冲突：
  - 现状是系统生成；目标要求是手工正文；两者需要拆分或改名以收敛职责。
- D. PEOPLE 同步缺少“结构化合并器”：
  - `PEOPLE.md` frontmatter 是嵌套结构（list of objects）；若用行级 patch 或整块覆盖，容易破坏用户维护字段（如 `display_name`/`telegram_*`），导致数据丢失或无意义 diff。

---

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `USER.md`/`PEOPLE.md`/`people/<name>/IDENTITY.md` 的“字段更新”只允许修改 frontmatter managed keys，正文 byte-for-byte 保持不变。
  - 目录与可见性必须符合 Type 4 Spec：
    - 公共：仅 `USER.md` 与 `PEOPLE.md` 对所有 agent 可读/可用于 prompt 注入。
    - 私有：仅当前 agent 的 `people/<name>/**` 可读/可用于 prompt 注入；不得读取其它 agent 的 `people/<other>/**`。
  - 默认 agent 必须以 `people/default/**` 作为 prompt 拼装来源（spawn/main/web ui 默认会话等）。
  - `PEOPLE.md` 必须只包含系统内 agent（即存在 `people/<name>/` 目录者）；若条目目录缺失则告警但不删除（按用户决策）。
  - 一致性机制必须启用且可预测：
    - 启动时：校验并修复回写 `PEOPLE.md`（同步 `emoji/creature`）。
    - identity 字段变更时：每次通过程序更新 `people/<name>/IDENTITY.md` 字段后，必须同步一次 `PEOPLE.md`。
  - identity/user 的字段 SOT 必须唯一：
    - identity：来自 `people/<name>/IDENTITY.md`（默认 `name=default`）。
    - user：来自 `USER.md`。
    - 不得再出现“toml + 文件 overlay”的双 SOT 语义。
  - `moltis.toml` 中 persona/资料相关字段（`[identity]` / `[user]` / `identity.soul`）必须退场删除（按用户决策一步到位，不做兼容迁移）：
    - 代码层面：不再读取/不再写入；schema/template/validate 不再保留这些字段。
- 不得：
  - UI/RPC 不得提供“保存正文”的能力（只读展示可以）。
  - `PEOPLE.md` 不得写入任何内部实现信息（token/secret、内部路由键、存储主键等）。
  - channel 变更不得再整文件覆盖 `PEOPLE.md`（不得破坏“正文只能手工编辑”的边界）。

---

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：字段级 RPC + 后端 frontmatter patch（正文严格保留）+ PEOPLE 自动对齐
- 核心思路：
  - 将 persona 目录整体替换为 `people/<name>/`（含 `people/default` seed），并收敛所有 prompt 拼装与 UI/RPC 到新路径。
  - 为 `USER.md` / `PEOPLE.md` / `people/<name>/IDENTITY.md` 提供字段级 get/update RPC（patch 语义）：
    - 后端只改 frontmatter 字段，正文 byte-for-byte 保持不变。
    - UI 仅展示字段表单；正文改为只读预览 + 提示“手工编辑文件路径”。
  - PEOPLE 自动对齐：
    - 启动时：从 `people/<name>/IDENTITY.md` 读取 `emoji/creature` 修复回写 `PEOPLE.md`。
    - identity 字段变更时：同步修复回写 `PEOPLE.md`。
    - UI 对 `PEOPLE.md` 的 `emoji/creature` 字段置灰只读。
  - 禁止 channel 变更覆盖 `PEOPLE.md`（移除 `regenerate_people_md()` 的整文件重写链路，或将其改为不写 `PEOPLE.md`）。
  - 同步退场删除 `moltis.toml` 的 persona/资料字段（`[identity]` / `[user]` / `identity.soul`），按用户决策一步到位（不做迁移兼容）。
- 优点：
  - 机制上保证“改字段不动正文”，可测试、可审计。
  - 收敛 SOT：公共文件（`USER.md`/`PEOPLE.md`）与私有文件（`people/<name>/**`）边界清晰；identity/user 来源唯一；PEOPLE 展示字段一致性可自动保证。
- 风险/缺点：
  - 需要调整现有 RPC 与 UI（breaking UX）；但按用户决策不做迁移兼容，可直接一步到位。

#### 方案 2（备选）：继续使用 `personas.save`，但后端忽略正文变更（强制回填原正文）
- 核心思路：
  - 继续维持“整文件 raw 保存”的 UI/RPC，但后端保存时只抽取 frontmatter 字段，正文从磁盘原文件读取回填，确保正文不变。
- 优点：UI 改动较小。
- 风险/缺点：
  - API 语义不透明（“你传了正文但不会生效”），容易困惑；长期仍建议演进到字段级 RPC。

### 最终方案（Chosen Approach）
采用方案 1（字段级 RPC + PEOPLE 自动对齐 + 一步到位替换为 `people/<name>/`）。

---

## 验收标准（Acceptance Criteria）【不可省略】
- [x] UI 更新 `USER.md` 字段后，文件正文完全不变。
- [x] UI 更新 `people/<name>/IDENTITY.md` 字段后，文件正文完全不变，且自动同步修复 `PEOPLE.md`（至少 `emoji/creature`）。
- [x] `PEOPLE.md` 字段更新同上（正文不变）；且不再被 channel 变更整文件覆盖写回。
- [x] 启动时会执行一次 PEOPLE 一致性校验与修复回写（不阻塞启动；失败告警即可）。
- [x] `PEOPLE.md` 用于“系统内 agent”通信录；若条目存在但 `people/<name>/` 目录缺失，仅告警、不删除条目。
- [x] 公共/私有边界回归：任一 agent 的 prompt 拼装只读取 `USER.md`/`PEOPLE.md` + 自己的 `people/<name>/**`，不会读取其它 agent 的 `people/<other>/**`。
- [x] 默认 agent 回归：未指定 agent 时，身份与 prompt 拼装来自 `people/default/**`。
- [x] identity/user 字段的 SOT 明确且唯一；不再存在长期 overlay（toml ↔ 文件），且 `moltis.toml` persona/资料字段已退场。

---

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] frontmatter patch（user/identity）：更新字段后保留正文与未管理字段：`crates/config/src/loader.rs:604`
- [x] PEOPLE frontmatter sync：给定 `PEOPLE.md` + `people/<name>/IDENTITY.md`，同步后：
  - `PEOPLE.md` 正文 byte-for-byte 不变
  - `people[].emoji/creature` 被正确修复回写
  - 目录缺失条目仅告警不删除
  - 重复 `people[].name`：仅同步第一个，后续重复项告警并跳过
- [x] `<name>` 校验：`default/ops_1/ops-1` 通过；空/空格/路径分隔符/`..` 拒绝

### Integration
- [x] UI/RPC：更新 `USER.md` / `people/<name>/IDENTITY.md` 字段 → 验证文件正文 byte-for-byte 不变（快照/哈希对比）。
- [x] 启动路径：启动后自动执行 PEOPLE 一致性修复（验证 `PEOPLE.md` 被修复回写，且不会阻塞启动）。
- [x] 隐私边界：任一 agent 的 prompt 拼装不会读取其它 agent 的 `people/<other>/**`（通过 sandbox bind mount public view 机制保证工具侧不可见）。
- [x] 工具/沙盒边界（如启用 sandbox）：在 sandbox 内尝试读取其它 agent 的 `people/<other>/**` 应失败（bind mount 下不暴露私有目录；并有单测验证 view 仅包含 USER/PEOPLE）。
- [x] channel 变更路径：channels.add/update/remove 不再整文件覆盖 `PEOPLE.md` 正文。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：某些 UI 交互与文件编辑路径可能难以在 CI 完整覆盖。
- 手工验证步骤：
  1) 手工在 `USER.md`/`PEOPLE.md`/`people/default/IDENTITY.md` 正文写入一段自定义文本
  2) 仅通过 UI 修改字段（或触发 identity 字段保存）
  3) 确认三者正文不变，且 `PEOPLE.md` 的 `emoji/creature` 自动对齐

---

## 发布与回滚（Rollout & Rollback）
- 发布策略（用户决策：一步到位）：
  - 直接切换到 `people/<name>/` 新目录与 `people/default` 默认 agent；不提供 `personas/<persona_id>/` 的迁移兼容。
  - `PEOPLE.md` 变为公共通信录（字段 + 正文）并启用启动修复与 identity 变更同步。
  - `moltis.toml` persona/资料字段一次性退场删除（不做迁移兼容）。
- 回滚策略：
  - 若必须回滚，需整体回滚到旧版本二进制与旧数据布局；由于不做迁移兼容，回滚属于“版本回退”，不保证新布局数据可被旧版本读取。

---

## 实施拆分（Implementation Outline）
- Step 1: 引入 `people/<name>/` 目录布局与路径 API，确保 `people/default` 启动 seed（4 文件）。
- Step 2: `people/<name>/IDENTITY.md`：
  - 仅字段可写、正文只读；校验 `name` 与目录一致
- Step 3: `PEOPLE.md`：
  - 定义 v1 schema（frontmatter：`schema_version` + `people[]`）
  - 禁止写入内部键；正文只读
  - 启动时一致性校验 + 修复回写（同步 `emoji/creature`）
- Step 4: identity 变更同步：
  - 通过程序修改 `people/<name>/IDENTITY.md` 字段后，触发一次 `PEOPLE.md` 同步修复
  - UI 将 `PEOPLE.md` 的 `emoji/creature` 置灰只读
- Step 5: 收敛 prompt 拼装与可见性边界：
  - prompt 注入只读取公共 `USER.md`/`PEOPLE.md` + 当前 agent 私有 `people/<name>/**`
  - 默认未指定 agent 时使用 `people/default/**`
- Step 6: 移除旧 PEOPLE 自动生成链路：
  - 删除/停用 `regenerate_people_md()` 及 channels 变更触发的整文件覆盖写回
- Step 7: `moltis.toml` persona/资料字段退场删除（`[identity]` / `[user]` / `identity.soul`；一步到位，无迁移兼容）：
  - 移除读取/写入路径；更新 schema/template/validate
- 受影响文件（预估）：
  - `crates/config/src/loader.rs`
  - `crates/gateway/src/personas.rs`（legacy；将被替换/删除）
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/assets/js/page-settings.js`
  - `crates/agents/src/prompt.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/tools/src/spawn_agent.rs`
  - `crates/gateway/src/people.rs`、`crates/gateway/src/channel.rs`
  - `crates/onboarding/src/service.rs`
  - `crates/config/src/schema.rs`
  - `crates/config/src/template.rs`
  - `crates/config/src/validate.rs`

---

## 交叉引用（Cross References）
- Related issues/docs：
  - <TBD>

## 未决问题（Open Questions）
- None（用户决策已冻结；实现按 Type 4 Spec 落地）。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

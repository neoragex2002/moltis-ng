# Issue: 预定义多个命名 persona/agent（支持按 session / channel 绑定）

## 实施现状（Status）【增量更新主入口】
- Status: TODO（备档）
- Priority: P2（体验/可控性增强；非阻断）
- Components: config / sessions metadata / gateway prompt / Web UI / channels
- Affected providers/models: all（system prompt 组装层）

**已实现**
- 暂无（本单为设计备档）

**已覆盖测试**
- 暂无新增（需补齐：见 Test Plan）

---

## 背景（Background）
目前 Moltis 的“人格/人设”主要由全局数据文件驱动（`IDENTITY.md` / `SOUL.md` / `AGENTS.md` / `TOOLS.md`），会被注入 system prompt，并对所有会话统一生效。

这带来两个限制：
1) 无法在同一个实例里同时维护多个“命名 persona”（例如：`coder` / `ops` / `research`），也无法对不同会话/渠道绑定不同 persona。
2) 虽然存在 `spawn_agent` 子代理工具，但它的 system prompt 是工具内固定模板，缺少“命名 persona profile”与“继承/选择 persona”的机制。

> 现阶段的 workaround 是“换一套全局文件”或“用不同 `MOLTIS_DATA_DIR` 跑多个实例”，但这不是 per-session/per-channel 的一等支持。

## 概念与口径（Glossary & Semantics）
- **Persona Profile（命名 persona）**：一组可复用的人设/规则输入源，至少包含：
  - agent identity（name/emoji/creature/vibe）
  - soul（自由人格/行为指令）
  - workspace agent rules（AGENTS.md）
  - tool preferences（TOOLS.md）
  - （可选）额外 persona 专属规则文件
- **Session-bound Persona**：某个 session 固定绑定一个 persona profile（默认绑定 `default`）。
- **Channel-bound Persona**：某个 channel（如 Telegram account/chat）绑定 persona profile，用于该渠道触发的会话（或其派生 session）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 支持预定义多个命名 persona profile（可列举/可选择/可默认）。
- [ ] 支持按 **session** 绑定 persona（不同 session 可不同 persona）。
- [ ] 支持按 **channel** 绑定 persona（例如 Telegram 不同 bot account/不同 chat 使用不同 persona）。
- [ ] `spawn_agent` 子代理能继承父 session 的 persona（或显式指定 persona/model）。
- [ ] Web UI 可查看/切换当前 session 的 persona（至少 debug/context 可见）。

### 非功能目标（Non-functional）
- 兼容性：不破坏现有单 persona 行为；未配置 persona 时等价于当前实现。
- 安全隐私：persona 文件路径必须固定在 data_dir 下，禁止路径穿越；persona 内容注入遵循现有 prompt 安全策略。
- 可观测性：运行态（debug/context）能看到“当前 persona id + 来源文件”，便于排障。

### Non-goals（明确不做）
- [ ] 不要求一次性提供“多 persona 的完整 UI 管理后台”（可先提供文件定义 + RPC 切换）。
- [ ] 不在本单强制做“persona 的权限隔离/多租户”（单机个人助手优先）。

## 现状核查与证据（As-is / Evidence）
### A) persona 目前是全局加载（非 session 绑定）
- persona merge 逻辑：`crates/gateway/src/chat.rs:723`（`load_prompt_persona()` 每次读取同一套 identity/user/soul/agents/tools）
- data_dir 文件位置：`crates/config/src/loader.rs:249`（`data_dir()`）以及 `IDENTITY.md` / `SOUL.md` / `USER.md` / `AGENTS.md` / `TOOLS.md` 路径 helpers（例如 `identity_path()`：`crates/config/src/loader.rs:275`）
- system prompt 注入点：`crates/agents/src/prompt.rs:170`（identity + soul），以及 workspace files：`crates/agents/src/prompt.rs:242`（AGENTS.md / TOOLS.md）

### B) Session 元数据没有 persona 字段
- `SessionEntry` 字段列表：`crates/sessions/src/metadata.rs:15`（目前无 `persona`/`profile` 字段）

### C) 子代理没有命名 persona 支持
- 子代理 prompt 由工具内固定字符串构造：`crates/tools/src/spawn_agent.rs:141`
- 子代理 loop 无 history：`crates/tools/src/spawn_agent.rs:172`

## 问题陈述（Problem Statement）
当前“人格/规则”只能全局一套，导致：
- 多用途场景（coding/ops/research）需要手工改 SOUL/AGENTS 或开多个实例，不直观、不可控、容易混用。
- 不同渠道（Telegram bot A vs bot B）无法稳定体现不同 persona。
- 子代理难以复用“特定角色”的行为规范，只能临时塞 `context` 指令，易漂移且不可观察。

## 期望行为（Desired Behavior / Spec）
- 必须：支持 `persona_id`（string）作为一等配置/绑定目标，至少覆盖 session 与 channel。
- 必须：默认 persona（例如 `default`）等价于当前行为（读 `~/.moltis/IDENTITY.md` / `SOUL.md` / `AGENTS.md` / `TOOLS.md`）。
- 必须：prompt 构建时使用“解析后的 effective persona”，并在 debug/context 中可见（persona id + 来源）。
- 不得：允许 persona 引用 data_dir 之外的任意文件路径（防止读取系统敏感文件）。
- 应当：子代理继承父 persona；允许覆盖（显式指定 persona/model）。

## 方案（Proposed Solution）
### Phase 0：定义 persona 存储结构（先文件，后 UI）
建议在 data_dir 引入目录结构（示例）：

```
~/.moltis/
  personas/
    default/
      IDENTITY.md
      SOUL.md
      AGENTS.md
      TOOLS.md
    ops/
      IDENTITY.md
      SOUL.md
      AGENTS.md
      TOOLS.md
```

- `default/` 可软链接/复制现有全局文件，或直接把“全局文件”视为 default persona 的来源（兼容期策略见 Open Questions）。

### Phase 1：加载与绑定
- 增加 persona loader：
  - `list_personas()`：列出可用 persona ids
  - `load_persona(persona_id)`：读取该 persona 下的四类文件并返回结构体
- 增加 session metadata 字段：`persona_id: Option<String>`
- gateway prompt 组装改为：
  1) resolve session 的 persona_id（无则 default）
  2) load effective persona（identity/soul/agents/tools）
  3) 注入 system prompt
- `spawn_agent` 工具：
  - 默认继承 `_session_key` 所绑定 persona（或父调用传入 persona_id）
  - 支持显式指定 `persona`（与 `model` 类似，作为可选参数）

### Phase 2：RPC 与 UI（最小可用）
- RPC：
  - `persona.list`
  - `session.set_persona`（或 `sessions.patch` 扩展字段）
- UI：
  - 至少在 debug/context 显示当前 persona id
  - 可选：在会话 header/设置中切换 persona

### Phase 3：Channel 绑定
- channel config 增加 `persona_id`（例如 Telegram account/chat）
- channel dispatch 创建 session 时写入 session.persona_id（或继承 channel binding）

## 验收标准（Acceptance Criteria）
- [ ] 允许存在 `personas/<id>/...` 多套 persona，并可被列举。
- [ ] 同一实例中两个 session 绑定不同 persona 后，system prompt 的 identity/soul/workspace files 注入内容可明显区分（可通过 debug/context 或 raw_prompt 验证）。
- [ ] Telegram（或其它 channel）可配置 persona，并影响该渠道触发的会话。
- [ ] `spawn_agent` 默认继承 persona，且可显式覆盖 persona/model。
- [ ] 未配置 persona 时行为与当前一致（兼容）。
- [ ] 任何 persona 文件读取都被限制在 data_dir（无路径穿越）。

## 测试计划（Test Plan）
### Unit
- [ ] persona loader：读取/缺文件回退/空文件行为
- [ ] session metadata：persona_id 字段序列化/反序列化与默认值兼容
- [ ] prompt 注入：给定 persona A/B，断言 system prompt 发生对应变化（identity/soul/workspace 段落）

### Integration / Gateway
- [ ] `chat.context` / `raw_prompt`：显示 effective persona id 与来源（最小契约）

### UI E2E（可选）
- [ ] 新建 session → 切换 persona → 刷新页面后仍保持

## 发布与回滚（Rollout & Rollback）
- 建议默认开启“只读 persona（文件定义）”能力，但 UI 切换可先隐藏在实验入口。
- 回滚策略：保留 default persona 行为路径；persona loader 故障时回退到现有全局文件。

## 交叉引用（Cross References）
- system prompt 架构说明：`docs/src/system-prompt.md:9`
- 全局 persona loader：`crates/gateway/src/chat.rs:723`
- prompt 注入：`crates/agents/src/prompt.rs:170`、`crates/agents/src/prompt.rs:242`
- 子代理工具：`crates/tools/src/spawn_agent.rs:141`
- session metadata：`crates/sessions/src/metadata.rs:15`

## 未决问题（Open Questions）
- persona 的 default 兼容策略：
  - A) 继续使用现有全局文件作为 default（不要求 `personas/default/` 存在）
  - B) 迁移到 `personas/default/` 并把旧文件视为 deprecated
- `USER.md` 是否应参与 persona（通常不建议：用户信息是全局一致的）
- persona 是否允许“继承/叠加”（例如 base persona + per-project rules），还是只做单一绑定
- 子代理是否应继承 history（目前不继承；会影响 persona 一致性与成本）


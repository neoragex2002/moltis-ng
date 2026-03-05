# Issue: Type4 模板化拼接 v1（{{var}} / 中文化 / 稳定性）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-05
- Owners: <TBD>
- Components: config / agents/prompt / gateway
- Affected providers/models: all（openai-responses 与 non-responses 均受影响）

**已实现（如有，写日期）**
- (2026-03-04) Strict `{{var}}` 模板替换 + 转义 + 校验（UTF-8 安全）：`crates/config/src/prompt_subst.rs:60`
- (2026-03-04) Canonical System Prompt v1（四文件拼接 + vars 替换 + requiredness matrix）：`crates/agents/src/prompt.rs:385`
- (2026-03-04) tools schemas 稳定排序（name/source/mcpServer）：`crates/agents/src/tool_registry.rs:92`
- (2026-03-04) skills prompt 中文 wrapper + 稳定排序：`crates/skills/src/prompt_gen.rs:4`
- (2026-03-04) gateway 全入口改为 canonical v1（preflight/run/send_sync/debug/streaming）：`crates/gateway/src/chat.rs:2401`
- (2026-03-04) spawn_agent 改为 canonical v1（单条 system message）：`crates/tools/src/spawn_agent.rs:235`

**已覆盖测试（如有）**
- Strict substitution + escape + UTF-8：`crates/config/src/prompt_subst.rs:157`
- Canonical v1（native/non-native/no-tools/escape/requiredness/warnings）：`crates/agents/src/prompt.rs:1718`
- ToolRegistry::list_schemas 稳定排序：`crates/agents/src/tool_registry.rs:363`
- gateway debug endpoints asSentPreamble（openai-responses 单条 developer）：`crates/gateway/src/chat.rs:8298`
- spawn_agent（openai-responses provider 下仍只注入 1 条 system）：`crates/gateway/tests/spawn_agent_openai_responses.rs:43`

**已知差异/后续优化（非阻塞）**
- 目前 `asSent` 证据链已覆盖 openai-responses / anthropic / local-llm；其余 provider 的 as-sent 摘要可按需逐步补齐（见：`issues/done/issue-prompt-as-sent-observability.md`）。

---

## 背景（Background）
- 场景：用户希望通过 `people/<name>/*.md` 完整掌控 persona/type4 prompt 的“布局与章节”（而不仅是内容碎片），并在不同 provider 下保持一致的内容治理口径。
- 约束：
  - 不引入模板语言（no if/loop/表达式），只允许 `{{var}} → String` 的纯字符串替换。
  - tools/skills/runtime 等属于运行时事实，必须由系统提供 vars，且必须稳定（可缓存/可对比）。
  - 标题/说明性硬编码尽量中文（技能/工具描述可原样）。
- Out of scope：
  - 不在 v1 中引入条件渲染/循环（避免不可控 prompt）。
  - 不在 v1 中重写 provider adapter（只要求其遵循 canonical/renderer 分层）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **Type4 persona 模板正文**（主称呼）：用户维护的四文件拼接文本（仅指 persona/type4 自定义部分，不含系统固定指南或 provider 协议包装）。
  - Source/Method：configured → effective（拼接 + vars 替换）
- **vars map**（主称呼）：运行时由系统提供的 `{{var}} → String` 映射表。
  - Source/Method：effective（由 runtime/tools/skills 生成，必须稳定）
- **稳定性（stability）**（主称呼）：在同一输入下，输出必须字节级稳定（含排序与 JSON key 顺序）。
  - Why：prompt cache、debug diff、回归测试依赖稳定性。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 支持四文件拼接：`people/<name>/{IDENTITY,SOUL,AGENTS,TOOLS}.md`，按固定顺序拼成 persona 模板正文（仅拼接非空段落，段间用 `\n\n` 连接；不自动插入固定分隔线或占位符）。
- [x] 支持 `{{var}}` 纯字符串替换（对四文件均生效），无其它模板逻辑。
- [x] 提供 v1 vars（至少覆盖 skills/tools 的三类运行模式：native / non-native / no-tools）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：所有 vars 输出必须稳定（排序 + canonicalize）；空字符串 vars 必须“自然消隐”（模板引用也不产生空标题）。
  - 不得：系统不得擅自插入/重排用户模板的章节结构（除了固定的四文件拼接与分隔线）。
- 兼容性：老的硬编码 prompt 结构必须有迁移策略（可先保持默认 persona 模板等价于旧布局）。
- 安全与隐私：vars 中不得包含 secrets；runtime 敏感字段必须脱敏或省略。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 现状 prompt 结构高度硬编码，用户无法通过文件完全控制布局（只能改内容片段）。
2) tools/skills 列表输出顺序不稳定，导致 prompt cache 与 diff/回归不可用。

### 影响（Impact）
- 可控性：用户难以建立“改文件即可稳定生效”的心智模型。
- 可靠性：输出不稳定导致 cache 失效、排障困难。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - tools schemas 来自 HashMap 遍历（顺序不稳定）：`crates/agents/src/tool_registry.rs:92`
  - skills 发现目录项未排序：`crates/skills/src/discover.rs:77`
  - non-native tools parameters pretty JSON 未 canonicalize key 顺序：`crates/agents/src/prompt.rs:565`
- 当前测试覆盖：
  - 已有：prompt builder 基础测试（不验证稳定排序/模板 vars）：`crates/agents/src/prompt.rs:775`
  - 缺口：缺少模板化输出的 golden 测试。

## 根因分析（Root Cause）
- A. prompt builder 把布局写死在代码里，且与 provider 分支耦合。
- B. tools/skills 数据源使用 HashMap/read_dir，未稳定排序。
- C. JSON pretty 输出 key 顺序依赖输入 Value 的生成顺序（可能漂移）。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
### 模板所有权与拼接规则（必须写死）
- 必须：Type4 persona 模板正文只由下列四个文件按顺序拼接得到：
  1) `IDENTITY.md`
  2) `SOUL.md`
  3) `AGENTS.md`
  4) `TOOLS.md`

  拼接规则（现行 v1）：
  - 对每个文件先 strip YAML frontmatter（如存在），再做 strict `{{var}}` 替换（同一 vars map）。
  - 仅拼接非空段落；段落之间用 `\n\n` 连接；系统不自动插入固定分隔线或“（未配置）”占位符。

- 必须：对四个文件都执行 `{{var}}` 替换（同一 vars map）。
- 必须：替换仅做纯字符串替换（无 if/loop/表达式）；可选段落通过“var=空字符串”实现。
- 应当：为支持 frontmatter 元数据，注入前对每个文件先 strip YAML frontmatter（如果存在）。

### v1 vars（最小集合）
> 变量值必须要么为 `""`（空字符串），要么为一个“完整段落”（自带标题），避免模板出现空标题。

- `{{skills_md}}`
  - 值：空或 `## 可用技能` 段落（包含 `<available_skills>` 块与中文启用说明）。
  - 稳定性：skills 必须稳定排序（建议键：`(name, source, path)`；其中 `path` 仅作同名 skills 的 tie-breaker）。

- `{{native_tools_index_md}}`
  - 值：仅在本次运行是 **native tool-calling** 且 tools 非空时为非空；否则必须为 `""`。
  - 内容：`## 可用工具` + compact list（desc 截断 160）。
  - 稳定性：schemas 必须按 `(name, source, mcpServer)` stable sort。

- `{{non_native_tools_catalog_md}}`
  - 值：仅在本次运行是 **non-native tool-calling** 且 tools 非空时为非空；否则必须为 `""`。
  - 内容：中文标题 + per-tool 描述 + 参数 JSON（pretty）。
  - 稳定性：
    - tool 级顺序稳定（同上排序）。
    - `parameters` 的 JSON 对象 key 必须递归 canonicalize（确保 pretty 输出稳定）。

- `{{non_native_tools_calling_guide_md}}`
  - 值：仅在本次运行是 non-native tool-calling 且 tools 非空时为非空；否则必须为 `""`。
  - 内容：中文标题 + ` ```tool_call ... ``` ` 规范（runner 依赖）。

- `{{long_term_memory_md}}`
  - 值：当存在 `memory_search` 工具时应为非空完整段落；否则必须为 `""`。
  - 注意：该段落应避免鼓励“凭记忆猜测”，只强调“需要时先搜索”。

- `{{voice_reply_suffix_md}}`
  - 值：当 `reply_medium == 语音` 时应为非空完整段落；否则必须为 `""`。
  - 注意：作为模板变量值时必须归一化（不以换行开头、以 `\\n\\n` 结尾），避免拼接造成双重空行与不稳定。

### Template Validation（必须，不允许“靠约定”）
> v1 的核心风险不是“能不能替换”，而是“模板漏引用/拼错变量”导致运行时 silently drift。

- 占位符语法必须写死：仅识别 **无空格** 的 `{{var}}`，其中 `var` 仅允许 `[a-z0-9_]+`。
- 渲染前必须扫描四文件拼接后的模板文本，提取所有 `{{var}}` 集合。
- 对于当前运行模式 Requiredness Matrix 标记为 **硬依赖（hard-required）** 的变量：
  - 若模板未包含对应占位符，必须 fail-fast（拒绝构造 prompt），返回明确错误码（例如 `PROMPT_TEMPLATE_MISSING_REQUIRED_VAR`）。
- 对于当前运行模式标记为 **软依赖（soft-required）** 的变量：
  - 若模板未包含对应占位符，不得 fail-fast；必须输出 warning（并在 debug 中标注缺失）。
- 对于模板中出现的 `{{var}}`：
  - 若 `var` 不在本次 vars_map 中，必须 fail-fast（避免 typo 被悄悄吞掉）。
- 渲染后必须再次扫描，确保不存在任何仍符合语法的 `{{var}}`（未替换占位符）；否则必须 fail-fast。
- 字面量 `{{...}}` 的写法约束（避免误判为占位符）：
  - 需要保留字面量时：
    - 推荐：使用转义 `{{{{` → `{{`，`}}}}` → `}}`（可在模板中安全展示 `{{var}}` 示例）。
    - 或：写成不匹配语法的形式（例如 `{{ foo }}` 带空格），避免出现 `{{[a-z0-9_]+}}`。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：所有 tools/skills 的渲染必须先稳定排序，再生成文本。
- 规则 2：non-native tools 的 `parameters` 在 pretty 输出前必须递归排序对象 key。
- 规则 3：模板渲染只做**严格** `{{var}}`（无空格，且 `var` 匹配 `[a-z0-9_]+`）的**单次**字符串替换；若模板引用了不存在的 strict var 必须 fail-fast（避免 typo 被悄悄吞掉）；非 strict 的 `{{ ... }}` 视为字面量文本（不替换）。

#### 接口与数据结构（Contracts）
- 渲染入口建议（名称以实现为准）：
  - `render_type4_persona_template(persona_files, vars_map) -> String`
  - `build_type4_vars(runtime_context, tools, skills, native_tools) -> HashMap<String, String>`

#### 失败模式与降级（Failure modes & Degrade）
- 文件缺失：对应段落替换为明确占位（例如 “（未配置）”），并保证整体拼接结构稳定。
- tools/skills 为空：对应 vars 必须为 `""`（避免无意义标题）。

#### 安全与隐私（Security/Privacy）
- vars 中不得包含：API keys、完整 remote_ip、精确 location（如需则只输出 coarse/布尔摘要）。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 四文件拼接 + 固定分隔线生效，且 `{{var}}` 替换对四文件均生效。
- [x] tools/skills 输出在同输入下字节级稳定（排序 + JSON canonicalize）。
- [x] `skills_md` / tools vars 的标题与说明中文化（与 overall v1 口径一致）。
- [x] `long_term_memory_md` / `voice_reply_suffix_md` 的 inclusion rule 生效（不适用时必须为 `""`）。
- [x] Template Validation 生效：缺失 hard-required var / unknown var / 遗留占位符都必须 fail-fast；缺失 soft-required var 必须 warning。
- [x] 增加测试覆盖 native/non-native/no-tools 三种模式（至少 1 组固定输入；canonical v1 unit tests + integration 覆盖）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 模板渲染：给定固定四文件文本 + vars_map，输出与断言一致（含 UTF-8/escape）。
- [x] 稳定性：对乱序输入（HashMap 顺序）重复渲染，输出顺序稳定（排序 + JSON key canonicalize）。

### Integration
- [x] gateway debug/raw_prompt 能看到模板化后的 persona 内容（按 provider/product）。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：部分 provider 适配差异需要运行时验证（Anthropic/local-llm）。
- 手工验证步骤：
  1. 配置一个 persona，在四文件中引用 `{{skills_md}}` 与 tools vars。
  2. 分别用 native tools 与 non-native tools provider 跑一轮，确认最终 prompt 中只出现对应段落，且无空标题。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先在默认 persona 上保证模板输出与现状硬编码等价（减少行为变化）；再开放给用户自定义变量引用。
- 回滚策略：保留旧硬编码 builder 作为 fallback（feature flag），出现兼容问题可切回。

## 实施拆分（Implementation Outline）
- Step 1: 引入 stability primitives（tools/skills 排序 + JSON canonicalize）。
- Step 2: 实现 vars_map 生成（skills/tools/runtime）。
- Step 3: 实现四文件拼接 + `{{var}}` 替换渲染器。
- Step 4: 将现有 prompt builder 迁移为“默认模板 + vars”路径（保持等价）。
- Step 5: 添加 golden/稳定性测试。
- 受影响文件：
  - `crates/agents/src/prompt.rs`
  - `crates/agents/src/tool_registry.rs`
  - `crates/skills/src/discover.rs`
  - `crates/skills/src/prompt_gen.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_type4_system_prompt_assembly_v1.md`
  - `issues/issue-persona-prompt-configurable-assembly-and-builtin-separation.md`

## 未决问题（Open Questions）
- Q1: Template Validation 的严格 fail-fast 是否要分阶段 rollout（feature flag / 提示期 / 直接强制）？
- Q2: 字面量 `{{...}}` 是否需要提供显式 escape 机制（还是仅规定“必须带空格/破坏语法”即可）？
- Q3: strip frontmatter 是否对四文件统一执行（建议统一执行，提升一致性）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

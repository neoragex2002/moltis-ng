# Issue: Persona Prompt 可配置化收敛（拆分 输入/固定块/布局；旧称 Source/Builtin/Assembly）

## 实施现状（Status）【增量更新主入口】
- Status: Phase 0 DONE（2026-03-01）；Phase 1+ TODO
- Priority: P0
- Updated: 2026-03-05
- Owners: <TBD>
- Components: agents / config / gateway / tools / ui
- Affected providers/models: openai-responses / others（当前存在 provider 路径差异）

**已实现（如有，写日期）**
- Persona 内容文件（可编辑）：`<data_dir>/people/<persona_id>/{IDENTITY.md,SOUL.md,TOOLS.md,AGENTS.md}`（UI 提示）：`crates/gateway/src/assets/js/page-settings.js:671`
- Persona 文件加载（默认 persona 路径与 named persona 路径规则）：`crates/config/src/loader.rs:251`
- (2026-03-04) Canonical System Prompt v1（跨 provider 层统一为 1 条 `ChatMessage::System`）：`crates/agents/src/prompt.rs:385`
- NOTE（2026-03-05）：Phase 0 初版设想的 `PromptBundle`/“Responses 三段 developer items”已被 canonical v1 的“单条 system → provider adapter 映射”模型取代；本文中 `PromptBundle`/三段 layering 的描述保留作历史审计与对比，不再作为现行实现依据。
- Prompt 生成入口（历史/legacy：OpenAI Responses 三段 developer preamble + 非 Responses 单段 system prompt）：`crates/agents/src/prompt.rs:478`
- gateway/chat 侧按 session/channel 选择 persona_id 并加载 persona：`crates/gateway/src/chat.rs:237`
- spawn_agent tool 侧也实现了“按 persona_id 合并加载”的一套逻辑（重复实现）：`crates/tools/src/spawn_agent.rs:42`
- Legacy（2026-03-01）：OpenAI Responses 三段 developer preamble（system/persona/runtime_snapshot）文案/布局冻结为中文，并在 `build_openai_responses_developer_prompts(...)` 中落地（保留为历史实现与兼容入口）。
- Phase 0（2026-03-01）：`include_tools=true/false` 不再导致 Responses developer item 1（system）分叉；该分叉点已从 builder 参数层面移除（Responses system 文案固定一份）。
- (2026-03-04) Responses as-sent 折叠：跨 provider 层仅 1 条 `ChatMessage::System`，openai-responses adapter 映射为单条 `role=developer` input item；debug endpoints 输出 `asSentPreamble`（长度=1）与 provider-aware `asSent` 摘要，并展示 `personaIdEffective`。
  - Evidence：`crates/gateway/src/chat.rs:254`、`crates/agents/src/providers/openai_responses.rs:34`、`crates/gateway/src/chat.rs:3679`
- Phase 0（2026-03-01）：`send_sync` 的两条错误持久化路径（keep-window overflow + run failed）不再写入 persisted `role=system`；改为 `role=assistant` 且带 `moltis_internal_kind="ui_error_notice"`，并在 LLM prompt 构造时过滤掉。
- Phase 0（2026-03-01）：写回止血：`save_user()` / `save_identity()` 仅更新 YAML frontmatter 的 managed keys，**正文永远原样保留**，且不再自动删除文件。
- Phase 0（2026-03-01）：Responses 注入 `IDENTITY.md` raw markdown 时剥离 YAML frontmatter（避免重复/噪声）。
- (2026-03-04) 语音模式固定块作为 canonical v1 模板变量 `voice_reply_suffix_md` 提供（不再在 gateway 针对 Responses runtime_snapshot 特判追加）；长期记忆提示块中文化并按 tool registry 生成。
  - Evidence：`crates/agents/src/prompt.rs:177`、`crates/agents/src/prompt.rs:321`

**已覆盖测试（如有）**
- Prompt 生成基础测试（包含 workspace files 注入等）：`crates/agents/src/prompt.rs:775`
- Personas CRUD/seed 测试：`crates/gateway/src/personas.rs:204`
- Baseline（2026-03-01）：`cargo test`（workspace，全量通过）

**已知差异/后续优化（非阻塞）**
- prompt 结构与大量文案目前硬编码散落，且 gateway/chat 与 spawn_agent 存在 persona merge 逻辑重复，易漂移。
- provider 路径存在“注入内容不一致”的硬编码差异（例如 `IDENTITY.md` raw 是否注入）。
- 术语收敛（低优先级）：将 UI/文档/对外口径中 “USER/USER.md/UserProfile/user” 的称呼统一为 **Owner**（更贴近实际语义：primary operator）；内部代码类型/文件名是否改名另行评估（改名面广、优先级可后置）。

---

## 背景（Background）
- 目标：让用户能够以“极简一致的心智模型”灵活调整 persona prompt（结构/顺序/哪些块出现/哪些块归属到 system/persona/runtime），同时**不损失 persona 的结构与内容**。
- 现状：persona 的“内容”大多来自文件，但“结构与规则文案”大量硬编码在 prompt builder 中；并且出现了“不同 provider 路径拼装行为不一致”的情况，导致难以预测与维护。
- 当前审计聚焦（legacy）：本文 Phase 0 最初聚焦 **OpenAI Responses / role=developer 的三段 persona preamble**（`system` / `persona` / `runtime_snapshot`）相关治理问题；自 2026-03-04 起已收敛为 canonical v1（跨 provider 层单条 `ChatMessage::System`，在 adapter 层映射），三段相关内容保留为历史审计记录。
- Phase 0 实施范围（Scope freeze, historical）：
  - Phase 0 最初只保证 `openai-responses` 路径的三段 developer preamble 口径正确生效；现已在 v1 中进一步收敛为“单条 system message → provider 映射”的统一模型（见：`issues/overall_type4_system_prompt_assembly_v1.md`）。
  - 非 Responses provider 的中文文案/章节结构仍可作为后续治理项，但不再阻断 v1 的入口收敛与 as-sent 证据链。
- Out of scope：
  - 不引入模板语言（如 mustache/jinja）以免用户构造无限复杂 prompt（冗余 + 不可控）。
  - 不改变 persona 内容的四文件模型（IDENTITY/SOUL/TOOLS/AGENTS）与 USER/PEOPLE 的存在意义。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **Prompt Input**（主称呼）：从磁盘/配置读取的可编辑内容（persona 四件套、USER、PEOPLE、project_context 等）。
  - Source/Method：configured（用户文件）+ effective（合并后的生效内容）。
- **Prompt FixedBlock**（主称呼）：由系统内置并保证一致的块（如 tool 使用指南、执行路由规则、voice suffix、people 引用提示等）。
  - Source/Method：authoritative（随版本发布的固定内容；可选支持 override，但必须明确优先级）。
- **Prompt Layout**（主称呼）：把 inputs + fixed blocks 按“块清单”拼成最终 prompt 的规则（顺序、标题、归属层、是否显示）。
  - Source/Method：effective（layout 合并后的生效清单）。
- **Layers**（层）：最终发送给模型的逻辑层（system / persona / runtime_snapshot）。
- **Owner**（主称呼）：Moltis 的 primary operator（用户本人）的档案信息（name/timezone/location 等）以及其对应的可编辑文本来源。
  - Source/Method：configured（`moltis.toml [user]` + `<data_dir>/USER.md` frontmatter / 或未来拆分文件）+ effective（合并后的生效 owner profile）
  - Aliases（仅记录，不在正文使用）：User / USER.md / UserProfile / user

> Aliases（仅记录，不在正文使用）：Prompt Source / Prompt Builtin / Prompt Assembly（旧称，逐步淘汰）

## 需求与目标（Requirements & Goals）
### 优先级口径（Prioritization）
> 本 issue 的阶段目标（现阶段）：**心智模型简洁**、**结构清晰明确**、**功能正确/可预测** 优先；安全/隐私/脱敏属于后续可迭代项，避免本末倒置。

### 功能目标（Functional）
- [ ] 明确并收敛 prompt 的总体结构：system / persona / runtime（以及 voice suffix），并把每一段归类为 Input 或 FixedBlock（不可再混写）。
- [ ] persona 的结构与内容不得损失：
  - persona 四件套（IDENTITY/SOUL/TOOLS/AGENTS）必须保留；
  - USER/PEOPLE 的语义必须保留（至少以 reference 形式）。
- [ ] Layout 可配置（灵活但不冗余）：
  - 允许调整块顺序、标题、是否显示；
  - 允许决定某些 builtins 注入到 system 或 runtime（但保持默认行为不变）。
- [ ] provider 路径行为一致：同一套 Layout 输出应在各 provider 上保持“内容一致”（仅映射到 API message roles 的方式不同）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：默认行为与当前一致（无 layout 配置时输出与现状等价）。
  - 不得：引入用户侧模板语言导致无限组合/难排障。
  - 必须：错误提示清晰（layout 配置非法/缺必需块时 fail-fast 并指向具体字段）。
- 兼容性：
  - 新增配置为可选；不要求用户迁移现有 persona 文件。
- 可观测性：
  - 允许在 debug/context 输出“effective prompt layout”摘要（块列表、来源、是否 override）。
- 安全与隐私：
  - 默认不内联 PEOPLE roster（保持 cache-friendly 与避免过大 prompt）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Persona prompt 结构复杂、梳理不清：缺乏一份“代码层面可读的集中管理表述”（到底有哪些块、每块属于哪一层、来源是什么、默认顺序是什么、哪些必选/可选）。
2) 可配置（文件/配置）与不可配置（硬编码）混杂：同一类内容的“来源/优先级/注入层级”不一致，且存在 provider 路径分叉，导致行为不可预测。
3) 大量硬编码散落各处：固定文案、默认 seed、引用路径、缺失占位符等没有统一的 FixedBlock 注册表；难以统一调整与评审，也不利于后续做更灵活但不冗余的配置化。
4) prompt 表述效果不佳：系统级 guidelines 与 persona 级内容的边界不够清晰，部分段落冗长/重复，影响模型理解与用户心智模型一致性。
5) USER.md 可能发生“数据被覆盖/丢失”：用户在 `<data_dir>/USER.md` 中写了自然语言 prompt/备注正文后，系统在自动持久化 timezone/location 时会调用 `save_user()` 覆盖写回模板化文件（并可能删除正文），导致用户辛苦维护的内容被意外抹掉（dirty behavior / data-loss risk）。
6) “我是谁”类问题效果差：当前 prompt 没有在 `Owner (USER.md)` 小节显式指令 agent “遇到 owner/user 身份相关问题要主动读取/参考 `<data_dir>/USER.md`”，导致当用户问“我是谁/我的信息是什么”时，agent 往往不会主动去该文件查找信息（心智模型与行为不一致）。
7) “你认识哪些人/有哪些 bots”类问题效果差：`People (reference)` 小节当前只给出路径引用且强调“不要内联 roster”，但没有显式指令 agent 在被问到“你认识哪些人/有哪些机器人/有哪些账号”时应主动读取并基于 `/moltis/data/PEOPLE.md` 作答，导致 agent 往往不会主动溯源该文件。
8) PEOPLE.md 的“可编辑性”心智模型不一致：当前 PEOPLE roster 文件会被 gateway 按 channels 配置自动再生（覆盖写回），若用户手工编辑 PEOPLE.md，后续 channels.add/remove/update 会覆盖掉手工内容（这在实现上是设计如此，但需要在 prompt/docs 中明确，否则属于 surprise / data-loss risk）。
9) IDENTITY.md 可能发生“数据被覆盖/丢失”：`agent.identity.update` / onboarding 会调用 `save_identity()` 用模板化文件覆盖写回 `<data_dir>/people/default/IDENTITY.md`，若用户在该文件中写了自然语言 prompt/备注正文，会被覆盖抹掉（dirty behavior / data-loss risk，与 USER.md 类似）。
10) “编辑入口/文件落点”口径不一致：UI 文案仍在暗示 identity 等落点是“workspace root”，但实际 canonical 路径已是 `<data_dir>/people/default/IDENTITY.md` / `<data_dir>/USER.md` 等，用户容易编辑错文件、认为“不生效”。
11) 文件“溯源可执行性”普遍不足：多处小节展示的是“结构化摘取结果”（如 Identity 字段行、Owner 两行），但未提供明确的“去哪里看完整源文件”的指引（路径、何时需要查），导致 agent 在被问到相关信息时不主动查源文件（治理不当/心智模型不一致）。
12) provider 分支导致 People 能力不一致：OpenAI Responses 路径存在 `People (reference)` 小节；非 Responses 路径当前没有等价的小节，导致“你认识哪些人/有哪些 bots”类问题在不同 provider 下表现不一致。
13) People reference 在非 sandbox 场景下可能不可执行：当前 People reference 固定引用 `/moltis/data/PEOPLE.md`（sandbox 内路径），但在 sandbox 未启用时该路径可能不存在，prompt 仍会给出该路径，造成“给了路径但不可达”的指引问题。
14) Workspace Files 章节口径可能误导：非 Responses prompt 的 `## Workspace Files` 小节会展示 AGENTS/TOOLS 内容，但标题写的是 `AGENTS.md (workspace)` / `TOOLS.md (workspace)`；而实际注入来源是 persona 的 `AGENTS.md`/`TOOLS.md`（`<data_dir>/people/<persona_id>/...`），容易导致 agent/用户去查错文件、认为修改不生效。
15) prompt build/layout 入口分散：同一类“生成最终 prompt”的逻辑在 gateway/chat 的多个路径（send / run_with_tools / run_streaming）与 `tools.spawn_agent` 各自内联实现；缺少单一权威入口导致 drift 风险高，也不利于统一治理与测试。
16) “UI 工具面板”假设泄漏到所有场景：Guidelines/Silent Replies 明确假设“用户 UI 有 tool 输出面板、无需复述”，并鼓励 tool 后“空回复”；但在 Telegram 等 channel 场景下用户并没有 tool 面板，这会导致“工具执行了但用户看不到结果/甚至无回复”的体验问题（prompt 治理不当：未按 surface 分层）。
17) prompt 行为受“运行模式”影响：同一 provider 下，`stream_only`/`run_streaming`/`run_with_tools` 等路径会生成不同的 prompt（是否 include_tools、是否注入 tool schemas/skills 等），导致行为随调用路径漂移，而非仅由“配置/布局/provider”决定（治理不可控）。
18) runtime/project context 的空占位符噪声：OpenAI Responses 路径在 project_context 缺失时会注入 `<no project context injected>`，同时保持固定小节标题，属于无意义 token 噪声，且可能影响模型对“是否应当寻找 project context”的判断。
19) `include_tools` 语义不够一致：在 OpenAI Responses builder 中，`include_tools` 主要控制 system guidelines/Execution routing，但 runtime_snapshot 仍可能包含 skills prompt / tool list（取决于调用方传入的 registry/skills），导致“include_tools=false 到底意味着什么”不清晰（心智模型不一致）。
20) 工具命名与调用指引不一致：同一产品内同时存在 `web_fetch` 与 `browser` 两个“网页相关工具”，但不同 prompt 分支的 Guidelines 分别引用不同工具名（Responses 引用 `web_fetch`，非 Responses 引用 `browser`），且没有在 prompt 内明确两者差异/优先级，容易导致 agent 调错工具或给出不一致行为。
21) runtime PII/敏感信息脱敏口径不一致：OpenAI Responses 的 runtime_snapshot 会显式清空 `remote_ip` 与 `location`，但非 Responses 的 `## Runtime` 会通过 `format_host_runtime_line` 注入 `remote_ip`/`location` 等字段；同时 runtime 还可能包含 channel 标识（account/chat id/handle）、sandbox image、data_mount 等信息，但目前没有一份“允许/禁止/需脱敏字段清单”的冻结口径。这导致不同 provider/模式下泄露风险与行为不一致（prompt 治理不当：隐私口径未冻结）。
22) “非 persona prompt” 的系统提示仍散落：除主 prompt builder 外，代码里还有若干 ad-hoc system prompt（对话压缩 summarizer、silent memory flush、TTS phrase 生成等），其质量/隐私/术语口径未纳入同一治理框架，后续极易成为新的 hardcode 漂移点。
23) Base intro 文案与能力不一致：当 include_tools=true 时，base intro 声称“access to tools for executing shell commands”，但随后同一段 Guidelines 又要求使用 `web_fetch` 做网页抓取；intro 与实际能力/工具集合不一致（结构不清晰、影响心智模型）。
24) Project Context 的“双重语义”混乱：persona 层有 `Workspace/Project Context (reference)`（仅提示“可能被注入”），runtime_snapshot 又有 `Project Context (snapshot, may change)`（实际注入/或 `<no project context injected>`）；同一概念拆成 reference+snapshot 两段，且位置跨 persona/runtime，容易造成“到底哪里是权威、是否需要查文件/是否已注入”的困惑。
25) “as-sent prompt” 不可追溯：gateway/chat 在 send 入口会先构造一份 `system_prompt` 仅用于 token 估算/compaction 决策；真正发送给模型时又会在 `run_with_tools` / `run_streaming` 里重新生成 prompt（且 registry/skills/include_tools 可能不同）。此外，OpenAI Responses 的 as-sent 形态是多条 `role=developer` items，但估算时常把三段先拼成一个字符串当作单条 system message 来估算，进一步放大“估算 vs 实发”的潜在偏差。这导致缺乏一个统一的可观测入口输出 as-sent 文本/块清单。
26) role=developer layering 形态不稳定：虽然 OpenAI Responses 设计是三段 developer preamble（system/persona/runtime_snapshot）并可按多条 message 保序发送，但当前不同调用点对 layering 的表达方式不一致（有的走 prefix_messages 多条 system message，有的把三段先拼成单字符串/或仅用于估算），增加治理与测试复杂度。
27) sub-agent 追加段落不一致：`tools.spawn_agent` 在 openai-responses 路径会额外硬编码追加 `## Sub-agent...` 指令，而主 agent 路径没有等价机制/可配置入口；同类“运行形态差异”缺少集中治理与明确口径。
28) IDENTITY.md raw 注入包含 frontmatter，导致重复与噪声：OpenAI Responses 路径在 `## Identity` 小节会先注入“结构化字段行”（从 frontmatter 抽取），随后又原样注入 `IDENTITY.md` raw markdown；由于 raw 当前包含 YAML frontmatter（seed 也包含），这会把同一信息重复两次并把 YAML 暴露给模型，降低结构清晰度与可预测性。
29) debug/context 口径与 OpenAI Responses “as-sent” 不一致：`chat.context` / `chat.full_context` 当前构造的是“单段 system prompt + openai chat-completions 格式 messages”，并声称“shows what the LLM actually saw”，但对 openai-responses 来说实际发送的是 `input[]` items 且 system 会被映射为 `role=developer`；因此 context view 会误导排障（看不到三段 developer layering，也看不到 as-sent input item 形态）。
30) hook/trace 的 messages 序列化口径不一致：runner 在 `BeforeLLMCall` hook payload 中一律把 typed messages 转成 `to_openai_value()`（chat-completions 形态）；对 openai-responses 来说这不是 as-sent 请求体形态，导致可观测性层面进一步丢失“最终发送给 Responses API 的 developer items”证据（与 Symptoms 25/29 叠加）。
31) debug/raw_prompt/context 在 persona 选择上可能漂移：`chat.raw_prompt` / `chat.context` / `chat.full_context` 当前直接调用 `load_prompt_persona()`（default persona），没有走 `resolve_session_persona_id(...)`（Telegram 绑定/会话 persona 路由），因此 debug 面板可能展示的并非“effective persona_id / effective persona sources”，进一步削弱可观测性与排障可信度。
32) `chat.raw_prompt` 不是 OpenAI Responses-aware：它只返回“单段 system prompt 字符串”（走 `build_system_prompt_*`），无法表达 Responses 的三段 developer preamble（system/persona/runtime_snapshot），因此不能作为 Responses 的 as-sent 证据。
33) channels/API 的 `send_sync` 预检/估算与 as-sent 可漂移：其 token 估算/compaction gating 使用的 prompt 构造路径与真正执行 `run_with_tools` / `run_streaming` 的 Responses 三段 layering/registry/skills 等可能不一致，导致“估算→触发 compaction/拒绝”基于另一份 prompt 作出决策。
34) `send_sync` 未传播 `_acceptLanguage`：调用 `run_with_tools` 时把 accept_language 传 `None`，导致 runtime_snapshot 与 web UI `chat.send` 路径不同（主路径 drift）。
35) `send_sync` 失败时会把 error 以 `role=system` 写入会话历史：之后这些 persisted `system` messages 会被 `values_to_chat_messages` 重新注入；在 OpenAI Responses 下会被映射为 `role=developer` input items，等价于把 `"[error] ..."` 变成高优先级 developer 指令，可能污染后续运行（poisoning risk）。
36) sub-agent 的 Responses runtime/project context 缺失：`tools.spawn_agent` 在 openai-responses 路径调用 `build_openai_responses_developer_prompts` 时传 `project_context=None`、`runtime_context=None`，导致 runtime_snapshot 缺少 host/sandbox snapshot 行（但仍可能包含 Execution routing），让子代理对实际执行环境判断更模糊。
37) sub-agent 缺少 hooks 可观测性：`tools.spawn_agent` 调用 `run_agent_loop_with_context_prefix(...)` 时把 `hook_registry=None`，即便主 agent 有 hooks，sub-agent 的 as-sent/hook 证据链也会断掉。

> Update（2026-03-05）：
> - 已在 Phase 0 落地中修复/缓解：29/30/31/32/33/34/35（canonical v1 builder、debug endpoints 的 `asSentPreamble`/`asSent`、hooks 的 `asSentSummary`、`_acceptLanguage` 传播、`ui_error_notice` 过滤等）。
> - 仍需后续评估：36/37（sub-agent 是否需要补齐 runtime/project context 与 hooks 证据链）。

### 归纳总结（Issue Taxonomy / Themes）
> 目标：把“散落的症状”收敛为少数几类可治理的问题，便于后续方案与验收对齐。

1) **溯源缺失（Traceability gap）**：prompt 输出了结构化摘要（Identity/Owner/Workspace Files），但缺少“source of truth 在哪、何时必须去读”的可执行指引，导致 agent 不主动查文件、用户难以排障（Symptoms 6/7/11/14）。
2) **自动写回导致内容丢失（Auto-write data loss）**：`save_user()`/`save_identity()` 覆盖写回模板，PEOPLE.md 自动再生覆盖写回，都会在用户“把 md 当 prompt 文档写”的场景下造成 surprise 与 data-loss risk（Symptoms 5/8/9）。
3) **provider 分支漂移（Provider divergence）**：Responses vs 非 Responses 的章节/内容不一致，导致同一问题在不同 provider 下行为不同（Symptoms 2/12/14 + 相关证据）。
4) **口径与命名不收敛（Mental model mismatch）**：UI/章节标题（workspace root / “(workspace)”）与实际文件落点/注入来源不一致，诱发“改了不生效/看错文件”（Symptoms 10/14）。
5) **治理结构缺失（Governance / Build sprawl）**：缺少“块清单 + 默认顺序 + required/optional + 层归属 + FixedBlock 注册表”的集中表述与校验，且 prompt build/layout 入口分散/受运行模式影响，导致硬编码散落与重复注入、难以统一审阅与测试（Symptoms 1/2/3/4/15/17/19）。
6) **Surface 未分层（UI vs Channel）**：prompt 中存在“UI 工具面板/空回复”假设，但没有按运行 surface（gateway UI / telegram 等）做差异化，导致 channel 体验退化（Symptoms 16）。
7) **结构化/手工内容未解耦（Auto vs Manual）**：结构化 profile（Owner/Identity frontmatter）与手工自然语言 prompt 文档混用同一文件，并存在自动覆盖写回；此外 PEOPLE roster 作为“reference”但本质为 auto-generated 文件。缺乏明确边界与写入策略，导致 data-loss risk 与治理困难（Symptoms 5/8/9/22）。
8) **可观测口径不一致（Observability mismatch）**：debug/context 与 hooks 仍以 chat-completions 的 messages 形态表达上下文，无法表达 OpenAI Responses 的 as-sent `input[]` developer items（含三段 layering），导致排障证据链断裂（Symptoms 25/29/30）。

### 影响（Impact）
- 用户体验：想做“极简但可控”的 persona 编排只能改代码；即便能改文件，也很难预测最终 prompt 结构与效果。
- 可靠性：重复实现与 provider 分叉易漂移，修一次容易漏另一处；散落硬编码导致一致性难以保障。
- 排障成本：缺乏“effective layout 清单”，出现“为什么这个 provider 有/没有某段 prompt”的困惑，难以复盘与对齐口径。
- 质量风险：prompt 文案难以集中 review，导致整体指令密度/清晰度不稳定，模型行为可能随迭代波动。
- 数据安全：`USER.md`/`IDENTITY.md`/`PEOPLE.md` 等被自动覆盖/再生会造成“看似 prompt 文档、实为系统管理文件”的心智模型崩坏，并带来内容丢失风险。

### 复现步骤（Reproduction）
1. 修改 persona 的 `IDENTITY.md` 正文，观察不同 provider 模式下是否被注入（现状可能不一致）。
2. 想把 `People` 章节改成内联/不同引用方式：当前只能改硬编码。
3. 想把 system 的 guidelines 关掉或换到 runtime：当前只能改硬编码。
4. 在已有 `<data_dir>/USER.md` 的情况下询问“我是谁？”：期望 agent 主动参考 `<data_dir>/USER.md` 给出基于文件的回答；实际经常只基于对话上下文/猜测，不会主动溯源到文件。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 可配置（source）证据：
  - persona 文件目录与读取路径规则：`crates/config/src/loader.rs:265`
  - Persona prompt 注入 SOUL/TOOLS/AGENTS 等：`crates/agents/src/prompt.rs:106`
  - gateway/chat persona 加载与 merge：`crates/gateway/src/chat.rs:763`
  - spawn_agent persona 加载与 merge（重复）：`crates/tools/src/spawn_agent.rs:42`
- 硬编码（builtin/layout）证据：
  - OpenAI Responses 将 `ChatMessage::System` 映射为 `role=developer` 的 input items（保序、多条）：`crates/agents/src/providers/openai_responses.rs:33`
  - system guidelines / silent replies 文案：`crates/agents/src/prompt.rs:20`
  - 执行路由规则文案：`crates/agents/src/prompt.rs:14`
  - People reference 固定路径引用（`/moltis/data`）：`crates/agents/src/prompt.rs:34`
  - voice reply suffix 文案：`crates/agents/src/prompt.rs:250`
  - 默认 persona seed 文本（IDENTITY/TOOLS/AGENTS 等）：`crates/gateway/src/personas.rs:33`
  - DEFAULT_SOUL 内置模板与缺文件自动 seed：`crates/config/src/loader.rs:378`
  - USER.md 自动覆盖写回（覆盖式模板渲染 + 可能删除正文）：`crates/config/src/loader.rs:568`
  - 自动持久化触发点（会调用 `save_user()` 覆盖写回）：
    - ws 首次连接自动写 timezone：`crates/gateway/src/ws.rs:299`
    - location.result 自动写 location：`crates/gateway/src/methods.rs:816`
    - channel 位置更新写 location：`crates/gateway/src/channel_events.rs:638`
  - UI 侧 Owner 编辑是“整文件原文写回”（与 `save_user()` 模板化覆盖写存在潜在冲突）：`crates/gateway/src/owner.rs:12`
  - PEOPLE.md 再生机制（覆盖写回 + 声明“Do not edit manually”）：`crates/gateway/src/people.rs:3`、`crates/gateway/src/people.rs:46`
  - PEOPLE.md 自动再生触发点（channels.add/remove/update）：`crates/gateway/src/channel.rs:213`
  - IDENTITY.md 模板化覆盖写回：`crates/config/src/loader.rs:527`
  - 覆盖写回触发点（identity 更新会同时写 IDENTITY.md 与 USER.md）：`crates/onboarding/src/service.rs:159`（`identity_update` 内调用 `save_identity`/`save_user`）、`crates/onboarding/src/wizard.rs:64`
  - UI 文案口径可能误导（workspace root vs `<data_dir>`）：`crates/gateway/src/assets/js/page-settings.js:417`
  - People reference 固定为 sandbox 路径：`crates/agents/src/prompt.rs:123`（`SANDBOX_DATA_DIR` 拼接）
  - 非 Responses prompt 未包含 People reference 小节：`crates/agents/src/prompt.rs:380`（见其章节清单）
  - Workspace Files 标注为 workspace 但实际注入 persona 文件：`crates/agents/src/prompt.rs:479`（渲染小节标题）+ persona 来源：`crates/gateway/src/chat.rs:799`
  - “UI 工具面板 / 空回复”假设：`crates/agents/src/prompt.rs:20`（SYSTEM_GUIDELINES_AND_SILENT_REPLIES）与 `crates/agents/src/prompt.rs:548`（非 Responses Guidelines/Silent Replies）
  - OpenAI Responses project_context 空占位符：`crates/agents/src/prompt.rs:174`
  - 运行模式导致 prompt 分叉（stream_only/run_with_tools/run_streaming）：`crates/gateway/src/chat.rs:2370`、`crates/gateway/src/chat.rs:4332`、`crates/gateway/src/chat.rs:5713`
  - 网页工具命名不一致（`web_fetch` vs `browser`）：`crates/agents/src/prompt.rs:20`（web_fetch）+ `crates/agents/src/prompt.rs:548`（browser）
  - runtime 脱敏不一致（Responses 清空 remote_ip/location；非 Responses 注入）：`crates/agents/src/prompt.rs:154`（clear）+ `crates/agents/src/prompt.rs:622`（remote_ip）+ `crates/agents/src/prompt.rs:623`（location）
  - 额外 system prompt（ad-hoc）：`crates/gateway/src/chat.rs:5543`（compaction summarizer）、`crates/agents/src/silent_turn.rs:131`（silent memory turn）、`crates/gateway/src/methods.rs:2557`（tts.generate_phrase）
  - Base intro 与工具能力不一致（shell exec vs web_fetch）：`crates/agents/src/prompt.rs:55`（base intro）+ `crates/agents/src/prompt.rs:22`（web_fetch guideline）
  - Project Context reference + snapshot 并存：`crates/agents/src/prompt.rs:148`（reference）+ `crates/agents/src/prompt.rs:174`（snapshot）
  - as-sent 不可追溯（估算用 prompt 与实际发送路径分离）：`crates/gateway/src/chat.rs:2370`（预构造用于估算）+ `crates/gateway/src/chat.rs:4332` / `crates/gateway/src/chat.rs:5713`（实际 run 内重建）
  - role=developer layering 形态分裂（多条 system vs 拼接字符串）：`crates/gateway/src/chat.rs:2391`（拼接字符串）+ `crates/gateway/src/chat.rs:4352`（prefix_messages 多条）+ `crates/tools/src/spawn_agent.rs:267`（prefix_messages 多条）
  - sub-agent 额外硬编码追加：`crates/tools/src/spawn_agent.rs:266`
  - IDENTITY.md raw 注入会包含 frontmatter：raw 加载不剥离 YAML：`crates/config/src/loader.rs:313`；raw 注入位置：`crates/agents/src/prompt.rs:97`；seed 默认 identity 含 frontmatter：`crates/gateway/src/personas.rs:53`
  - debug/context 当前不区分 openai-responses：一律用 `build_system_prompt_*` 生成单段 system prompt 并输出 openai messages（与 Responses as-sent developer items 不一致）：`crates/gateway/src/chat.rs:3429`
  - raw_prompt 当前不区分 openai-responses：一律用 `build_system_prompt_*` 生成单段 system prompt（无法表达 Responses 三段 developer preamble）：`crates/gateway/src/chat.rs:3781`（入口）+ `crates/gateway/src/chat.rs:3843`（实际构造）
  - raw_prompt/context/full_context 未走 session persona 路由：使用 `load_prompt_persona()`（default persona）：`crates/gateway/src/chat.rs:3665`、`crates/gateway/src/chat.rs:3906`（对比 persona 路由：`crates/gateway/src/chat.rs:237`）
  - channels/API 的 send_sync 预检/估算使用 default persona + 非 Responses prompt builder：`crates/gateway/src/chat.rs:2985`（`load_prompt_persona()`）+ `crates/gateway/src/chat.rs:2986`（`build_system_prompt_*`）+ `crates/gateway/src/chat.rs:3038`（估算入口）；后续实际执行走 `run_with_tools` / `run_streaming`（Responses 三段）：`crates/gateway/src/chat.rs:3113`
  - send_sync 未传播 accept_language：`crates/gateway/src/chat.rs:3151`
  - send_sync 失败时持久化 system error（后续注入→Responses developer items）：`crates/gateway/src/chat.rs:3046`（keep-window overflow）+ `crates/gateway/src/chat.rs:3202`（run failed）+ `crates/agents/src/model.rs:192` + `crates/agents/src/providers/openai_responses.rs:33`
  - spawn_agent 的 Responses runtime/project context 缺失且禁用 hooks：`crates/tools/src/spawn_agent.rs:250`、`crates/tools/src/spawn_agent.rs:263`、`crates/tools/src/spawn_agent.rs:281`
  - `BeforeLLMCall` hook payload 一律走 `to_openai_value()` 序列化（chat-completions 形态；非 Responses as-sent）：`crates/agents/src/runner.rs:768`
  - 文档口径漂移（影响 as-sent/可观测性理解）：
    - `docs/src/system-prompt.md:9`：仍按“单段 system prompt”描述 layout/build，不包含 OpenAI Responses 多 developer-item as-sent 形态。
    - `issues/done/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md:574`：声称 raw_prompt/as-sent 可观察到不同 persona developer message，但当前 raw_prompt/context 并未走 effective persona 路由且非 Responses-aware。
    - `issues/done/issue-named-personas-per-telegram-bot-identity-and-openai-developer-role.md:596`：提到 `ChatMessage::Developer`（当前 typed messages 并不存在该 role）。

### Persona Prompt 总体结构清单（As-is）【逐章：可配置/来源/证据】
> 说明：当前存在两种“最终 prompt 形态”：
> 1) **OpenAI Responses**：三段 developer preamble（`system` / `persona` / `runtime_snapshot`）。入口：`crates/agents/src/prompt.rs:40`
> 2) **非 Responses provider**：单段 system prompt（内部包含多个小节）。入口：`crates/agents/src/prompt.rs:380`
>
> 下文按“章节”逐一列出：每章是否可配置（不改代码）、可配置来源（文件在哪里）、以及硬编码位置（代码在哪里）。

#### A) OpenAI Responses / `system`（固定系统层）
1) Base intro（有/无 tools 两个版本）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:55`
   - 硬编码文本（精确）：
     - include_tools=true：
       ```text
       You are a helpful assistant with access to tools for executing shell commands.
       ```
     - include_tools=false：
       ```text
       You are a helpful assistant. Answer questions clearly and concisely.
       ```
2) Execution routing rules
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:14`
   - 硬编码文本（精确，`EXECUTION_ROUTING_RULES`）：
     ```text
     Execution routing:
     - `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
     - When sandbox is disabled, `exec` runs on the host and may require approval.
     - `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.
     - If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.
     ```
3) Guidelines + Silent Replies（系统通用行为准则）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:20`
   - 硬编码文本（精确，`SYSTEM_GUIDELINES_AND_SILENT_REPLIES`；仅 include_tools=true 注入）：
     ```text
     ## Guidelines
     
     - Use the `exec` tool to run shell commands when the user asks you to perform tasks that require system interaction (file operations, running programs, checking status, etc.).
     - Use the `web_fetch` tool to open URLs and fetch web page content when the user asks to visit a website, check a page, read web content, or perform web browsing tasks.
     - Always explain what you're doing before executing commands or fetching pages.
     - If a command or fetch fails, analyze the error and suggest fixes.
     - For multi-step tasks, execute one step at a time and check results before proceeding.
     - Be careful with destructive operations — confirm with the user first.
     - IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, summarize what happened, highlight key findings, or explain errors. Simply parroting the output wastes the user's time.
     
     ## Silent Replies
     
     When you have nothing meaningful to add after a tool call — the output speaks for itself — do NOT produce any text. Simply return an empty response.
     The user's UI already shows tool results, so there is no need to repeat or acknowledge them. Stay silent when the output answers the user's question.
     ```
   - 备注：当 include_tools=false 时，会注入另一段硬编码的“简版 Guidelines”（见 `crates/agents/src/prompt.rs:67`）：
     ```text
     ## Guidelines
     
     - Be helpful, accurate, and concise.
     - If you don't know something, say so rather than making things up.
     - For coding questions, provide clear explanations with examples.
     ```

#### B) OpenAI Responses / `persona`（persona 本体：结构固定，内容多为文件）
1) `# Persona: <persona_id>`
   - 可配置：部分（`persona_id` 可变；标题结构硬编码）
   - 来源：persona 选择（见“persona_id 选择与来源”）
   - 硬编码位置：`crates/agents/src/prompt.rs:75`
   - 硬编码文本（模板）：
     ```text
     # Persona: {persona_id}
     ```
2) `## Identity`
   - 可配置：
     - 内容：是（identity 字段、IDENTITY.md raw）
     - 结构/标题/合并规则：否（硬编码）
   - 来源：
     - `moltis.toml [identity]`（基础默认值）
     - `<data_dir>/people/<persona_id>/IDENTITY.md`（frontmatter + raw markdown）
     - 默认 persona：`<data_dir>/people/default/IDENTITY.md`
   - 证据：
     - 拼装位置：`crates/agents/src/prompt.rs:78`
     - 默认 persona identity 路径：`crates/config/src/loader.rs:275`
   - 硬编码文本（结构/标题/模板）：
     - 章节标题：
       ```text
       ## Identity
       ```
     - identity 字段行（模板，来自代码拼装而非文件原文）：
       ```text
       Your name is {name} {emoji}.
       Your name is {name}.
       You are a {creature}.
       Your vibe: {vibe}.
       ```
   - 现状限制（重要）：
     - 这几行是“结构化摘取”结果，但当前缺乏一条“溯源指引”告诉 agent：如果需要更完整/更准确的身份描述，应查看 `<data_dir>/people/<persona_id>/IDENTITY.md`（尤其是 raw markdown 部分），否则 agent 倾向只依赖这几行或上下文猜测。
3) `<IDENTITY.md missing>`
   - 可配置：否（硬编码占位符）
   - 硬编码位置：`crates/agents/src/prompt.rs:103`
   - 硬编码文本（精确）：
     ```text
     <IDENTITY.md missing>
     ```
4) `## Soul`
   - 可配置：
     - 内容：是（SOUL.md）
     - 结构/标题：否（硬编码）
   - 来源：
     - `<data_dir>/people/<persona_id>/SOUL.md`
     - 默认 persona：`<data_dir>/people/default/SOUL.md`
     - 缺文件兜底：`DEFAULT_SOUL`（会 seed 到磁盘）
   - 证据：
     - 拼装位置：`crates/agents/src/prompt.rs:106`
     - 默认 persona soul 路径：`crates/config/src/loader.rs:265`
     - 默认 soul 模板与 seed：`crates/config/src/loader.rs:379`
   - 硬编码文本（结构/标题）：
     ```text
     ## Soul
     ```
   - 备注：当 `SOUL.md` 缺失时的兜底“默认 SOUL 模板”是硬编码常量：`crates/config/src/loader.rs:379`
5) `## Owner (USER.md)`
   - 可配置：
     - 内容：是（USER.md frontmatter）
     - 结构/标题/字段格式：否（硬编码）
   - 来源：
      - `moltis.toml [user]`（基础默认值）
      - `<data_dir>/USER.md`（frontmatter 覆盖）
   - 现状限制（重要）：
     - 当前只注入 `name` 与 `timezone` 两项；`location_*`（latitude/longitude/place/updated_at）虽能被解析为 `UserProfile.location`，但不会进入 prompt。
     - 证据：解析 `location_*`：`crates/config/src/loader.rs:646`；Owner 注入仅 name/timezone：`crates/agents/src/prompt.rs:110`
     - 缺口：Owner 小节目前没有任何“可执行的溯源指引”，不会显式引导 agent 去查看 `<data_dir>/USER.md`（source of truth），导致用户难以建立一致心智模型（Owner 这两行从哪里来、如何改、如何确认生效）。
     - 风险：`USER.md` 被同时用于“用户可写 prompt 文本”和“结构化 UserProfile（自动持久化）”。当用户写了正文后，自动持久化会调用 `save_user()` 覆盖写回模板化文件，可能抹掉正文内容（data-loss risk）。
   - 证据：
     - 拼装位置：`crates/agents/src/prompt.rs:110`
     - USER 路径：`crates/config/src/loader.rs:280`
   - 硬编码文本（结构/标题/模板）：
     - 章节标题：
       ```text
       ## Owner (USER.md)
       ```
     - 字段行（模板）：
       ```text
       Owner / primary operator: {name}
       Timezone: {tz}
       ```
     - 缺失占位符（精确）：
       ```text
       <USER.md missing>
       ```
6) `## People (reference)`（仅引用，不内联 roster）
   - 可配置：
     - roster 文件内容：是（PEOPLE.md 可编辑）
     - 引用方式/提示文案/引用路径：否（硬编码）
   - 来源：
     - 文件：`<data_dir>/PEOPLE.md`
     - prompt 内引用的是 sandbox 路径：`/moltis/data/PEOPLE.md`
   - 现状限制（重要）：
     - prompt 明确写了 “do not inline the roster”，但没有补充“当用户问你认识哪些人/哪些 bots 时应主动读取并基于该文件回答”的指令，容易导致 agent 不主动溯源（与用户心智模型不一致）。
       - 证据：People reference 文案：`crates/agents/src/prompt.rs:123`
     - PEOPLE.md 文件当前由 gateway 基于 channels 配置自动再生并覆盖写回，文件头也明确标注 “Auto-generated … Do not edit manually.”；因此“用户可编辑 roster”在现实里并不稳定。
       - 证据：再生逻辑：`crates/gateway/src/people.rs:3`、`crates/gateway/src/people.rs:46`；触发点：`crates/gateway/src/channel.rs:213`
     - 可执行性风险：People reference 当前固定指向 `/moltis/data/PEOPLE.md`（sandbox 内路径）。当 sandbox 未启用时，该路径可能不可达，prompt 仍给出该路径会误导 agent（应当根据是否启用 sandbox 选择可达路径，或同时给出 host 与 sandbox 路径且明确何时用哪个）。
   - 证据：
     - 拼装与提示文案：`crates/agents/src/prompt.rs:123`
     - `SANDBOX_DATA_DIR="/moltis/data"`：`crates/agents/src/prompt.rs:34`
     - PEOPLE 文件路径：`crates/config/src/loader.rs:290`
   - 硬编码文本（精确；其中路径使用硬编码常量拼接）：
     ```text
     ## People (reference)
     
     For other agents/bots managed by this Moltis instance, see:
     - /moltis/data/PEOPLE.md
     Note: do not inline the roster here; keep this message cache-friendly.
     ```
7) `## Tools`（persona TOOLS.md）
   - 可配置：
     - 内容：是（TOOLS.md）
     - 结构/标题/缺失占位：否（硬编码）
   - 来源：`<data_dir>/people/<persona_id>/TOOLS.md`（默认 persona：`<data_dir>/people/default/TOOLS.md`）
   - 证据：
     - 拼装：`crates/agents/src/prompt.rs:128`
     - 默认 persona tools 路径：`crates/config/src/loader.rs:285`
   - 硬编码文本（结构/标题）：
     ```text
     ## Tools
     ```
8) `<TOOLS.md missing>`
   - 可配置：否（硬编码占位符）
   - 硬编码位置：`crates/agents/src/prompt.rs:135`
   - 硬编码文本（精确）：
     ```text
     <TOOLS.md missing>
     ```
9) `## Agents`（persona AGENTS.md）
   - 可配置：
     - 内容：是（AGENTS.md）
     - 结构/标题/缺失占位：否（硬编码）
   - 来源：`<data_dir>/people/<persona_id>/AGENTS.md`（默认 persona：`<data_dir>/people/default/AGENTS.md`）
   - 证据：
     - 拼装：`crates/agents/src/prompt.rs:138`
     - 默认 persona agents 路径：`crates/config/src/loader.rs:270`
   - 硬编码文本（结构/标题）：
     ```text
     ## Agents
     ```
10) `<AGENTS.md missing>`
   - 可配置：否（硬编码占位符）
   - 硬编码位置：`crates/agents/src/prompt.rs:145`
   - 硬编码文本（精确）：
     ```text
     <AGENTS.md missing>
     ```
11) `## Workspace/Project Context (reference)`（固定提示）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:148`
   - 硬编码文本（精确）：
     ```text
     ## Workspace/Project Context (reference)
     
     Project/workspace rules may be injected separately per run. If present, treat them as authoritative for that scope.
     ```

#### C) OpenAI Responses / `runtime_snapshot`（运行时快照：结构硬编码，内容来源多样）
1) `## Runtime (snapshot, may change)`（Host + Sandbox 行）
   - 可配置：部分（运行时字段来自环境/会话；展示格式硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:151`（含 sandbox 行格式：`crates/agents/src/prompt.rs:645`）
   - 硬编码文本（结构/标题）：
     ```text
     ## Runtime (snapshot, may change)
     ```
   - 硬编码文本（Host/Sandbox 行格式模板）：
     - Host 行：由 `format_host_runtime_line` 拼装（key 列表硬编码）：`crates/agents/src/prompt.rs:581`
       - 模板形态（示意）：`Host: key=value | key=value | ...`
     - Sandbox 行：`crates/agents/src/prompt.rs:687`
       - 模板形态（示意）：`Sandbox(exec): enabled=true | mode=... | backend=... | ...`
2) Execution routing rules（重复注入）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:169`
3) `## Project Context (snapshot, may change)`
   - 可配置：部分（project_context 可变；标题/结构硬编码）
   - 来源：project_context（由项目上下文注入，典型是 CLAUDE.md/AGENTS.md 等汇总文本）
   - 硬编码位置：`crates/agents/src/prompt.rs:174`
   - 硬编码文本（结构/标题 + 缺省占位符）：
     ```text
     ## Project Context (snapshot, may change)
     
     <no project context injected>
     ```
4) Skills prompt（可用 skills 列表）
   - 可配置：部分（skills 安装/发现会变；结构硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:184`
5) `## Long-Term Memory`（当 memory_search tool 存在）
   - 可配置：部分（是否出现取决于工具注册；结构硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:188`
   - 硬编码文本（精确）：
     ```text
     ## Long-Term Memory
     
     You have access to a long-term memory system. Use `memory_search` to recall past conversations, decisions, and context. Search proactively when the user references previous work or when context would help.
     ```
6) `## Available Tools`（compact list 或 full schemas）
   - 可配置：部分（tools 来自 ToolRegistry；结构硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:202`
   - 硬编码文本（结构/标题）：
     ```text
     ## Available Tools
     ```
   - 硬编码文本（native_tools=true 的“紧凑列表”格式模板）：
     - 无描述：`- `{name}``
     - 有描述：`- `{name}`: {desc_truncated_to_160_chars}`
     - 证据：`crates/agents/src/prompt.rs:205`
   - 硬编码文本（native_tools=false 的“全 schema 列表”格式模板）：
     ```text
     ### {name}
     {desc}
     
     Parameters:
     ```json
     {pretty_json_schema}
     ```
     ```
     - 证据：`crates/agents/src/prompt.rs:217`

#### D) 非 Responses provider（单段 system prompt：章节更少，且与 Responses 不完全一致）
1) Base intro（有/无 tools 两个版本）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:400`
   - 硬编码文本（精确）：
     - include_tools=true：
       ```text
       You are a helpful assistant with access to tools for executing shell commands.
       ```
     - include_tools=false：
       ```text
       You are a helpful assistant. Answer questions clearly and concisely.
       ```
2) Identity 字段（name/emoji/creature/vibe）
   - 可配置：是（内容）；结构硬编码
   - 来源：`moltis.toml [identity]` + `<data_dir>/people/<id>/IDENTITY.md` frontmatter（注意：此路径不注入 raw markdown）
   - 硬编码位置：`crates/agents/src/prompt.rs:407`
   - 硬编码文本（模板）：
     ```text
     Your name is {name} {emoji}.
     Your name is {name}.
     You are a {creature}.
     Your vibe: {vibe}.
     ```
3) `## Soul`
   - 可配置：是（内容）；结构硬编码
   - 来源：`<data_dir>/people/<id>/SOUL.md`（或默认 persona / 默认模板）
   - 硬编码位置：`crates/agents/src/prompt.rs:425`
   - 硬编码文本（结构/标题）：
     ```text
     ## Soul
     ```
4) User name line（The user's name is …）
   - 可配置：是（内容）；结构硬编码
   - 来源：`moltis.toml [user]` + `<data_dir>/USER.md` frontmatter
   - 硬编码位置：`crates/agents/src/prompt.rs:430`
   - 硬编码文本（模板）：
     ```text
     The user's name is {name}.
     ```
   - 备注：该路径当前不注入 timezone/location 等额外字段（与 Responses 的 Owner 小节不一致）。
5) Project context 注入（CLAUDE.md/AGENTS.md 等）
   - 可配置：部分（project_context 可变；结构硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:439`
6) `## Runtime`（host+sandbox）
   - 可配置：部分（运行时字段来自环境/会话；结构硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:446`
   - 硬编码文本（结构/标题）：
     ```text
     ## Runtime
     ```
   - 备注：在该小节内，当 include_tools=true 时会注入与 `EXECUTION_ROUTING_RULES` 等价的“Execution routing”固定文案：`crates/agents/src/prompt.rs:461`
7) Skills prompt
   - 可配置：部分
   - 硬编码位置：`crates/agents/src/prompt.rs:473`
8) `## Workspace Files`（注入 AGENTS/TOOLS 文本）
   - 可配置：是（内容）；结构硬编码
   - 来源：当前把 persona 的 `AGENTS.md/TOOLS.md` 注入到此小节
   - 硬编码位置：`crates/agents/src/prompt.rs:479`
   - 硬编码文本（结构/标题）：
     ```text
     ## Workspace Files
     
      ### AGENTS.md (workspace)
      
      ### TOOLS.md (workspace)
      ```
   - 现状限制（重要）：
     - 小节标题标注为 “(workspace)”，但实际注入的是 persona 文件（`<data_dir>/people/<persona_id>/AGENTS.md` 与 `.../TOOLS.md`）；此外 gateway/server 还会 seed `<data_dir>/AGENTS.md` 与 `<data_dir>/TOOLS.md`（历史遗留），进一步加剧“到底该看哪个文件”的心智模型混乱。
9) `## Available Tools` / `## How to call tools` / `## Guidelines` / `## Silent Replies`
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:507` / `:536` / `:548`
   - 硬编码文本（结构/标题；与 Responses runtime_snapshot 同口径）：
     ```text
     ## Available Tools
     ```
   - 硬编码文本（`## How to call tools`；仅 `native_tools=false` 且存在 tools 时注入）：`crates/agents/src/prompt.rs:536`
     ```text
     ## How to call tools
     
     To call a tool, output ONLY a JSON block with this exact format (no other text before it):
     
     ```tool_call
     {"tool": "<tool_name>", "arguments": {<arguments>}}
     ```
     
     You MUST output the tool call block as the ENTIRE response — do not add any text before or after it.
     After the tool executes, you will receive the result and can then respond to the user.
     ```
   - 硬编码文本（`## Guidelines` + `## Silent Replies`；仅 include_tools=true 注入；注意这里使用 `browser` tool 而非 `web_fetch`）：`crates/agents/src/prompt.rs:548`
     ```text
     ## Guidelines
     
     - Use the exec tool to run shell commands when the user asks you to perform tasks that require system interaction (file operations, running programs, checking status, etc.).
     - Use the browser tool to open URLs and interact with web pages. Call it when the user asks to visit a website, check a page, read web content, or perform any web browsing task.
     - Always explain what you're doing before executing commands or opening pages.
     - If a command or browser action fails, analyze the error and suggest fixes.
     - For multi-step tasks, execute one step at a time and check results before proceeding.
     - Be careful with destructive operations — confirm with the user first.
     - IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, summarize what happened, highlight key findings, or explain errors. Simply parroting the output wastes the user's time.
     
     ## Silent Replies
     
     When you have nothing meaningful to add after a tool call — the output speaks for itself — do NOT produce any text. Simply return an empty response.
     The user's UI already shows tool results, so there is no need to repeat or acknowledge them. Stay silent when the output answers the user's question.
     ```
10) Voice reply suffix（当 medium=voice 时追加）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:290`
   - 硬编码文本（精确，`VOICE_REPLY_SUFFIX`）：`crates/agents/src/prompt.rs:290`
     ```text
     ## Voice Reply Mode
     
     The user will hear your response as spoken audio. Write for speech, not for reading:
     - Use natural, conversational sentences. No bullet lists, numbered lists, or headings.
     - NEVER include raw URLs. Instead describe the resource by name (e.g. "the Rust documentation website" instead of "https://doc.rust-lang.org").
     - No markdown formatting: no bold, italic, headers, code fences, or inline backticks.
     - Spell out abbreviations that a text-to-speech engine might mispronounce (e.g. "API" → "A-P-I", "CLI" → "C-L-I").
     - Keep responses concise — two to three short paragraphs at most.
     - Use complete sentences and natural transitions between ideas.
     ```
11) include_tools=false 时的简版 Guidelines（固定文案）
   - 可配置：否（硬编码）
   - 硬编码位置：`crates/agents/src/prompt.rs:569`
   - 硬编码文本（精确）：
     ```text
     ## Guidelines
     
     - Be helpful, accurate, and concise.
     - If you don't know something, say so rather than making things up.
     - For coding questions, provide clear explanations with examples.
     ```

#### persona_id 选择与来源（当前行为）
- Telegram bot 可绑定 persona：`TelegramAccountConfig.persona_id`：`crates/telegram/src/config.rs:80`
- session 侧只对 `channel=telegram` 解析 persona_id（否则为 default persona）：`crates/gateway/src/chat.rs:237`
- gateway/chat persona 文件合并加载：`crates/gateway/src/chat.rs:763`
- spawn_agent tool persona 文件合并加载（重复一套）：`crates/tools/src/spawn_agent.rs:42`

#### 默认 persona seed（当前行为）
- 默认 persona 必存在且会 seed 四个文件（IDENTITY/SOUL/TOOLS/AGENTS），seed 文案写死：`crates/gateway/src/personas.rs:47`
- `SOUL.md` 缺文件时还会被 `DEFAULT_SOUL` 自动写入（写死模板）：`crates/config/src/loader.rs:379`

### “拼接完整 prompt” 的入口分布（As-is，按当前代码结构）
> 结论：当前并不存在单一的“统一入口函数”贯穿所有调用方；prompt build/layout（sources merge + provider 分支 + 最终传入 runner）是**内联在多个调用点**中的。
>
> 本节用于在代码层明确“今天真实的拼装入口在哪里”，并记录“分散/重复”的事实（先剖析问题，不急于定方案）。

1) 主入口 A（用户对话 / 非 streaming）：`gateway/chat.send` 内部拼装
   - 位置：`crates/gateway/src/chat.rs:2368`（persona 加载）+ `crates/gateway/src/chat.rs:2370`（provider 分支）
   - Responses 路径调用三段 preamble：`crates/gateway/src/chat.rs:2372` / `crates/gateway/src/chat.rs:2410`（stream_only vs 非 stream_only 两条分支）
   - 非 Responses 路径调用单段 system prompt：`crates/gateway/src/chat.rs:2461` / `crates/gateway/src/chat.rs:2474`
   - persona sources 合并加载：`load_prompt_persona_with_id`：`crates/gateway/src/chat.rs:763`

2) 主入口 B（用户对话 / with tools runner）：`run_with_tools` 内部拼装
   - 位置：`crates/gateway/src/chat.rs:4293`（函数入口）+ `crates/gateway/src/chat.rs:4332`（provider 分支）
   - Responses 路径：`crates/gateway/src/chat.rs:4333`（并额外构造 `prefix_messages`，与 send 入口形态不同）
   - 非 Responses 路径：`crates/gateway/src/chat.rs:4368` / `crates/gateway/src/chat.rs:4382`
   - persona sources 合并加载：`crates/gateway/src/chat.rs:4317`

3) 主入口 C（用户对话 / streaming runner）：`run_streaming` 内部拼装
   - 位置：`crates/gateway/src/chat.rs:5665`（persona 加载）+ `crates/gateway/src/chat.rs:5713`（provider 分支）
   - Responses 路径：`crates/gateway/src/chat.rs:5714`（与 send 入口一样用 ToolRegistry::new 且 include_tools=false）
   - 非 Responses 路径：`crates/gateway/src/chat.rs:5737`

4) 次入口（子代理）：`tools.spawn_agent` 内部拼装
   - 位置：`crates/tools/src/spawn_agent.rs:243`（`is_openai_responses` 分支入口）
   - Responses 路径：`crates/tools/src/spawn_agent.rs:250`
   - 额外硬编码追加（仅 sub-agent / openai-responses 路径）：`crates/tools/src/spawn_agent.rs:266`
     ```text
     ## Sub-agent
     
     You are a sub-agent spawned to complete the user's task thoroughly and return a clear result.
     ```
   - 非 Responses 路径：`crates/tools/src/spawn_agent.rs:287` / `crates/tools/src/spawn_agent.rs:300`
   - persona sources 合并加载（重复实现）：`load_persona`：`crates/tools/src/spawn_agent.rs:42`

5) 实际渲染器（最终产出文本的核心 builder）
   - OpenAI Responses 三段 developer preamble：`crates/agents/src/prompt.rs:40`
   - 非 Responses 单段 system prompt：`build_system_prompt_full`：`crates/agents/src/prompt.rs:381`

### 最终拼接 prompt 示例（As-is：突出硬编码；可配置内容用占位符）
> 说明：以下示例仅用于展示“结构 + 硬编码文案”，不会内联真实 persona 文件内容、真实项目上下文、真实工具 schema。

#### 示例 1：OpenAI Responses（三段 developer preamble 被拼接为一个 system_prompt 字符串）
> 形态：`{system}\n\n{persona}\n\n{runtime_snapshot}`（见 `crates/gateway/src/chat.rs:2372` / `crates/tools/src/spawn_agent.rs:267`）

```text
You are a helpful assistant with access to tools for executing shell commands.

Execution routing:
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
- When sandbox is disabled, `exec` runs on the host and may require approval.
- `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.
- If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.

## Guidelines

- Use the `exec` tool to run shell commands when the user asks you to perform tasks that require system interaction (file operations, running programs, checking status, etc.).
- Use the `web_fetch` tool to open URLs and fetch web page content when the user asks to visit a website, check a page, read web content, or perform web browsing tasks.
- Always explain what you're doing before executing commands or fetching pages.
- If a command or fetch fails, analyze the error and suggest fixes.
- For multi-step tasks, execute one step at a time and check results before proceeding.
- Be careful with destructive operations — confirm with the user first.
- IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, summarize what happened, highlight key findings, or explain errors. Simply parroting the output wastes the user's time.

## Silent Replies

When you have nothing meaningful to add after a tool call — the output speaks for itself — do NOT produce any text. Simply return an empty response.
The user's UI already shows tool results, so there is no need to repeat or acknowledge them. Stay silent when the output answers the user's question.


# Persona: <persona_id>

## Identity

Your name is <name> <emoji>. You are a <creature>. Your vibe: <vibe>.

<IDENTITY.md raw markdown>  # (可配置文件：<data_dir>/people/<persona_id>/IDENTITY.md)

## Soul

<SOUL.md content>           # (可配置文件：<data_dir>/people/<persona_id>/SOUL.md；缺失时用 DEFAULT_SOUL)

## Owner (USER.md)

Owner / primary operator: <user_name>
Timezone: <timezone>

## People (reference)

For other agents/bots managed by this Moltis instance, see:
- /moltis/data/PEOPLE.md
Note: do not inline the roster here; keep this message cache-friendly.

## Tools

<TOOLS.md content>          # (可配置文件：<data_dir>/people/<persona_id>/TOOLS.md)

## Agents

<AGENTS.md content>         # (可配置文件：<data_dir>/people/<persona_id>/AGENTS.md)

## Workspace/Project Context (reference)

Project/workspace rules may be injected separately per run. If present, treat them as authoritative for that scope.


## Runtime (snapshot, may change)

Host: host=<host> | os=<os> | arch=<arch> | shell=<shell> | provider=<provider> | model=<model> | sessionId=<session_id> | channel=<channel> | channel_account_id=<acct_id> | channel_account_handle=<acct_handle> | channel_chat_id=<chat_id> | sudo_non_interactive=<bool> | sudo_status=<status> | timezone=<tz> | accept_language=<lang>
Sandbox(exec): enabled=<bool> | mode=<mode> | backend=<backend> | scope=<scope> | image=<image> | data_mount=<data_mount> | network=<enabled_or_disabled> | session_override=<bool>

Execution routing:
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
- When sandbox is disabled, `exec` runs on the host and may require approval.
- `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.
- If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.

## Project Context (snapshot, may change)

<project_context>           # (可配置输入：项目上下文注入；缺省为 "<no project context injected>")

<skills_prompt>             # (动态：skills 发现结果)

## Long-Term Memory          # (仅当 memory_search tool 存在)

You have access to a long-term memory system. Use `memory_search` to recall past conversations, decisions, and context. Search proactively when the user references previous work or when context would help.

## Available Tools

- `exec`: <desc...>          # (native_tools=true 时为紧凑列表；否则为全 schema)
- `web_fetch`: <desc...>
- ...
```

#### 示例 2：非 Responses provider（单段 system prompt；native_tools=true）
> 形态：`build_system_prompt_full(... include_tools=true)`（见 `crates/agents/src/prompt.rs:380`）

```text
You are a helpful assistant with access to tools for executing shell commands.

Your name is <name> <emoji>. You are a <creature>. Your vibe: <vibe>.

## Soul

<SOUL.md content>

The user's name is <user_name>.

<project_context>           # (可配置输入：项目上下文注入)

## Runtime

Host: ...
Sandbox(exec): ...
Execution routing:
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
- ...

<skills_prompt>             # (动态：skills)

## Workspace Files

### AGENTS.md (workspace)

<AGENTS.md content>         # (可配置文件：<data_dir>/people/<persona_id>/AGENTS.md)

### TOOLS.md (workspace)

<TOOLS.md content>          # (可配置文件：<data_dir>/people/<persona_id>/TOOLS.md)

## Long-Term Memory          # (仅当 memory_search tool 存在)

You have access to a long-term memory system. Use `memory_search` to recall past conversations, decisions, and context. Search proactively when the user references previous work or when context would help.

## Available Tools

- `exec`: <desc...>
- `browser`: <desc...>
- ...

## Guidelines

- Use the exec tool to run shell commands when the user asks you to perform tasks that require system interaction (file operations, running programs, checking status, etc.).
- Use the browser tool to open URLs and interact with web pages. Call it when the user asks to visit a website, check a page, read web content, or perform any web browsing task.
- Always explain what you're doing before executing commands or opening pages.
- If a command or browser action fails, analyze the error and suggest fixes.
- For multi-step tasks, execute one step at a time and check results before proceeding.
- Be careful with destructive operations — confirm with the user first.
- IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, summarize what happened, highlight key findings, or explain errors. Simply parroting the output wastes the user's time.

## Silent Replies

When you have nothing meaningful to add after a tool call — the output speaks for itself — do NOT produce any text. Simply return an empty response.
The user's UI already shows tool results, so there is no need to repeat or acknowledge them. Stay silent when the output answers the user's question.
```

#### 示例 3：非 Responses provider（单段 system prompt；native_tools=false，会额外出现 tool_call 规范）
```text
...（同示例 2 的前半部分：intro/identity/soul/user/project/runtime/skills/workspace/tools）...

## How to call tools

To call a tool, output ONLY a JSON block with this exact format (no other text before it):

```
{"tool": "<tool_name>", "arguments": {<arguments>}}
```

You MUST output the tool call block as the ENTIRE response — do not add any text before or after it.
After the tool executes, you will receive the result and can then respond to the user.
```

## 根因分析（Root Cause）
- A. “输入/固定块/布局（Input/FixedBlock/Layout）”未分层：导致硬编码文案与文件内容混在一个 builder 内。
- B. Persona merge 逻辑重复：gateway/chat 与 spawn_agent 各自实现合并，增加漂移概率。
- C. provider 分叉：不同 provider 路径对相同输入采取不同注入策略，导致行为不一致。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - prompt 的每一段必须有明确归属：**输入（Input）**或**固定块（FixedBlock）**；**布局（Layout）**只做“拼装”，不再直接写大段硬编码文案。
  - persona 四件套 + USER/PEOPLE 不丢失（至少保留 reference 口径）。
  - `Owner (USER.md)` 小节必须包含一条简短、明确的溯源指引：显式提示 agent 以 `<data_dir>/USER.md` 为 source of truth，并保持提示文本极简。
  - 对于“我是谁/我的信息是什么/用户身份”类问题，agent 必须被 prompt 明确引导：优先溯源 `<data_dir>/USER.md`（以及其 frontmatter 约定）后再回答，避免凭空猜测。
  - 对于“你认识哪些人/有哪些 bots/有哪些账号”类问题，agent 必须被 prompt 明确引导：优先溯源 `/moltis/data/PEOPLE.md`（reference）后再回答，并在文案中明确 PEOPLE.md 是否可编辑/是否会被自动再生（避免 surprise）。
  - 默认输出与现状等价（无 layout 配置时）。
  - provider 之间 prompt 内容一致（仅 role 映射不同）。
- 不得：
  - 不得引入复杂模板语言与“自由拼字符串”导致冗余与不可控。
- 应当：
  - 允许用一个极简 layout 文件完成 90% 的结构调整需求（顺序/标题/开关/层归属）。
  - 提供一个“集中管理表述”：开发者能在一个位置看清 prompt 的块清单、默认顺序、required/optional、来源与注入层级，并可在 debug/context 查看 effective layout。
  - 对外口径的命名应当收敛：UI/文档/Prompt 小节标题优先使用 **Owner**，避免混用 User/USER.md/UserProfile/user 造成歧义（此项可后置实现）。

## 方案（Proposed Solution）
### 术语收敛（Renaming，避免混淆）
> 说明：这里先收敛“概念名”，后续落代码时再决定是否沿用为结构体/模块名。

- `PromptSources` → `PromptInputs`：来自文件/配置/运行态拼装前的只读输入（可配置来源）。
- `PromptBuiltins` → `PromptFixedBlocks`：代码内置、固定且可测试的文本块（不让调用点散落硬编码文案）。
- `PromptAssembly` → `PromptLayout`：块清单/顺序/分层规则（只负责“怎么拼装”，不直接写大段文案）。

### 方案对比（Options，可选）
#### 方案 1（推荐）：分层收敛 + 受限 layout（无模板语言）
- 核心思路：
  - 定义 `PromptInputs`（只读输入：文件/配置）+ `PromptFixedBlocks`（固定块）+ `PromptLayout`（块清单/顺序/分层）。
  - 新增可选 `LAYOUT.toml`（全局或 persona 级）只表达“块清单”，不允许自由模板。
- 优点：心智模型极简、可验证、可测试、可回滚；默认行为可保持完全一致。
- 风险/缺点：需要一次性重构 prompt builder 的组织方式（但不要求更改用户 persona 内容）。

#### 方案 2（不推荐）：用户自定义 PROMPT.md 模板（占位符拼接）
- 优点：极灵活。
- 缺点：难以保证“结构不丢失”、容易冗余/重复/不可控，且校验复杂，违背极简心智模型。

### 最终方案（Chosen Approach）
采用方案 1，但**必须分阶段落地**：先做 Phase 0 的 quick wins（收敛“统一入口 + as-sent 可观测 + prompt 关键文本治理 + 止血写回”），再推进 layout 的更深收敛。

#### 阶段计划（Phases）
> 约束：本节只细化 **Phase 0**；Phase 1+ 仅列出大方向，避免一次性设计过度。

- **Phase 0（Quick Wins，最优先）**
  - **P-Prep：实施前筹备（Phase 0 gate，先做再动代码）**
    1) 冻结 Phase 0 的“对外可观测契约”（避免边改边漂移）
       - 明确 `PromptBundle` 的字段清单与含义（as-sent vs estimate vs debug），以及缺省行为（默认应与现状等价）。
       - 冻结 debug RPC 的返回形态：
         - `chat.raw_prompt` / `chat.context` / `chat.full_context` 在 **OpenAI Responses** 下必须能直接展示 as-sent 的 `input[]` developer items（三段 + 顺序），并显示 `personaIdEffective`。
         - 非 Responses provider：仍展示单段 system prompt（但也必须显示 `personaIdEffective`）。
       - 约束：所有 debug surfaces 必须可回答“到底发给 Responses 的 developer prompt 是哪三段、每段是什么、顺序是什么”。
    2) 列清“必须迁移到统一入口”的调用点清单（并在代码里加 fail-fast 防漏）
       - gateway/chat：`send` / `run_with_tools` / `run_streaming` / `send_sync`。
       - debug endpoints：`chat.raw_prompt` / `chat.context` / `chat.full_context`。
       - tools：`tools.spawn_agent`。
       - 要求：Phase 0 合入后不允许这些调用点继续直接调用旧的 prompt builders（否则视为未完成 Phase 0）。
    3) 锁定两条“策略选择”（Phase 0 视为已决，避免实现反复返工）
       - `send_sync` 的错误可见性：保留可见，但**不得**以 persisted `role=system` 写入会话历史（Responses 下会变 developer poisoning）；Phase 0 采用最小改动口径：持久化为 `role=assistant` 且仍用 `[error] ...` 前缀。
       - `save_user()` / `save_identity()`：**不允许自动删除** `USER.md` / `IDENTITY.md`（它们是用户资产文档）；系统只更新 YAML frontmatter 的 managed keys，正文永远原样保留（即便 managed keys 全为空，也只移除/清空 frontmatter，不删文件）。
    4) 提前评估 UI 影响面（避免后端改完 UI 立刻炸）
       - debug/context 展示需要支持 `role=developer`（Responses as-sent）或提供单独的 “as-sent developer items” 展示结构。
       - 约束：UI 必须能清晰区分 preamble（三段）与历史消息（history），并能复制/导出用于排障。
    5) 先跑一遍基线测试（把旧红当新红会浪费大量时间）
       - 建议最少跑：`cargo test -p moltis-agents`、`cargo test -p moltis-gateway`（如涉及 UI/E2E，再补 Playwright）。

  - **P0：统一入口 + as-sent 可观测（OpenAI Responses / role=developer）**
    1) 建立一个**唯一权威**的“整体 prompt 生成入口（PromptBundle builder）”，一次返回：
       - OpenAI Responses：三段 developer preamble（`system`/`persona`/`runtime_snapshot`）的 **as-sent** 形态（可直接映射为 `input[]` items 的文本序列）
       - 估算/compaction 用的稳定文本（method/source 明确：estimate vs as-sent）
       - `persona_id_effective`（debug/RPC 对外暴露为 `personaIdEffective`）、`include_tools`、`native_tools`、skills、runtime_context、project_context 等元信息
       - 建议落点：
         - `crates/agents/src/prompt.rs`：新增 `PromptBundle` + `build_prompt_bundle(...)`（内部复用现有 `build_openai_responses_developer_prompts(...)` 与非 Responses builders）
         - `crates/gateway/src/chat.rs`：新增一个 gateway wrapper（负责 resolve persona_id / load persona sources / filter registry / discover skills / build runtime_context / project_context），最终只调用 `build_prompt_bundle(...)`
    2) 把以下调用点全部改为**只调用该入口**（否则视为未完成 Phase 0）：
       - `gateway/chat.send`（含 token estimate / preflight compaction）
       - `run_with_tools`、`run_streaming`
       - `chat.raw_prompt` / `chat.context` / `chat.full_context`
       - `send_sync`
       - `tools.spawn_agent`
    3) debug endpoints 的输出口径冻结为“as-sent 视图”：
       - OpenAI Responses：展示 developer items（三段 + 顺序 + `personaIdEffective`）
       - 非 Responses：展示最终单段 system prompt
    4) **Prompt 关键文本治理（显性任务，Phase 0 必须触达）**
       - 目标：立刻提升“问我是谁/你认识谁/身份细节”类问答的命中率与一致性，避免仅靠结构化摘要/猜测。
       - 交付：在 Responses 的 developer persona（或其固定块）中加入**极简但可执行**的溯源规则：
         - 当被问到 Owner/身份/个人信息：必须先读取 `<data_dir>/USER.md` 再回答。
         - 当被问到认识哪些人/有哪些账号：必须先读取 `<data_dir>/PEOPLE.md` 再回答。
         - 当被问到 persona 身份细节：必须先读取 `<data_dir>/people/<persona_id>/IDENTITY.md` 再回答（raw 注入时需剥离 frontmatter，避免噪声）。
         - 必须明确说明 PEOPLE.md 的治理口径（避免 surprise）：是否允许手工编辑、是否会被系统自动再生覆盖；并指示 agent 不要“靠记忆/猜测 roster”，以 PEOPLE.md 为权威来源。
       - 约束：规则要“短、硬、可执行”，且在 **as-sent** 的三段 preamble 中位置固定（便于 debug/排障）。
       - 范围：本次 Phase 0 的“中文化与标题统一”只要求在 `openai-responses` 路径生效；非 Responses provider 的 system prompt 文案与章节结构可暂不改（遗留到后续 Phase）。
       - 冻结（已决）：OpenAI Responses 路径的三段 developer item “输出布局 + 文案”在 Phase 0 先按如下版本冻结（后续若要改，必须在本 issue 中显式更新并附带回归点）。
         - 注意：必须严格区分“硬编码固定文案（PromptFixedBlocks）”与“动态填充内容（PromptInputs/运行态）”，禁止混淆。
         - 硬编码固定文案（Phase 0 需要在代码里写死且可测试）：
           - 三段分层的标题/小节标题/编号顺序与所有“规则/说明”句子（包括括号内中英提示）。
           - system 段内容不大动，但整体翻译为中文，作为 Responses 的 developer item 1 文案。
         - 动态填充内容（Phase 0 必须按来源正确填充；缺失时用明确占位/错误）：
           - `<persona_id>`（effective persona id）
           - `<IDENTITY.md 正文>`（从 `<data_dir>/people/<persona_id>/IDENTITY.md` 读取；注入时需剥离 YAML frontmatter）
           - `<SOUL.md 正文>`（从 `<data_dir>/people/<persona_id>/SOUL.md` 读取）
           - `<AGENTS.md 正文>`（从 `<data_dir>/people/<persona_id>/AGENTS.md` 读取；这是“个人偏好/长期规则”）
           - `<TOOLS.md 正文>`（从 `<data_dir>/people/<persona_id>/TOOLS.md` 读取；这是“个人偏好/工具说明”）
           - 运行环境（host/sandbox/provider/model/session 等快照，来自 `runtime_context`）
           - 项目级上下文（来自 `project_context`，其中可能包含项目目录层级的 `CLAUDE.md/CLAUDE.local.md/AGENTS.md/.claude/rules/*.md` 等）
           - 项目级可用技能（来自 discovered/activated skills）
           - 项目级可用工具（来自 ToolRegistry + provider/native_tools/过滤器）
         - 可选附加固定块（Phase 0：Responses 路径也要覆盖；属于 PromptFixedBlocks）：
           - **语音回复模式（Voice Reply Mode）**：当本次 `desired_reply_medium == Voice` 时，将以下块追加到 developer item 3（运行环境）末尾，用于约束输出为 TTS 友好文本。
             - 位置：developer item 3 末尾（在 “项目级可用工具” 之后追加，不影响前面 1..5 小节）。
             - 文案（固定，Phase 0 需中文化；对应现有 `VOICE_REPLY_SUFFIX`）：

               ```text
               ## 语音回复模式 (Voice Reply Mode)
               用户会以语音（TTS）听到你的回复。请按“说出来”的方式写，而不是按“读文档”的方式写：
               - 用自然、口语化的完整句子；不要用项目符号列表、编号列表或标题。
               - 绝对不要输出原始 URL。请用名称描述资源（例如“Rust 官方文档网站”，而不是具体链接）。
               - 不要使用任何 Markdown：不要加粗/斜体/标题/代码块/行内反引号。
               - 对可能被 TTS 误读的缩写进行拼读（例如“API”写成“A-P-I”，“CLI”写成“C-L-I”）。
               - 保持简洁：最多两到三段短段落。
               - 段落之间要自然过渡。
               ```

           - **长期记忆提示（Long-Term Memory）**：当本次 ToolRegistry 中存在 `memory_search` 工具时，追加一段固定提示，指导模型在需要时主动检索长期记忆。
             - 位置：developer item 3（运行环境）内，建议放在 “## 4. 项目级可用技能” 之后、 “## 5. 项目级可用工具” 之前（不改变 1..5 主结构，只作为段内附加提示）。
             - 文案（固定，Phase 0 需中文化；对应现有 `Long-Term Memory` 块）：

               ```text
               ## 长期记忆 (Long-Term Memory)
               你可以使用长期记忆系统。用户提到“之前/上次/以前/历史决定/既有约定”等信息，或当上下文会显著提升回答质量时，应主动使用 `memory_search` 进行检索，再基于检索结果回答。
               ```

         - 约束（Phase 0 先收敛变体，减少多份“role=developer prompt”口径漂移）：
           - OpenAI Responses 路径：developer item 1（system）固定使用上面这份“中文 system 文案”，不再因为 `include_tools=true/false` 分叉出两套不同 system 文案。
         - 冻结的 as-sent 三段文本（用于 debug/raw_prompt/context/full_context 展示的权威参考；其中 `<persona_id>` 会在运行时替换为 effective persona id）：

           ```text
           ===== developer item 1（system / 系统层）=====
           # 系统（System）

           你是一个乐于助人的助手，可以使用工具执行 shell 命令。

           执行路由：
           - exec 在 Sandbox(exec): enabled=true 时在 sandbox 内运行。
           - 当 sandbox 被禁用时，exec 在宿主机（host）上运行，且可能需要用户审批。
           - Host: sudo_non_interactive=true 表示宿主机可进行非交互式 sudo 安装；否则在宿主机安装软件包前必须询问用户。
           - 如果 sandbox 缺少必要工具/软件包且需要宿主机安装，必须先询问用户，再申请宿主机安装或调整 sandbox 模式。

           ## Guidelines
           - 当用户要求你执行需要系统交互的任务（文件操作、运行程序、检查状态等）时，使用 exec 工具运行 shell 命令。
           - 当用户要求你访问网站、检查页面、读取网页内容或进行网页浏览任务时，使用 web_fetch 工具打开 URL 并抓取网页内容。
           - 在执行命令或抓取网页之前，始终先说明你要做什么。
           - 如果命令或抓取失败，先分析错误原因，再给出修复建议。
           - 对于多步骤任务，一次执行一步，并在继续前检查结果。
           - 对破坏性操作要谨慎——先向用户确认。
           - IMPORTANT：用户的 UI 已在专用面板展示工具执行结果（stdout、stderr、exit code）。不要在回复中重复或回显原始工具输出；应总结发生了什么、突出关键发现或解释错误。简单复读输出会浪费用户时间。

           ## Silent Replies
           当一次工具调用后你没有任何有意义的补充（输出本身已说明问题），不要输出任何文本，直接返回空响应。
           用户的 UI 已显示工具结果，无需重复或确认；当输出已经回答问题时保持安静。

           ===== developer item 2（persona / 人格层）=====
           # 人格（Persona: <persona_id>）
           ## 1. 身份 (Identity, Who are you?)
           <IDENTITY.md 正文>
           ## 2. 灵魂 (Soul, What is your soul?)
           <SOUL.md 正文>
           ## 3. 主操作者 (Owner, Who is your owner?)
           关于 Owner 的信息，详见 /moltis/data/USER.md。
           规则：
           - 当你被问到“我是谁 / 主操作者是谁 / 主操作者资料 / 与主操作者相关身份信息”等问题时：
               1. 必须先读取 /moltis/data/USER.md
               2. 再基于该文件内容回答
               3. 不得凭空猜测
               4. 禁止修改其中信息
           ## 4. 人物清单 (People, Who are the people you know?)
           关于你认识的熟人信息，详见 /moltis/data/PEOPLE.md。
           规则：
           - 当你被问到“你认识哪些人 / 有哪些账号或 bots / 有哪些代理或角色”等问题时：
               1. 必须先读取 /moltis/data/PEOPLE.md
               2. 再基于该文件内容回答
               3. 不得靠记忆或猜测其中名单
               4. /moltis/data/PEOPLE.md 由系统自动生成/更新，禁止修改其中内容
           ## 5. 对工作区规则的个人偏好
           <AGENTS.md 正文>
           说明：
           - 以上是你个人的工作区长期规则/偏好。
           - “当前项目/工作区”的规则与上下文会出现在“运行环境 / 项目级上下文”里；一旦出现，以运行环境为准。
           ## 6. 对工具说明的个人偏好
           <TOOLS.md 正文>
           说明：
           - 以上是你个人的工具使用约定/偏好。
           - 本次运行“到底有哪些工具可用、每个工具的能力/参数是什么”属于事实信息，会出现在“运行环境 / 项目级可用工具”里；以运行环境为准。
           ## 7. 对项目上下文的个人偏好
           说明：项目/工作区上下文会在“运行环境”中以本次注入内容的形式出现；一旦出现，视为本次运行范围内的权威规则。

           ===== developer item 3（runtime / 运行环境层）=====
           # 运行环境（Runtime）
           ## 1. 运行环境
           <host/sandbox/provider/model/session 等本次运行环境>
           ## 2. 执行路由
           <本次 exec 走 sandbox/host、网络/权限等与执行相关的环境与规则>
           ## 3. 项目级上下文
           <当前 project/worktree 的上下文注入块；可能包含项目级的 CLAUDE.md / CLAUDE.local.md / AGENTS.md / .claude/rules/*.md 等，非个人偏好。出现则对本次运行具有最高优先级。>
           ## 4. 项目级可用技能 (Available Skills)
           <本次运行发现/启用的 skills 列表与说明（如有）>
           ## 5. 项目级可用工具 (Available Tools)
           <本次运行 ToolRegistry 的可用工具摘要/参数结构（随 provider/native_tools/过滤器而变化）>
           ```
  - **P1：消除主路径漂移源（poisoning / acceptLanguage / persona drift）**
    1) `raw_prompt/context/full_context` 必须走 `resolve_session_persona_id(...)`，不允许固定 default persona（修复排障漂移）。
    2) `send_sync`：
       - 预检估算与实际 as-sent 必须复用同一入口（同一 persona / 同一 runtime_context / 同一 tool registry gating）。
       - 必须传播 `_acceptLanguage`（避免 runtime_snapshot 漂移）。
       - **禁止**把错误以 persisted `role=system` 写入会话历史（否则在 Responses 下会变成 `role=developer` input item，形成 poisoning）。
         - Phase 0 路径（已决）：将这些错误条目持久化为 `role=assistant`（文本仍用 `[error] ...` 前缀，确保 UI 可见）。
         - 同时要求：这些 `[error] ...` 可见条目不得进入后续 LLM prompt 的 history（避免污染模型对话）；它们仅用于 UI/排障可见。
         - 实施建议（Phase 0）：写入时附带明确标记（避免靠文本前缀做脆弱启发式）：
           - 例如在 persisted JSON 里加 `moltis_internal_kind: "ui_error_notice"`（或同等语义字段）。
           - prompt/history 构造时显式过滤该标记的条目（包括 compaction 输入与 as-sent history）。
    3) `spawn_agent`（Phase 0 选择 B，quick win）：子代理保持 `runtime_context=None` / `project_context=None` 且 `hook_registry=None`，但 debug/as-sent 必须明确标注缺失（避免误导）；后续 Phase 3 再评估补齐。
  - **P2：结构化/手工内容解耦（止血版，采用方案 2）**
    1) 保持 “单 `.md` + YAML frontmatter” 不变，但把写回策略改为：**只更新 frontmatter、正文完全保留**。
    2) 优先止血两个最脏路径：`save_user()`（`USER.md`）与 `save_identity()`（`IDENTITY.md`），并补充回归测试（正文不丢、未知 keys 不丢、只改 managed keys）。
    3) 同步：在 prompt 注入 `IDENTITY.md` raw markdown 时应剥离 YAML frontmatter（避免重复/噪声）；该项可作为 Phase 0 的 P2（若实现成本低）或 Phase 1（若牵连较大）。

- **Phase 1（块化 Layout：默认清单复刻现状）**：把硬编码 prompt 组织方式收敛为 block 清单渲染，但暂不开放用户 layout 文件；先用测试把“默认输出等价”钉死。
- **Phase 2（可选 layout 配置）**：在 Phase 1 稳定后再引入 `LAYOUT.toml`（严格校验 required blocks / 错误提示 / 回滚）。
- **Phase 3（Surface 分层与质量收敛）**：UI vs Channel（Telegram 等）差异化注入 guidelines/silent replies，并清理 spawn_agent 与主路径的剩余漂移点；同时补齐 hooks/trace 的 as-sent 证据链（例如 `BeforeLLMCall` 对 openai-responses 输出 `input[]` developer items 口径，而不是 chat-completions 的 `messages[]` 口径）。
- **Phase 4（术语/文档收敛）**：Owner 命名收敛、`docs/src/system-prompt.md` 更新为 Responses 多 developer-item as-sent 形态、清理 done doc 的过时描述。

#### 行为规范（Normative Rules）
- R0（Phase 0）：所有“会发给模型 / 用于估算 / 用于 debug 展示”的 prompt，都必须由同一套 **PromptBundle** 入口生成；禁止各调用点自行拼接/自行选 persona。
- R1（Phase 0）：debug/raw_prompt/context/full_context 的展示口径必须与 **as-sent** 等价，并显式展示 `personaIdEffective` 与 provider 形态（Responses=developer items；非 Responses=single system）。
- R2（Phase 0）：内部错误不得以 persisted `role=system` 的形式进入后续 LLM prompt（在 OpenAI Responses 下会变成 `role=developer` 指令链，存在 poisoning 风险）。
- R2.1（Phase 0）：`send_sync` 的 `[error] ...` 需要可见（持久化为 `role=assistant`），但不得进入后续 LLM prompt history（仅 UI/排障可见），避免隐性干扰后续对话与 compaction。
  - 要求：过滤必须基于**显式标记字段**（例如 `moltis_internal_kind`），而不是仅靠 `[error]` 文本前缀匹配。
- R3（Phase 0）：结构化写回必须止血：`save_user()` / `save_identity()` 仅更新 YAML frontmatter 中的 managed keys，**正文必须原样保留**，且不得自动删除 `USER.md` / `IDENTITY.md` 文件。
- R4（Phase 0）：`IDENTITY.md` raw markdown 注入必须剥离 YAML frontmatter（避免重复与噪声）。
- R5（Phase 1+）：进入块化 Layout / layout 配置阶段后，layout 仅表达“块清单”，不允许自由模板语言。

#### 接口与数据结构（Contracts）
- Phase 0 必须先定义一个统一产物：`PromptBundle`（建议落在 `crates/agents/src/prompt.rs` 或新模块 `crates/agents/src/prompt_bundle.rs`），用于同时驱动：
  - **as-sent**（runner 真实发送的 preamble messages）
  - **estimate**（token estimate / compaction preflight）
  - **debug**（raw_prompt/context/full_context 展示）
- `PromptBundle`（建议字段，不强制一字不差）：
  - `as_sent_prefix_messages: Vec<ChatMessage>`：用于 runner；OpenAI Responses 形态为 3 条 preamble（system/persona/runtime_snapshot，以 `ChatMessage::System` 表达，provider 映射为 `role=developer`）；非 Responses 为 1 条 system。
  - `estimate_text: String`：用于估算/compaction，必须与 as-sent 文本同源（避免 drift）。
  - `debug_view: Vec<{layer, role_hint, text}>`：供 debug 面板展示（Responses 下为三段 developer items）。
  - `persona_id_effective: String` + 关键元信息（`include_tools` / `native_tools` / skills 统计 / 是否注入 runtime/project context 等）。
- Gateway/chat 侧需要一个 wrapper（Phase 0）负责收集输入并调用 `PromptBundle` builder：
  - provider/model + filtered_registry + skills + runtime_context + project_context + persona sources（按 `persona_id_effective` 合并加载）
- debug endpoints（Phase 0）契约：
  - `chat.raw_prompt`：展示 `PromptBundle.debug_view`（Responses 下为三段 developer items）。
  - `chat.context/full_context`：展示 “as-sent preamble（来自 PromptBundle） + 历史消息（role=user/assistant/tool）”，并明确标注 preamble 与 history 的边界。
- debug RPC 输出契约（Phase 0 冻结，便于 UI/排障；允许后续 Phase 1+ 扩展字段，但不得改变语义）：
  - 必须字段（Responses/非 Responses 均存在）：
    - `providerKind`：`openai-responses` | `chat-messages`
    - `providerId` / `providerName`（用于定位具体 provider/model）
    - `personaIdEffective`
    - `asSentPreamble`：数组；每项含 `layer`（`system|persona|runtime_snapshot`）与 `text`
    - `asSentHistory`：数组；每项为 as-sent 的历史消息（不含任何 UI-only notice/error）
      - 最小字段：`role`（`user|assistant|tool`）+ `content`（string）
      - 可选字段：`toolCallId` / `toolCalls`（用于呈现工具调用与输出关联）
  - 约束：`chat.full_context` 必须显式区分 `asSentPreamble` vs `asSentHistory`（避免 UI/排障把 preamble 当 history 或相反）。
  - UI-only 可见性（Phase 0 约束）：
    - `send_sync` 的 `[error] ...` 仍可出现在普通会话历史/聊天流中（便于用户理解发生了什么）。
    - 但它们不得进入 `asSentHistory`（也不得进入任何后续 LLM prompt history）；`asSentHistory` 只代表“模型真实看见的历史”。
- 结构化写回（Phase 0）契约：
  - `USER.md` / `IDENTITY.md`：frontmatter 可被系统更新；frontmatter 之外正文为用户资产，必须原样保留；系统不得自动删除文件（即便 managed keys 全为空）。

#### 失败模式与降级（Failure modes & Degrade）
- Phase 0：canonical v1 prompt 构建失败时，必须返回可定位错误（provider/model/session/personaIdEffective），并禁止 fallback 到旧的“各处自行拼装”逻辑（否则 drift 更难排）。
- Phase 0：`send_sync` 的错误可见性必须保留，但不得写入 persisted `role=system`（避免 Responses 下 poisoning）。
- Phase 0：`spawn_agent` 的 runtime/project context：
  - 选择 B（quick win）：保持 `runtime_context=None` / `project_context=None`，但在 debug_view 中**明确标注缺失**，避免误导；后续 Phase 3 再评估补齐。

#### 安全与隐私（Security/Privacy）
- Phase 0 不扩大敏感信息注入范围；debug/as-sent 展示必须复用现有 runtime 脱敏口径（尤其避免把 tool outputs/volatile 字段当作 developer preamble 注入）。
- People roster 默认仍只做 reference（cache-friendly），不在 preamble 内联。

## 验收标准（Acceptance Criteria）【不可省略】
### Phase 0（Quick Wins）
- [x] AC0.1：存在 canonical v1 builder（单一权威入口：`build_canonical_system_prompt_v1`），且 gateway/chat 与 tools/spawn_agent 的主路径不再自行拼装 prompt（全部改为只调用该入口）。
- [x] AC0.2：`chat.raw_prompt` / `chat.context` / `chat.full_context` 在 openai-responses 下能展示 **as-sent developer preamble**（折叠为 1 条 developer item）+ `personaIdEffective`（并提供 provider-aware `asSent` 摘要），不再退化为“只展示内部拼接的 system prompt、无法复盘 as-sent 语义”的口径。
- [x] AC0.3：debug endpoints 与主 run 的 persona 口径一致：不再固定 default persona；展示的 `personaIdEffective` 必须与实际 run 一致（可用于排障）。
- [x] AC0.4：`send_sync` 的预检估算/compaction gating 与实际 as-sent 复用同一入口（同一 persona / 同一 runtime_context / 同一工具过滤口径），并传播 `_acceptLanguage`。
- [x] AC0.5：`send_sync` 错误不再以 persisted `role=system` 进入会话历史（避免 OpenAI Responses 下 developer poisoning）；必须有回归测试覆盖两条写入路径（keep-window overflow + run failed）。
- [x] AC0.5b：`send_sync` 的 `[error] ...` 仍可在 UI/history 中可见（以 `role=assistant` 持久化，并带 `moltis_internal_kind="ui_error_notice"` 标记），但在后续 LLM prompt 构造时会被过滤掉（不进入 as-sent history）。
- [x] AC0.6：结构化写回止血：`save_user()` / `save_identity()` 只更新 frontmatter、正文完全保留（含用户自定义段落/未知 keys），并有单元测试覆盖。
  - [x] AC0.7：`IDENTITY.md` raw markdown 注入剥离 YAML frontmatter（无重复/无噪声），并有单元测试覆盖。
  - [x] AC0.8：语音回复模式：当 `reply_medium == Voice` 时，canonical v1 提供模板变量 `voice_reply_suffix_md`；当 Type4 模板引用该变量时，最终 system prompt 会包含“语音回复模式”固定块（中文文案）；未引用时必须 warning（不 fail-fast）。
  - [x] AC0.9：长期记忆提示：当 ToolRegistry 存在 `memory_search` 时，canonical v1 提供模板变量 `long_term_memory_md`；当 Type4 模板引用该变量时，最终 system prompt 会包含“长期记忆”固定提示（中文文案）；未引用时必须 warning（不 fail-fast）。

### Phase 1+（后续，暂不在 Phase 0 实施）
- [ ] AC1：无任何 layout 配置时，生成的 prompt 与现状等价（至少通过 snapshot/golden tests 验证）。
- [ ] AC2：存在 layout 配置时，可调整 persona 各块顺序/标题/开关（required blocks 不可关闭）。
- [ ] AC3：gateway/chat 与 spawn_agent 共享同一套 persona sources merge 与 layout/build 逻辑，不再重复实现。
- [ ] AC4：同一输入在 openai-responses 与非 responses provider 上 prompt 内容一致（仅 message role 映射不同）。
- [ ] AC5：layout 非法时错误信息清晰、可定位、不会产生半截/不可预期 prompt。
- [ ] AC6：硬编码集中管理：Builtins 文案/默认 seed/引用路径/缺失占位符在一个模块（或少数固定文件）内可集中维护；新增 prompt 文案必须走同一入口。
- [ ] AC7：默认 prompt 文案 review 通过：system/persona/runtime 结构清晰、避免重复与冗余，且在不减少信息量的前提下更精炼。

## 测试计划（Test Plan）【不可省略】
### Phase 0（Quick Wins）
#### Unit
- [x] canonical v1 builder：覆盖 native/non-native/no-tools 三种模式 + hard/soft requiredness（并对 escape/稳定排序做断言）。
- [x] debug endpoints：`chat.raw_prompt` / `chat.context` / `chat.full_context` 在 Responses 下展示 as-sent developer items（且包含 `personaIdEffective`）。
- [x] persona 路由回归：debug endpoints 必须走 `resolve_session_persona_id(...)`，不允许固定 default persona。
- [x] `send_sync`：预检估算与实际 as-sent 复用同一入口（并覆盖 `_acceptLanguage` 传播）。
- [x] developer poisoning 回归：send_sync 的 `[error] ...` 不得以 persisted `role=system` 进入后续 LLM prompt（覆盖 keep-window overflow + run failed）。
- [x] `[error]` 可见但不污染：`send_sync` 写入的 `[error] ...`（persisted assistant，带 `moltis_internal_kind="ui_error_notice"` 标记）在后续 prompt 构造中必须被过滤（覆盖 keep-window overflow + run failed）。
- [x] frontmatter 写回止血：`save_user()` / `save_identity()` 仅更新 frontmatter、正文保留（含未知 keys）并覆盖测试。
- [x] IDENTITY raw 注入去 frontmatter：`IDENTITY.md` raw markdown 注入不含 YAML frontmatter（避免重复/噪声）。

#### Integration
- [x] gateway/chat 主路径：send / run_with_tools / run_streaming / send_sync / debug endpoints 全部走统一入口（并验证 `personaIdEffective` 一致）。

### Phase 1+（后续）
#### Unit
- [ ] layout 解析与 required blocks 规则：`crates/agents/...` 或 `crates/config/...`
- [ ] default layout golden test：对比现状输出（system/persona/runtime 分段）
- [ ] provider 一致性测试：同一 layout/build 在 responses/non-responses 输出一致（或同源分段一致）

### 自动化缺口（如有，必须写手工验收）
- 若 prompt 文本过长导致 snapshot 维护困难：仅对 “block header + 顺序 + 关键片段” 做结构化断言。

## 发布与回滚（Rollout & Rollback）
- Phase 0 发布策略：默认开启（无新配置开关），因为主要是“入口收敛 + 可观测性修正 + 写回止血”，不引入用户侧 layout 迁移。
- Phase 0 回滚策略：如出现问题，可回滚到旧版本；重点回滚风险在 `save_user()`/`save_identity()` 的写回行为（需通过测试保障“正文不丢”）。
- Phase 1+ 发布策略（后续）：引入 layout 时默认关闭/缺省走默认 layout，确保不破坏现网。
- 上线观测：Phase 0 先把 debug/context 的 as-sent 视图做成“可信证据”；Phase 1+ 再追加 `promptLayout`（块列表与来源）。

## 实施拆分（Implementation Outline）
- Phase 0（Quick Wins）
  - Step 0.0（Prep，Phase 0 gate）：冻结 `PromptBundle`/debug RPC 契约、列清迁移点、定死写回与错误持久化策略、评估 UI 影响面并跑基线测试。
  - Step 0.1：实现 `PromptBundle` + builder（agents），复用现有 Responses/非 Responses prompt builders。
  - Step 0.2：gateway/chat 增加 wrapper（resolve persona_id / load persona sources / filter registry / discover skills / runtime_context / project_context），并迁移 send / run_with_tools / run_streaming / send_sync / raw_prompt / context / full_context。
  - Step 0.3：修复 `send_sync` 漂移源：persona 路由、`_acceptLanguage` 传播、错误持久化不再使用 `role=system`（避免 developer poisoning）；错误 notice 持久化为 `role=assistant` + `moltis_internal_kind="ui_error_notice"`，并在 as-sent history/compaction 输入构造时显式过滤。
  - Step 0.4：修复 `tools.spawn_agent`：改为调用统一 builder，并在 debug_view 明确标注 sub-agent 缺失 runtime/project context（Phase 0 选择 B）。
  - Step 0.5：止血写回：`save_user()` / `save_identity()` 只改 frontmatter、不动正文；补齐单测。
  - Step 0.6：补齐 Phase 0 回归测试（见 Test Plan）。
- Phase 1：默认 layout 块化（复刻现状）并补齐 golden/provider 一致性测试。
- Phase 2：引入可选 layout 文件解析与覆盖（严格校验 required blocks + 错误提示）。
- Phase 3：Surface 分层与文案一致性 review（去重、分层边界清晰、必要时拆分/合并块）。
- Phase 4：术语/文档收敛与清理过时 done 文档口径。
- 受影响文件（预估）：
  - `crates/agents/src/prompt.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/tools/src/spawn_agent.rs`
  - `crates/config/src/loader.rs`
  - `crates/gateway/src/personas.rs`（seed 文案归类/可覆盖策略）

## 交叉引用（Cross References）
- 本单以“当前代码实现”为唯一权威依据；不依赖历史 issue 文档作为规范来源。
- 相关代码入口（便于跳转）：
  - Prompt 生成（Responses/非 Responses）：`crates/agents/src/prompt.rs:40`
  - gateway/chat persona merge：`crates/gateway/src/chat.rs:763`
  - spawn_agent persona merge（重复实现）：`crates/tools/src/spawn_agent.rs:42`
  - persona 文件路径与默认 SOUL：`crates/config/src/loader.rs:251`
  - 默认 persona seed：`crates/gateway/src/personas.rs:47`

## 未决问题（Open Questions）
### Phase 0（Quick Wins）
- Q0.1（已决）：`send_sync` 错误条目持久化为 `role=assistant`（保留 `[error] ...` 前缀），禁止 persisted `role=system`（避免 OpenAI Responses developer poisoning）。
- Q0.2（已决）：debug endpoints 的 JSON 输出契约必须显式区分 `asSentPreamble` vs `asSentHistory`（便于 UI/排障）。

### Phase 1+（后续）
- Q1：layout 非法时是“回退默认 + warning”还是“fail-fast 拒绝启动/拒绝请求”？
- Q2：layout 的作用域：仅全局一个 layout，还是 persona 级可覆盖？
- Q3：是否允许 override builtins（例如 system guidelines）？如果允许，override 的来源路径与优先级如何定义？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

# Overall Issues (v1) — Type4 System Prompt Assembly（Audit + Roadmap）

Updated: 2026-03-05

## 范围与约束（Scope & Constraints）

- 覆盖范围：**Type4（workspace/people）相关的 system prompt / developer preamble 拼接**，以及其在 gateway chat / spawn_agent / debug endpoints 中的执行路径与 provider 适配。
- 重点场景：
  - `openai-responses`：developer preamble（现行 v1：折叠为 1 条 developer item；legacy 仍保留三段 builder 供兼容/历史审计）
  - 非 `openai-responses` provider：单段 system prompt（含 tools/skills/runtime 等 sections）
  - `stream_only` vs `run_with_tools`（是否能同步 tool calls）
  - `supports_tools`（native tools vs text tool_call 指引）
- Out of scope：
  - Heartbeat/cron 的 prompt（另线治理）
  - Skills/Hooks 的 `SKILL.md`/`HOOK.md` frontmatter+body 格式本身（本单仅关注它们“如何被注入 prompt”）
  - 具体代码重构/落地实现（本文是 audit + roadmap；实现拆分在独立 issue）
- 状态标记：
  - **[DONE]** 已落地（含测试/证据）
  - **[TODO]** 方案已收敛，待实施
  - **[SURVEY]** 仅记录/调研，不承诺

---

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】

完整术语表（含 aliases 与 source/method 口径）见：`issues/glossary-type4-prompt-assembly-terms.md`

- **Type4**（主称呼）：workspace/people 持久化资料（`USER.md`/`PEOPLE.md`/`people/<name>/**`）用于 prompt 与 UI 的那一类数据。
  - Source/Method：configured（文件）→ effective（合并/默认后）
- **Prompt Product**（主称呼）：最终“发送给 provider”的 prompt 产物形态。
  - `ResponsesDeveloperPreamble`：openai-responses 的 developer preamble（现行 v1：单条 developer item；legacy：三段 system/persona/runtime_snapshot）。
  - `SystemPrompt`：非 responses 的单段 system prompt 文本。
- **as-sent**（主称呼）：最终写入请求体、实际发送给上游 provider 的内容/结构。
  - 例：openai-responses 的 `input[]` items（developer/user/assistant/tool）形态。
- **estimate**（主称呼）：本地用来估算 token / compaction gating 的拼接文本（可能与 as-sent 结构不同，但内容应等价）。
- **native tools**（主称呼）：provider 具备原生 tool calling（schemas 通过 API 字段传递，不需要 prompt 内 JSON schema）。
  - Evidence：`supports_tools()` / prompt builder 的 `native_tools` 分支。
- **stream_only**（主称呼）：gateway 当前进程无法同步执行工具（`!has_tools_sync()`），因此 prompt 构造与工具/技能注入会走简化路径。

---

## Executive Summary（结论与优先级）

### 已完成（本轮落地）

- [DONE] Type4 数据 SOT 与 UI 治理完成：`USER.md`/`PEOPLE.md`/`people/<name>/**` 的字段/正文边界与同步机制已落地（详见：`issues/done/issue-workspace-persona-frontmatter-fields-only-single-sot.md:1`、`issues/done/issue-workspace-people-ui-governance-onboarding-settings-terminology.md:1`）。
- [DONE] v1 canonical system prompt assembly 落地：跨 provider 层仅生成 1 条 `ChatMessage::System`，provider adapter 自行映射（openai-responses → developer / anthropic → top-level system / local-llm → template）。
  - Evidence：`crates/agents/src/prompt.rs:385`、`crates/gateway/src/chat.rs:2401`、`crates/tools/src/spawn_agent.rs:235`。
- [DONE] as-sent 证据链已覆盖关键差异（openai-responses/anthropic/local-llm）：debug endpoints 输出 `asSent`（+ Responses 的 `asSentPreamble`），hooks（BeforeLLMCall）输出 `asSentSummary`。
  - Evidence：`crates/gateway/src/chat.rs:3679`、`crates/gateway/src/chat.rs:8298`、`crates/agents/src/runner.rs:770`。

### MUST-FIX（阻断可靠性/可治理性）

- [DONE] prompt assembly 入口收敛：gateway chat（preflight/run/send_sync/debug/streaming）与 spawn_agent 统一走 canonical v1 builder（不再分散拼装旧分支），显著降低 drift 风险。
  - Evidence：`crates/gateway/src/chat.rs:2401`、`crates/gateway/src/chat.rs:4276`、`crates/tools/src/spawn_agent.rs:235`。
- [SURVEY] Provider 适配层的“角色语义”不统一：同一个 `ChatMessage::System` 在 openai-responses 会被映射成 `role=developer`，Anthropic 则抽成 top-level `system` 字段；对 debug/trace 造成理解与证据链成本（单独跟踪：`issues/issue-provider-role-normalization.md`）。
  - Evidence：`crates/agents/src/providers/openai_responses.rs:33`、`crates/agents/src/providers/anthropic.rs:81`。
- [DONE] 落地 v1 模板化拼接（Type4 prompt templates）：
  - 用户维护 `people/<name>/{IDENTITY,SOUL,AGENTS,TOOLS}.md` 的正文模板；系统仅做 `{{var}}` → String 的**纯字符串替换**。
  - 必须输出稳定（可缓存）且中文化（标题/说明性硬编码尽量中文）。
  - Evidence：`crates/config/src/prompt_subst.rs:60`、`crates/agents/src/prompt.rs:385`。

### SHOULD-FIX（提升一致性/可控性/排障）

- [TODO] 统一定义 PromptParts（canonical 中间表示）+ renderer（按 provider/product 渲染），消除“同一内容在不同路径/不同 provider 表达不一致”。
- [TODO] 明确并冻结“Type4 注入内容清单与层归属”：哪些来自 `USER.md`、哪些来自 `PEOPLE.md` reference、哪些来自 `people/<name>/**`，以及在 Responses 的哪个 layer / 非 Responses 的哪个 section。

---

## Issue Index（强制维护，防遗留）

| ID | Status | Pri | Title | Owner | Component | Depends On | Evidence | Tests | Doc |
|---:|:---:|:---:|---|---|---|---|---|---|---|
| 1 | DONE | P0 | Canonical PromptParts + renderer（v1: 统一 system 文本） | TBD | agents/prompt + gateway |  | `crates/agents/src/prompt.rs:385` | `crates/agents/src/prompt.rs:1718` | `issues/issue-persona-prompt-configurable-assembly-and-builtin-separation.md` |
| 2 | DONE | P0 | 去重：gateway chat / spawn_agent prompt assembly | TBD | gateway + tools | 1 | `crates/gateway/src/chat.rs:2401` | `crates/gateway/tests/spawn_agent_openai_responses.rs:43` | `issues/done/issue-prompt-assembly-entrypoint-dedup.md` |
| 3 | DONE | P0 | 统一 as-sent 证据链（debug + hooks） | TBD | gateway + agents/runner | 1 | `crates/gateway/src/chat.rs:3679` | `crates/gateway/src/chat.rs:8298`、`crates/gateway/src/chat.rs:8448` | `issues/done/issue-prompt-as-sent-observability.md` |
| 4 | SURVEY | P1 | Surface-aware Guidelines（Web UI vs Telegram） | TBD | agents/prompt | 1 | `crates/gateway/src/chat.rs:2274` | 缺口 | `issues/issue-surface-aware-guidelines.md` |
| 5 | SURVEY | P2 | Provider 角色模型统一（system/developer） | TBD | agents/model + providers | 1 | `crates/agents/src/providers/openai_responses.rs:37` | 缺口 | `issues/issue-provider-role-normalization.md` |
| 6 | DONE | P0 | Type4 模板化拼接 v1（{{var}} + 中文化 + 稳定性） | TBD | config + gateway + agents/prompt | 1,2,3 | `crates/config/src/prompt_subst.rs:60` | `crates/config/src/prompt_subst.rs:157` | `issues/done/issue-type4-template-assembly-v1.md` |

说明：ID=1 已存在且覆盖面最大（Phase 0 DONE / Phase 1+ TODO）；其余为本 audit 拆出的收尾项。

---

## 实施前置条件（Implementation Readiness Gates）

> 目的：把“能不能开干实现”从主观判断变成可检查的 gating。P0 实施前必须全部满足。

- [x] **稳定性（Stability）已冻结**：tools/skills 的列表顺序、schema 的 pretty JSON key 顺序在同一输入下稳定（排序 + canonicalize）。
- [x] **Responses preamble 折叠策略明确且有测试**：openai-responses 的 developer preamble as-sent 为 **1 条 developer message item**（内部 typed messages 侧为 1 条 system message），覆盖 gateway + spawn_agent。
- [x] **PromptProducts 中间表示明确**：跨 provider 层生成 canonical typed messages（1 条 `ChatMessage::System`），provider 适配层负责协议映射；并通过 `LlmProvider::debug_as_sent_summary(...)` 暴露 provider-aware 的 as-sent 摘要用于 debug/hook（避免在 gateway 复制协议逻辑）。
  - Evidence：`crates/agents/src/model.rs:425`、`crates/agents/src/providers/mod.rs:335`、`crates/gateway/src/chat.rs:3679`
- [x] **最小验证矩阵可跑通**：Validation Matrix（openai-responses/anthropic/local-llm）已具备自动化断言（debug endpoints 的 `asSent`/`asSentPreamble`）。
  - Evidence：`crates/gateway/src/chat.rs:8298`、`crates/gateway/src/chat.rs:8448`、`crates/gateway/src/chat.rs:8532`

## 建议实施顺序（Sequencing）

> 原则：先把“输出不稳定/证据链不完整/入口漂移”这些会扩大返工面的风险收敛，再引入模板化与 PromptParts。

1) **Stability primitives（稳定性底座）**
   - tools：按 `(name, source, mcpServer)` stable sort；non-native 的 `parameters` pretty JSON 前递归 canonicalize key 顺序。
   - skills：FsSkillDiscoverer 对目录项排序；prompt_gen 输出顺序稳定。
2) **Responses 折叠为单条 developer item（收敛 as-sent 结构）**
   - gateway：`openai_responses_as_sent_from_prompts(...)` 生成 1 条 `ChatMessage::System`（文本内保留 system/persona/runtime 三段 section 边界）。
   - spawn_agent：同样折叠（且保留 sub-agent runtime 追加段落）。
3) **PromptParts + renderer（canonical 中间表示）**
   - 定义 PromptParts（canonical 内容块 + 固定块 + Type4 inputs 的有效值）。
   - renderer：按 provider/product 渲染为 typed messages / as-sent 结构（provider adapter 只做协议映射，不再决定内容布局）。
4) **Entry points 去重（gateway / spawn_agent / debug / send_sync）**
   - 统一走同一套 “PromptParts 构造 + renderer” 入口，彻底消除 drift。
5) **Type4 模板化 v1（{{var}} + 中文化 + 稳定性）**
   - 引入四文件拼接 + 纯字符串替换；把 skills/tools/runtime 等动态事实以 vars 形式提供给模板引用（不引入 if/loop）。
6) **As-sent 证据链补齐（debug + hooks）**
   - Debug：所有 provider 统一输出 provider-aware 的 as-sent（并标注 method：authoritative/as-sent vs estimate）。
   - Hooks：至少把 “preamble / system / tools” 的 as-sent 形态落到 hook event 中。

## 最小验证矩阵（Validation Matrix）

> 跨 issue 的“最小回归面”；增量追加，不重排。

| Item | 验证点 | 最小自动化 |
|---|---|---|
| ID=2 去重入口 | gateway 与 spawn_agent 的 prompt products 不漂移（同输入同输出） | integration（或 unit + golden） |
| ID=3 as-sent | debug/hook 能复盘 **as-sent**（Responses input[] / Anthropic top-level system / local 模板渲染） | integration |
| ID=6 模板化 v1 | `{{var}}` 替换、空字符串消隐、稳定排序、中文标题稳定 | unit（golden） |
| Responses 折叠 | 最终 as-sent developer item 数量=1，且保序保边界 | unit |

## As-is：Prompt Products（当前“产物”形态）

### 1) OpenAI Responses：developer preamble（现行 v1：折叠=1；legacy：三段）

- 现行主路径（canonical v1）：`crates/agents/src/prompt.rs:385`（`build_canonical_system_prompt_v1`）→ gateway/tools 仅生成 1 条 `ChatMessage::System`；openai-responses adapter 映射为 1 条 `role=developer` input item（且省略顶级 `instructions`）。
- Builder：`crates/agents/src/prompt.rs:60`（`build_openai_responses_developer_prompts`）
- 三段内容：
  - `system`：固定中文系统说明（系统/指南/静默回复等）
  - `persona`：Identity（来自 `IDENTITY.md` 正文，剥离 YAML frontmatter）+ Soul + Owner/People reference + AGENTS/TOOLS 偏好
  - `runtime_snapshot`：运行环境 + 执行路由 + 项目上下文 + skills + tools + memory hint
    - Evidence：`crates/agents/src/prompt.rs:134`
- 备注：以上“三段”仅作为 legacy layers（兼容/历史）；v1 canonical 已在跨 provider 层折叠为单条系统提示，并由 adapter 做协议映射（保留清晰的 section 边界）。
- as-sent：在 gateway 层会构造 `asSentPreamble`（layer 索引 + role=developer），并在 openai-responses provider 侧把 `ChatMessage::System` 映射为 `role=developer` input item。
  - Evidence：`crates/gateway/src/chat.rs:4014`、`crates/agents/src/providers/openai_responses.rs:33`

#### As-is Evidence Chain（Responses：从拼接到 as-sent）

- Layer builder（3 段）：`crates/agents/src/prompt.rs:60`
- as-sent preamble（gateway 构造 prefix_messages + estimate 拼接，并处理 voice suffix 放置在 runtime_snapshot）：`crates/gateway/src/chat.rs:257`
  - voice suffix 追加到 runtime_snapshot：`crates/gateway/src/chat.rs:267`
  - token estimate 用 joined 文本：`crates/gateway/src/chat.rs:271`
  - prefix_messages 仍以 `ChatMessage::system(...)` 承载（后续由 provider adapter 改写为 developer role）：`crates/gateway/src/chat.rs:274`
- Provider adapter（真正 as-sent 结构）：Responses `input[]` 中 system→developer：`crates/agents/src/providers/openai_responses.rs:33`
  - system message → developer role item：`crates/agents/src/providers/openai_responses.rs:37`
  - tool message → `function_call_output`（不丢弃）：`crates/agents/src/providers/openai_responses.rs:105`

### 2) 非 Responses provider：单段 system prompt

- 现行主路径（canonical v1）：`crates/agents/src/prompt.rs:385`（`build_canonical_system_prompt_v1`）
- Legacy builder（兼容/历史）：`crates/agents/src/prompt.rs:367`（`build_system_prompt_with_session_runtime`）或 `crates/agents/src/prompt.rs:395`（`build_system_prompt_minimal_runtime`）
- 结构（核心 sections）：
  - Base intro（含/不含 tools）
  - Identity（结构化字段注入）+ `## Soul`（当 identity 存在时）
  - User name（当 user.name 存在时）
  - Project context（如有）
  - Runtime（Host/Sandbox 行）
  - Skills（当 include_tools=true 且 skills 不空时）
  - Workspace Files（AGENTS/TOOLS 文本）
  - Long-Term Memory（当存在 `memory_search` 工具）
  - Available Tools（compact list 或参数 schema）+ How to call tools（非 native tools）
  - Guidelines + Silent Replies（include_tools=true）
  - Evidence：`crates/agents/src/prompt.rs:420`、`crates/agents/src/prompt.rs:514`、`crates/agents/src/prompt.rs:542`

#### As-is Evidence Chain（Non-Responses：从拼接到 as-sent）

- Builder：
  - full system prompt（含 tools/skills/runtime，include_tools=true）：`crates/agents/src/prompt.rs:367`
  - minimal system prompt（不含 tools schemas，include_tools=false）：`crates/agents/src/prompt.rs:395`
- Provider adapter（as-sent 形态按 provider 变化，必须显式记录）：
  - OpenAI chat completions 兼容：`ChatMessage::to_openai_value()` 生成 `messages[]`：`crates/agents/src/model.rs:104`
  - Anthropic：system messages 抽取/合并为 top-level `system`：`crates/agents/src/providers/anthropic.rs:87`
  - GenAI：tool messages 明确丢弃：`crates/agents/src/providers/genai_provider.rs:71`
  - Local LLM（模板渲染）：Mistral 模板把 system 融入首个 user `[INST]`：`crates/agents/src/providers/local_llm/models/chat_templates.rs:133`
    - DeepSeek/Mistral 对未知 role（含 tool）走 `_ => {}` 丢弃：`crates/agents/src/providers/local_llm/models/chat_templates.rs:154`

---

## As-is：Entry Points（执行路径）

### Gateway chat

- Persona/Type4 数据装载（避免在 run_with_tools/run_streaming 内重复 merge）：`crates/gateway/src/chat.rs:811`（`load_prompt_persona_with_id`）
- Prompt 构造在多个入口点出现（同分支结构重复）：
  - chat 主流程（token estimate / auto-compact preflight）：`crates/gateway/src/chat.rs:2405`
    - Responses branch：`crates/gateway/src/chat.rs:2407`（stream_only）/ `crates/gateway/src/chat.rs:2436`（tools enabled）
    - non-Responses branch：`crates/gateway/src/chat.rs:2453`（stream_only）/ `crates/gateway/src/chat.rs:2478`（native_tools）/ `crates/gateway/src/chat.rs:2491`（non-native tools）
    - voice suffix（non-Responses 拼在 system_prompt 尾）：`crates/gateway/src/chat.rs:2502`
  - chat 主流程（真正 run phase）：
    - run_with_tools：`crates/gateway/src/chat.rs:4562`（内部分支：Responses `crates/gateway/src/chat.rs:4602`；non-Responses `crates/gateway/src/chat.rs:4623`/`crates/gateway/src/chat.rs:4637`）
    - run_streaming：`crates/gateway/src/chat.rs:5902`（内部分支：Responses `crates/gateway/src/chat.rs:5969`；non-Responses `crates/gateway/src/chat.rs:5984`）
  - send_sync（channels / API callers）：`crates/gateway/src/chat.rs:2924`
    - Responses branch：`crates/gateway/src/chat.rs:3012`/`crates/gateway/src/chat.rs:3041`
    - non-Responses branch：`crates/gateway/src/chat.rs:3057`/`crates/gateway/src/chat.rs:3082`/`crates/gateway/src/chat.rs:3095`
  - debug endpoints（as-sent 证据链的主入口）：
    - chat.context：`crates/gateway/src/chat.rs:3499`
    - chat.raw_prompt：`crates/gateway/src/chat.rs:3895`
    - chat.full_context：`crates/gateway/src/chat.rs:4076`

### spawn_agent

- 现行：统一走 canonical v1 builder，并在 system prompt 末尾追加 “Sub-agent” 固定段落：`crates/tools/src/spawn_agent.rs:235`

### Channel send / compact

- channels 的消息分发最终走 chat.send：`crates/gateway/src/channel_events.rs:326`
- `/compact` 命令走 chat.compact：`crates/gateway/src/channel_events.rs:1083`

---

## As-is：Type4 填充内容（Inputs → prompt）

### Type4 sources（磁盘）

- `USER.md`：字段解析为 `UserProfile`（name/timezone/location 等），正文不在 prompt 中直接内联（Responses 以 reference 方式要求读取）。
  - Evidence：`crates/config/src/loader.rs:596`
- `PEOPLE.md`：公共 roster（frontmatter+body）。Responses persona 会引用 `/moltis/data/PEOPLE.md` 并要求被问到时读取。
  - Evidence：`crates/agents/src/prompt.rs:99`
- `PEOPLE.md` 的同步机制（只同步 emoji/creature，保留 body）：`crates/config/src/loader.rs:345`
- `people/<name>/IDENTITY.md`：
  - 结构化字段：`load_persona_identity`（emoji/creature/vibe/name）
  - 正文：`load_persona_identity_md_raw`（Responses 会注入正文且 strip frontmatter）
  - Evidence：`crates/config/src/loader.rs:578`、`crates/agents/src/prompt.rs:77`
- `people/<name>/SOUL.md` / `TOOLS.md` / `AGENTS.md`：正文文本（注入到 Responses persona 或非 Responses 的 sections）。
  - Evidence：`crates/config/src/loader.rs:687`、`crates/config/src/loader.rs:708`

### Runtime / project / tools

- Runtime context：由 gateway 构造（Host + Sandbox exec routing）并注入到 Responses runtime_snapshot 或非 Responses `## Runtime`。
  - Evidence：`crates/gateway/src/chat.rs:846`
- Sandbox public view（仅复制 USER.md/PEOPLE.md 到 `.sandbox_views/<key>` 并 mount 到 `/moltis/data`）：`crates/tools/src/sandbox.rs:2798`
- Project context：`resolve_project_context(...)` 输出字符串注入。
  - Evidence：`crates/gateway/src/chat.rs:1842`
- Tools/skills：由 registry + discovered skills 注入；`native_tools` 决定“compact list vs 带 schema”。
  - Evidence：Responses 工具列表：`crates/agents/src/prompt.rs:229`；非 Responses 工具列表：`crates/agents/src/prompt.rs:542`

---

## 治理规范（Governance Norms）— 共性处理 vs Provider 适配（必须分割清楚）

> 目的：把“跨 provider 的 prompt 拼接（共性）”与“上游协议差异（适配）”彻底解耦。  
> 这样才能：1) 统一治理 system prompt 模板；2) 避免同一内容在多 provider/多入口重复拼接导致 drift；3) 让 as-sent 证据链可复盘。

### A) 共性处理（不需要 provider 适配）— Canonical `role=system` 系统提示

- **定义**：先构造一个跨 provider 统一的“大 system prompt 文本”，并把它作为内部 typed messages 的首条 `ChatMessage::System`。
- **适用范围**：该 canonical 文本对 provider 无关；即使最终 as-sent 不是 `role=system`（例如 openai-responses 的 `role=developer`、Anthropic 的 top-level `system`、local-llm 的模板字符串），内部也仍统一以 `ChatMessage::System` 承载。
- **规则确认（现行 v1）**：canonical v1 不再自动添加任何“包装标题”（例如 `# 系统（System）`、`# Type4 Persona…`）或额外固定块；最终文本完全由 `people/<persona_id>/{IDENTITY,SOUL,AGENTS,TOOLS}.md` 模板内容 + `{{var}}` 替换决定（project context/runtime/tools/skills 也必须通过模板显式引用对应变量才会出现）。
- **MUST**：所有 provider 路径都必须以 canonical v1 builder 产出该文本（`build_canonical_system_prompt_v1`）；legacy 的 `build_system_prompt_*` / `build_openai_responses_developer_prompts` 仅保留作兼容入口，不作为主路径依赖。
- **MUST**：gateway/tools 在组装 typed messages 时只负责把 `system_prompt` 放进 `ChatMessage::System`，其余历史消息/用户消息保持 typed message 语义不变。
  - Example（streaming/no-tools 路径）：`crates/gateway/src/chat.rs:6001`
- **MUST NOT**：在共性处理阶段引入 provider-specific 的 role/字段概念（例如 Responses 的 developer role、Anthropic 的 top-level system 字段、local 模板字符串）。

### B) Provider 适配（需要 provider 适配）— 从 typed messages 到 as-sent

- **定义**：把内部 typed messages（`ChatMessage::{System,User,Assistant,Tool}`）映射为各 provider 的真实请求体形态（as-sent）。
- **MUST**：所有 role/结构改写只允许发生在 provider adapter（或 renderer）层。
- **MUST NOT**：provider adapter 不得重新决定“system prompt 的内容结构/章节布局”，只能做协议必需的映射与降级。

#### B.1 openai-responses（协议语义：developer）

- 适配规则：`ChatMessage::System` → Responses `input[]` 中 `role=developer` message item。
  - Evidence：`crates/agents/src/providers/openai_responses.rs:33`
- **治理规范确认（to-be）**：openai-responses 的 developer preamble **必须折叠为 1 条 `role=developer` message item**。
  - **MUST**：允许内部仍维持多段（system/persona/runtime_snapshot）的逻辑层表示，但在 provider 适配层（as-sent）合并为单条 developer message。
  - **MUST**：内部 typed messages 侧也应只保留 1 条 preamble `ChatMessage::System`（将逻辑层合并为单段文本或通过稳定 section 分隔保留边界），避免多条 system message 在不同路径被误用/误映射。
  - **MUST**：合并时保序（system → persona → runtime_snapshot），并用稳定分隔（建议 `\n\n`）保留清晰 section 边界。
  - **MUST NOT**：不要把“层”的概念泄露成多条 developer items（除迁移期）；不要把 provider 协议差异回渗到 prompt 内容结构治理。
  - **WHY**：减少 as-sent 结构复杂度，降低 debug/适配分叉；同时保持 prompt 文本以“环境熟悉”为目的，而不是暴露内部实现细节。

#### B.2 OpenAI Chat Completions 兼容（协议语义：messages[] + role=system）

- 适配规则：typed message → `messages[]`（`role=system/user/assistant/tool`）。
  - Evidence：`crates/agents/src/model.rs:99`

#### B.3 Anthropic（协议语义：top-level system）

- 适配规则：抽取/合并 system messages → top-level `system`；其余 messages → `messages[]`。
  - Evidence：`crates/agents/src/providers/anthropic.rs:81`

#### B.4 GenAI（协议限制：无 tool message）

- 适配规则：tool messages 被跳过；multimodal 被降级。
  - Evidence：`crates/agents/src/providers/genai_provider.rs:54`

#### B.5 Local LLM（协议语义：模板字符串）

- 适配规则：把 typed messages 渲染为模型家族 chat template；system 可能被合并进首个 user turn（例如 Mistral `[INST]`）。
  - Evidence：`crates/agents/src/providers/local_llm/models/chat_templates.rs:133`

---

## Provider 适配（Provider Adapters）

### openai-responses

- `ChatMessage::System` → Responses `input[]` 中 `role=developer` 的 message item。
  - Evidence：`crates/agents/src/providers/openai_responses.rs:33`

### OpenAI Chat Completions 兼容类（openai / copilot / kimi 等）

- `ChatMessage::to_openai_value()` 映射 `system/user/assistant/tool` roles。
  - Evidence：`crates/agents/src/model.rs:99`

### Anthropic

- 把所有 system messages 抽取并拼接成 top-level `system` 字段；其余 messages 放入 `messages[]`。
  - Evidence：`crates/agents/src/providers/anthropic.rs:81`

### GenAI（genai crate）

- 只支持 system/user/assistant 文本；tool messages 被跳过、multimodal 被降级。
  - Evidence：`crates/agents/src/providers/genai_provider.rs:54`

### Local LLM (chat templates)

- 把 typed messages 格式化为模型家族的 chat template（ChatML/Llama3/Mistral/DeepSeek），system role 作为模板的一部分。
  - Evidence：`crates/agents/src/providers/local_llm/models/chat_templates.rs:38`

---

## Prompt 模板化（Template Contract, v1）— Skills / Tools（纯字符串替换，无模板逻辑）

> 目标：用户提供 `*.md` 模板字符串，系统在运行时提供 `{{var}} → String` 的 map。  
> 模板引擎只做**字符串替换**（无 if/loop/表达式），因此“可选段落”必须用“变量为空字符串”实现。

### 模板所有权与拼接规则（必须写死，避免越俎代庖）

- **所有 prompt 模板文本由用户维护**（位于当前 agent 的 `people/<name>/` 私有目录）。系统不得再“额外插入”任何自定义段落结构。
- **Type4 persona 模板正文（user-owned）**只由下列四个文件（按顺序）拼接得到，并使用固定分隔线：
  1) `IDENTITY.md`
  2) `SOUL.md`
  3) `AGENTS.md`
  4) `TOOLS.md`

  拼接规则（现行 v1）：
  - 按上述顺序渲染四个文件（对每个文件执行 strict `{{var}}` 替换，且 strip YAML frontmatter）。
  - 仅拼接非空段落；段落之间用 `\n\n` 连接；系统不再自动插入任何固定分隔线或占位符文本。
- 运行环境事实（Runtime vars）由用户在 `AGENTS.md` 中自行引用；工具/技能声明由用户在 `TOOLS.md` 中自行引用。
- 变量替换必须对四个文件都生效（用户可在任意文件中引用 `{{var}}`）。

> 说明：这里的“Type4 persona 模板正文”仅指可配置的 persona/type4 内容（四文件）。它不等于 provider as-sent 的 developer/system 角色包装，也不包含系统固定指南或 provider 适配层逻辑。

### 约束（来自现状实现）

- skills/tools 的集合在运行时是动态的（不可在模板里静态写死），因为：
  - tools 会因 config/provider/memory/MCP 变化：`crates/gateway/src/server.rs:2370`、`crates/gateway/src/mcp_service.rs:55`
  - session `mcp_disabled` 会移除 MCP tools：`crates/gateway/src/chat.rs:2270`、`crates/gateway/src/chat.rs:957`
  - runtime policy allow/deny 过滤 tools：`crates/gateway/src/chat.rs:963`、`crates/gateway/src/chat.rs:969`
- skills 的 `allowed_tools` **不会**用于全局 runtime 工具过滤（避免意外移除工具）：`crates/gateway/src/chat.rs:964`
- prompt 内对 tools 的信息密度已按 `native_tools` 分流：
  - native_tools=true：只显示 compact list（desc 截断 160），schema 通过 API 传：`crates/agents/src/prompt.rs:544`
  - native_tools=false：在 prompt 内嵌入 tool parameters JSON + `tool_call` 调用格式：`crates/agents/src/prompt.rs:559`、`crates/agents/src/prompt.rs:571`

### v1 模板变量（全部为 String；允许多行；空字符串表示“不展示该段”）

### 已确认事实（本轮讨论结论，后续实现/治理必须遵守）

- **事实 1**：每个 run 的 skills 列表与 tools 列表都可能变化（取决于当次运行环境与会话状态）；模板层不得假设它们稳定不变。
- **事实 2**：每个 run 的 system prompt（或 openai-responses developer preamble）中 skills/tools 信息必须来自当次运行的“实际可用 inventory”，并以本节约定的模板变量形式输出。
- **原则 1（稳定性/缓存，适用于所有模板变量）**：动态生成的模板变量内容必须保持**确定性与稳定性**（在环境不变时输出文本必须字节级稳定），以最大化上游 LLM 的 prompt cache 命中。
  - MUST：所有变量的渲染都使用稳定格式（固定缩进、固定换行、固定标题、固定单位与枚举值）。
  - MUST：任何列表型内容都做稳定排序（例如按 `name` 升序；必要时二级键为 `source`），并避免将“发现顺序/注册顺序”当作输出顺序。
    - Evidence（tools 列表顺序不稳定）：`ToolRegistry` 内部为 `HashMap`：`crates/agents/src/tool_registry.rs:36`；`list_schemas()` 迭代 `.values()`：`crates/agents/src/tool_registry.rs:92`
    - Evidence（skills 列表顺序保留输入顺序）：`generate_skills_prompt(skills)` 直接 for-loop：`crates/skills/src/prompt_gen.rs:12`
  - MUST：所有截断/省略规则必须固定且一致（例如 tool description 截断 160 chars），并在同一变量内保持可预测。
  - MUST NOT：依赖 HashMap 迭代顺序或运行时非确定性（例如 MCP 同步返回顺序）直接渲染到任何模板变量。
- **边界说明**：skills/tools 的“如何 discover/如何过滤/何时变化”的细节路径不属于本节讨论范围（将在 skills/tool 专项治理中细化）；本节只冻结**模板变量契约与输出形式**。

#### `{{skills_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含标题与正文）。
- v1 中文化要求（新增）：该段落中由系统拼接的标题与说明性文字必须为中文（技能名/描述允许保留英文原文）。
- 实施建议：保留 `<available_skills>...</available_skills>` 与 `<skill ...>` 结构不变，只将外围标题与说明文案中文化。
  - Evidence：现有生成函数（当前为英文，需要在实现阶段调整其 wrapper 文案）：`crates/skills/src/prompt_gen.rs:4`
  - 注入点（现状）：
    - Responses runtime_snapshot：`crates/agents/src/prompt.rs:218`
    - non-Responses system prompt：`crates/agents/src/prompt.rs:510`
- 注意：现有输出包含 `path=`，可能暴露本机路径（是否脱敏/改写属于 v2 议题）。
- 稳定性要求（必须）：`skills_md` 的渲染顺序不得依赖 skills 发现顺序；渲染前必须做稳定排序（建议：按 `skill.name` 升序，二级键为 `source`）。
  - Evidence：`generate_skills_prompt` 保留输入顺序：`crates/skills/src/prompt_gen.rs:12`
  - 排序规范（写死）：按 UTF-8 codepoint/字节序进行比较，区分大小写，不做 locale collation；必须使用稳定排序（stable sort）。

示例（非空；标题/说明中文，技能描述可原样）：

```md
## 可用技能

<available_skills>
<skill name="commit" source="skill" path="<...>/commit/SKILL.md">
Create git commits
</skill>
<skill name="playwright" source="plugin" path="<...>/playwright.md">
Browser automation
</skill>
</available_skills>

启用技能：阅读对应的 SKILL.md（或插件 .md）以获取完整说明，然后按其中步骤执行。
```

示例（空）：`""`

> 注：本文档用 `""` 表示“空字符串值”（展示用）；实际替换时 value 为空字符串本身，不包含引号字符。

#### `{{native_tools_index_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含标题与 compact list）。
- 适用范围：仅在 **本次运行的 native tool-calling** 时允许为非空；否则必须渲染为 `""`。
- 格式建议（与现状 native 分支一致）：
  - `## 可用工具`\n\n`- \`name\`: <desc...>`
  - desc 截断 160 chars（现状为 160）：`crates/agents/src/prompt.rs:550`
- 数据来源：运行时 `ToolRegistry::list_schemas()`（包含 name/description/parameters/source/mcpServer）：`crates/agents/src/tool_registry.rs:92`
- 稳定性要求（必须）：渲染前必须对 schemas 做稳定排序（建议：按 `name` 升序；二级键为 `source`；三级键为 `mcpServer`）。
  - Evidence：`ToolRegistry` 内部为 `HashMap`，`list_schemas()` 的遍历顺序不稳定：`crates/agents/src/tool_registry.rs:36`、`crates/agents/src/tool_registry.rs:92`
  - 排序规范（写死）：排序键为 `(name, source, mcpServer)`；缺失字段视为空字符串；按 UTF-8 codepoint/字节序比较，区分大小写，不做 locale collation；必须使用稳定排序（stable sort）。

规则（写死）：

- native tool-calling（本次运行）：模板**可选**引用 `native_tools_index_md`（用于环境熟悉；schema 仍通过 API `tools` 字段提供）。
  - Evidence（schemas_for_api gating）：`crates/agents/src/runner.rs:712`
  - Evidence（provider 仅在 tools 非空时写入 request body）：
    - OpenAI chat completions：`crates/agents/src/providers/openai.rs:428`
    - OpenAI Responses：`crates/agents/src/providers/openai_responses.rs:825`
    - Anthropic：`crates/agents/src/providers/anthropic.rs:210`
- 非 native tool-calling / no-tools（本次运行）：渲染器必须将其置为 `""`；模板可以引用该占位符，但不得依赖其非空（避免错误理解为“能用 native tool-calling”）。

示例（非空）：

```md
## 可用工具

- `exec`: Execute shell commands
- `web_fetch`: Fetch web content
- `memory_search`: Search long-term memory
```

示例（空）：`""`

#### `{{non_native_tools_catalog_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含标题 + 工具清单 + 每个 tool 的 parameters JSON）。
- 命名解释：这里的 `catalog` 指“工具目录（inventory）+ 详细参数”，不是“仅 schema”。该变量本身就包含每个 tool 的 `name` 与 `description`，因此也能承担“工具列表”的作用。
- inclusion rule：
  - native tool-calling（本次运行）→ 必须为空字符串（避免重复巨大 schema；schema 走 API）：`crates/agents/src/prompt.rs:545`
  - non-native tool-calling（本次运行）→ 输出 schemas（现状输出 per-tool `parameters` pretty JSON）：`crates/agents/src/prompt.rs:559`
  - no-tools（本次运行，包括 stream_only 或 tools inventory 为空）→ 必须为空字符串
- 稳定性要求（必须）：与 `native_tools_index_md` 相同，schemas 必须稳定排序；每个 tool 的参数 JSON pretty 格式必须稳定（固定缩进/键顺序由 serializer 决定，但 tool 级顺序必须稳定）。
  - v1 稳定性补充（必须）：对 `parameters` 的 `serde_json::Value` 在 pretty 输出前做“对象 key 的递归排序/canonicalize”，确保 key 顺序不依赖 HashMap/生成顺序。

示例（non-native 时非空；标题/说明中文，tool 描述与参数 JSON 可原样）：

````md
## 工具目录与参数

### exec
Execute shell commands

参数（Parameters）：
```json
{
  "type": "object",
  "required": ["command"],
  "properties": {
    "command": {"type": "string"}
  }
}
```
````

示例（native 时为空）：`""`

#### `{{non_native_tools_calling_guide_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含中文标题与 ` ```tool_call ... ``` ` 规范）。
- inclusion rule：
  - 仅当 non-native tool-calling（本次运行）且 tools inventory 非空时输出（与现状一致）：`crates/agents/src/prompt.rs:571`
  - 否则必须为空字符串（包括 native tool-calling 与 no-tools）。
- WHY：非 native tool-calling 的 runner 依赖该规范从文本中提取 tool call：`crates/agents/src/runner.rs:136`

示例（non-native 时非空；说明中文，但 `tool_call` fence 与 JSON 格式必须保持稳定）：

````md
## 如何调用工具

要调用工具，你必须输出且只输出一个 JSON 代码块，格式如下（前后不能有任何其它文字）：

```tool_call
{"tool": "<tool_name>", "arguments": {<arguments>}}
```

你必须把该 `tool_call` 代码块作为**整段回复**，前后不要添加任何解释（即便当前是语音输出模式；工具调用回合不属于最终语音回复）。
工具执行完成后，你会收到结果，然后再正常回复用户。
````

示例（native 时为空）：`""`

### 模板写法（推荐）

模板中只需按顺序引用变量（变量内部自带标题，避免空标题）。

> v1 推荐：**单一模板即可**。模板可以同时引用 native 与 non-native 的 tools 变量；不适用的变量在该运行模式下必须被渲染为 `""`（空字符串），从而在最终 prompt 中自然消失。

---

## v1 实施方案（修正版，2026-03-04）

> 本节是“可执行的实施方案摘要”，用于把本文从 audit 升级为 roadmap。细节与验收在独立 issue 文档中维护（见 Issue Index 的 Doc）。

### Phase A（先决：稳定性 + Responses 折叠，P0）

- 目标：让 prompt 输出“可复现、可对比、可缓存”，并把 Responses 的 as-sent 结构收敛到 1 条 developer item（降低 drift 面）。
- 交付物：
  - 稳定排序与 stable pretty JSON（tools/skills）。
  - openai-responses preamble 折叠（gateway + spawn_agent）。
  - 对应单元/集成测试（golden 或断言）。

### Phase B（PromptParts + renderer + 入口去重，P0）

- 目标：把“内容治理（canonical）”与“协议适配（renderer/adapter）”彻底分离，消除多入口重复拼装。
- 交付物：
  - PromptParts（canonical 中间表示）与 renderer；
  - gateway chat/send_sync/debug 与 tools.spawn_agent 统一入口；
  - as-sent 证据链字段（debug + hooks）统一口径。

### Phase C（Type4 模板化 v1，P0）

- 目标：让用户拥有 persona 布局的所有权（四文件拼接 + `{{var}}`），系统只提供稳定的 vars 与固定块，不再擅自改布局。
- 交付物：
  - `people/<name>/{IDENTITY,SOUL,AGENTS,TOOLS}.md` 拼接与 `{{var}}` 替换；
  - `skills_md` / `native_tools_index_md` / `non_native_tools_catalog_md` / `non_native_tools_calling_guide_md` 等 vars 渲染实现；
  - 稳定性与回归测试。

```md
## 能力

{{skills_md}}

{{native_tools_index_md}}

{{non_native_tools_catalog_md}}
{{non_native_tools_calling_guide_md}}

{{long_term_memory_md}}
```

语音相关约束建议放在模板末尾（作为输出格式约束），例如：

```md
{{voice_reply_suffix_md}}
```

### Requiredness Matrix（必须性矩阵，供模板作者理解）

#### Effective Flags（必须先收敛口径，不允许实现侧自行发明）

- `supports_tools`：provider capability（`LlmProvider::supports_tools()`）。
- `stream_only`：本次运行无法同步执行工具（例如 gateway 的 `stream_only = explicit_stream_only || !has_tools_sync()`）。
  - Evidence：`crates/gateway/src/chat.rs:2026`
- `tools_inventory_non_empty`：本次运行经过过滤后的 tools inventory 非空（例如 `ToolRegistry::list_schemas().len() > 0`）。
- `tools_usable`（本次运行）：`stream_only == false && tools_inventory_non_empty == true`。
- `native_tool_calling`（本次运行）：`tools_usable == true && supports_tools == true`。
- `non_native_tool_calling`（本次运行）：`tools_usable == true && supports_tools == false`。

> 说明：所有 tools 相关变量是否可为非空，必须以“本次运行”的 `native_tool_calling/non_native_tool_calling/tools_usable` 为准，而不是仅看 provider capability。

#### 变量要求（按本次运行模式）

- **no-tools（tools_usable=false，包括 stream_only 或 tools inventory 为空）**：
  - `native_tools_index_md`：必须为空
  - `non_native_tools_catalog_md`：必须为空
  - `non_native_tools_calling_guide_md`：必须为空
  - `long_term_memory_md`：必须为空
- **native tool-calling（native_tool_calling=true）**：
  - `native_tools_index_md`：可选引用（用于环境熟悉；工具能力由 API `tools` 提供）
  - `non_native_tools_catalog_md`：必须为空
  - `non_native_tools_calling_guide_md`：必须为空
- **non-native tool-calling（non_native_tool_calling=true）**：
  - `native_tools_index_md`：必须为空（模板可以引用但不得依赖其非空）
  - `non_native_tools_catalog_md`：**硬依赖（hard-required）**：必须引用且必须非空（目录+schemas；让模型知道工具名/用途/参数）
  - `non_native_tools_calling_guide_md`：**硬依赖（hard-required）**：必须引用且必须非空（runner 的文本 tool-call fallback 依赖该格式）
- **skills_md**：**软依赖（soft-required）**：强烈建议引用（用于让模型“知道有哪些 skills 可启用”）；当本回合无 skills 时其值允许为空字符串 `""`。
  - 允许模板未引用（不应阻断本次运行），但必须输出 warning 并在 debug 中标注缺失。
- **memory hint（长期记忆）**：
  - `long_term_memory_md`：当存在 `memory_search` 工具时应为非空；不存在时必须为空。
  - 允许模板未引用（不应阻断），但建议 warning。
- **voice（语音输出约束）**：
  - `voice_reply_suffix_md`：当 `reply_medium == "语音"` 时应为非空；否则必须为空。
  - 允许模板未引用（不应阻断），但建议 warning。

#### Template Validation（必须，不允许“靠约定”）

- 占位符语法必须写死：仅识别 **无空格** 的 `{{var}}`，其中 `var` 仅允许 `[a-z0-9_]+`。
- 渲染前必须扫描四文件拼接后的模板文本，提取所有 `{{var}}` 占位符集合。
- 对于当前运行模式 Requiredness Matrix 标记为 **硬依赖（hard-required）** 的变量：若模板未包含对应占位符，必须 fail-fast（拒绝构造 prompt），返回明确错误（例如 `PROMPT_TEMPLATE_MISSING_REQUIRED_VAR`）。
- 对于当前运行模式标记为 **软依赖（soft-required）** 的变量：若模板未包含对应占位符，不得 fail-fast；必须输出 warning（并在 debug 中标注缺失）。
- 对于模板包含但当前运行模式要求必须为空字符串的变量：其 value 必须为 `""`（但不因为模板引用而自动启用该能力）。
- 对于模板中出现的 `{{var}}` 占位符：若 `var` 不在本次 vars_map 中，必须 fail-fast（避免 typo 被悄悄吞掉）。
- 渲染后必须再次扫描，确保不存在任何仍符合语法的 `{{var}}`（未替换占位符）；否则必须 fail-fast。

### 一致性要求（Responses / non-Responses）

- `skills_md` / `native_tools_index_md` / `non_native_tools_*` 变量必须由同一份运行时数据渲染，供：
  - non-Responses 的 canonical `ChatMessage::System` system prompt
  - openai-responses 的 canonical `ChatMessage::System`（适配后为单条 `role=developer`）
- 避免出现“Responses 与 non-Responses 看到的 tools/skills inventory 不一致”。

---

## Prompt 模板化（Template Vars, v1）— 运行环境（Runtime，极简/充分/不暴露实现细节）

> 目标：仅提供“让 agent 做出正确行为决策所必需”的运行环境事实；不提供内部实现细节与高波动字段。  
> 所有变量均为 String（可直接 `{{var}}` 替换），且必须遵守“稳定性/缓存”原则（环境不变时字节级稳定）。

### v1 中文化策略（i18n：尽可能输出中文取值）

> 新增需求：**所有环境变量**（短语级与段落级）只要能翻译成中文，就必须翻译成中文。

- v1 约定：本节 Runtime 变量与本节“能力段落”变量的取值**默认输出简体中文**。
- v1 扩展（新增）：Skills/Tools 段落变量（`skills_md` / `native_tools_index_md` / `non_native_tools_catalog_md` / `non_native_tools_calling_guide_md`）中由系统拼接的**段落标题与说明性文字**也应尽可能输出简体中文。
- 保留原样的内容（不可翻译/不应翻译）：
  - 路径（`/moltis/data`、`/home/...`、`C:\\...`）
  - tool 名称（例如 `memory_search`）与代码标识符
  - `session_id` 原始字符串（它本质是标识符，不是自然语言）
  - skills/tools 的 `name`/`source`/`path` 字段、以及工具参数 JSON（它们属于协议/契约的一部分，避免翻译造成解析或认知歧义）
- 目标：让用户模板（通常为中文）可以直接把这些变量插入句子/段落而不显得“中英夹杂乱”。
- 注意：若未来需要英文模板输出（例如 `PROMPT.en.md` 风格），应在 v2 引入并治理 `*_en` 平行变量（v1 不扩展变量集合）。

#### 实现落点（Implementation Notes，仅作为落地提示）

> 目的：明确哪些“硬编码拼接文案”需要在实现阶段中文化；避免只改了 issue 文档但实现侧忘记改。  
> 约束：只中文化“系统拼接的标题与说明性文字”，不翻译 tool/skill 名称、description 原文、JSON schema。

- `skills_md`（标题与说明中文化）：
  - 当前生成函数使用英文硬编码：`crates/skills/src/prompt_gen.rs:11`（`## Available Skills`）与 `crates/skills/src/prompt_gen.rs:33`（To activate...）。
  - v1 目标输出：`## 可用技能` + 中文说明（描述内容允许保留英文原文）。
- `native_tools_index_md` / `non_native_tools_catalog_md` / `non_native_tools_calling_guide_md`（标题与说明中文化）：
  - 当前 non-Responses prompt builder 使用英文硬编码：
    - `crates/agents/src/prompt.rs:543`（`## Available Tools`）
    - `crates/agents/src/prompt.rs:564`（`Parameters:`）
    - `crates/agents/src/prompt.rs:573`（`## How to call tools`）
  - v1 目标输出：
    - `## 可用工具`
    - `参数（Parameters）：`（可保留括注避免歧义）
    - `## 如何调用工具`
  - 兼容性注意：runner 的非 native tool-call fallback 只依赖围栏标记 ` ```tool_call ` 与 JSON 格式，不依赖标题文案，因此标题中文化不会影响解析。
    - Evidence：解析器查找 `"```tool_call"`：`crates/agents/src/runner.rs:242`

#### 成组示例（Environment Vars Map，v1）

> 说明：这里展示的是“模板变量导出结果”的**成组示例**，用于你确认：  
> 1) 取值是否足够中文化；2) 段落级变量的换行/空白契约是否清晰；3) sandbox 下 `agent_data_dir_path` 是否确认为 `""`。

##### 示例 A：Host + 文字输出 + 有长期记忆（memory tools 已注册）

短语级（单行）变量：

```txt
host_os = "Linux 系统"
session_id = "main"
reply_medium = "文字"
exec_location = "宿主机上"
sandbox_reuse_policy = "不适用（未启用沙盒）"
system_data_dir_path = "/home/luy/.moltis"
agent_data_dir_path = "/home/luy/.moltis/people/alma"
data_dir_access = "读写"
network_policy = "允许联网"
host_privilege_policy = "宿主机不支持非交互 sudo（任何宿主机安装/系统改动必须先征求用户同意）"
```

段落级（多行）变量：

```txt
long_term_memory_md = "## 长期记忆\n\n你可以使用一个长期记忆系统来回忆过去的对话、决策与上下文。\n- 当用户提到\"之前做过什么/上次讨论到哪/你还记得吗\"时，优先使用 `memory_search` 主动检索再回答。\n\n"
voice_reply_suffix_md = ""
```

##### 示例 B：Sandbox + 语音输出 + 无长期记忆（memory tools 未注册）

短语级（单行）变量：

```txt
host_os = "Linux 系统"
session_id = "main"
reply_medium = "语音"
exec_location = "沙盒中"
sandbox_reuse_policy = "会话复用"
system_data_dir_path = "/moltis/data"
agent_data_dir_path = ""
data_dir_access = "只读"
network_policy = "禁止联网"
host_privilege_policy = "不适用（本回合不在宿主机执行）"
```

段落级（多行）变量：

```txt
long_term_memory_md = ""
voice_reply_suffix_md = "## 语音回复模式\n\n用户将以语音形式听到你的回复。请为\"听\"而写，而不是为\"读\"而写：\n- 使用自然、口语化的完整句子；不要使用项目符号列表、编号列表或标题。\n- 禁止输出原始 URL。请用资源名称描述（例如用\"Rust 官方文档网站\"，而不是具体链接）。\n- 不要使用任何 Markdown 格式：不要加粗/斜体/标题/代码块/行内反引号。\n- 对可能被 TTS 误读的缩写进行拼读（例如把\"API\"写成\"A-P-I\"，\"CLI\"写成\"C-L-I\"）。\n- 保持简洁：最多两到三段短段落。\n- 使用自然的衔接与过渡，避免生硬的堆砌。\n\n"
```

##### 示例 C：Host + 语音输出 + 有长期记忆（memory tools 已注册）

> 用于验证：当 voice 与 memory 同时启用时，两个段落变量都为非空；模板中按顺序引用即可。

短语级（单行）变量：

```txt
host_os = "Linux 系统"
session_id = "main"
reply_medium = "语音"
exec_location = "宿主机上"
sandbox_reuse_policy = "不适用（未启用沙盒）"
system_data_dir_path = "/home/luy/.moltis"
agent_data_dir_path = "/home/luy/.moltis/people/alma"
data_dir_access = "读写"
network_policy = "允许联网"
host_privilege_policy = "宿主机不支持非交互 sudo（任何宿主机安装/系统改动必须先征求用户同意）"
```

段落级（多行）变量：

```txt
long_term_memory_md = "## 长期记忆\n\n你可以使用一个长期记忆系统来回忆过去的对话、决策与上下文。\n- 当用户提到\"之前做过什么/上次讨论到哪/你还记得吗\"时，优先使用 `memory_search` 主动检索再回答。\n\n"
voice_reply_suffix_md = "## 语音回复模式\n\n用户将以语音形式听到你的回复。请为\"听\"而写，而不是为\"读\"而写：\n- 使用自然、口语化的完整句子；不要使用项目符号列表、编号列表或标题。\n- 禁止输出原始 URL。请用资源名称描述（例如用\"Rust 官方文档网站\"，而不是具体链接）。\n- 不要使用任何 Markdown 格式：不要加粗/斜体/标题/代码块/行内反引号。\n- 对可能被 TTS 误读的缩写进行拼读（例如把\"API\"写成\"A-P-I\"，\"CLI\"写成\"C-L-I\"）。\n- 保持简洁：最多两到三段短段落。\n- 使用自然的衔接与过渡，避免生硬的堆砌。\n\n"
```

##### 示例 D：Skills/Tools 段落变量（中文化输出，v1）

> 用于确认：skills/tools 段落变量中的“段落标题/说明性文字”已中文化；但 tool/skill 的 `name`、tool 参数 JSON 与原始 description 允许保留英文。

场景 D1：native tools（supports_tools=true）

```txt
skills_md = "## 可用技能\n\n<available_skills>\n<skill name=\"commit\" source=\"skill\" path=\"<...>/commit/SKILL.md\">\nCreate git commits\n</skill>\n</available_skills>\n\n启用技能：阅读对应的 SKILL.md（或插件 .md）以获取完整说明，然后按其中步骤执行。\n\n"

native_tools_index_md = "## 可用工具\n\n- `exec`: Execute shell commands\n- `web_fetch`: Fetch web content\n\n"

non_native_tools_catalog_md = ""
non_native_tools_calling_guide_md = ""
```

场景 D2：non-native tools（supports_tools=false）

```txt
skills_md = "## 可用技能\n\n<available_skills>\n<skill name=\"commit\" source=\"skill\" path=\"<...>/commit/SKILL.md\">\nCreate git commits\n</skill>\n</available_skills>\n\n启用技能：阅读对应的 SKILL.md（或插件 .md）以获取完整说明，然后按其中步骤执行。\n\n"

native_tools_index_md = ""

non_native_tools_catalog_md = "## 工具目录与参数\n\n### exec\nExecute shell commands\n\n参数（Parameters）：\n```json\n{\n  \"type\": \"object\",\n  \"required\": [\"command\"],\n  \"properties\": {\n    \"command\": {\"type\": \"string\"}\n  }\n}\n```\n\n"

non_native_tools_calling_guide_md = "## 如何调用工具\n\n要调用工具，你必须输出且只输出一个 JSON 代码块，格式如下（前后不能有任何其它文字）：\n\n```tool_call\n{\"tool\": \"<tool_name>\", \"arguments\": {<arguments>}}\n```\n\n你必须把该 `tool_call` 代码块作为**整段回复**，前后不要添加任何解释。\n工具执行完成后，你会收到结果，然后再正常回复用户。\n\n"
```

### 字符串替换约束（String Substitution Invariants）

- 模板系统只做**纯字符串替换**：`{{var}}` → value（按字节插入），不做转义、不做条件判断、不做循环、不做表达式。
- 运行环境类变量（本节 Runtime 变量）**建议为单行字符串**（不包含换行），以便安全地插入到用户模板中的 prompt **句子**里。具体如下（v1）：
  - 建议单行：`host_os`、`session_id`、`reply_medium`、`exec_location`、`sandbox_reuse_policy`、`system_data_dir_path`、`agent_data_dir_path`、`data_dir_access`、`network_policy`、`host_privilege_policy`
- Skills/Tools 类变量允许多行（包含换行），因为它们预期在用户模板中作为独立的 prompt **段落**被整体引用：
  - 多行段落：`skills_md`、`native_tools_index_md`、`non_native_tools_catalog_md`、`non_native_tools_calling_guide_md`、`long_term_memory_md`、`voice_reply_suffix_md`
- 多行段落变量的空白/换行约定（写死，避免双重空行与不稳定拼接）：
  - 要么为 `""`（空字符串），要么必须满足：
    1) **不以换行开头**（首字符必须是 `#`，即以 `## ...` 标题开头）
    2) **以恰好一个空行结束**（末尾必须是 `\n\n`）
    3) 段落内部换行与缩进必须稳定（固定缩进、固定空行数）
- 特例：`voice_reply_suffix_md` 的上游来源 `VOICE_REPLY_SUFFIX` 当前以 `\n\n` 开头（便于 append），但作为模板变量值时必须做归一化：trim 掉前导换行，并保证以 `\n\n` 结尾（必要时追加换行）以满足本约定。
- 路径类变量（`system_data_dir_path` / `agent_data_dir_path`）必须输出为真实绝对路径；可能包含空格或 Windows 反斜杠，模板作者应在 Markdown 中用反引号包裹以提升可读性。
  - 路径类变量（`system_data_dir_path` / `agent_data_dir_path`）的输出约束：
    - host 模式：必须输出真实绝对路径（不允许隐藏）。
    - sandbox 模式：`system_data_dir_path` 固定为 `/moltis/data`；`agent_data_dir_path` 必须为空字符串 `""`（表示未挂载/不可访问）。
    - 可能包含空格或 Windows 反斜杠，模板作者应在 Markdown 中用反引号包裹以提升可读性。

### v1 模板变量总表（全量枚举，便于逐一确认）

> 说明：本仓库 v1 约定的“运行时生成模板变量”仅限于此表。  
> 所有变量均为 String；默认允许空字符串 `""` 表示“省略该段/该行”，除非在 Requiredness Matrix 中标记为必须非空。

- **Runtime（环境）**：`host_os`、`session_id`、`reply_medium`、`exec_location`、`sandbox_reuse_policy`、`system_data_dir_path`、`agent_data_dir_path`、`data_dir_access`、`network_policy`、`host_privilege_policy`
- **Skills（技能）**：`skills_md`
- **Tools（工具）**：`native_tools_index_md`、`non_native_tools_catalog_md`、`non_native_tools_calling_guide_md`
- **Memory（长期记忆）**：`long_term_memory_md`
- **Voice（语音输出约束）**：`voice_reply_suffix_md`

### v1 运行环境变量（仅这些；建议默认全提供，模板可选择引用）

#### `{{host_os}}`

- 含义：宿主机操作系统标识（仅用于环境熟悉；不要用于推断用户居住地）。
- 候选值（自然语言）：`Windows 系统` / `Linux 系统` / `macOS 系统` / `未知系统`
- Evidence（来源）：`PromptHostRuntimeContext.os` 由 `std::env::consts::OS` 填充：`crates/gateway/src/chat.rs:907`

示例（变量值）：

```txt
Linux 系统
```

#### `{{session_id}}`

- 含义：当前会话标识（仅用于“本回合/本会话范围”的环境识别，不用于推断用户身份信息）。
- 候选值（自然语言）：任意非空字符串（例如 `main`、`telegram:chat:-100...`、`cron:<name>`）。
- 缓存说明：openai-responses 的 prompt cache 以 request 的 `prompt_cache_key` 为准（由 `LlmRequestContext.session_id` 派生），而不是从 prompt 文本中解析；因此 `session_id` 是否出现在 prompt 文本中不会影响 `prompt_cache_key`。
  - Evidence：`crates/agents/src/providers/openai_responses.rs:569`
- Evidence（来源）：`PromptHostRuntimeContext.session_id` 由 `session_key` 填充：`crates/gateway/src/chat.rs:912`

示例（变量值）：

```txt
main
```

#### `{{reply_medium}}`

- 含义：本回合“用户输入媒介 / 期望输出媒介”的最终判定结果（用于让模板作者在句子中自然描述当前输出约束；不要用于推断用户身份信息）。
- 候选值（自然语言，禁止输出“unknown/未知”）：`文字` / `语音`
- Evidence（来源）：gateway 使用 `ReplyMedium`（Text/Voice）推断：
  - 枚举定义：`crates/gateway/src/chat.rs:116`
  - `_input_medium` 参数解析：`crates/gateway/src/chat.rs:671`
  - 推断/覆盖优先级：`crates/gateway/src/chat.rs:714`

示例（直接作为变量值）：

```txt
文字
```

```txt
语音
```

示例（模板句子用法）：

```md
本回合期望输出媒介：{{reply_medium}}。
```

#### `{{exec_location}}`

- 含义：本回合 `exec` 命令实际运行位置。
- 候选值（自然语言，禁止输出“unknown/未知”）：`沙盒中` / `宿主机上`
- Evidence：沙箱是否启用来自 router.is_sandboxed → `PromptSandboxRuntimeContext.exec_sandboxed`：`crates/gateway/src/chat.rs:858`、`crates/gateway/src/chat.rs:861`

示例（变量值）：

```txt
沙盒中
```

#### `{{sandbox_reuse_policy}}`

- 含义：沙盒复用边界（容器复用粒度）。
- 候选值（自然语言，禁止输出“unknown/未知”）：`会话复用` / `聊天复用` / `账号复用` / `全局复用` / `不适用（未启用沙盒）`
- Evidence：scope 来源于 sandbox config：`crates/gateway/src/chat.rs:864`；scope→复用 key 的实际实现：`crates/tools/src/sandbox.rs:2423`

示例（变量值）：

```txt
会话复用
```

#### `{{system_data_dir_path}}`

- 含义：本回合“公共数据目录”的路径（用于引用 `USER.md`/`PEOPLE.md`）。
- 候选值：
  - sandbox：固定为 `/moltis/data`
  - host：宿主机实际 data_dir **绝对路径（必须真实输出，不允许隐藏）**
- Evidence（sandbox 固定路径）：`SANDBOX_GUEST_DATA_DIR = "/moltis/data"`：`crates/tools/src/sandbox.rs:323`
- v1 约束（必须）：sandbox 内的 `/moltis/data` 必须是 **public data view**（只包含 `USER.md` / `PEOPLE.md`），不得泄露 `people/<name>/`。
  - Evidence（bind mount 通过 public view 目录）：`prepare_public_data_view(...)`：`crates/tools/src/sandbox.rs:973`
  - 风险提示：当 `data_mount_type=volume` 时当前实现会直接挂载 volume 到 `/moltis/data`（不经过 public view 过滤）：`crates/tools/src/sandbox.rs:976`
  - v1 治理要求：若无法保证 volume 内容为 public view，则启动期必须 fail-fast（拒绝该配置），或在实现侧把 volume mount 也改为 public view。

示例（变量值；sandbox）：

```txt
/moltis/data
```

#### `{{agent_data_dir_path}}`

- 含义：当前 agent 的私有化数据目录（`people/<name>/` 目录的绝对路径），用于引导 agent 在宿主机上自行读取/查看其私有文档（如 `IDENTITY.md`/`SOUL.md`/`TOOLS.md`/`AGENTS.md`）。
- 候选值：
  - host：`<data_dir>/people/<name>`（必须真实输出绝对路径）
  - sandbox：空字符串 `""`（v1 契约：沙盒 `/moltis/data` 必须是 public view，仅暴露 `USER.md`/`PEOPLE.md`，不暴露 `people/<name>/`）
    - 若 sandbox 实际挂载方式不满足 public view（例如 volume 直挂且包含 `people/`），则该配置不符合 v1 契约，必须 fail-fast 或在实现侧收敛为 public view。
- 取值约束：`<name>` 必须通过 `is_valid_person_name` 校验（ASCII + 长度限制）。
- Evidence：
  - people 根目录：`people_dir()` → `<data_dir>/people`：`crates/config/src/loader.rs:521`
  - person 目录：`person_dir(name)` → `<data_dir>/people/<name>`：`crates/config/src/loader.rs:543`
  - 沙盒 public view 仅复制 `USER.md`/`PEOPLE.md`：`crates/tools/src/sandbox.rs:344`

示例（变量值；host）：

```txt
/home/luy/.moltis/people/alma
```

示例（变量值；sandbox）：

```txt
""
```

#### `{{data_dir_access}}`

- 含义：公共数据目录在本回合的访问权限。
- 候选值（自然语言，禁止输出“unknown/未知”）：`只读` / `读写` / `未挂载`
- Evidence：sandbox data_mount 来自 config：`crates/gateway/src/chat.rs:866`；mount enum：`crates/tools/src/sandbox.rs:403`

示例（变量值）：

```txt
只读
```

#### `{{network_policy}}`

- 含义：本回合联网策略（尤其影响沙盒内是否允许联网）。
- 候选值（自然语言，禁止输出“unknown/未知”）：`允许联网` / `禁止联网`
- Evidence：no_network 来自 config：`crates/gateway/src/chat.rs:867`；Docker 后端会用 `--network=none`：`crates/tools/src/sandbox.rs:1236`

示例（变量值）：

```txt
禁止联网
```

#### `{{host_privilege_policy}}`

- 含义：宿主机系统改动/安装依赖的权限策略（只描述行为约束，不描述内部实现）。
- 候选值（自然语言，禁止输出“unknown/未知”）：
  - `宿主机支持非交互 sudo（可自动安装依赖）`
  - `宿主机不支持非交互 sudo（任何宿主机安装/系统改动必须先征求用户同意）`
  - `不适用（本回合不在宿主机执行）`
  - `无法确定宿主机提权能力（任何宿主机安装/系统改动必须先征求用户同意）`
- Evidence：host sudo 探测（用于 prompt runtime context）在构造函数中 join：`crates/gateway/src/chat.rs:852`、`crates/gateway/src/chat.rs:924`

示例（变量值）：

```txt
宿主机不支持非交互 sudo（任何宿主机安装/系统改动必须先征求用户同意）
```

---

### v1 运行态“能力段落”变量（可选，多行段落；空字符串表示省略该段）

> 说明：以下变量属于“多行段落”，用于让模板作者把某些“运行态能力/输出约束”作为独立段落引用。  
> 它们不属于 Skills/Tools inventory 本身，但其启用/禁用由当次运行的实际能力（tools inventory / reply_medium）决定。

> 边界提醒（实现侧治理规范）：当前 as-is 实现中，voice 与 memory 提示可能由 gateway/prompt builder **硬编码追加**（append）。当模板系统落地并开始把它们暴露为模板变量后，必须保证“只注入一次”：要么用户模板引用 `{{voice_reply_suffix_md}}`/`{{long_term_memory_md}}`，要么系统 append（两者不可同时存在），否则会出现重复段落。

#### `{{long_term_memory_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含标题与正文）。
- 触发条件（写死）：当且仅当当次运行的工具 inventory 中存在 `memory_search` 工具时，才渲染为非空。
  - Evidence（工具名定义）：`MemorySearchTool::name() == "memory_search"`：`crates/memory/src/tools.rs:21`
  - Evidence（当前 as-is 注入条件）：prompt builder 通过 `tool_schemas` 检测 `memory_search`：`crates/agents/src/prompt.rs:530`
- 建议内容（与现状一致，避免 drift）：复用当前 prompt builder 的 Long-Term Memory 文案（工具名由该变量本身提供；模板作者不要在自己的文本中硬编码 `memory_search`）。
  - Evidence（现状 non-Responses 文案）：`crates/agents/src/prompt.rs:534`
  - Evidence（现状 Responses 文案）：`crates/agents/src/prompt.rs:263`
- 稳定性要求：段落标题/换行/列表格式必须固定；v1 不在该段落中额外引入 `memory_get`（避免与现状提示文案 drift）。
  - Evidence（memory_get 工具存在但当前提示文案未提及）：`crates/memory/src/tools.rs:99`

示例（非空；中文段落，示意；末尾必须是 `\n\n`）：

```md
## 长期记忆

你可以使用一个长期记忆系统来回忆过去的对话、决策与上下文。
- 当用户提到“之前做过什么/上次讨论到哪/你还记得吗”时，优先使用 `memory_search` 主动检索再回答。

```

示例（空）：

```txt
""
```

示例（以“原始字符串”形式明确换行语义；末尾必须是 `\n\n`）：

```txt
"## 长期记忆\n\n你可以使用一个长期记忆系统来回忆过去的对话、决策与上下文。\n- 当用户提到\"之前做过什么/上次讨论到哪/你还记得吗\"时，优先使用 `memory_search` 主动检索再回答。\n\n"
```

#### `{{voice_reply_suffix_md}}`

- 值：要么为空字符串 `""`，要么是一个**完整段落**（包含标题与正文）。
- 触发条件（写死）：当且仅当 `reply_medium == "语音"` 时，才渲染为非空。
- 内容来源：复用现有固定常量 `VOICE_REPLY_SUFFIX` 的文本（以保持与现状一致，避免 voice 模式行为漂移）；但作为模板变量值时必须按本节约定做**归一化**（trim 前导换行，并保证以 `\n\n` 结尾）。
  - Evidence（常量定义）：`crates/agents/src/prompt.rs:332`
  - Evidence（non-Responses 追加位置）：`crates/gateway/src/chat.rs:2503`、`crates/gateway/src/chat.rs:4650`、`crates/gateway/src/chat.rs:5996`
  - Evidence（Responses 追加位置：追加到 runtime_snapshot）：`crates/gateway/src/chat.rs:257`、`crates/gateway/src/chat.rs:267`
  - Evidence（Responses 追加行为单测）：`crates/gateway/src/chat.rs:8645`
- 注意：该变量的存在是为了让“voice 输出约束”完全由用户模板可控地引用（以空字符串实现可选段落），而不是由系统在模板之外强行追加。
  - 实施治理目标：当模板系统落地后，应删除 gateway 中对 `VOICE_REPLY_SUFFIX` 的硬编码追加，避免重复注入。
- 例外（必须，避免与 tools 规范冲突）：当本回合需要调用工具（尤其是 non-native tool-calling fallback）时，允许输出 ` ```tool_call ... ``` ` 代码块作为“工具调用回合”的专用格式；该代码块不是面向用户的最终语音回复。
  - 工具执行完成后，最终自然语言回复必须严格遵守“语音回复模式”（不含 Markdown / 不含代码块 / 不含原始 URL）。

示例（非空；作为变量值时必须无前导空行，且末尾必须是 `\n\n`）：

```md
## 语音回复模式

用户将以语音形式听到你的回复。请为“听”而写，而不是为“读”而写：
- 使用自然、口语化的完整句子；不要使用项目符号列表、编号列表或标题。
- 禁止输出原始 URL。请用资源名称描述（例如用“Rust 官方文档网站”，而不是具体链接）。
- 不要使用任何 Markdown 格式：不要加粗/斜体/标题/代码块/行内反引号。
- 对可能被 TTS 误读的缩写进行拼读（例如把“API”写成“A-P-I”，“CLI”写成“C-L-I”）。
- 保持简洁：最多两到三段短段落。
- 使用自然的衔接与过渡，避免生硬的堆砌。

```

示例（空）：

```txt
""
```

示例（模板段落用法；放在模板末尾更合适）：

```md
{{voice_reply_suffix_md}}
```

### 禁止“未知值”策略（必须）

- **目标**：模板变量的最终渲染值不得出现占位符 `unknown` / `未知`（作为“缺省/没取到”的替代）。
- **约定**：对于可选信息，允许以空字符串 `""` 表示“省略/不展示该信息”，并且推荐用空字符串替代 `unknown/未知`。
- **允许的有意义降级**：例如 `host_os` 允许输出 `未知系统`（它是一个明确的自然语言语义，而不是占位符）。
- **建议执行点（双层）**：
  1) **启动期配置验证（config-derived）**：对来自 `moltis.toml` 的字段（sandbox mode/scope/data_mount/no_network 等）在启动前做 validate；发现无法确定/非法取值则警告或 fail-fast。
     - 证据：现有 validator 已覆盖 sandbox backend/scope/data_mount 语义校验：`crates/config/src/validate.rs:810`
     - 现有命令入口：`moltis config check`：`crates/cli/src/config_commands.rs:38`；`moltis doctor`：`crates/cli/src/doctor_commands.rs:188`
  2) **运行期变量导出校验（runtime-detected / request-derived）**：对只能运行时得到的值（例如 `session_id`、sudo 探测结果、是否沙盒执行等）在每次构造 prompt 之前做“导出校验”。
     - 若无法可靠确定：优先使用“保守但有意义”的自然语言降级（不输出 `unknown/未知`）。
     - 若该变量在当前运行模式下属于强依赖（例如 non-native 工具调用变量）：必须 fail-fast（拒绝该次 prompt 构建），避免生成不可用 prompt。

### 必须执行“运行期导出校验”的模板变量（详细枚举）

> 目标：在把变量值注入模板之前，确保：1) 不为空/不含“unknown/未知”；2) 与同一次运行的实际能力一致；3) 输出稳定（排序/格式）。

#### A) 运行环境变量（本节 v1 变量）

- 结论：这些变量在当前实现下都可以确定性产出；因此不需要单独的“运行期 fail-fast 校验”，只需要做 **归一化（normalization）** 与 **保守降级**（避免 `unknown/未知` 文案污染）。
  - `host_os`：`std::env::consts::OS` 可能不是 windows/linux/macos；若不在白名单则输出 `未知系统`（自然语言）。
  - `session_id`：来自 gateway 的 `session_key`，应当总是非空（无需额外校验）。
  - `reply_medium`：由 gateway 的 `infer_reply_medium` 得到最终值；必须稳定输出 `文字` 或 `语音`（不输出 `unknown/未知`）。
  - `exec_location`：由 router.is_sandboxed 判定，router 缺失时固定输出 `宿主机上`。
  - `sandbox_reuse_policy`：由 sandbox scope 映射；scope 缺失时输出 `不适用（未启用沙盒）`。
  - `system_data_dir_path`：沙盒固定 `/moltis/data`；宿主机必须输出真实 data_dir 绝对路径（不允许隐藏）。
  - `agent_data_dir_path`：宿主机必须输出真实 `<data_dir>/people/<name>` 绝对路径；沙盒内不可直接访问。
  - `data_dir_access`：由 data_mount（ro/rw/none）映射成自然语言。
  - `network_policy`：由 no_network 映射成自然语言。
  - `host_privilege_policy`：sudo 探测失败时必须降级为“任何宿主机安装/系统改动必须先征求用户同意”。

#### B) Tools 模板变量（见上一节 Skills/Tools 契约）

- `native_tools_index_md`：在 native 模板中必须可被渲染为“稳定段落”（工具为空则允许空字符串，但不得出现不稳定顺序/随机抖动）。
- `non_native_tools_catalog_md`：仅在 non-native tool-calling（本次运行）且 tools inventory 非空时必须非空；否则必须为空字符串。输出必须稳定排序、稳定格式；必须包含工具名与参数（目录+schemas）。
- `non_native_tools_calling_guide_md`：仅在 non-native tool-calling（本次运行）且 tools inventory 非空时必须非空；否则必须为空字符串。其 `tool_call` fence 格式必须稳定且可被 runner 解析。

#### C) Skills 模板变量

- `skills_md`：软依赖（soft-required）：强烈建议模板引用该变量（当本回合无 skills 时其值允许为空字符串 `""`），用于让模型“知道有哪些 skills 可启用”。若模板未引用不得阻断运行，但必须 warning（并在 debug 中标注缺失）。

#### D) Mode / Memory / Voice 段落变量（本节新增）

- `long_term_memory_md`：当 memory tools 不存在时必须为空字符串；存在时必须稳定输出且不得硬编码 “unknown/未知”。
- `voice_reply_suffix_md`：当 reply_medium 为文字时必须为空字符串；为语音时必须稳定输出且不得被重复注入。

### 明确不提供（v1 禁止作为模板变量）

- 高波动/隐私/实现细节字段：`channel_*` 标识符、`remote_ip`、`location`、sandbox `image`、backend `docker/cgroup`、data_mount_type/source、任何 debug/RPC 名称。
  - Evidence：这些字段确实存在于 runtime context 或请求元信息（例如 `remote_ip`：`crates/gateway/src/chat.rs:2285`），但不满足“不暴露实现细节/隐私”的目标。

### 必选组合（4 种情况逐一冻结，后续模板/实现必须遵守）

> 说明：这里的“必选”指“模板必须提供的变量组合”，以保证模型在该运行模式下能够可靠地发现并调用工具。  
> 内置工具与外部 MCP 工具在本层契约上不做区别：它们都以 `ToolRegistry::list_schemas()` 的结果呈现；区别仅在 inventory 内容本身。

0) **no-tools（tools_usable=false，包括 stream_only 或 tools inventory 为空）**
   - 必须为空：`native_tools_index_md`、`non_native_tools_catalog_md`、`non_native_tools_calling_guide_md`、`long_term_memory_md`
   - 建议引用：`skills_md`（其值允许为空字符串）
1) **native + 内置工具**
   - 可选：`native_tools_index_md`
   - 建议引用：`skills_md`
2) **native + 外部 MCP 工具**
   - 可选：`native_tools_index_md`
   - 建议引用：`skills_md`
3) **non-native + 内置工具**
   - 前提：tools_usable=true 且 supports_tools=false
   - 必选：`non_native_tools_catalog_md`
   - 必选：`non_native_tools_calling_guide_md`
   - 建议引用：`skills_md`
4) **non-native + 外部 MCP 工具**
   - 前提：tools_usable=true 且 supports_tools=false
   - 必选：`non_native_tools_catalog_md`
   - 必选：`non_native_tools_calling_guide_md`
   - 建议引用：`skills_md`

## 建议实施顺序（Sequencing）

1) P0：先把“prompt assembly 入口去重”与“as-sent 证据链”补齐（减少 drift，便于后续改动验证）。
2) P0：落地 PromptParts（canonical IR）+ renderer（Responses / 非 Responses / spawn_agent），把 provider 差异收敛到 renderer 层。
3) P0：落地 Type4 模板化拼接 v1（纯字符串替换 + 中文化 + 稳定性），并确保与 PromptParts/renderer 口径一致。
4) P1：再做 surface-aware guidelines（Web UI vs Telegram），避免“静默回复”等规则影响无 UI 面板的 surface。

## 修复可实施性评估（Feasibility Review）— 是否已具备充分实施条件

> 目的：回答“这个 issue 文档是否已经具备足够明确的实施条件”。  
> 结论：**具备（P0 可开工）**，但以下口径必须在实现前写死并落实到代码与测试中（否则会返工或出现隐性分叉）。

### 已具备的实施前置条件（Ready）

- **模板输入来源明确**：`people/<name>/{IDENTITY,SOUL,AGENTS,TOOLS}.md` 已有稳定 loader API。
  - Evidence：`crates/config/src/loader.rs:591`（identity_md_raw）、`crates/config/src/loader.rs:688`（soul）、`crates/config/src/loader.rs:715`（agents）、`crates/config/src/loader.rs:727`（tools）
- **运行时上下文可获取**：gateway 已构造 `PromptRuntimeContext` 并在多条 prompt 路径中传入。
  - Evidence：构造：`crates/gateway/src/chat.rs:846`；传入 builders：`crates/gateway/src/chat.rs:2407`、`crates/gateway/src/chat.rs:2478`
- **工具/技能 inventory 有统一来源**：tools 来自 `ToolRegistry::list_schemas()`，skills 来自 skills discovery；且已在本 issue 冻结“稳定排序/稳定格式”的规则。
  - Evidence（tools schema 来源）：`crates/agents/src/tool_registry.rs:92`
- **非 native 工具调用的解析契约稳定**：runner 只依赖 ` ```tool_call ` fence 与 JSON，不依赖标题文案。
  - Evidence：`crates/agents/src/runner.rs:242`

### 仍需在实施阶段补齐的关键点（Gaps / Must Decide）

- **`{{var}}` 模板替换引擎尚不存在**：当前 repo 只有 `${ENV_VAR}` 的 config env 替换；persona markdown 未做 `{{var}}` 替换。
  - Evidence：`crates/config/src/env_subst.rs:1`（仅 `${...}`）
- **`reply_medium` 目前不在 `PromptRuntimeContext`**：gateway 有 `ReplyMedium`，但未导出为 runtime/context 字段（因此模板变量 `reply_medium` 需要实现侧补齐映射）。
  - Evidence：ReplyMedium 推断：`crates/gateway/src/chat.rs:714`；PromptRuntimeContext 字段：`crates/agents/src/prompt.rs:321`
- **中文化输出需要落地到渲染层**：目前 `skills_md` 与 non-Responses 的 tools sections 仍是英文硬编码，需要按本 issue 的中文化策略改写。
  - Evidence：`crates/skills/src/prompt_gen.rs:11`、`crates/agents/src/prompt.rs:543`
- **voice × non-native tool_call 的冲突必须明确处理**：语音模式禁止 Markdown/代码块，但 non-native tool calling 必须使用 ` ```tool_call ... ``` ` 代码块。
  - v1 策略（已写入本 issue）：允许 tool_call fence 作为“工具调用回合”的专用格式；语音约束仅作用于最终面向用户的自然语言回复。
- **Requiredness Matrix 的“必须引用”必须可执行**：必须实现“模板占位符扫描 + requiredness 校验 + 遗留占位符检测”，否则治理规则无法 enforce。
  - Evidence：Template Validation 小节（本文前部）
- **sandbox `/moltis/data` 的可见性契约必须写死**：bind mount 经过 public view 过滤，但 volume mount 当前会直接挂载整个 volume（不经过过滤），会破坏 `agent_data_dir_path==""` 的隐私契约。
  - Evidence：bind mount public view：`crates/tools/src/sandbox.rs:973`；volume 直挂：`crates/tools/src/sandbox.rs:976`
  - v1 要求：若无法保证 volume 内容为 public view，则启动期必须 fail-fast 或把 volume mount 也改为 public view。
- **stream_only / no-tools run 的 tools vars 必须为空**：必须按本 issue 的 `tools_usable` 定义（而不是仅 provider capability）决定 tools 段落变量是否非空。
  - Evidence：gateway stream_only 判定：`crates/gateway/src/chat.rs:2026`
- **移除 v1 禁止字段的现状注入**：当前 runtime 注入仍可能包含 `remote_ip/location/sandbox image/backend` 等高波动/隐私字段；v1 落地必须删除或避免注入它们，保证“模板变量总表”是唯一出口。
- **消除 `/moltis/data` 硬编码引用**：现状（尤其 OpenAI Responses 的 system/persona 文案）可能硬编码 `/moltis/data/...`，在 host 模式下路径不成立；v1 必须迁移为 `{{system_data_dir_path}}`。

### 实施目标（Goals, v1）

1) **纯字符串替换**：对四个 persona 文件做 `{{var}}` → value 的按字节替换（无 if/loop/表达式/转义）。
2) **单一模板可用**：同一模板可同时引用 native 与 non-native 的 tools 段落变量；不适用变量渲染为 `""`。
3) **稳定性/缓存友好**：所有变量输出字节级稳定（排序 + 固定格式 + 固定换行）。
4) **中文化**：环境变量 + skills/tools 段落变量的标题/说明性硬编码尽量中文。

### 实施计划（Plan, v1）

#### Step 1：实现 `{{var}}` 纯替换引擎（库函数）

- 位置建议：新增 `crates/config/src/prompt_subst.rs`（或放在 `moltis_config` 中类似 `env_subst.rs` 的独立模块）。
- 行为规范：
  - 输入：模板字符串 + `HashMap<String,String>`（建议用 `BTreeMap` 作为导出 map，确保可预测遍历与可测试性）。
  - 占位符语法（写死）：仅支持**无空格**的 `{{var}}`，其中 `var` 必须匹配 `^[a-z0-9_]+$`（小写 + 数字 + 下划线）。
    - 任何不匹配该语法的 `{{ ... }}`（例如 `{{ var }}`）一律视为**字面量文本**，不得报错、不得参与替换（用于在模板中书写示例/解释）。
  - 字面量花括号转义（写死）：支持 `{{{{` → 字面量 `{{`，`}}}}` → 字面量 `}}`。
    - 实现建议：先把 `{{{{` / `}}}}` 替换为哨兵，再执行占位符替换，最后把哨兵还原为字面量花括号。
  - 替换算法（写死）：**单次扫描、非递归**。
    - 只替换模板文本中出现的占位符；插入的 value 不再被二次扫描（禁止递归替换）。
  - 缺失变量策略：模板中出现的占位符若未在 map 中提供，必须 fail-fast（避免默默漏替换）。
  - 遗留占位符策略：替换完成后若输出仍包含任何**符合占位符语法**的 `{{var}}`，必须 fail-fast（避免漏替换）。
  - 不支持嵌套/表达式/条件（仅支持上述 `{{{{` / `}}}}` 字面量花括号转义，不引入模板逻辑）。

#### Step 2：定义并导出 v1 template vars（运行时生成 map）

- 构造点：gateway 构造 prompt 之前（现已有 runtime_context/skills/tools）。
- 必须包含：本 issue 的 v1 总表中的变量（Runtime + Skills + Tools + Memory + Voice）。
- 关键实现：把 `ReplyMedium` 映射为 `reply_medium = "文字"|"语音"`（中文短语）。

#### Step 3：对四个 persona 文件应用替换（并保留 user-owned 文本所有权）

- 在 `load_prompt_persona_with_id`（gateway）或 loader 层对 `identity_md_raw/soul_text/agents_text/tools_text` 做替换。
- 替换必须对四个文件都生效（允许任意文件引用任意变量）。
- `IDENTITY.md` 特例（必须写死，避免破坏 frontmatter 解析）：
  - structured identity 使用 YAML frontmatter 解析（用于 UI/身份字段），不得被模板替换机制意外破坏。
  - v1 仅保证对“进入 prompt assembly 的 markdown 正文部分”（frontmatter strip 之后）执行替换；frontmatter 不参与替换。
- 输入归一化（与现状一致）：loader 读取 markdown 时会去除 leading HTML comments 并 `trim()`；模板作者不得依赖首尾空白表达语义。

#### Step 4：中文化 skills/tools 变量渲染（仅硬编码部分）

- `skills_md`：标题/说明文案中文化，保留 `<available_skills>` 与 skill description 原文。
- tools sections：标题/说明文案中文化（不影响 `tool_call` fence）。

#### Step 5：删除/避免重复注入（voice/memory）

- 当 `voice_reply_suffix_md`/`long_term_memory_md` 开始由模板引用时，必须避免 gateway/prompt builder 仍 append 同一段落导致重复。

### 验收标准（Acceptance Criteria, v1）

- [x] 四个 persona 文件均支持 `{{var}}` 替换，且缺变量时 fail-fast。
- [x] `reply_medium`/`long_term_memory_md`/`voice_reply_suffix_md` 能在 host/sandbox、text/voice、memory on/off 组合下正确渲染（见本 issue 成组示例）。
- [x] tools/skills 输出顺序稳定（工具按 name 排序，skills 按 name 排序）。
- [x] non-native tool_call fallback 仍能解析并执行（不因标题中文化受影响）。

## 最小验证矩阵（Validation Matrix）

| Item | 验证点 | 最小自动化 |
|---|---|---|
| openai-responses | `asSentPreamble` 单条 developer item（包含 System/Persona/Runtime 三段，顺序稳定） | unit（gateway debug endpoints） |
| non-responses native tools | system prompt 含 tool list（compact）且无 tool_call JSON 规则 | unit（prompt.rs） |
| non-responses non-native tools | system prompt 含 tool_call JSON 规则 | unit（prompt.rs） |
| anthropic | 多 system messages 拼接成 top-level `system` | integration（gateway debug `asSent`） |
| local-llm | system/user/assistant roles 进入 chat template | unit（chat_templates.rs） + integration（gateway debug `asSent`） |

---

# Issues（逐条）

## 1) [DONE] Canonical PromptParts + renderer（v1: 统一 system 文本）

### Metadata

- Priority: P0
- Owner: TBD
- Component: `crates/agents/src/prompt.rs`
- Affected paths/providers/models: openai-responses / anthropic / openai compat / local-llm
- Dependencies: 2,3

### 问题陈述（Problem）

- v1 目标：跨 provider 层产出**唯一的 canonical system prompt 文本**（1 条 `ChatMessage::System`），并在 provider 适配层做协议映射（Responses→developer / Anthropic→top-level system / local-llm→模板渲染），以避免内容布局治理被协议差异污染。
- Phase 1+（后续）：如需更细粒度的 PromptParts（多块/多 section 的 typed parts）与 renderer，可在 v2 继续推进（不阻断 v1 的模板化拼接与入口去重）。

### Evidence

- canonical v1 builder：`crates/agents/src/prompt.rs:385`
- gateway 全入口统一调用：`crates/gateway/src/chat.rs:2401`

### Doc

- 详见：`issues/issue-persona-prompt-configurable-assembly-and-builtin-separation.md`

### Tests

- canonical v1 unit：`crates/agents/src/prompt.rs:1718`
- debug as-sent 覆盖关键差异：`crates/gateway/src/chat.rs:8298`、`crates/gateway/src/chat.rs:8448`、`crates/gateway/src/chat.rs:8532`

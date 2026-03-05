# Issue: Prompt as-sent 证据链统一（debug + hooks）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-05
- Owners: <TBD>
- Components: gateway / agents/runner / providers
- Affected providers/models: all（openai-responses 已部分具备）

**已实现（如有，写日期）**
- (2026-03-04) openai-responses debug 输出 asSentPreamble（折叠为 1 条 developer item）：`crates/gateway/src/chat.rs:3651`
- openai-responses 的 system→developer 适配（as-sent 协议映射）：`crates/agents/src/providers/openai_responses.rs:37`
- (2026-03-04) provider-aware as-sent 摘要接口：`LlmProvider::debug_as_sent_summary(...)`（并由 wrapper 转发）：`crates/agents/src/model.rs:425`、`crates/agents/src/providers/mod.rs:335`
- (2026-03-04) Anthropic/local-llm as-sent 摘要实现：`crates/agents/src/providers/anthropic.rs:196`、`crates/agents/src/providers/local_llm/mod.rs:152`
- (2026-03-04) gateway debug endpoints 输出 `asSent`（provider-aware 摘要）：`crates/gateway/src/chat.rs:3679`、`crates/gateway/src/chat.rs:3825`、`crates/gateway/src/chat.rs:4008`
- (2026-03-04) hooks（BeforeLLMCall）附带 `asSentSummary`：`crates/common/src/hooks.rs:122`、`crates/agents/src/runner.rs:770`

**已覆盖测试（如有）**
- asSentPreamble 在 debug endpoints 的基础断言（长度=1）：`crates/gateway/src/chat.rs:8298`
- Anthropic/local-llm 的 debug asSent 摘要断言：`crates/gateway/src/chat.rs:8448`、`crates/gateway/src/chat.rs:8532`

**已知差异/后续优化（非阻塞）**
- 目前 `asSent`/`asSentSummary` 仅覆盖 openai-responses / anthropic / local-llm；其它 provider 仍返回 null（可按需逐步补齐实现）。
- 目前 `asSent` 属于“摘要”而非完整 request body dump；如需更强对齐可在 provider 内部暴露 request body renderer（需额外隐私/体积治理）。

---

## 背景（Background）
- 场景：用户排障时需要“最终发给 provider 的内容/结构（as-sent）”作为证据链；否则无法判断是 prompt、provider 适配还是 runner 解析的问题。
- 约束：
  - 不同 provider 协议差异很大：Responses input[]、OpenAI messages[]、Anthropic top-level system、local-llm chat templates、GenAI 不支持 tool messages。
  - 工具与隐私：debug/hook 输出需要脱敏并限制体积（避免泄露敏感 runtime 或巨大 schema）。
- Out of scope：本 issue 不决定 prompt 内容布局（由 PromptParts/renderer 负责）；这里只做“证据链结构与展示口径”。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **as-sent**（主称呼）：最终写入请求体、实际发送给上游 provider 的内容/结构。
  - Why：排障与治理必须以 as-sent 为准，而不是本地拼接的 estimate。
  - Source/Method：as-sent（method=provider_request）
- **estimate**（主称呼）：本地用于 token 估算/compaction gating 的拼接文本。
  - Why：estimate 不能当真值，必须在 UI/日志中明确标注 method。
  - Source/Method：estimate（method=join_text_heuristic）
- **debug endpoints**（主称呼）：`chat.raw_prompt` / `chat.context` / `chat.full_context` 这类用于展示 prompt/上下文的 RPC。
  - Source/Method：effective + as-sent（必须区分字段）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] debug endpoints 对关键 provider 输出“可复盘的 as-sent 摘要”（openai-responses/anthropic/local-llm；其余 provider 未实现时返回 null）。
- [x] hooks（runner events）在不泄露敏感信息的前提下，附带 as-sent 摘要（BeforeLLMCall: `asSentSummary`）。
- [x] 对 estimate 与 as-sent 的 method/source 标注清晰（避免混用导致误判）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：debug/hook 的 as-sent 与真实 request body 一致（或按同一 renderer 生成）。
  - 不得：把 provider 协议差异回渗到 prompt 内容布局治理（只做展示/映射）。
- 安全与隐私：
  - 必须：脱敏策略明确（remote_ip/location/api_key 等绝不输出）。
  - 必须：限制体积（避免在 debug/hook 输出完整巨型 schemas；用摘要/哈希/截断）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) openai-responses 已能看到 asSentPreamble，但其它 provider 仍主要展示 canonical/system_prompt，无法看到“真正发出去的结构”（例如 Anthropic top-level system）。
2) hooks 中缺少 as-sent 证据链，导致线上问题难以离线复盘。

### 影响（Impact）
- 排障成本：用户/开发无法判断是 builder、adapter 还是 runner 问题。
- 治理风险：同一内容在不同 provider 的协议映射差异可能长期漂移而不可见。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - openai-responses adapter：`crates/agents/src/providers/openai_responses.rs:37`
  - Anthropic 抽取 system：`crates/agents/src/providers/anthropic.rs:81`
  - local-llm chat template 合并 system：`crates/agents/src/providers/local_llm/models/chat_templates.rs:133`
- 当前测试覆盖：
  - 已有：openai-responses debug 的 asSentPreamble 断言：`crates/gateway/src/chat.rs:8711`
  - 缺口：未覆盖 Anthropic/local-llm 的 as-sent 摘要一致性。

## 根因分析（Root Cause）
- A. debug 输出侧更多在展示 canonical/system_prompt，而不是 provider-as-sent。
- B. provider adapters/runner 里生成 request body 的逻辑没有被复用到 debug/hook（或缺少可复用的 renderer）。
- C. 缺少统一的数据结构来承载“as-sent 摘要 + method/source”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - debug endpoints 输出字段必须明确区分：`effectivePrompt`（canonical/typed messages） vs `asSent`（provider request body 摘要） vs `estimate`（token 估算文本/方法）。
  - 对 Responses：`asSent` 必须能表达 `input[]` 的 item 类型（developer/user/assistant/function_call/...）与关键片段（截断）。
  - 对 Anthropic：`asSent` 必须能表达 top-level `system`（截断）+ messages[] 摘要。
  - 对 local-llm：`asSent` 必须能表达最终渲染模板文本（截断）或关键片段哈希。
- 不得：
  - 不得在 debug/hook 输出完整 tool schema JSON（除非明确 debug 开关且有体积上限）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（as-sent 摘要生成）：debug/hook 输出的是 **as-sent 摘要**（非完整 request body dump）。
  - 优先：由 provider adapter 直接返回摘要（避免 gateway 复制协议逻辑）。
  - 允许：为避免巨大 payload/隐私泄露（images/tool schemas），摘要可显式标注 `omitsImages/omitsToolSchemas`，并用 hash + 截断预览保证可对比/可复盘。
- 规则 2（体积控制）：所有文本字段必须截断（例如 4KB）+ 提供 hash（例如 SHA256）用于一致性对比。
- 规则 3（脱敏）：严格过滤 runtime 敏感字段与 secrets；必要时只输出布尔/枚举摘要（例如 `sandbox_no_network=true`）。

#### 接口与数据结构（Contracts）
- 在 gateway debug 输出中新增（或扩展）结构：
  - `asSent`: provider-aware object（结构随 provider 变化，但字段命名稳定、可版本化）
  - `estimate`: `{ method, joinedTextTruncated, hash }`
- 在 runner/hook event 中新增：
  - `asSentSummary`: `{ provider, model, preambleHash, toolsMode, truncatedPreview }`

#### 失败模式与降级（Failure modes & Degrade）
- 若 provider adapter 无法生成摘要（例如第三方 SDK 限制）：降级为 `asSent` 为空 + 输出 `effectivePrompt` 与 `estimate`，并在 debug 中标注 reason。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] debug endpoints（`chat.context`/`chat.raw_prompt`/`chat.full_context`）对 openai-responses / anthropic / local-llm 输出 as-sent 摘要（`asSent`）。
- [x] hooks/runner events 输出 as-sent 摘要（BeforeLLMCall: `asSentSummary`，包含 hash）。
- [x] 自动化测试覆盖至少 2 个 provider 的 as-sent 摘要存在性与关键字段（Anthropic/local-llm）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] provider adapter 摘要生成：对 key provider 的摘要生成逻辑单测/集成断言（目前以 gateway debug endpoint 断言为主）。

### Integration
- [x] gateway debug endpoint 返回中包含 `asSent` 且结构符合 spec（字段存在、截断生效）。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：部分 provider 需要真实 SDK 行为才能完全复现 as-sent。
- 手工验证步骤：
  1. 分别选择 openai-responses / anthropic / local-llm provider 运行一次对话。
  2. 打开 `chat.raw_prompt`，确认 `asSent` 存在且与预期协议一致（developer/system 映射正确）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先 debug-only（不影响真实请求）；hook 输出先加 hash/摘要（低风险）。
- 回滚策略：新增字段可保持向后兼容；出现泄露风险可快速关闭输出（feature flag）。

## 实施拆分（Implementation Outline）
- Step 1: 定义统一的 `as_sent_summary` 数据结构（放在 gateway 或 agents/model）。
- Step 2: openai-responses/anthropic/local-llm 实现摘要生成（复用 adapter/renderer）。
- Step 3: gateway debug endpoints 输出 `asSent`（并标注 method/source）。
- Step 4: runner/hook event 输出 `asSentSummary`。
- Step 5: 添加稳定性测试（hash/golden）。
- 受影响文件：
  - `crates/gateway/src/chat.rs`
  - `crates/agents/src/providers/*`
  - `crates/agents/src/runner.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_type4_system_prompt_assembly_v1.md`
  - `issues/issue-prompt-assembly-entrypoint-dedup.md`

## 未决问题（Open Questions）
- Q1: as-sent 摘要由 gateway 生成还是由各 provider adapter 生成（避免重复）？
- Q2: 截断/哈希的上限与算法是否需要配置化（默认固定即可）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

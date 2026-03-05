# Issue: Provider 角色模型统一（system/developer 等语义归一）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P2
- Updated: 2026-03-04
- Owners: <TBD>
- Components: agents/model / providers / prompt renderer
- Affected providers/models: openai-responses / anthropic / local-llm / genai

**已实现（如有，写日期）**
- 内部 typed messages 使用 `ChatMessage::{System,User,Assistant,Tool}`：`crates/agents/src/model.rs:15`
- openai-responses 将 System 映射为 developer role：`crates/agents/src/providers/openai_responses.rs:37`
- Anthropic 把 system 抽取为 top-level `system`：`crates/agents/src/providers/anthropic.rs:81`

**已覆盖测试（如有）**
- 无（survey）

**已知差异/后续优化（非阻塞）**
- 同一内部 `System` 在不同 provider 下的 as-sent 位置/角色不同，会给 debug/trace/治理带来认知成本。

---

## 背景（Background）
- 场景：项目内部以 typed messages 表达语义，但不同 provider 对 “system/developer” 的协议字段不同，且部分 provider 还会把 system 合并进模板字符串。
- 约束：
  - openai-responses：developer role 是主要系统指令入口。
  - anthropic：top-level `system` 承载系统指令，messages[] 不含 system。
  - local-llm：system 可能被合并进首个 user turn（取决于 chat template）。
- Out of scope：本 issue 为 survey，不在当前迭代强制落地；落地应与 PromptParts/renderer 一并完成。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **canonical role**（主称呼）：项目内部统一语义角色（System/User/Assistant/Tool）。
  - Source/Method：effective
- **provider role**（主称呼）：上游协议字段（developer/system/user/assistant/...）或模板位置。
  - Source/Method：as-sent

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 定义一套“role normalization spec”：从 canonical role 到各 provider 的映射规则与不变量（例如保序、合并规则、丢弃规则）。
- [ ] 在 debug/hook 输出中以统一口径解释映射结果（降低认知成本）。

### 非功能目标（Non-functional）
- 正确性口径：
  - 必须：内容治理（PromptParts）不得依赖 provider role；provider role 只在 renderer/adapter 层决定。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一 `ChatMessage::System` 在不同 provider 下会变成 developer/top-level system/模板拼接，导致证据链难以直观对齐。

### 影响（Impact）
- 排障成本：用户看 debug 时需要理解每个 provider 的协议差异。
- 治理复杂度：容易出现“内容布局治理”与“协议映射”混在一起的实现。

## 现状核查与证据（As-is / Evidence）【不可省略】
- `crates/agents/src/providers/openai_responses.rs:37`：System → developer
- `crates/agents/src/providers/anthropic.rs:81`：system 抽取为 top-level
- `crates/agents/src/providers/local_llm/models/chat_templates.rs:133`：system 合并进模板

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - canonical role 语义不变；任何 provider 特有的 role/字段只在 renderer/adapter 层体现。
  - debug/hook 必须能同时呈现 canonical（effective）与 provider（as-sent）的对应关系（至少摘要）。

## 方案（Proposed Solution）
- 在 PromptParts/renderer 引入“role mapping table”（文档化 + 测试化）。
- 对每个 provider 给出：
  - system/developer 的合并规则（单条 vs 多条）
  - tool messages 是否允许、如何降级
  - multimodal 支持与降级

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 形成冻结的 role normalization spec 文档（本文 + overall 引用）。
- [ ] 后续落地时：新增/修改 provider adapter 必须更新 mapping table 与测试。

## 测试计划（Test Plan）【不可省略】
### 自动化缺口（如有，必须写手工验收）
- 缺口原因：survey 期不落地代码。
- 手工验证步骤（落地后）：
  1. 对同一对话输入分别用 openai-responses 与 anthropic 运行。
  2. 对比 debug `asSent` 与 canonical messages 的映射解释是否一致且可理解。

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_type4_system_prompt_assembly_v1.md`

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

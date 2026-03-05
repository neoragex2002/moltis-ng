# Issue: Surface-aware Guidelines（Web UI vs Telegram 等运行 surface）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Updated: 2026-03-04
- Owners: <TBD>
- Components: agents/prompt / gateway / channels
- Affected providers/models: all（但主要影响“工具面板/静默回复”等行为假设）

**已实现（如有，写日期）**
- 部分 prompt 文案假设“UI 有工具面板、可静默回复”：`crates/agents/src/prompt.rs:30`

**已覆盖测试（如有）**
- 无（survey）

**已知差异/后续优化（非阻塞）**
- Telegram 等 channel surface 不具备“工具面板”与“空回复可见性”，需要差异化指引。

---

## 背景（Background）
- 场景：同一个 agent 既可能跑在 Web UI（有工具结果面板、可以空回复），也可能跑在 Telegram/其他 channel（无工具面板、空回复体验差）。
- 约束：
  - prompt 中的“静默回复/不复述 stdout”在 Web UI 是优化，但在 channel surface 可能导致用户误以为机器人没回应。
  - tools 的可用性与执行路由在 channel surface 也可能不同（例如 sandbox/network policy）。
- Out of scope：本 issue 不落地实现（survey）；落地需要在 PromptParts/renderer 完成后做 surface-aware 渲染。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **surface**（主称呼）：用户接触 agent 的呈现与交互载体（web-ui / telegram / api caller）。
  - Source/Method：effective（由 runtime_context.host.channel 等字段决定）
- **guidelines**（主称呼）：prompt 中的行为指引（工具使用、静默回复、输出密度等）。
  - Source/Method：authoritative（由系统固定块提供，可随 surface 选择不同版本）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 明确哪些 guidelines 是 “Web UI 专属假设”，哪些是 “channel 通用”。
- [ ] 给出最小的 surface-aware 渲染规则（to-be），供后续实现落地。

### 非功能目标（Non-functional）
- 正确性口径：
  - 必须：channel surface 下不得鼓励“静默回复导致用户无可见回应”的行为。
  - 应当：在 channel surface 下用更明确的“工具执行中/完成”短句替代静默。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Web UI 假设（工具面板/空回复）被写入通用 prompt，导致 channel 体验退化。

### 影响（Impact）
- 用户体验：Telegram 里工具回合可能“无回复”，用户困惑或重复提问。

## 现状核查与证据（As-is / Evidence）【不可省略】
- `crates/agents/src/prompt.rs:30`：包含“用户 UI 已展示工具执行结果…静默回复”等假设。
- `crates/gateway/src/chat.rs:2274`：runtime_context 捕获 channel 相关字段（可用于 surface 判断）。

## 根因分析（Root Cause）
- A. guidelines 固定块未按 surface 分层。
- B. 缺少 canonical PromptParts/renderer 的“按 surface 渲染”能力。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - Web UI surface：保留“不要复述输出/允许空回复”的优化指引。
  - Channel surface（如 telegram）：禁用空回复策略；工具回合应输出最小可见提示（例如 “已执行/正在执行”）。
- 应当：
  - surface 判断来源固定：`runtime_context.host.channel`（或等价字段），不得由模型猜测。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
- 在 PromptParts 中引入 `surface` 枚举（web_ui/telegram/api），由 gateway/runtime_context 决定。
- guidelines 固定块拆为：
  - `guidelines_web_ui_md`
  - `guidelines_channel_md`
  - `silent_replies_policy_*`（如需更细分）
- renderer 按 surface 注入对应块。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 形成一份冻结的 surface-aware guidelines 规范（本文档完成 + 在 overall 索引中引用）。
- [ ] 后续落地时：telegram channel 下不再出现“工具回合空回复”的策略。

## 测试计划（Test Plan）【不可省略】
### 自动化缺口（如有，必须写手工验收）
- 缺口原因：survey 期不落地代码。
- 手工验证步骤（落地后）：
  1. 绑定 telegram channel，触发一次工具调用。
  2. 确认用户能看到明确的可见提示，而不是空回复。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先只影响 channel surface；Web UI 保持现状。
- 回滚策略：按 surface 开关可快速禁用。

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_type4_system_prompt_assembly_v1.md`

## 未决问题（Open Questions）
- Q1: channel surface 的“工具回合可见提示”由 runner 统一注入，还是由 prompt 指引模型生成？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

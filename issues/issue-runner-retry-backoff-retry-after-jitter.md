# Issue: Agents Runner 重试/退避收敛（429 / jitter / Retry-After）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P3
- Owners: <TBD>
- Components: agents/runner / providers / gateway error surfacing
- Affected providers/models: all（尤其 streaming / tool-loop 场景）

**已实现（如有，写日期）**
- 暂无（当前仅有基础重试/退避逻辑，需按本单收敛增强）

**已覆盖测试（如有）**
- 暂无（建议新增 deterministic 单测覆盖退避与 Retry-After 解析）

**已知差异/后续优化（非阻塞）**
- 本单默认不引入“自动 fallback 模型/自动换 provider”策略；仅收敛重试/退避与可观测性。

---

## 背景（Background）
- 场景：LLM provider 在高峰/配额/网络波动时会返回 429/5xx 或出现临时性连接错误；当前 runner 的重试/退避策略不够明确与一致，导致：
  - 过快重试（放大拥塞/触发更严 rate limit）
  - 过慢/不重试（降低可用性）
  - 不尊重 `Retry-After`（与上游契约不一致）
- 约束：
  - 需要保持“默认行为可预测且不刷屏”，避免对 Telegram/Web 渠道造成重复回执或噪声。
  - 需要可测试（deterministic），不能依赖真实睡眠/真实网络。
- Out of scope：
  - 自动重试导致的“用户可见行为变更”（例如自动重新发起工具调用/自动再次执行副作用工具）——本单不做。
  - 自动 fallback（换模型/换 provider）——另议题。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **retryable**：短期可重试的失败（网络抖动、429、部分 5xx）；重试不应改变业务语义。
- **backoff**：重试前等待时间（应支持 jitter，避免羊群效应）。
- **jitter**：对 backoff 加随机扰动（建议 full jitter 或 equal jitter）；必须可注入 RNG 以便单测。
- **Retry-After**（authoritative）：若上游返回该 header，应优先作为 backoff 的上限/基准（需明确单位秒/HTTP-date）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 429（RateLimit）时：支持尊重 `Retry-After`，并使用带 jitter 的指数退避。
- [ ] 5xx / 网络错误时：使用带 jitter 的指数退避（与 429 可不同初始值/上限）。
- [ ] 明确最大重试次数与总耗时上限（避免无限拖死 run）。
- [ ] 对 streaming（包括 tool loop 多迭代）重试策略口径一致且可解释。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：在日志/Debug 中可看到每次重试的原因、attempt、最终 backoff（脱敏）。
  - 不得：对有副作用的工具调用“自动重放”导致重复执行（runner 必须区分：重试 LLM 请求 vs 重试工具执行）。
- 兼容性：
  - 默认配置下行为与当前尽量接近，只在“明显应当重试”的错误上改善稳定性。
- 可观测性：
- 关键字段（主称呼；口径见 `docs/src/concepts-and-ids.md`）：`runId/sessionId/provider/model/attempt/maxAttempts/backoffMs/retryAfter/errKind`。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同样的 429/临时错误在不同 provider/模式下重试行为不一致。
2) 不尊重 `Retry-After` 时，可能被上游持续拒绝，导致“看起来一直失败”。

### 影响（Impact）
- 用户体验：偶发失败更频繁暴露给用户；或者单次 run 无意义地拖很久。
- 成本：无效重试增加请求成本与日志噪声。
- 排障成本：看不清“到底重试了几次/等了多久/为什么最终失败”。

### 复现步骤（Reproduction）
1. 构造 provider 返回 429（带/不带 `Retry-After`）或模拟网络超时。
2. 观察 runner 行为：是否按预期 backoff、是否有 jitter、最终是否在耗时上限内停止。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 历史上下文：`issues/overall_issues_1.md` 的 P3 TODO（已拆分至本单）。
- 代码入口（待进一步定位并补齐本节）：`crates/agents/src/runner.rs:45`。

## 根因分析（Root Cause）
- A. 重试策略在不同路径/错误类型上缺少统一规范（429 vs 5xx vs 网络错误）。
- B. 缺少 `Retry-After` 的 authoritative 解析与优先级规则。
- C. jitter / 上限 / attempt 记录缺少可测试的实现（容易在 refactor 中漂移）。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 429：若 `Retry-After` 存在且可解析，backoff 必须至少为该值（并可叠加 jitter/上限规则需冻结）。
  - 网络/5xx：指数退避 + jitter，且有最大等待上限。
  - 重试次数与总耗时上限可配置（或至少常量明确），默认值保守。
  - 所有重试仅限“LLM 请求阶段”；不得自动重放已执行的 tool side effects。
- 不得：
  - 不得出现无限重试/无限等待。
  - 不得在渠道层重复发送多条“错误回执”（失败出口应去重；见 cross-ref）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：统一重试分类（至少：RateLimit/Network/ProviderUnavailable/Transient5xx）。
- 规则 2：指数退避基线 + jitter（实现需可注入 RNG 供单测）。
- 规则 3：优先解析并尊重 `Retry-After`（秒/HTTP-date；解析失败则回退默认 backoff）。
- 规则 4：attempt 与 backoff 记录为结构化字段（日志/Debug 可见，不泄露敏感信息）。

#### 失败模式与降级（Failure modes & Degrade）
- `Retry-After` 无法解析：记录 debug 字段（例如 `retryAfterParseFailed=true`），回退到默认 backoff。
- 达到上限：以统一错误语义失败（由 single egress 负责渠道回执与去重）。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 429（带 `Retry-After: 3`）时：attempt=1 的 backoff >= 3s，且有 jitter（但单测可固定 RNG）。
- [ ] 429（无 Retry-After）时：使用默认 backoff（可配置）+ jitter。
- [ ] 网络超时/5xx：指数退避递增且有上限；达到 `maxAttempts` 或总耗时上限后停止。
- [ ] 日志/Debug 可见 attempt/backoff/reason 字段，且不重复发送多条渠道错误回执（依赖 single egress）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `retry_after_seconds_is_respected_for_429`：`crates/agents/src/runner.rs`
- [ ] `retry_after_http_date_is_respected_for_429`：`crates/agents/src/runner.rs`
- [ ] `full_jitter_is_deterministic_with_seeded_rng`：`crates/agents/src/runner.rs`
- [ ] `stops_after_max_attempts_or_deadline`：`crates/agents/src/runner.rs`

### 自动化缺口（如有，必须写手工验收）
- 若 runner 目前不可注入 clock/RNG：���落地注入点，再补齐上述单测。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用（保守默认值）；必要时提供 config 开关以降级到旧策略。
- 回滚策略：保留旧实现路径（或通过 feature/config 退回）。

## 实施拆分（Implementation Outline）
- Step 1: 在 runner 中抽出 `BackoffPolicy`（可注入 RNG/clock），并引入 `RetryAfter` 解析。
- Step 2: 为 429/5xx/network 分类应用策略；冻结默认参数（initial/max/maxAttempts/deadline）。
- Step 3: 增补结构化日志字段与单测。

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_issues_1.md`（P3 原始 TODO）
  - `issues/done/issue-error-handling-taxonomy-single-egress.md`（失败出口去重/渠道回执一致性；避免重试时多次回执）

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

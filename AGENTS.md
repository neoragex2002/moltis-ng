CLAUDE.md

Issue docs must follow the templates in `issues/template/`:
- Single issue: `issues/template/TEMPLATE-issue-single.md`
- Multi-issue (overall/audit): `issues/template/TEMPLATE-overall-multi.md`
- Incremental update guide: `issues/template/TEMPLATE-update-guide.md`

Naming conventions:
- Internal code identifiers use `snake_case` (e.g. Rust functions/vars/struct fields).
- External JSON/RPC fields use `camelCase` (UI/API contracts).
- Prefer explicit mapping (e.g. serde rename / `json!` keys) instead of mixing conventions.

---

## Agent 文件与 Git 安全规则（强制）

必须严格遵守：`docs/agent-file-and-git-safety-rules.md`

简要落地要求（不替代原文，仅用于执行时自检）：
- 默认只做最小增量修改（minimal diff），严格限定在用户意图范围内。
- 未经用户明确确认，禁止任何可能导致内容丢失/不可恢复的操作（删除/清空/整文件覆盖式重写、`git reset --hard`、`git checkout --`、`git clean` 等），尤其是针对untracked文件的类似高危操作。
- 对 `issues/*`、`docs/*` 等关键决策载体，优先增量 Update，避免 Delete + Add 覆盖式改写；非必要或未经用户明确指示，不得整文件重写。
- 不确定是否会造成丢失或影响范围明显扩大时，必须先询问用户。尽量事先评估、事先询问用户意见。

---

## 可观测性与测试（强制）

### 可观测性（Observability）
- 任何“策略/护栏/开关/限制”一旦会**无声地**改变用户体验（例如：不触发推理、不派发 relay、不回复、不写入 session、不执行某分支），必须补齐可观测性：
  - 必须有结构化日志（带 `reason code`），能让排障不依赖猜测/倒推。
  - 日志级别要可控、避免过度噪声：优先仅在“命中候选但被策略拦截/降级”时记录；必要时加简单去重/限频。
  - 日志不得打印敏感字段（token、完整正文等）；正文如需辅助排障只能做短预览/哈希。

### 测试（Tests）
- 关键 issue（无论 feat 还是 fix），原则上必须配套测试用例（优先 Unit / Integration）。
- 若确实无法自动化覆盖，必须在 issue 中明确记录：
  - 为什么无法自动化（缺口原因）
  - 手工验收步骤与验收口径（Acceptance）

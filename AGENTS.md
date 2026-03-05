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

CLAUDE.md

Issue docs must follow the templates in `issues/template/`:
- Single issue: `issues/template/TEMPLATE-issue-single.md`
- Multi-issue (overall/audit): `issues/template/TEMPLATE-overall-multi.md`
- Incremental update guide: `issues/template/TEMPLATE-update-guide.md`

Naming conventions:
- Internal code identifiers use `snake_case` (e.g. Rust functions/vars/struct fields).
- External JSON/RPC fields use `camelCase` (UI/API contracts).
- Prefer explicit mapping (e.g. serde rename / `json!` keys) instead of mixing conventions.

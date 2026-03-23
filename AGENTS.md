# AGENTS.md

Project-level agent execution rules for this repository. This file complements
`CLAUDE.md` (engineering guide). When you need to write issue docs or touch
key decision artifacts, follow the safety rules strictly.

## References (must-follow)
- `CLAUDE.md` — engineering conventions and Rust guidance.
- `docs/agent-file-and-git-safety-rules.md` — agent file & git safety rules.

## Issue docs (must-follow)
Issue docs must follow the templates in `issues/template/`:
- Single issue: `issues/template/TEMPLATE-issue-single.md`
- Multi-issue (overall/audit): `issues/template/TEMPLATE-overall-multi.md`
- Incremental update guide: `issues/template/TEMPLATE-update-guide.md`
- Requirements-only: `issues/template/TEMPLATE-requirements.md`

## Naming conventions (must-follow)
Naming conventions:
- Internal code identifiers use `snake_case` (e.g. Rust functions/vars/struct fields).
- External JSON/RPC fields use `camelCase` (UI/API contracts).
- Prefer explicit mapping (e.g. serde rename / `json!` keys) instead of mixing conventions.

---

## 硬切换重构（强制）
- 凡是用户**明确要求**的硬切换/one-cut/严格重构操作，实施时不得保留 fallback、alias、compat shim、silent degrade 等尾巴。
- 这类任务默认**不考虑后向兼容**、**不做自动数据迁徙**、**不做自动 schema rename / 自动字段映射 / 自动目录回退读取**。
- 命中 legacy 输入/配置/持久化形状时，必须按严格标准处理：直接报错或强告警，并给出明确 remediation；不得“先兼容跑起来再说”。
- 不得为了“识别 legacy 并专门拒绝它”而额外引入新的特例探测、legacy guard、预判分支或兜底机制；优先删除 legacy 路径/默认语义，让系统在新口径下按正常主路径自然失败。
- 若你判断保留兼容尾巴是唯一合理方案，必须先停下并征求用户明确确认；未经确认不得自行加入兼容路径。

### 重构收敛与高内聚（强制）
- 对用户明确要求的重构/收口类任务，必须先做**范围收敛**与**职责切分**审查；在 issue/计划未证明“边界清楚、最小闭环明确、测试面收敛”之前，不得直接铺开实现。
- 实施方案必须优先选择**高内聚、单点收口、最小闭环**的做法：能在现有边界内闭合的，就不得把复杂度扩散到更多模块、层次或抽象。
- 非绝对必要，不得新增跨渠道/跨模块的通用抽象、公共概念、总线、框架层或“为未来预留”的接口；尤其不得借修一个渠道/一个 issue 之机顺手做泛化设计。
- 代码改动必须避免离散、散乱：同一类规则应集中在单一职责位置，不得在多个文件/多层路径里各写半套逻辑。
- 若某项“顺手优化/顺手统一/顺手抽象”不是当前 issue 达成闭环的必要条件，默认不做；确有必要扩大范围时，必须先在 issue 中补齐理由、边界与受影响面，再实施。
- 评审与复审时，必须主动检查：
  - 是否引入了非必要概念膨胀
  - 是否把渠道专属复杂性错误外溢到 core/gateway/common 层
  - 是否存在同一规则分散在两处以上实现
  - 是否可以通过删除冗余路径/复用既有结构进一步收敛
- 文档、issue、测试也必须遵守同样的收敛原则：只覆盖当前闭环所需内容，不扩写无关设计，不堆无关 case。

---

## Agent 文件与 Git 安全规则（强制）

必须严格遵守：`docs/agent-file-and-git-safety-rules.md`

简要落地要求（不替代原文，仅用于执行时自检）：
- 默认只做最小增量修改（minimal diff），严格限定在用户意图范围内。
- 未经用户明确确认，禁止任何可能导致内容丢失/不可恢复的操作（删除/清空/整文件覆盖式重写、`git reset --hard`、`git checkout --`、`git clean` 等），尤其是针对 `untracked` 文件的类似高危操作。
- 对 `issues/*`、`docs/*` 等关键决策载体，优先增量 Update，避免 Delete + Add 覆盖式改写；非必要或未经用户明确指示，不得整文件重写。
- 不确定是否会造成丢失或影响范围明显扩大时，必须先询问用户。尽量事先评估、事先询问用户意见。

---

## 可观测性与测试（强制）

### 可观测性（Observability）
- 任何“策略/护栏/开关/限制”一旦会**无声地**改变用户体验（例如：不触发推理、不派发 relay、不回复、不写入 session、不执行某分支），必须补齐可观测性：
  - 必须有结构化日志（带 `reason_code`），能让排障不依赖猜测/倒推。
  - 结构化日志至少应包含：`event`、`reason_code`、`decision`、`policy`；上下文允许时再补 `session_key`、`channel_type`、`tool_name`、`remediation`，避免后续各写各的。
  - 日志级别要可控、避免过度噪声：优先仅在“命中候选但被策略拦截/降级”时记录；必要时加简单去重/限频。
  - 日志不得打印敏感字段（token、完整正文等）；正文如需辅助排障只能做短预览/哈希。
  - 凡是命中 strict one-cut / 硬切换规则而在**现有主路径**上被直接拒绝的 legacy 输入、配置或持久化形状，必须留下结构化拒绝日志；禁止为了留痕而新增 legacy 专用识别分支。

### 测试（Tests）
- 关键 issue（无论 feat 还是 fix），原则上必须配套测试用例（优先 Unit / Integration）。
- 测试必须**精简且完备**：聚焦覆盖关键 feat/fix 主路径、关键边界与关键失败面；不要堆大量重复用例去证明 legacy 已不再支持、fallback/alias 已被移除这类已冻结约束。
- 对 strict reject / policy block 类改动，只保留少量能证明“拒绝生效 + `reason_code`/强告警可观测 + 无 silent degrade”的关键用例。
- 若确实无法自动化覆盖，必须在 issue 中明确记录：
  - 为什么无法自动化（缺口原因）
  - 手工验收步骤与验收口径（Acceptance）

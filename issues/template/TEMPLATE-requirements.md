# Issue 文档模板需求（单 Issue / 多 Issue）

本文件记录本仓库对 issue 文档模板的统一要求：后续 issue 的创建、实现、更新、关闭均应以 `issues/template/` 下模板为准，并保持“按代码实施状态可增量更新”。

## 一、两类模板共同要求（Global）

### 1) 增量更新友好（Incremental-friendly）
- 文档结构必须支持“每次只改少量区块”即可反映最新实施状态。
- 章节顺序稳定，尽量追加式更新，避免频繁重排导致 diff 噪音。

### 2) 证据驱动（Evidence-driven）
- 关键结论必须有可定位证据：
  - `path/to/file:line`
  - 测试名 + `path/to/file:line`
  - 或明确的复现步骤/日志关键词（当自动化不可行时）
- 禁止“无证据 DONE”。

### 3) 口径清晰、避免歧义（Semantics clarity）
- 必须显式标注来源/口径：
  - authoritative（权威值：provider 返回 usage / 回包）
  - estimate（估算值：启发式/推导，必须标注 method）
  - configured / effective / as-sent（配置值/生效值/实际发送值）
- 对 token/usage/estimate/`sessionId`/`chanChatKey`/prompt-cache bucket key/tool/tool_result 等关键概念，必须有统一口径（权威口径优先引用 `docs/src/concepts-and-ids.md`）。

### 4) 概念收敛、清晰直观（Concept convergence & intuitive）
- 同一概念在正文只允许一个“主称呼”；别名只可在术语表记录，不可在正文混用。
- 每个主概念至少包含：What / Why / Not / Source(or Method)。
- 呈现优先级：常量在前、变量在后；重要在前、不重要在后。

### 5) 闭环交付（Close the loop）
- 必须覆盖：背景/问题/现状/根因/方案/验收/测试/风险/回滚/交叉引用/当前进展。
- 能回答：做了什么、还差什么、怎么验收、怎么回滚、依赖什么。

### 6) 防漂移与断链（Anti-drift & hygiene）
- 规则/Spec/口径只允许一个权威位置（Glossary & Semantics / Spec），其他位置引用它，避免重复叙述漂移。
- 删除/移动文档前：先迁移关键规则 → 再移除引用 → 最后删除文件。

## 二、单 Issue 模板要求（Single）

### 1) 顶部 `实施现状（Status）` 为增量更新主入口
- 日常更新应优先只改该块：实现点、测试点、已知差异。

### 2) Spec 尽量冻结
- Spec 用“必须/不得/应当”写清；后续更新优先改实现与测试，不频繁改 Spec。

### 3) 验收与测试 checklist 化
- Acceptance Criteria / Test Plan 必须可勾选。
- 若存在自动化缺口：必须写明原因 + 手工验收步骤 + 后续补测计划。

## 三、多 Issue（Overall/Audit）模板要求（Multi）

### 1) Issue Index（总控表）强制维护
- 每次增量更新优先改 Index：Status / Evidence / Tests / Doc / Depends。
- P0/P1 的 TODO 必须有独立 issue 文档链接（Doc 列），避免长文埋雷。

### 2) 每个 issue 小节的不可省略字段
- As-is/Evidence、Goals/Non-goals、Spec、Acceptance、Test Plan、Close Checklist 必须出现。
- 允许“暂缺”，但必须显式标注缺口与补齐路径。

### 3) Close Checklist 防漏项
- 行为按 Spec、测试补齐/缺口说明、文档同步、兼容性/迁移、安全隐私、回滚、Index 同步。

## 四、模板清单
- 单 Issue：`issues/template/TEMPLATE-issue-single.md`
- 多 Issue：`issues/template/TEMPLATE-overall-multi.md`
- 增量更新指南：`issues/template/TEMPLATE-update-guide.md`

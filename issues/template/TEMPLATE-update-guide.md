# Issue 文档增量更新指南（Single / Overall）

> 目标：每次代码落地后，**最小改动**把文档同步到“可审阅/可关单”的状态，避免漂移与断链。

## 0) 通用规则（两类文档都适用）

### 0.1 更新顺序（推荐）
1) 先更新 **证据**（`path/to/file:line`、测试名、日志关键词）
2) 再更新 **状态**（TODO → IN-PROGRESS → DONE）
   - 同步更新文档顶部的 `Updated: YYYY-MM-DD`（或 Status 区块的 `Updated:` 字段）
3) 最后更新 **交叉引用**（Doc/Index/Related issues）

### 0.2 术语/口径变更
- 如果出现新概念或需要改口径：**只改 Glossary & Semantics**（权威位置），正文只引用，不重复定义。
- 必须标注 source/method：
  - authoritative / estimate
  - configured / effective / as-sent

### 0.3 DONE 的最小条件（建议执行）
- 有至少 1 条实现证据：`file:line`
- 有至少 1 条测试证据（或明确的“自动化缺口 + 手工验收步骤”）
- Cross references 不断链（删文档前先迁移要点并移除引用）

---

## 1) 单 Issue（`issues/TEMPLATE-issue-single.md`）怎么增量更新

### 1.1 日常更新只改这块：`实施现状（Status）`
当你完成一个实现点：
- 在“已实现”里追加一条：`path/to/file:line`
当你补了测试：
- 在“已覆盖测试”里追加：`path/to/test:line`
当你发现非阻塞缺口：
- 在“已知差异/后续优化”里追加（并说明不阻塞原因）

### 1.2 什么时候需要改 Spec
- 只有当“原 Spec 被证明不对/不可行”或“需求变更”才改。
- 改 Spec 必须同时更新：
  - Acceptance Criteria
  - Test Plan（新增/删减）

### 1.3 关单前的最小动作
1) 勾完 `Close Checklist`
2) 在 Evidence 里补齐关键 `file:line`
3) 如果缺自动化：写“手工验收步骤 + 原因 + 后续补测计划”

---

## 2) 多 Issue（`issues/TEMPLATE-overall-multi.md`）怎么增量更新

### 2.1 每次改动必须先改 `Issue Index`
你每完成一个 issue 的一个阶段，都应该在 Index 表里同步：
- Status（TODO/IN-PROGRESS/DONE）
- Evidence（新增 `file:line` 或日志关键词）
- Tests（新增 test refs，或把“缺口”写清楚）
- Doc（如果拆分出独立 issue，填 `issues/issue-xxx.md`）

> 经验：Index 是“防遗留总控面板”；任何空白列（Evidence/Tests/Doc）都会暴露问题。

### 2.2 Issue 小节推荐写法（避免漂移）
- Executive Summary 只写摘要，不写细节（细节放 issue 小节或独立 issue 文档）。
- Issue 小节更新优先改：
  - Evidence
  - Progress
  - Close Checklist

### 2.3 何时拆分独立 issue 文档
满足任一条件就建议拆分：
- P0/P1
- 涉及复杂 Spec（字段口径/计算/失败模式）
- 涉及跨模块（gateway + agents + UI）
- 需要多人并行（Owner/Depends 明显）

### 2.4 关单同步动作
当某个 issue 标 DONE：
1) Index：Status→DONE，补齐 Evidence/Tests/Doc
2) Issue 小节：勾完 Close Checklist
3) Validation Matrix：必要时补一条回归项

---

## 3) 常见坑（强提醒）
- 只改了代码没补 `file:line`：审阅很难验证，后续会漂移。
- 把 estimate 当 authoritative：必须显式标注 method/source。
- 在多处重复写规则：会漂移；规则只放 Spec/Glossary 处。
- 删除/移动文档没清引用：先迁移要点、再删引用、最后删除文件。

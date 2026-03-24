# Overall Issues (<v?>) — <主题>（Audit + Roadmap）

Updated: <YYYY-MM-DD>（必填；每次增量更新都要改。若某个子 issue 已收口为 DONE，对应 issue 文档/小节也必须保留最近完成日期）

## 范围与约束（Scope & Constraints）
- 覆盖范围：<模块/路径/场景>
- 重点场景：<如 openai-responses + base_url + api key>
- Out of scope：<明确不做>
- 状态标记：
  - **[DONE]** 已落地（含测试/证据）
  - **[TODO]** 方案已收敛，待实施
  - **[SURVEY]** 仅记录/调研，不承诺

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 本文档出现的“主概念/主称呼”必须在此处定义；正文不再引入新别名。

- authoritative / estimate / configured / effective / as-sent：<按项目口径补齐>
- <项目特有主概念1>：<What/Why/Not/Source/Method>
- <项目特有主概念2>：…

---

## Executive Summary（结论与优先级）
### 已完成（本轮落地）
- <DONE 项>：<一句话收益>（impl/tests 证据见 Index）

### MUST-FIX（阻断可靠性/安全性）
- <TODO 项>：<一句话风险>

### SHOULD-FIX（提升可控性/成本/体验）
- <TODO 项>：<一句话收益>

### Survey-only（仅记录/调研）
- <SURVEY 项>：<一句话结论>

---

## Issue Index（强制维护，防遗留）
> **增量更新主入口**：每次改动必须同步更新本表。
>
> 约束：
> - `Evidence` 至少 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。
> - `Tests` 必须写“已有/新增/缺口”（不能为空）。
> - P0/P1 的 TODO 必须有 `Doc` 指向独立 issue 文档（避免长文埋雷）。
> - 每个 issue 必须有可见的最近更新时间；DONE issue 必须能看出完成/收口日期。

| ID | Status | Pri | Updated | Title | Owner | Component | Depends On | Evidence | Tests | Doc |
|---:|:---:|:---:|:---:|---|---|---|---|---|---|---|
| 1 | TODO | P0 | <YYYY-MM-DD> | <标题> | <owner> | <component> | <ids> | <file:line/log> | <unit/e2e/缺口> | `issues/issue-xxx.md` |
| 2 | DONE | P1 | <YYYY-MM-DD> | <标题> | <owner> | <component> | <ids> | <file:line> | <tests> | `issues/issue-yyy.md` |

---

## 建议实施顺序（Sequencing）
1) <先做什么，为什么（依赖/风险半径）>
2) …

## 最小验证矩阵（Validation Matrix）
> 跨 issue 的“最小回归面”；增量追加，不重排。

| Item | 验证点 | 最小自动化 |
|---|---|---|
| <issue id/title> | <what> | unit/integration/e2e/手工 |

---

# Issues（逐条）
> 建议：P0/P1 独立 issue 另写 `issues/issue-*.md`，这里保留摘要与索引即可。

## <ID>) [STATUS] <标题>
### Metadata
- Priority:
- Updated: <YYYY-MM-DD>（必填；若本小节已 DONE，这里必须是最近完成/收口日期）
- Owner:
- Component:
- Affected paths/providers/models:
- Dependencies:
- Rollout scope: (dev/prod/telegram/web)

### 背景（Background）
- …

### 问题陈述（Problem）
#### 现象（Symptoms）
- …
#### 影响（Impact）
- …

### 现状核查与证据（As-is / Evidence）【不可省略】
- `path/to/file:line`：…
- <日志/复现>：…

### 目标与非目标（Goals / Non-goals）【不可省略】
- Goals：
- Non-goals：

### 收敛口径 / 规范（Spec / Rules）【不可省略】
> 用“必须/不得/应当”，并标注 authoritative/estimate/configured/effective/as-sent。

- 必须：
- 不得：
- 应当：

### 方案（Proposed Solution）
- 方案要点：
- 失败模式与降级：
- 安全与隐私：

### 验收标准（Acceptance Criteria）【不可省略】
- [ ] …
- [ ] …

### 测试计划（Test Plan）【不可省略】
- Unit：
- Integration：
- UI E2E（如适用）：
- 自动化缺口（如有）：<原因 + 手工验收步骤>

### 发布与回滚（Rollout & Rollback）
- 发布策略：
- 回滚策略：
- 监控点：

### Cross References
- Related issues/docs:
- Related commits/PRs:

### Progress（实施现状）
- 当前状态：
- 已完成证据（逐条带日期）：
- 已知差异/后续优化（非阻塞）：

### Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 安全隐私检查通过
- [ ] 回滚策略明确
- [ ] Issue Index 表已同步（Status/Evidence/Tests/Doc）

---

## Appendices（证据附录，可选）
## Appendix A: 外部协议/字段证据
- <链接/摘要>
## Appendix B: Survey 参考
- <链接/摘要>

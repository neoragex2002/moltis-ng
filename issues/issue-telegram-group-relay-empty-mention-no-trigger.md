# Issue: Telegram 群聊 relay 空点名（仅 @bot）不触发目标 bot（空任务 / 点名唤醒）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P2
- Updated: 2026-03-06
- Owners: <TBD>
- Components: gateway / telegram
- Affected providers/models: <N/A>

**已实现（相关基础能力，写日期）**
- relay 抽取“点名组 + 任务文本”：`crates/gateway/src/chat.rs:6293`
- relay 触发投递：`crates/gateway/src/chat.rs:6411`

**已知差异/后续优化（非阻塞）**
- Telegram 入站（人→bot）对“仅 @this_bot”有固定短回复（不跑 LLM），但 relay（bot→bot）当前没有对应语义。

---

## 背景（Background）
- 场景：Telegram 群里，bot A 想“叫醒/戳一下” bot C，只发一行 `@C`，不带任何任务正文。
- 目标：这种“空点名”到底算不算有效触发，需要一个明确、可解释的口径（至少避免现在这种“看起来点名了但完全没触发”的困惑）。
- Out of scope：不讨论 V4 的 WAIT/RootMap/TaskCard/epoch。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **空点名**：一行里只有目标 bot 的 `@username`（可能夹杂空白/标点），不包含任何可作为任务的文本。
- **relay 触发**：gateway 从 outbound_text 抽取“点名组+任务”，并向目标 bot 的 session 发送注入消息以触发处理。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 明确并实现：当 bot A 发出空点名 `@C` 时，目标 bot C 的期望行为。

### 非功能目标（Non-functional）
- 正确性口径：
  - 必须：行为可解释、可预测（产品口径写清楚）。
  - 不得：出现“点名了但静默无任何系统解释”的黑盒体验（至少要可观测/可诊断）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) bot A 发出一行 `@C`（无任务文本）后，目标 bot C 不会被 relay 触发。

### 影响（Impact）
- 用户体验：在群聊中，“点名/戳一下”是常见动作；当前表现会让人误以为 relay/mention 机制不稳定。
- 排障成本：很难区分是“没触发”还是“触发了但选择 silence/不回复”。

### 复现步骤（Reproduction）
1. 在群里让 bot A 发送：`@C`
2. 观察：gateway 不会对 C 发起 relay（C 不会收到 relay 注入触发）。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/chat.rs:6343`：只有当 `!resolved.is_empty() && !task.is_empty()` 才会产出 `RelayMentionGroup`，因此空点名不会进入后续 relay 流程。

## 根因分析（Root Cause）
- relay 抽取阶段把“任务文本为空”的点名组直接过滤掉（以避免误触发），导致空点名永远不会触发目标 bot。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 下面口径需二选一并冻结（或明确新增开关）。

- 选项 A（空点名也算触发，但不跑 LLM）：
  - 必须：当 bot A 发 `@C` 时，系统应当触发 bot C 给出固定短回执（例如“我在/请说明任务”）。
  - 不得：空点名触发昂贵推理。

- 选项 B（空点名不触发，但要可诊断）：
  - 必须：明确规定空点名不触发，并提供可观测证据（例如日志/事件标记）说明“因任务为空而跳过”。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1：支持空点名回执（推荐偏 UX）
- 核心思路：允许 `task_text` 为空时也形成一个“空任务触发”，下游由目标 bot 返回固定短语（不跑 LLM）。
- 优点：更符合群聊直觉，减少“到底有没有触发”的困惑。
- 风险/缺点：需要定义“空点名回执”的线程/回复目标与去重策略。

#### 方案 2：保持不触发，但补齐观测
- 核心思路：继续过滤空任务，但记录明确的 skip reason。
- 优点：实现最小、避免误触发。
- 风险/缺点：交互仍然偏硬；需要用户理解“必须带任务正文”。

### 最终方案（Chosen Approach）
- <TBD>

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 空点名 `@C` 的行为口径写清楚且稳定。
- [ ] 用户可从群内表现或日志/事件中判断：是“未触发”还是“触发后选择不回复”。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] 覆盖：空点名是否被抽取/触发（按选定口径）：`crates/gateway/src/chat.rs`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：需要端到端模拟多 bot 群聊触发。
- 手工验证步骤：
  1) A 发 `@C`；
  2) 观察 C 的行为是否符合选定口径（短回执 / 不触发但可诊断）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：如改变默认行为，优先加开关/默认关闭。
- 回滚策略：关闭开关恢复“空点名不触发”。

## 实施拆分（Implementation Outline）
- Step 1: 冻结口径（选项 A / B / 新增开关）。
- Step 2: 调整 relay 抽取与下游处理（或补齐 skip reason 的可观测）。
- Step 3: 补齐最小测试与手工验收清单。

## 交叉引用（Cross References）
- Related docs：
  - `issues/discussions/telegram-group-at-rewrite-mirror-relay-as-is.md`
- Related code：
  - `crates/gateway/src/chat.rs:6343`

## 未决问题（Open Questions）
- Q1: 空点名的产品口径选哪一个（回执 vs 不触发）？
- Q2: 若回执：回执应 reply 到哪条消息（A 的那条）？是否需要去重？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 回滚策略明确

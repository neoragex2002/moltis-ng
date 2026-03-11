# Issue: Telegram 群聊 relay 场景下 “silent（空输出）” 不可见（silent=true / 无回执）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Updated: 2026-03-09
- Owners:
- Components: gateway / telegram / ui
- Affected providers/models: (any)

**已实现（如有，写日期）**
- 当前 gateway 会将空输出判定为 `silent=true` 并在日志里输出：`crates/gateway/src/chat.rs:5207`
- 当前 Telegram 通道若 `text.is_empty()` 会直接跳过发送（仅 info 日志）：`crates/gateway/src/chat.rs:6298`

**已覆盖测试（如有）**
- （暂无）

**已知差异/后续优化（非阻塞）**
- 当前“silent”在 Telegram 群里完全不可见；用户常误判为“没收到/没触发/宕机/没权限”。

---

## 背景（Background）
- 场景：Telegram 群聊中，bot A 行首点名/派活多个 bot（通过 relay 机制逐个激活推理）；其中某个 bot 的 LLM 输出为空字符串/全空白，系统判定为 silent，最终不向群里发送任何回复。
- 约束：
  - “silent”本意是节流/不打扰；默认不应刷屏。
  - 但被明确点名/激活时，“完全无信号”会造成困惑与误判。
- Out of scope（本单默认不做）：
  - 不引入可靠投递/outbox；不改 Telegram 的重试/重连语义。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **silent 回复**（主称呼）：一次 agent run 的最终可展示文本 `trim()` 后为空，系统将其视为“沉默/不发言”，因此不会向 Telegram 群发送回执。
  - Why：用户需要知道“系统是否已触发 + 是否有意沉默”。
  - Not：不是失败/超时/取消；也不是“未 mention/未触发”。
  - Source/Method：effective（由 gateway 以 `display_text.trim().is_empty()` 判定）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 当 Telegram 群聊 relay 激活了 bot，但该 bot 最终 `silent=true` 时，必须有**显性可观测信号**让操作者能判断“已触发但选择沉默/空输出”，而不是误判为未触发。
- [ ] 该信号必须可关联到：`run_id`、`session_key`、`trigger_id`、Telegram `chat_id`、bot 身份（username/account_handle）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：不因增加可观测性而改变默认群聊语义（默认仍“silent 就不发 Telegram 消息”）。
  - 不得：在群里为每次 silent 都发一条可见消息（会刷屏），除非显式开启 debug/receipt 开关。
- 可观测性：
  - 至少包含一条结构化日志（带稳定 reason code，例如 `event=channel_delivery.suppressed code=silent_response`）。
  - Web UI（如适用）：能看到该次 run 的 `silent=true` 与关联信息（runId 可复制）。
- 安全与隐私：不得打印 token/完整正文；正文只允许长度/短预览/哈希。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 群聊中点名了多个 bot，但只有部分 bot 在群里可见回复；另一些 bot“看起来像没收到/没触发”。
2) 日志里能看到被激活 bot 的 `agent run complete ... silent=true`，但群内用户无法从 Telegram 侧区分“沉默”与“未触发/失败/权限问题”。

### 影响（Impact）
- 用户体验：误判系统不稳定；需要人工反复手工 @ 叫醒确认。
- 可靠性认知：把“silent（有意沉默）”当成“漏触发/丢消息/不工作”。
- 排障成本：需要翻日志对 run_id/relay 分发进行人工比对。

### 复现步骤（Reproduction）
1. 在 Telegram 群里，由 bot A 发一条消息，行首点名/文本内包含多个 bot 的 @（触发 relay 激活多个 bot）。
2. 观察：其中一个 bot 在日志中完成 run（`silent=true`），但群里没有其回复消息。
3. 期望 vs 实际：
   - 期望：至少在日志/Web UI 有明确“沉默抑制发送”的结构化证据，并可关联到 chat/bot/run。
   - 实际：群里无任何信号；（若只看群）无法判断发生了什么。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/chat.rs:5207`：`let is_silent = display_text.trim().is_empty();`（将空输出判定为 silent）
  - `crates/gateway/src/chat.rs:6298`：`if text.is_empty() { ... "telegram reply delivery skipped: empty response text" }`（Telegram 通道直接跳过发送）
- 日志证据（关键词）：
  - `agent run complete ... silent=true`
  - `telegram outbound relay: dispatched ... target_account_id=...`
  - （可选）`telegram reply delivery skipped: empty response text`（当前为 info，且不带强关联字段）
- 当前测试覆盖：
  - 缺口：silent 抑制发送的结构化观测、UI 展示与关联信息暂无单测覆盖。

## 根因分析（Root Cause）
- A. relay 确实触发了目标 bot 的 `chat.send` 并运行推理（见 relay dispatched + agent run complete）。
- B. 目标 bot 的最终输出为空/全空白，被 gateway 归类为 `silent=true`。
- C. silent 的下游语义是“跳过 Telegram 发送”，且缺少足够显性的结构化日志/UI 展示，使得“沉默”在群里表现为“完全无信号”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当一个 channel-bound run 最终 `silent=true` 时，必须输出 1 条结构化日志：
    - `event=channel_delivery.suppressed code=silent_response run_id=... session_key=... trigger_id=... chat_id=... bot=...`
  - Web UI（如连接）必须可见该次 run 的 `silent=true`，并能复制 runId 进行排障。
- 不得：
  - 默认情况下不得在 Telegram 群里发送“我沉默了”的可见回执（避免刷屏）。
- 应当：
  - 若开启 debug/receipt 模式（可配置），则允许向群/或仅向操作者 DM 发送最短“silent receipt”，例如：`(silent) code=silent_response run=xxxx`。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐，观测性优先，不改默认群语义）
- 核心思路：
  - gateway：在决定 `silent=true` 且存在 Telegram reply targets 时，记录结构化日志 `event=channel_delivery.suppressed code=silent_response`，字段齐全可关联。
  - Web UI：在 error/notice 之外补充一种轻量 notice（或在 final footer）标明 `silent=true`（包含 runId）。
- 优点：不刷群、不改变语义；排障速度提升明显。
- 风险/缺点：群内普通用户仍看不到“沉默”的解释（但操作者可通过日志/UI 快速确认）。

#### 方案 2（备选，可见回执，可配置开关）
- 核心思路：为“被明确点名/-> you 激活”的 silent run 发送一条最短回执（默认关闭）。
- 优点：群内用户也能理解发生了什么。
- 风险/缺点：可能刷屏；需要仔细定义何时算“明确点名”。

### 最终方案（Chosen Approach）
- 先落地方案 1；方案 2 作为可选增强，后续按群噪声实际体验决定是否启用。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 复现“relay 激活但 silent=true”时，日志出现 `event=channel_delivery.suppressed code=silent_response` 且字段齐全（run/session/trigger/chat/bot）。
- [ ] Web UI（如连接）能看到该 run 的 `silent=true`，并可复制 `runId` 做链路关联排障。
- [ ] 默认情况下 Telegram 群里仍不会新增 silent 回执消息（不刷屏）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] gateway：silent 抑制发送时会记录结构化日志（可通过 test writer / subscriber 断言字段）。

### Integration
- [ ] 手工：在 Telegram 群里触发一次 silent run，确认“群里无回执 + 日志/UI 可见 suppressed 证据”。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：日志断言需要统一 test subscriber；Telegram 真实链路需手工环境。
- 手工验证步骤：见 Integration。

## 发布与回滚（Rollout & Rollback）
- 发布策略：仅新增日志/UI 展示字段；不改默认 Telegram 行为。
- 回滚策略：回滚新增日志/UI 分支（不影响核心功能）。

## 实施拆分（Implementation Outline）
- Step 1: gateway：在 `deliver_channel_replies(...)` 的 `text.is_empty()` early return 路径补齐结构化日志（含 run_id/trigger_id/chat_id/bot）。
- Step 2: Web UI：对 silent final 增补更明确的 footer/notice（runId 可复制）。
- Step 3: 补单测 + 手工验收步骤落地。
- 受影响文件：
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/assets/js/websocket.js`
  - `crates/gateway/src/assets/js/chat-ui.js`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-observability-llm-and-telegram-timeouts-retries.md`（更广义的超时/失败可观测性）

## 未决问题（Open Questions）
- Q1: “bot 身份”在日志里以 `account_handle` 还是 `@username` 为主键展示？
- Q2: 是否需要可配置的“silent receipt”（默认关闭）来减少群内误判？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确


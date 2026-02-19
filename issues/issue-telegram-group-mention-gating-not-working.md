# Issue: Telegram 群聊 `mention_mode=mention` 失效（@mention 误判 / false negative / 导致不响应）

## 实施现状（Status）【增量更新主入口】
- Status: TODO（待核实与修复）
- Priority: P1（群聊场景直接“无回复”，影响可用性）
- Components: telegram / channels gating / access control
- Affected channels: Telegram（Group / Channel）

**已实现**
- 现有实现：群聊默认 `mention_mode=mention`，仅当检测到消息文本包含 `@bot_username` 才响应。

**已覆盖测试**
- 已有 access 逻辑单测（不含 “@mention 检测” 本身）：`crates/telegram/src/access.rs`
- 缺口：`check_bot_mentioned()` 未覆盖实体(entity)/大小写/命令等边界，容易回归。

---

## 背景（Background）
在 Telegram 群/频道中，为避免刷屏，Moltis 提供 `mention_mode` 门禁：
- `mention`：必须 @mention bot 才响应
- `always`：响应所有消息
- `none`：群里不响应

用户反馈：即使在群里明确 `@bot_username`，Moltis 也不响应，表现为“@机制不起作用”。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **mention_mode**：群聊消息是否需要被 @mention 才处理的策略（配置层）。
- **bot_mentioned**：telegram handler 推导出的布尔值，表示本条消息是否“算作提及 bot”。
- **false negative**：用户实际提及了 bot，但 `bot_mentioned=false`，导致访问控制拒绝（NotMentioned）并静默丢弃消息。
- **DM vs Group**：本单只讨论 Group/Channel；DM 不走 mention 门禁。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 当用户在群里 `@bot_username` 时，`mention_mode=mention` 必须放行并触发响应（不再误判）。
- [ ] 明确并实现“哪些情况也算唤醒”（规范化）：至少包含
  - 显式文本 `@bot_username`（大小写不敏感）
  - （可选但建议）回复 bot 的消息（reply_to bot）
  - （可选但建议）`/command@bot_username`（bot command addressed）
- [ ] 当消息被拒绝时，日志必须能解释“为什么”（不泄露用户消息正文）。

### 非功能目标（Non-functional）
- 不得扩大响应面：`mention_mode=mention` 不得退化为 `always`（除非明确配置）。
- 兼容 Telegram 隐私模式（BotFather privacy mode）：在隐私模式下，bot 可能只收到“mention/command/reply”等消息，但本地门禁也必须识别这些唤醒方式，避免“收到却拒绝”。
- 安全隐私：拒绝日志不得记录完整消息文本；可记录实体类型、是否包含 `@`、以及 bot username 等低敏信息。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram 群聊里发送 `@bot_username 你好`，bot 无响应。
2) server 日志可见 “access denied: bot was not mentioned”（或同语义），即本地门禁误判。

### 影响（Impact）
- 用户体验：群聊基本不可用（必须 @ 才响应但 @ 又不生效）。
- 排障成本：用户难以区分是 Telegram 平台未投递，还是 Moltis 本地门禁拒绝。

### 复现步骤（Reproduction）
1. 将 bot 加入一个 Telegram 群。
2. 将该 bot 的 `mention_mode` 配置为 `mention`（默认即 `mention`）。
3. 在群里发送：`@<bot_username> ping`
4. 期望：bot 响应。
5. 实际：bot 无响应；日志可能出现 `handler: access denied reason="bot was not mentioned"`。

## 现状核查与证据（As-is / Evidence）【不可省略】
- mention 门禁配置与枚举：
  - `crates/channels/src/gating.rs:52`（`MentionMode::{Mention,Always,None}`）
  - `crates/telegram/src/config.rs:33`（`TelegramAccountConfig.mention_mode` 默认 `Mention`）
- 门禁执行点：
  - `crates/telegram/src/access.rs:72`（群聊下 `MentionMode::Mention` → `bot_mentioned` 为 true 才放行）
- `bot_mentioned` 计算方式（当前为脆弱的字符串包含）：
  - `crates/telegram/src/handlers.rs:1460`：`text.contains(&format!("@{username}"))`
  - 该判断 **大小写敏感**、**不解析 Telegram entities**、也 **不考虑 reply_to bot**。
- 被拒绝的日志证据：
  - `crates/telegram/src/handlers.rs:184`：`warn!(... %reason ... "handler: access denied")`（reason 可能为 `NotMentioned`）

## 根因分析（Root Cause）
主要风险点（可同时存在）：
- A) **大小写敏感**：Telegram username 比较应视为 case-insensitive；用户输入/客户端补全可能与 `get_me().username` 的大小写不一致。
- B) **未解析 entities**：Telegram message 的 mention 可能以 `MessageEntity` 表达；纯 `contains()` 既脆弱也难以区分“@在 code block/引用里”等情况。
- C) **reply/command 不算唤醒**：在 Telegram 隐私模式下，bot 可能会收到“reply to bot”或“/cmd@bot”的消息，但本地门禁仍以 `@username` 子串判断，导致“收到了却拒绝”。
- D) **bot_username 缺失/异常**：若 `get_me().username` 为 None（理论上少见）或带前缀 `@`（不应发生但可被其它路径污染），则 `contains("@{username}")` 会恒 false。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `mention_mode=mention` 下，显式提及 bot 必须被识别（case-insensitive）。
  - 识别逻辑以 Telegram 的实体(entity)优先（若可用），字符串 fallback 作为兼容路径。
  - 对 `NotMentioned` 的拒绝应提供可定位日志（不含正文）。
- 不得：
  - 不得因修复误判而放开���有���聊消息（避免刷屏）。
- 应当：
  - 明确 spec：reply-to-bot / bot command addressed 是否算唤醒；并写入文档与测试（建议算唤醒，符合 Telegram 隐私模式直觉）。

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（快速修复：大小写不敏感 + 更稳的字符串匹配）
- 做法：
  - `check_bot_mentioned()` 改为 case-insensitive（lowercase 比较）。
  - 兼容 `bot_username` 可能带 `@` 的情况（normalize）。
- 优点：改动小、立竿见影。
- 风险/缺点：仍然不解析 entities；reply/command 场景可能仍失败。

#### 方案 2（推荐：基于 Telegram entities 的唤醒判定 + 明确 reply/command 规则）
- 做法：
  - 从 `MessageKind::Common` 中读取 `entities`（文本）与 `caption_entities`（媒体 caption）。
  - 规则优先级：
    1) entities 中存在 `Mention("@xxx")` 且 `xxx == bot_username`（case-insensitive）→ true
    2) entities 中存在 `BotCommand`，且命令形如 `/cmd@bot_username`（case-insensitive）→ true（可选）
    3) `reply_to_message` 的发送者是 bot（需要在 AccountState 额外缓存 bot user_id，启动时 `get_me().id`）→ true（可选）
    4) fallback：对 message text 做 case-insensitive 的 `@bot_username` substring（兼容）
- 优点：更符合 Telegram 平台语义；减少误判与回归；对隐私模式更稳。
- 风险/缺点：实现与测试略多；需要确认 teloxide 的 entity 类型/字段在当前版本可用。

### 最终方案（Chosen Approach）
建议先落地 **方案 2**；若需要快速止血，可先做方案 1 作为短期补丁，但应在同一 issue 中继续推进到方案 2。

#### 可观测性增强（建议）
- 当 access denied 原因为 `NotMentioned` 时，在 warn/debug 日志增加：
  - bot_username（规范化后）
  - `has_entities` / entity kinds 列表（不含正文）
  - `text_has_at_sign`（bool）
  - `chat_id` / `message_id` / `chat_type`

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 群聊 `mention_mode=mention` 下，`@bot_username`（含大小写变化）能稳定触发响应。
- [ ] 若消息包含 `/context@bot_username` 或 reply-to-bot（若纳入 spec），也能触发响应。
- [ ] `NotMentioned` 的拒绝日志能解释原因且不泄露正文。
- [ ] 新增单元测试覆盖 entity/大小写/命令/回复（至少覆盖其中 2–3 个关键路径）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] 为 `check_bot_mentioned()` 抽取纯函数并新增测试：
  - 大小写不敏感
  - entities mention 命中
  - `/cmd@bot` 命中（若纳入）
  - reply-to-bot 命中（若纳入）

### Integration
- [ ] （可选）在 Telegram handler 的现有 tests 模块中构造 `Message`（或 mock）覆盖 access deny/allow 路径，断言 `AccessDenied::NotMentioned` 不再在 mention 场景触发。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用新判定逻辑（不改变 mention_mode 的默认值）。
- 回滚策略：保留旧 substring fallback；若出现异常可快速切回仅 substring（可用 feature flag 或最小回滚 commit）。

## 实施拆分（Implementation Outline）
- Step 1: 明确 spec：reply/command 是否算唤醒（写入本单 Desired Behavior）。
- Step 2: 实现 entities 优先的 mention 检测（含大小写 normalize）。
- Step 3: 增补日志字段（NotMentioned 分支）。
- Step 4: 增补单元测试。

## 交叉引用（Cross References）
- Related docs：
  - `issues/overall_issues_2.md`（Survey: 群聊默认 mention 模式）
- Related code：
  - `crates/telegram/src/handlers.rs:1460`（当前 mention 检测）
  - `crates/telegram/src/access.rs:72`（门禁判定）

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（唤醒判定稳定）
- [ ] 已补齐自动化测试（覆盖 entities/大小写等关键路径）
- [ ] 日志可观测性增强到位（不泄露正文）
- [ ] 文档/配置说明更新（群聊唤醒规则清晰）

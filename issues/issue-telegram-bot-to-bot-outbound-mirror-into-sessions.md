# Issue: Telegram 多 Bot 群聊 “bot2 要知情 bot1 回复/工具结果” 无法旁听（需 Moltis 侧出站复制到会话）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Owners: <TBD>
- Components: telegram / gateway / sessions / ui / config

**已实现（相关前置）**
- 群聊 reply vs ingest 二维解耦（`group_ingest_mode` + ingest-only 写入）：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- 自我点名剥离与 `/cmd@bot` 规则冻结：`issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- 官方平台约束（Bot API 不投递 other bots messages）已写入上述两单及 mention gating 单：`issues/done/issue-telegram-group-mention-gating-not-working.md`
 
> 注：上述 3 张前置单已关单并移入 `issues/done/`，引用路径见下文已同步更新。

**已知差异/后续优化（非阻塞）**
- 本单聚焦 “bot1 的最终回复正文” 的镜像；是否同步 tool 输出摘要属于可选扩展（见 Open Questions）。

---

## 背景（Background）
- 场景：同一个 Telegram 群里拉多个 bot（例如 `lovely`/`fluffy`），希望：
  - bot1 被点名时干活并在群里回复
  - bot2 不抢答，但要“知情” bot1 的最终回复（后续用户问 bot2 时能引用 bot1 输出）
- 约束（平台）：Telegram Bot API 即使 Privacy Mode=OFF，也不会把“其他 bot 发送的消息”投递为 update，因此 bot2 **无法**靠“旁听群聊 updates”拿到 bot1 的最终回复正文。
  - 官方原文（Telegram Bots FAQ）：
    - “Bot admins and bots with privacy mode disabled will receive all messages except messages sent by other bots.”
    - “...bots will not be able to see messages from other bots regardless of mode.”
  - External refs：
    - `https://core.telegram.org/bots/faq#what-messages-will-my-bot-get`
    - `https://core.telegram.org/bots/faq#why-doesnt-my-bot-see-messages-from-other-bots`
- Out of scope：不改变 Telegram 平台投递行为；不尝试“让 bot 看到别的 bot update”（平台不可行）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **outbound mirror**（出站复制/镜像）：当 bot1 成功向群发送一条回复后，Moltis 在服务端把同一条回复“写入”到 bot2 的 session 历史中（仅写入，不再出站）。
  - Why：让 bot2 的 LLM 上下文包含 bot1 的最终输出，从而“知情”。
  - Not：不是 Telegram 平台层的转发；不是让 bot2 收到 bot1 的 update。
  - Source/Method：authoritative（来自 bot1 实际出站成功后的文本）；写入为本地 persist。
- **source bot / target bot**：触发出站的 bot（source）与接收镜像写入的 bot（target）。
- **chat scope**：同一个 Telegram 群/超级群的 `chat_id`（本仓库当前按 `(account_id, chat_id)` 做会话分桶；topic/thread 先不纳入）。
- **ingest-only**：仅写入 session，不触发 LLM，不产生任何 Telegram 出站。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 在同一群聊内，当 source bot 成功发送回复后，按配置把该回复镜像写入 target bot 的 session 历史（ingest-only）。
- [ ] 镜像写入不得触发 target bot 的 LLM run，不得导致任何 Telegram 出站（避免回环/刷屏）。
- [ ] 镜像必须可配置、默认关闭；并且仅限 Group/Supergroup（DM/私聊不涉及，保持原样）。
- [ ] 镜像写入内容必须可区分来源（例如前缀 `[@lovely_apple_bot] ...` 或 metadata 标记），避免 target bot 误以为那是自己说的。
- [ ] （V1）当 source bot 出站包含媒体（图片/文件/语音等）时，mirror 至少写入“文本占位 + 来源标记”（不镜像媒体本体）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：只在 **source bot 出站成功** 后镜像（避免把失败/中间流式片段写入）。
  - 必须：镜像写入只发生一次（去重），不得因为重试/断线导致重复写入。
  - 不得：不得把镜像再次触发“镜像”（防止多 bot 互镜像形成爆炸）。
  - 不得：不得让 mirror 绕过既有 access control（例如写入到错误 chat_id/session）。
- 兼容性：默认关闭；旧配置不受影响。
- 可观测性：
  - 需在日志与 `/context`（可选）暴露：mirror 配置是否启用、最近一次 mirror 的 source/target/chat_id。
- 安全与隐私：
  - 默认不镜像 DMs。
  - 镜像写入不应记录 bot token、敏感凭据等；日志只打印必要定位字段（account_id/chat_id/message_id/hashed ids）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 群里 bot1 回复了结果，但 bot2 无法“看到” bot1 回复内容；用户问 bot2 “你看不到 bot1 的结果吗？” bot2 回答“看不到”。
2) 即使两边 BotFather Privacy Mode 都设为 OFF，仍然如此（符合官方限制）。

### 影响（Impact）
- 用户体验：multi-bot 协作场景落空；需要人工复制粘贴 bot1 输出给 bot2。
- 可靠性：旁听 ingest-only 只能旁听人类消息，无法覆盖“最终答案”。
- 排障成本：用户容易误以为“旁听模式坏了”，实际是平台投递限制。

### 复现步骤（Reproduction）
1. 在同一群中加入 bot1 与 bot2，均设置 Privacy Mode=OFF。
2. 用户仅 @bot1 触发 bot1 干活并回复。
3. 用户问 bot2：bot1 刚才说了什么？
4. 期望：bot2 能引用 bot1 的回复；实际：bot2 看不到（符合官方限制）。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 平台证据（官方 FAQ）：见 Background（原文 + 链接）。
- 代码证据：
  - `group_ingest_mode=all_messages` 只旁听“bot 实际收到的 update”，无法覆盖 other bots messages（平台不投递）：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`
  - 现有 ingest-only 接口只写入 user inbound（channel_user），不会写入“assistant outbound”到其他 session：`crates/gateway/src/channel_events.rs`
- 当前测试覆盖：
  - 已有：listen-only ingest 的 handler 分支、channels.update merge/patch 等（见相关 issue）。
  - 缺口：没有 “bot1 outbound 后镜像到 bot2 session” 的单测/集成覆盖。

## 根因分析（Root Cause）
- A. 上游/触发：Telegram Bot API 为避免循环，**不投递 other bots messages** 给 bots。
- B. 中间逻辑：Moltis 当前仅基于“收到的 update”决定写入/回复；listen-only ingest 无法凭空得到 bot1 的回复文本。
- C. 下游表现：bot2 session 缺失 bot1 的最终输出 → LLM 无法引用。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当且仅当 source bot **出站成功**后，才进行镜像写入。
  - 镜像写入必须是 ingest-only（不触发 LLM、不出站）。
  - 镜像写入的记录必须明确来源（source bot identity）。
- 不得：
  - 不得默认开启（避免无意记录/同步）。
  - 不得镜像 DMs。
  - 不得引入 mirror→mirror 的链式回环。

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（推荐）：gateway 出站成功后 mirror → target sessions（ingest-only）
- 核心思路：
  - 在 gateway 的 “Telegram reply delivery 成功” 回调点拿到最终发送文本（authoritative），再按配置把该文本写入同 chat_id 的其他 bot 会话（target sessions）。
- 优点：
  - 不依赖 Telegram 投递；绕开平台限制，满足产品诉求。
  - 改动集中在 Moltis 自身（gateway/sessions），可控、可测、可回滚。
  - 不改变现有 reply/gating 行为；只增加可选镜像写入。
- 风险/缺点：
  - 需要定义“如何表示镜像内容”（role/metadata），避免模型误判为自身输出。
  - 需要去重与循环保护。

#### 方案 2（不推荐）：共享 transcript session（全 bot 共享群上下文）
- 风险：会话模型与 compaction/权限/调试口径复杂度大幅上升，不适合作为第一解。

### 最终方案（Chosen Approach）
采用 **方案 1：出站复制 mirror（默认关闭）**。

#### 行为规范（Normative Rules）
- R1（触发点）：仅在 source bot 的 Telegram 出站发送 **成功**后触发镜像（拿到最终文本）。
- R2（范围）：仅 Group/Supergroup；DM 不镜像。
- R3（目标）：仅镜像到显式配置的 target bot 列表（allowlist），且目标必须是“同一 chat_id”。
- R4（写入）：写入为 ingest-only；不得触发 LLM，不得产生 Telegram 出站。
- R5（去重）：同一条 source outbound 只镜像一次（建议 key：`(source_account_id, chat_id, telegram_sent_message_id|hash(text))`）。
- R6（循环保护）：镜像写入的记录应携带 metadata `mirror=true`（或前缀），并在 mirror pipeline 中明确 “mirror messages 不再触发 mirror”。

#### 接口与数据结构（Contracts）
- 配置（建议新增 TelegramAccountConfig 字段，默认空）：
  - `group_outbound_mirror_to_accounts: ["fluffy", "another_bot"]`
  - 仅对 Group/Supergroup 生效。
- 写入格式（建议其一，需冻结口径）：
  - 选项 A（推荐，最小侵入）：写入 `PersistedMessage::User`，文本前缀 `[@<source_bot_username>] <reply>`，并在 `channel` metadata 标记 `{ "mirror": true, "source_account_id": "...", "source_bot_handle": "@..." }`，避免 UI/LLM 混淆。
  - 选项 B：新增专用 message role（需要跨模块改动更大，不作为第一版）。
- 消息示例（落盘到 session JSONL 的单条记录；**不改 schema**）
  - 文本回复 mirror（V1）：
    ```json
    {
      "role": "user",
      "content": "[@lovely_apple_bot mirror] 当前挂载点里主要有：/ (overlay,rw), /proc (proc), /home/luy/.moltis (ext4,ro), /mnt/host/dev (9p,ro) ...",
      "created_at": 1739999999000,
      "channel": {
        "channel_type": "telegram",
        "message_kind": "text",
        "mirror": true,
        "source_account_id": "lovely",
        "source_bot_handle": "@lovely_apple_bot",
        "source_chat_id": "-5288040422",
        "source_message_id": "184"
      }
    }
    ```
    - 其中 **Moltis 额外添加**的是：`content` 前缀 `[@lovely_apple_bot mirror] ` 以及 `channel.mirror/source_*` 字段；其余正文是 source bot 的 authoritative 回复文本。
  - 媒体回复 mirror（V1，占位，不镜像媒体本体）：
    ```json
    {
      "role": "user",
      "content": "[@lovely_apple_bot mirror] （发送了一张图片）caption: 这是挂载列表截图",
      "created_at": 1739999999000,
      "channel": {
        "channel_type": "telegram",
        "message_kind": "text",
        "mirror": true,
        "source_account_id": "lovely",
        "source_bot_handle": "@lovely_apple_bot",
        "source_chat_id": "-5288040422",
        "source_message_id": "184",
        "media": { "kind": "photo" }
      }
    }
    ```
- 媒体（图片/文件）mirror 策略（先冻结为 V1，V2 作为后续扩展）：
  - **V1（本单范围）**：不镜像媒体本体；仅镜像“可读文本占位 + 来源标记”。
    - 例如：`[@lovely_apple_bot mirror] （发送了一张图片）caption: ...`
    - 并在 `channel` metadata 记录 `media.kind/mime` 等非敏感信息，便于排障/后续升级。
    - 原因：避免把大体积 base64/URL 写入上下文导致 token 膨胀；且 Telegram file URL 可能携带敏感 token，不应落盘。
  - **V2（后续增强，不在本单实现）**：将媒体 bytes 保存到 session media 目录（类似 tool_result screenshot），再以 user multimodal `image_url` 引用本地 `/api/sessions/<key>/media/...`，使 Web UI 可显示缩略图且 LLM 可在支持 vision 时使用。
- 可观测性（建议）：
  - gateway 日志：mirror 发生时记录 source/targets/chat_id 与去重 key（不打印正文）。
  - `/context`（可选）：显示 effective mirror 配置与最近 mirror 信息。

#### 失败模式与降级（Failure modes & Degrade）
- 如果 mirror 写入失败（session store 不可用/序列化失败）：仅 warn 日志，不影响 source bot 正常出站回复。
- 如果 target session 未初始化：允许创建/append（与 channel-bound session 同类规则），或选择仅在存在 active session 时写入（需明确）。

#### 安全与隐私（Security/Privacy）
- 默认关闭；开启需显式配置。
- 不镜像 DMs。
- 只在同 chat_id 内镜像（不跨群/跨 chat）。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 在同一群中：仅 @bot1 触发 bot1 干活并回复后，用户 @bot2 询问“bot1 刚才说了什么”，bot2 能引用 bot1 的最终回复（无需手动粘贴）。
- [ ] bot2 不会因为 mirror 写入而主动出站回复（无额外 Telegram 消息）。
- [ ] mirror 默认关闭；开启后只影响配置的 bot 对/目标列表。
- [ ] 官方平台限制已在文档与 debug 口径中明确（避免误解 “旁听能拿到 bot 回复”）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] mirror 去重 key 逻辑（同一 outbound 不重复写入）
- [ ] mirror 写入为 ingest-only（不触发 chat.send / 不 enqueue channel replies）
- [ ] mirror 消息标记（metadata/prefix）能被 UI 与 prompt 重建路径正确保留

### Integration
- [ ] 端到端：两个 Telegram bot + 同群，开启 mirror；验证 bot2 session 历史出现 bot1 回复的镜像条目（可用 `/context`/debug 或 session store 读取验证）。

### 自动化缺口（如有，必须写手工验收）
- 若 CI 无 Telegram 环境：记录手工验收步骤（2 bots、同群、配置、预期日志关键词）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认关闭（配置为空不启用）。
- 回滚策略：回滚仅影响镜像能力，不影响正常 reply/ingest/mention gating。
- 上线观测：统计 mirror 成功/失败计数；日志关键词 `telegram outbound mirror`.

## 实施拆分（Implementation Outline）
- Step 1: 定义并落地配置字段（serde default 空列表），并确保 `channels.update` merge/patch 能保留该字段。
- Step 2: 在 gateway 的 Telegram 出站成功回调处增加 mirror pipeline（拿到最终文本 → resolve target sessions → append mirror message）。
- Step 3: 增补去重与循环保护（metadata 标记 + guard）。
- Step 4: 增补测试（unit + 手工验收指引）。
- Step 5: 文档与 UI（可选）：在 Channels UI 增加 mirror 配置（或先仅支持配置文件/JSON patch）。
- 受影响文件（预估）：
  - `crates/telegram/src/config.rs`
  - `crates/gateway/src/chat.rs`（reply delivery 成功点）
  - `crates/sessions/src/message.rs`（若需新增 metadata/格式 helper）
  - `crates/gateway/src/assets/js/page-channels.js`（可选）

## 交叉引用（Cross References）
- `issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- `issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- `issues/done/issue-telegram-group-mention-gating-not-working.md`

## 未决问题（Open Questions）
- Q1: mirror 写入的 role/格式最终冻结为哪种？（User+prefix+metadata vs 更复杂的专用 role）
- Q2: 是否需要镜像 tool 输出摘要？如果需要，摘要口径是什么、上限是多少、是否脱敏？
- Q3: target session 不存在时是否自动创建/命名？命名策略如何避免污染（例如 “Telegram Mirror 1/2”）？
- Q4: mirror 是否只对 `group_ingest_mode=all_messages` 的 target 生效，还是独立开关？（建议独立，避免强耦合）
- Q5: V2 媒体 mirror 若要支持，是否需要统一的媒体落盘/引用协议（避免 Telegram/exec screenshot 各自为政）？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档已明确 Telegram 平台限制与 Moltis 侧补偿机制（避免误解）
- [ ] 默认关闭且可回滚（不影响现网）
- [ ] 安全隐私检查通过（不镜像 DM；日志不泄露敏感字段）
- [ ] 回滚策略明确

# Issue: Telegram 群聊多 Agent 会议协议（@chairbot 自然语言入口 / 预览确认 / 严格轮询 / 可干预不乱发言）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1（复杂协作的基础设施；不做会长期“乱发言/难复用/难排障”）
- Owners: <TBD>
- Components: telegram / gateway(channel_events, chat.send, session_metadata) / sessions(store+metadata)
- Affected providers/models: all（主要是编排与会话机制；LLM 只负责生成会议计划与发言内容）

**已实现（相关前置）**
- 群聊 reply vs ingest 二维解耦（旁听写入、不点名不回复）：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- 群聊点名/命令 gating 收敛（entities 优先、/cmd@bot 规则）：`issues/done/issue-telegram-group-mention-gating-not-working.md`
- 自我点名剥离与 /cmd@bot addressed 规则：`issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- Telegram 渠道失败回执 + drain reply targets/logbook（避免串线）：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
- Telegram `/context` 结构化 contract（context.v1）与裁剪策略：`issues/done/issue-telegram-context-debug-parity.md`

**已知差异/后续优化（非阻塞）**
- V1 会议状态可只存内存（重启即中止）；如需持久化恢复另开单。
- V1 只同步“最终文本”，不镜像 tool_result/媒体（避免敏感与噪声扩散）。

---

## 前置条件（Prerequisites）【实施前必须对齐】
> 本节把“已有基础 / 外部准备 / 本单阻断缺口 / 相关依赖 issue”列清，避免实施时边做边猜。

### A) 已具备（可直接复用）
- ✅ 群聊 reply vs ingest 二维解耦（旁听写入、不点名不回复）：`issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- ✅ 群聊点名/命令 gating 收敛（entities 优先、/cmd@bot 规则）：`issues/done/issue-telegram-group-mention-gating-not-working.md`
- ✅ 自我点名剥离与 /cmd@bot addressed 规则：`issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- ✅ Telegram 渠道失败回执 + drain reply targets/logbook（避免串线）：`issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`
- ✅ Telegram `/context` 结构化 contract（context.v1）与裁剪策略：`issues/done/issue-telegram-context-debug-parity.md`
- ✅ 代码积木（现状）：
  - `ChannelEventSink::dispatch_to_chat(...)`：触发一次发言（有出站）
  - `ChannelEventSink::ingest_only(...)`：只写入 session，不触发 LLM、不出站
  - session metadata 已有 `parent_session_key` 字段：适合用于 `#meeting=<id>` 派生 session 的血缘关系

### B) 外部准备（非代码，但实施前必须完成）
- [ ] 新建一个 Telegram bot（chairbot）并获取 token（BotFather）
- [ ] 在 Moltis 配置中新增一个 Telegram account（建议 `account_id="chair"`），并把 chairbot 拉进目标群
- [ ] 平台投递前提满足（Privacy Mode/权限/管理员等按你的群情况配置）：确保 chairbot 能收到群消息 update

### C) 本单阻断缺口（必须在本单实现）
- [ ] meeting registry（in-memory）+ meeting lock（按 `chat_id`）：同群同一时刻只允许一个活跃会议
- [ ] chairbot 专用 handler：自然语言解析 → 预览 → 等待确认 → 严格轮询推进
- [ ] 严格轮询调度器：一次只触发一个 speaker 出站，等待该轮完成再进入下一位
- [ ] Director Control 落地：Chair Notes（插话/纠偏/改下一问/暂停/继续/结束）记录与注入规则（不触发抢答）
- [ ] out-of-turn 治理：会议期间绕过 chairbot 直接 `@botX ...`（固定提示 + 记为 Chair Note + 节流）
- [ ] 会议派生 session：为每个参会 agent 创建 `telegram:<agent>:<chat_id>#meeting=<id>` 并写入 metadata（label/parent_session_key/channel_binding）
- [ ] 全量记录 fan-out：会议相关消息写入所有参会 agent 的会议派生 session（带来源标记与 metadata）

### D) 已有相关 issue（本单实现时需要参考/可能复用）
- `issues/issue-named-personas-and-per-session-agent-profiles.md`：per-agent profile/capabilities（本单要求“按 agent 能力集”最终要靠这张落地；V1 可先按 Telegram account 作为 agent 身份，能力集先沿用全局或 channel 默认）
- `issues/issue-terminology-and-concept-convergence.md`：术语收敛（agent/account/session/scope）
- `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`：非会议场景也要跨 bot 可见性时再做（本单会议场景优先走“会议派生 session 的全量 fan-out”，不依赖 Telegram bot-to-bot update）

## 背景（Background）
你希望在 Telegram 群里做复杂协作：
1) 多个 agent 共同讨论，你指挥多个 agent 分工干活；
2) 让多个 agent 互相讨论，但避免无序乱发言；
3) 支持“自然语言编排式”的开会方式，而不是一堆硬命令参数；
4) 你可以随时插话纠偏（导演权），但不应触发抢答/插话。

关键平台约束（必须先明确）：
- Telegram Bot API **不会**把 “其他 bot 发送的消息”作为 update 投递给另一个 bot。因此 bot-to-bot 的“可见性/互聊”不能依赖 Telegram 投递，只能由 Moltis 网关内部同步补偿。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **Agent**：群里一个可被点名、可发言的 bot（平级；差异只体现在 capabilities 与风格配置）。对用户只暴露“agent”这个概念。
- **chairbot**：一个专用 agent（也是平级 agent），承担会议编排与秩序执行；默认**不发表观点**，只做组织与总结。
- **Meeting**：一次群内会议，会占用该 `chat_id` 的一个“会议锁”（同群同一时刻只允许一个活跃会议）。
- **导演权（Director Control）**：你可以随时插话/纠偏/改下一问/暂停/结束；但不得破坏严格轮询，也不得让其它 agent 抢答。
- **Strict turn-taking（严格轮询）**：任何时刻只允许一个“当前 speaker”发言；每轮每人最多 1 条。
- **会议派生 session（meeting-derived session）**：每个参会 agent 在同一群内，为本次会议创建独立会话桶：
  - `telegram:<agent_account_id>:<chat_id>#meeting=<meeting_id>`
  - 会议过程全量写入这些派生 session，不污染日常群 session。

## 目标（Desired Behavior）
### 高层目标
- 会议入口只认 `@chairbot`（自然语言），避免“/meeting@bot2 但你主持”这类二阶复杂度。
- chairbot 永远先给“会议计划预览”，你确认后才开始（避免解析错误导致乱发言）。
- 所有发言由 chairbot 依次触发（严格轮询）。
- 你随时可干预，但干预不会触发抢答；chairbot 会把你的干预注入下一位发言提示。
- 全量会议记录写入每个参会 agent 的会议派生 session；结束后可选同步 summary 到日常 session。

### 约束与边界
- 会议状态 V1 可仅内存（重启中止）。
- V1 只处理文本与工具摘要层面的编排；媒体 mirror 不做或只占位。
- 会议期间，各 agent 的工具使用遵守各自 capabilities（例如有的 agent 禁止 exec/web）。

## 使用方式（User-facing UX，冻结为 V1）
> 备注：所有控制语句都必须 `@chairbot ...`，以避免多 bot 同时处理造成乱序。

### A) 发起会议（自然语言）
- 示例：
  - `@chairbot 召集 @bot2 @bot3 讨论「XXXX」。我想要的产出是：YYYY。`
  - `@chairbot 开个会：@bot2 负责 AAA；@bot3 负责 BBB。主题是：XXXX。`

### B) 预览确认（必须）
chairbot 必须先回一条 **预览卡片**，包含：
- participants（参会 agent 列表）
- agenda（1–3条议程）
- roles（每个 agent 的分工/立场）
- order + rounds（顺序与轮次）
- rules（严格轮询、模板、每轮每人 1 条、超时/字数上限）
- 退出条件（达到轮次/你结束/超时）

你回复（仍然 @chairbot）：
- `开始` / `取消` / `修改：...`

### C) 会议进行（严格轮询）
- chairbot 逐个触发参会 agent 发言（每次只触发一个 agent 出站一次）。
- 你的任何插话不会触发抢答，但会被记录并注入下一位发言提示（见 Chair Notes）。

### D) 导演权（你随时干预）
你对 chairbot 发自然语言指令：
- 插话纠偏：`@chairbot 我插一句：...`
- 改下一问：`@chairbot 下一位请 @bot3 回答：...`
- 暂停：`@chairbot 暂停`
- 继续：`@chairbot 继续`
- 结束：`@chairbot 结束会议`

chairbot 行为冻结：
- 记录 `Chair Note #n`
- 不立即触发其它 agent 抢答
- 下一次点名发言时，必须将 “since last turn 的 Chair Notes” 注入 prompt，并要求回应/考虑

### E) 乱序点名（绕过 chairbot）如何处理
会议 RUNNING/PAUSED 时，如果有人（包括你）直接 `@bot2 ...`：
- 该 bot 不得直接发言（不触发 LLM）
- 回复固定提示（可节流）：`会议进行中，请通过 @chairbot 点名/插话。`
- 同时把该消息内容作为 Chair Note 记录（不浪费输入）

## 会议输出模板（Fixed Output Template）
每位 agent 每次发言必须输出（短、可汇总）：
1) **Position**：一句话立场/结论
2) **Reasoning**：2–4条要点（短句）
3) **Evidence**：0–2条可核对依据（可空）
4) **Ask**：给 chairbot 的一个追问/需要澄清点（可空）

并要求：总字数上限（例如 800–1200 chars），超长必须自截断。

## 实施要点（Implementation Outline）
### 建议实施计划（Milestones，推荐顺序）
> 目标：先把“秩序/可控/不乱”跑通，再做“全量记录与可回放”，最后再补“按能力集严格约束”。

#### Milestone 1：会议协议最小闭环（先控秩序）
- meeting registry + lock（按 `chat_id`）
- chairbot 自然语言触发 → 预览 → 确认
- 严格轮询执行（一次只触发一个 speaker）
- Director Control：Chair Notes（插话/纠偏/改下一问/暂停/继续/结束）
- out-of-turn 治理（提示 + 记 note + 节流）

#### Milestone 2：会议派生 session + 全量记录 fan-out（你已选择）
- 创建 `#meeting=<id>` 派生 session（每个参会 agent 一份）
- 会议过程（你/预览/每轮发言/summary）全量写入所有参会派生 session
- （可选）结束后仅把 summary/行动项同步回日常群 session（不复制全量过程）

#### Milestone 3：按 agent 能力集严格约束（后续增强）
- per-agent capabilities（工具 allow/deny、sandbox/web 等）并在会议触发发言时强制执行
- 会议预览或 `/context` 展示 effective capabilities（便于排障）

### Phase 1：最小可用会议（先控秩序）
1) 新增 meeting registry（in-memory）
   - key：`chat_id`（同群单会议锁）
   - state：`IDLE/PLANNING/READY/RUNNING/PAUSED/SUMMARY/ENDED`
2) chairbot 专用 handler（Telegram inbound）
   - 识别 `@chairbot` 自然语言 meeting 触发（关键词 + mention entity）
   - 生成 MeetingPlan（PLANNING）
   - 发送预览（READY）
   - 解析确认（开始/取消/修改）
3) 严格轮询执行（RUNNING）
   - chairbot 依次触发参会 agent 发言（每次 1 个）
   - enforce：非当前 speaker 一律禁止发言（提示 + Chair Note）
4) 导演权（Chair Notes）
   - 用户插话写入 notes
   - notes 注入下一轮 prompt
5) 结束与总结
   - chairbot 输出总结 + 行动项 + @你

### Phase 2：会议派生 session + 全量记录 fan-out（你已选择）
1) 为每个参会 agent 创建会议派生 session（label：`Meeting <id> (<chat_id>)`）
2) 每条会议相关消息（你的输入、chairbot预览、每位agent发言、summary）：
   - 写入每个参会 agent 的会议派生 session（全量、带来源标记与 metadata）
3) 可选：会议结束后把 summary 同步回日常群 session（避免复制全量过程）

### Phase 3：per-agent 配置（能力集 + 风格）【依赖后续单子】
- agent 的 profile/capabilities 独立配置与强制执行（对用户仍只称“agent”）
- cross-ref：`issues/issue-named-personas-and-per-session-agent-profiles.md`（需按“agent”口径改写）

## 可实施性评估（Feasibility）
### 已具备的实施基础
- 群聊点名规则与 ingest-only 写入（旁听）已经具备（见前置 DONE issues）。
- gateway 已具备：
  - `chat.send` 的 run 序列化（session semaphore）
  - channel reply targets 与错误回执/drain（避免串线）
  - session_metadata 可创建/label/parent_session_key

### 仍需补齐的关键缺口（本单覆盖）
- meeting registry（chat_id 锁 + 状态机）
- chairbot handler：自然语言解析 → 预览 → 确认 → 轮询触发
- 轮询触发的“内部调度”：chairbot 能触发 bot2/bot3 各自发言（不依赖 Telegram bot-to-bot update）
- out-of-turn 抑制：会议期间禁止非当前 speaker 发言（提示节流 + 记录为 Chair Notes）
- 会议派生 session 的统一写入与 fan-out（全量过程写入）

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 入口：只有 `@chairbot` 才能发起会议；其他 bot 不会误触发开会。
- [ ] 预览确认：chairbot 必须先给预览；未确认不开始轮询。
- [ ] 严格轮询：会议中每次只允许一个 speaker 发言；其他 bot 不得抢答。
- [ ] 导演权：你插话会被记录为 Chair Note，并在下一位发言 prompt 中强制体现。
- [ ] 暂停/继续/结束：`暂停` 后不再触发任何发言；`继续` 恢复；`结束会议` 后释放锁。
- [ ] 全量记录：会议过程写入所有参会 agent 的会议派生 session；日常群 session 不被污染。
- [ ] 乱序点名：会议中直接 `@bot2 ...` 会收到固定提示且该内容被记录为 Chair Note（不触发 bot2 发言）。

## 测试计划（Test Plan）【不可省略】
### Unit（建议优先 gateway 层 mock）
- [ ] meeting registry：同 chat_id 只能一个活跃会议；结束释放锁。
- [ ] preview/confirm：未确认不进入 RUNNING；修改会重新生成预览。
- [ ] strict turn：非当前 speaker 的触发被拒绝（提示节流 + 记录 note）。
- [ ] chair notes：note 注入下一轮 prompt 的字段存在且顺序正确。
- [ ] derived session：为每个参会 agent 创建 `#meeting=<id>` 派生 session，且写入 fan-out 正确。

### Integration（可选）
- [ ] Telegram 手工：真实群内开会流程跑通（含插话/暂停/结束）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认关闭（仅 chairbot account 启用会议功能，避免影响现有 bots）。
- 回滚策略：禁用 chairbot meeting 功能即可；不影响普通聊天与现有会话格式。

## 交叉引用（Cross References）
- `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`（若后续需要“非会议”场景的 bot-to-bot 可见性补偿）
- `issues/issue-terminology-and-concept-convergence.md`（术语收敛：agent/account/session）
- `issues/issue-named-personas-and-per-session-agent-profiles.md`（按“agent”口径重写为内部 profile/capabilities 实现细节）

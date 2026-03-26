# Issue: Telegram 入/出站失败处理、静默失败与不可恢复故障治理（inbound / outbound / recoverability）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-15
- Owners:
- Components: telegram / gateway / channels / media
- Affected providers/models: 所有通过 Telegram 通道接收入站消息或回执出站结果的 agent run
- 实施准备结论：
  - 已具备开工条件：**是（针对现网 TG 主回复链路的 `run_scoped_typing`、失败观测与测试补齐）**
  - 当前阻塞项：**无**
  - 非阻塞遗留项：未来若把 TG 主回复切到 `send_stream_with_transport(...)`，其 typing lifecycle 仍需单独对齐 `run_scoped_typing`

**已实现（如有，写日期）**
- Telegram 文本出站与流式文本路径已具备统一的失败分类、有限次自动重试、`MessageNotModified` 成功等价收敛，以及结构化 retry/give-up 日志：`crates/telegram/src/outbound.rs:342`、`crates/telegram/src/outbound.rs:399`、`crates/telegram/src/outbound.rs:1362`
- Telegram polling loop 已具备连续失败聚合告警、恢复日志，以及与“其他实例占用同一 token”冲突时的自动 disable 请求：`crates/telegram/src/bot.rs:150`、`crates/telegram/src/bot.rs:238`
- DM allowlist 的空列表拒绝语义已被回归测试冻结，避免 Telegram DM 被误放开：`crates/telegram/src/access.rs:175`、`crates/telegram/src/access.rs:195`
- 2026-03-15：polling loop offset 仅在 update 终态后推进，并引入 runtime-local retry budget + batch stop + quarantine：`crates/telegram/src/bot.rs:160`
- 2026-03-15：入站媒体下载改为继承 `Bot::set_api_url(...)` 的 client/base_url，并补齐 timeout + max_bytes + 脱敏 reason_code：`crates/telegram/src/handlers.rs:2678`
- 2026-03-15：callback query 先 `answer_callback_query` 清 spinner，并为 answer 失败补齐 reason_code + 可重试边界：`crates/telegram/src/handlers.rs:2261`
- 2026-03-15：probe connected 口径扩展为 `auth_ok + polling_liveness(stale/state)` 摘要（不再只看 `get_me`）：`crates/telegram/src/plugin.rs:206`
- 2026-03-15：出站 `reply_to` 解析失败显式 reason-coded degrade（不再 silent）：`crates/telegram/src/outbound.rs:401`
- 2026-03-15：出站 media caption 超长按 `TELEGRAM_CAPTION_LIMIT` 拆分（caption + follow-up text）：`crates/telegram/src/outbound.rs:813`
- 2026-03-15：gateway 限定 `ChannelAttachment` 为 image-only，非图片附件不再被编码成 `image_url`：`crates/gateway/src/channel_events.rs:688`
- 2026-03-15：gateway dispatch 失败用户回执文案脱敏（不再透传 `⚠️ {e}`）：`crates/gateway/src/channel_events.rs:1871`
- 2026-03-15：gateway 对任意包含非图片的附件集合统一直接回用户固定提示，不再降级成普通 chat dispatch，也不再允许“图片+非图片”混合时部分穿透：`crates/gateway/src/channel_events.rs:669`
- 2026-03-15：generic slash command / callback 的 `dispatch_command(...)` 失败补齐结构化日志：`crates/telegram/src/handlers.rs:998`、`crates/telegram/src/handlers.rs:2429`
- 2026-03-15：`polling_liveness` 至少要求一次成功 poll 后才视为 connected，避免冷启动误报：`crates/telegram/src/plugin.rs:251`
- 2026-03-15：现网 TG 主回复链路的 typing 生命周期已冻结并实现为 `run_scoped_typing`，覆盖 `dispatch_to_chat(...) -> chat.send(...)` 整次 run，且在失败回执发送完成前不会提前停止：`crates/gateway/src/channel_events.rs:86`、`crates/gateway/src/channel_events.rs:381`
- 2026-03-15：`send_stream_with_transport(...)` 已补齐持续 typing keepalive，并对齐到 `run_scoped_typing` 兼容口径，不再只在 placeholder 前单发一次 typing：`crates/telegram/src/outbound.rs:317`、`crates/telegram/src/outbound.rs:1627`
- 2026-03-15：typing keepalive loop 已改为与主执行并发推进，不再因单次 `send_typing` 卡顿而阻塞 `chat.send(...)` / stream placeholder send，也不再按“请求完成后再 +4s”产生额外节拍漂移：`crates/gateway/src/channel_events.rs:136`、`crates/telegram/src/outbound.rs:1779`
- 2026-03-15：Telegram `send_chat_action(typing)` 已加独立短超时边界；`send_typing()` / `send_reply(...)` 不再继承 45s client timeout 或吞掉 typing 失败：`crates/telegram/src/outbound.rs:31`、`crates/telegram/src/outbound.rs:341`、`crates/telegram/src/outbound.rs:1499`、`crates/telegram/src/outbound.rs:1603`
- 2026-03-15：slash command / callback 的慢路径已补齐 run-scoped typing，覆盖 `dispatch_command(...) + helper send / user feedback` 全程；内部文本回包改走 silent send，避免 wrapper typing 与 `send_text(...)` 双 owner 重复发 typing；callback 仍先 `answer_callback_query` 再进入 typing + follow-up：`crates/telegram/src/handlers.rs:67`、`crates/telegram/src/handlers.rs:919`、`crates/telegram/src/handlers.rs:2586`
- 2026-03-15：进一步修正 TG 主回复链路的 typing 根因：`chat.send(...)` 只是启动后台 run 并立即返回 `runId`，旧实现会在 run 真正结束前提前停掉 typing；现已补齐 `wait_run_completion(...)` 契约与 gateway 后台 typing watcher，使 typing 绑定真实 run completion，而不是只绑定启动阶段：`crates/gateway/src/services.rs:531`、`crates/gateway/src/chat.rs:1750`、`crates/gateway/src/channel_events.rs:176`

**已覆盖测试（如有）**
- 文本出站 retry/unknown outcome/stream degrade 已有单测覆盖：`crates/telegram/src/outbound.rs:1692`、`crates/telegram/src/outbound.rs:1875`、`crates/telegram/src/outbound.rs:1927`
- DM allowlist 安全回归已覆盖：`crates/telegram/src/access.rs:195`
- callback spinner 清理（无 data / missing account 仍会 answer）：`crates/telegram/src/handlers.rs:5195`
- callback answer 传输失败可重试边界（transport_failed_before_send → Retryable）：`crates/telegram/src/handlers.rs:5330`
- `event_sink` 缺失不再 silent drop（DM best-effort 回执）：`crates/telegram/src/handlers.rs:5492`
- 下载路径继承 bot api_url（无硬编码 `api.telegram.org`）：`crates/telegram/src/handlers.rs:5588`
- 下载失败错误脱敏（不包含 token / `file/bot<token>`）：`crates/telegram/src/handlers.rs:5353`
- probe liveness 派生语义：`crates/telegram/src/plugin.rs:481`
- image-only attachment boundary & dispatch 脱敏：`crates/gateway/src/channel_events.rs:1871`
- 非图片附件直接用户反馈且不再 dispatch 到 chat：`crates/gateway/src/channel_events.rs:1994`
- 混合图片+非图片附件也会整体拒绝，不再部分 dispatch：`crates/gateway/src/channel_events.rs:2080`
- addressed slash command 执行失败时仍返回脱敏用户文案：`crates/telegram/src/handlers.rs:4769`
- 现网 TG 主回复链路 `run_scoped_typing`：长耗时 run 保活、失败回执前不提前停止：`crates/gateway/src/channel_events.rs:2222`、`crates/gateway/src/channel_events.rs:2299`
- `send_stream_with_transport(...)` typing lifecycle 对齐验证：`crates/telegram/src/outbound.rs:2307`
- typing keepalive 不阻塞主执行：gateway chat dispatch / telegram stream placeholder 均有回归测试：`crates/gateway/src/channel_events.rs:2411`、`crates/telegram/src/outbound.rs:2399`
- `send_chat_action(typing)` 短超时边界与 `send_reply(...)` 错误传播已覆盖：`crates/telegram/src/outbound.rs:2028`、`crates/telegram/src/outbound.rs:2041`、`crates/telegram/src/outbound.rs:2070`
- callback / addressed slash command 慢路径 typing 已覆盖，且冻结为单一 typing owner：`crates/telegram/src/handlers.rs:5711`、`crates/telegram/src/handlers.rs:5087`
- `chat.send(...)` 后台 run completion wait + gateway 背景 typing watcher 已覆盖：`crates/gateway/src/chat.rs:11307`、`crates/gateway/src/channel_events.rs:2437`

**已知差异/后续优化（非阻塞）**
- 已完成的 `issue-telegram-outbound-retry-policy-and-send-failure-recovery.md` 主要覆盖“文本 reply/流式文本 reply 的 retry 语义”，未覆盖本单的入站 ack 语义、下载安全、callback spinner、helper 静默失败、媒体/位置路径一致性。
- 已完成的 `issue-observability-llm-and-telegram-timeouts-retries.md` 主要覆盖“故障观测性增强”，未冻结本单的“哪些失败必须用户可见、哪些失败必须避免不可恢复丢消息”。
- 本单冻结的是“可靠性口径”；以下遗留问题先列为后续非阻塞优化：
  - 这些项均不阻塞本单 Phase 0/1；建议按“先落可靠性主干，再基于线上反馈或真实接入需要单独拆单”处理。
  - `probe` 结果当前有 `30s` cache，状态变化不会实时外显：`crates/telegram/src/plugin.rs:28`
    - 建议：先不调整 cache；待 `polling_liveness` 基础状态与摘要落地后，再依据 UI 迟滞体感决定是否单独收紧到更短 TTL 或增加主动刷新。
  - 当前 TG 主回复链路仍是“run 结束后统一回 Telegram”，而不是边收 delta 边 edit：`crates/gateway/src/chat.rs:6167`、`crates/gateway/src/chat.rs:6297`
    - 建议：保持现状即可；这不是可靠性缺陷。typing 已按 `run_scoped_typing` 覆盖整次 run。
  - 当 `chat.send(...)` 因同 session 已有活跃 run 而直接返回 `queued=true`、且未返回 `runId` 时，当前 gateway 只能记录 `telegram.typing.skipped reason_code=queued_without_run_id`，无法把 typing 严格延展到“排队等待 + 后继 replay run”整段生命周期。
    - 建议：单开后续小单，把 queued followup/collect replay 也纳入显式 lifecycle handle；当前不阻塞“单次已启动 run 的 typing 提前终止”这条主缺陷修复。
  - 若未来把 TG 主回复切到原生 stream outbound，仍需单独决定 typing loop owner（gateway 还是 Telegram outbound），避免双 loop 或 stop 条件分裂。
  - `prepare -> plan -> execute` 与 `InboundInteraction / ExecutionPlan / UpdateOutcome + retry_barrier` 目前仍以规范口径存在，尚未强制 struct 化落地。
    - 建议：本单作为可靠性治理单到此收口；若后续要继续降复杂度与扩展新媒体类型，再单开“Telegram adapter spine 重构”子单。

---

## 背景（Background）
- 场景：Telegram bot 通过 long polling 接收 `Message / EditedMessage / CallbackQuery`，并在 handler 中处理文本、语音、图片、位置、OTP、slash command、inline keyboard callback；随后经 `TelegramOutbound` 发送文本、流式文本、媒体、位置等回执。
- 约束：
  - Telegram update offset 具有“确认消费”语义，推进过早会扩大不可恢复丢消息窗口。
  - Telegram callback query 若不及时 `answer_callback_query`，客户端会持续显示 loading spinner。
  - Telegram file download URL 自带 bot token，任何错误日志若包含原始 URL 都属于敏感信息泄露。
  - 当前测试与潜在自定义 Telegram API endpoint 依赖 `Bot::set_api_url(...)`；下载路径若绕过 bot 自身的 `api_url/client`，不仅有 secret 风险，也会破坏 mock/self-hosted endpoint 语义。
  - 当前 channel 抽象里的 `ChannelAttachment` 只有 `media_type + data`，而 gateway 现状会把 attachment 统一编码成 `image_url` 发给模型；因此现状并不是真正的“通用附件协议”，而是“名义通用、实际偏 image-only”。
  - 当前 outbound `ReplyPayload` 只有单个 `media` 字段，说明 Telegram 出站现阶段天然更接近“单主媒体 + 文本/位置/typing/callback answer”的能力模型，而不是“任意多附件组合发送”。
  - 当前 `ChannelHealthSnapshot` 外部 surface 只有 `connected/details`；本单先冻结 `polling_liveness` 监督口径与 `probe()` 脱敏摘要，不把更细 UI 字段扩展当成 P0 blocker。
  - 现有系统没有 durable inbox/outbox；因此本单必须在“不承诺 exactly-once”的前提下，把“静默失败、无反馈、不可恢复故障”风险降到可控。
- Out of scope：
  - 不在本单内承诺跨进程 durable outbox / exactly-once delivery。
  - 不重做 Telegram relay/group transcript 主协议。
  - 不把所有 group 场景都改成强用户可见错误提示，避免公共群噪声失控。
  - 不在 Phase 0/1 里直接把跨 channel 的 `ChannelAttachment` / `ReplyPayload` 升级成完整的通用多部件协议；本单先在 Telegram 适配层把能力矩阵和扩展挂点冻结清楚。
  - 不在本单内直接实现 document/video/sticker/pdf OCR 等新增能力，但必须明确这些附件类型在现阶段的 unsupported 行为与未来扩展位置。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Telegram update ack**（主称呼）：long-poll 客户端将 `offset` 推进到某个 update 之后，Telegram 服务器据此认为更早的 update 已经被消费。
  - Why：这是“是否还能重投递”的边界。
  - Not：不是 handler 真正完成全部业务处理的证明。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：offset advance / acknowledged update

- **静默失败**（主称呼）：系统命中了失败路径，但用户、调用方或运维无法收到明确错误反馈，或者反馈被吞掉后只能靠猜。
  - Why：这是 Telegram 侧当前体验和排障成本的核心问题。
  - Not：不是“故意静默的策略”本身；故意静默若无 reason code 也视为观测缺口。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：silent failure / swallowed error

- **不可恢复故障**（主称呼）：一旦触发，单靠常规重试无法挽回先前输入/输出语义，或者已经造成安全事故（例如 secret 泄露）。
  - Why：需要最高优先级治理。
  - Not：不是所有临时失败都属于不可恢复。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：irrecoverable failure

- **用户可见反馈**（主称呼）：用户在 Telegram 客户端能直接看到的错误提示、toast、占位更新或明确回执。
  - Why：决定“失败后用户是知道出错了，还是误判系统没反应”。
  - Not：不是后台结构化日志。
  - Source/Method：[as-sent]
  - Aliases（仅记录，不在正文使用）：user-visible feedback

- **主动交互路径**（主称呼）：用户明确期待 bot 立即给出反馈的 Telegram 交互，包括 DM 对话、被 addressed 的群消息、slash command、callback、待处理 location request 等。
  - Why：决定失败时是否必须回用户可见反馈。
  - Not：不是 listen-only/group transcript 这类纯被动摄入路径。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：active path / interactive path

- **路由类型**（主称呼）：适配层对单条入站交互最终选择的执行去向，只允许 `chat / command / ingest_only / feedback_only` 四类。
  - Why：把“这条 update 最后要去哪里”收敛成一处决定，避免多个中间 mode 字段、局部 return、helper send 共同决策。
  - Not：不是 Telegram 原生 update kind，也不是用户输入类型。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：route kind / execution route

- **普通消息**（主称呼）：Telegram `Message / EditedMessage` 入站事件，承载文本、媒体、位置、slash command 等原始用户输入。
  - Why：这是 LLM chat、媒体下载、OTP、slash command 拦截的共同入口。
  - Not：不是按钮点击后的 callback 事件。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：message update / inbound message

- **slash command**（主称呼）：以 `/` 开头、但底层仍属于 `Message` 的 Telegram 命令输入；在本系统里分为“本地命令处理”与“普通消息继续 dispatch”两类。
  - Why：它和 callback 都是“主动交互”，但失败面和用户反馈方式不同。
  - Not：不是独立的 Telegram update kind。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：command message

- **callback**（主称呼）：用户点击 inline keyboard 按钮后产生的 Telegram `CallbackQuery` 事件。
  - Why：它有独立的客户端 spinner 生命周期，必须用 `answer_callback_query(...)` 显式结束。
  - Not：不是普通消息，也不是 slash command 本体。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：callback query / button click event

- **InboundInteraction**（主称呼）：系统已经理解好的一条 Telegram 入站交互，包含交互类别、是否属于主动交互、reply 目标，以及已经准备好的入站部件。
  - Why：这样后面的路由、失败处理、用户反馈就不再直接依赖原始 `Update` 细节。
  - Not：不是 Telegram 原始 update；也不是最终执行计划。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：inbound envelope / normalized interaction

- **入站部件**（主称呼）：把 Telegram 原始消息内容拆成统一的逻辑部件，例如 `text / image / audio_for_stt / location / unsupported_attachment`。
  - Why：只有先把“内容本体”归一化，后续 retry、用户反馈、LLM dispatch 才能统一。
  - Not：不是 Telegram 原生 `MediaKind` 的一比一镜像；它是适配层内部语义。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：input part / content part

- **prepare**（主称呼）：把 `file_id`、caption、location、callback data 等原始 Telegram 字段，转换成可供 dispatch 或本地处理的稳定输入的过程。
  - Why：下载、转写、图片优化、unsupported 判定都应在这一层收口。
  - Not：不是最终的 LLM dispatch，也不是 update ack 提交。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：preparation / resolve stage

- **ExecutionPlan**（主称呼）：针对一条 `InboundInteraction` 的唯一执行计划，负责一次性决定路由、barrier 前后动作以及最终 Telegram 出站请求。
  - Why：把“这条 update 到底怎么处理”集中到一处，避免 helper、callback、媒体分支各自临时决定。
  - Not：不是某个局部分支里的临时变量，也不是 polling loop 的 ack 结果。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：plan / execution route

- **retry_barrier**（主称呼）：一旦越过，就不再允许该 update 回到 `RetryableFailure` 的边界。
  - Why：这是整条 update 能否重试的唯一判定口径。
  - Not：不包括 callback 空 answer、typing、结构化日志这类 barrier 前动作；它们失败可以观测，但不代表本次交互已经提交。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：retry barrier / irreversible effect

- **渠道副作用**（主称呼）：适配层对 Telegram 客户端直接产生的动作，包括 `answer_callback_query`、typing、helper 发送、用户错误提示、最终回复。
  - Why：callback spinner、typing、最小错误反馈都属于这一层，而不是 `prepare` 本身。
  - Not：不是 gateway 内部 session 持久化或模型推理。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：channel effect / side effect

- **UpdateOutcome**（主称呼）：polling loop 对单条 update 唯一接受的处理结果，只允许 `AckSuccess / AckTerminal(reason_code) / RetryableFailure(reason_code)` 三种。
  - Why：ack、quarantine、batch stop 都必须围绕这个结果收口。
  - Not：不是中间态，也不是任意 `Result<(), E>`。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：inbound outcome / handler outcome

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] Telegram inbound 不得在尚未完成 handler 尝试前就推进 update ack，避免把“进程崩溃/任务被杀/handler panic”变成不可恢复丢消息。
- [x] Telegram file download 失败不得泄露 token、完整 file URL 或其他敏感信息。
- [x] Telegram file download 必须继承 bot 当前的 `api_url/client` 语义，不得硬编码到 `api.telegram.org`。
- [x] callback、command helper、OTP、media/location 等非文本主路径，必须具备和文本路径同等级别的失败观测与最小用户反馈策略。
- [x] helper 层的 `send_message` 失败不得再被 `let _ = ...` 静默吞掉；失败必须至少有结构化日志与 reason code。
- [x] `RetryableFailure` 必须冻结为“当前批次立即停止、不得继续处理同批后续 updates”的显式语义。
- [x] Telegram 用户可见错误文案不得直接回显原始 `Error: {e}` / anyhow / teloxide 错误串。
- [x] 会进入 chat/LLM 回复的主动路径，应 best-effort 提供 Telegram typing indicator；现网主链路以 `run_scoped_typing` 为准，typing send 失败不得静默伪装成成功。
- [x] Telegram channel health 不得只用 `get_me` 成功来表示“通道健康”，需要能反映 polling loop 是否仍在工作。
- [x] Telegram 适配层的可靠性主语义已冻结为“`prepare -> plan -> execute`”三段口径；后续若做 struct 化重构，不得再新增绕开该语义的独立失败分支。
- [x] 同一 Telegram account 的 polling loop 在本单 Phase 0/1 范围内必须保持按 `update_id` 顺序的串行处理；本单不引入同账号 update 并发执行。
- [x] Telegram capability matrix 已冻结为少量归一化类型口径，而不是继续直接绑定每个 Telegram `MediaKind` 分支。
- [x] 对当前 gateway 边界，`ChannelAttachment` 语义必须显式收窄为 image-only；非图片类附件不得再通过 `dispatch_to_chat_with_attachments(...)` 被隐式当成 `image_url` 发送给模型。
- [x] 主动交互路径上遇到 unsupported attachment 时，不得静默丢弃，也不得只把 caption 当纯文本继续 dispatch。
- [x] Telegram 出站的最小统一能力模型已在本单规范冻结为 `thread_target / delivery / content` 三个维度；后续若继续 struct 化实现，不得偏离该口径。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须区分“故意静默策略”与“错误被吞掉”。
  - 必须区分“明确未送达”“结果未知”“已成功但回包失败”。
  - 必须冻结“pre-dispatch 可重试 / post-dispatch 终态 ack”的边界，避免实现阶段自由发挥。
  - `reason_code` 必须是稳定、短小、可枚举的 `snake_case` 标识，不得把原始错误串直接当成 `reason_code`。
  - `event` 负责表达“哪一类动作失败/降级”，`reason_code` 负责表达“失败/降级的直接原因”；二者不得混用。
  - update 级 retry budget 在本单中必须明确为“runtime-local、best-effort”的预算，而不是跨重启持久化语义。
  - 不得把 token、完整 file URL、完整正文、二进制内容写入日志。
  - 不得把原始内部错误字符串直接回给 Telegram 用户。
  - 不得用“向 `ChatId(0)` 发送消息失败后静默结束”代替真实错误处理。
  - 不得把 unsupported attachment 混入现有 image-only multimodal 路径。
- 兼容性：优先做最小增量修复，不改变 Telegram relay/session 的主业务语义。
- 可观测性：所有“命中候选但被策略拦截/降级/吞掉”的路径，必须有 reason code；日志低噪声，必要时做去重或限频。
- 安全与隐私：bot token、完整下载 URL、用户原始正文、音频/图片原始内容一律不得直接进入日志。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) long polling 已收到 update，但如果进程在 handler 完成前崩溃，该 update 可能已被 ack，之后不会再收到。
2) callback query 在 `data == None`、`chat_id` 缺失、account state 缺失、answer 发送失败等部分路径没有及时且可观测地 `answer_callback_query`，Telegram 客户端会持续转圈。
3) callback/command 当前多处直接把原始 `Error: {e}` 回显给 Telegram 用户，既不稳定，也可能泄露内部实现细节。
4) slash command 的辅助发送、sessions/context/model/sandbox 键盘发送、OTP 发送等路径中大量存在 `let _ = bot.send_message(...).await`，失败后用户无感知、日志无结构化 reason code。
5) 语音/图片下载走手工拼接的 Telegram file URL；一旦下载失败，错误字符串可能把 token 带进日志。
6) 语音/图片下载当前绕过 bot 自身的 `api_url/client`，会与测试 mock endpoint、自托管 Telegram API endpoint 语义不一致。
7) 文本路径已有 retry 与结构化日志，但媒体、位置、caption 超长等路径仍沿用“单次调用 + 失败上抛或吞掉”的旧语义；入站媒体/STT 失败还会继续以 caption/占位文本 dispatch，用户感知与模型语义都不稳定。
8) TG 主回复链路当前理论上应由 gateway typing loop 覆盖整个 `chat.send(...)` 生命周期，但用户实测仍可能在长时间 run 中看不到稳定的 typing indicator；即使 loop 存在，`send_chat_action` 失败当前也可能观测不足，导致“看起来没在输入”且排障无据。
9) `probe()` 用 `get_me()` 代表健康；polling loop 即使已经退出，UI 仍可能显示 connected，形成“看起来在线，实际上不收消息”的静默故障。
10) `handlers.rs` 目前同时承担 update 分类、access gating、媒体下载/转写、command/callback/OTP、本地 helper 发送、LLM dispatch 与用户反馈；新增一个 Telegram 媒体类型往往要改动多处，并复制一套新的失败分支。
11) `ChannelAttachment` 名义上像“通用附件”，但 gateway 现状会把 attachment 统一编码成 `image_url`；而 Telegram 入站实际上只对 photo 真正填充 attachment，document/video/sticker 等要么只记日志后静默丢弃，要么在带 caption 时只 dispatch caption，附件本体被忽略。
12) gateway `dispatch_to_chat(...)` / `dispatch_to_chat_with_attachments(...)` 失败时会把原始 `⚠️ {e}` 直接回给 Telegram 用户，说明“对用户的错误文案脱敏”并没有在 channel 适配层统一收口。
13) 流式 reply 的 `StreamEvent::Error` 当前会直接拼进 Telegram 用户可见文本；截图/媒体发送失败等路径也还存在原始内部错误直出给用户的情况。
14) Telegram 出站在 `thread_target / silent / typing` 这些通用语义上并不一致：text 路径有较完整语义，media/location/stream 则存在静默忽略、未对齐或不可观测的漂移。
15) `send_stream_with_transport(...)` 这条 Telegram 原生 stream outbound 路径当前只会单发一次 typing，紧接着发送占位消息 `"…"`；若后续接入现网主回复链路，则长推理阶段 Telegram 客户端通常不再显示“正在输入”。

### 影响（Impact）
  - 用户体验：
    - 群里或 DM 里看到的是“没反应”“一直转圈”“看起来在线但消息不处理”，而不是明确故障提示。
    - command/callback/helper 失败时，用户无法判断是 Telegram 发送失败、解析失败，还是后端没执行；部分路径还会看到未经脱敏的内部报错字符串。
    - 当一次 LLM run 包含多轮推理/工具调用/再推理时，若 TG 客户端中途失去 typing 提示，用户无法判断系统仍在执行还是已经卡死。
- 可靠性：
  - update pre-ack 会把某些输入丢失变成不可恢复。
  - 若简单把 ack 推迟到 handler 结束，但不定义 retry/quarantine 语义，单条 poison update 又可能阻塞后续 update。
  - helper 吞错与 callback 未应答会把暂时性失败放大成“无响应”。
  - 媒体/位置路径与文本路径的可靠性能力不一致，导致同一通道不同消息类型的故障表现割裂。
  - document/video/sticker 等附件当前没有进入统一 capability matrix，扩展时非常容易踩穿隐藏的 image-only 假设。
- 安全：
  - token 泄露到日志一旦发生，必须轮换 bot token，属于高代价事故。
- 排障成本：
  - 现有日志在文本主路径之外缺少 reason code 和上下文字段，只能靠时间线猜。
  - 因为“交互分类 / `prepare` / 用户反馈 / ack”没有分层，很多问题只能在大函数里逆向推断，变更影响面难以收敛。

### 复现步骤（Reproduction）
1. 让 `get_updates()` 返回某条 Telegram message update，然后在 `handle_message_direct(...)` 执行期间杀掉进程。
2. 观察下一次启动后该 message 是否仍会被 Telegram 重投递。
3. 触发 inline keyboard callback，但让 callback query 只带 `inline_message_id` 或让后续 `chat_id` 解析失败。
4. 观察 Telegram 客户端 spinner 是否被消掉，以及是否有用户可见错误反馈。

补充复现：
1. 发送一条语音或图片消息，并人为让 Telegram file download 失败（超时、DNS、500）。
2. 检查日志是否包含原始 token/file URL；检查用户看到的是明确错误还是无反馈/占位文本。
3. 发送一个不带 caption 的 document/video/sticker。
4. 观察系统是否只打日志后静默结束，用户是否拿不到任何“当前不支持该附件类型”的反馈。
5. 发送一个“带 caption 的 unsupported attachment”（例如带 caption 的 document）。
6. 观察系统是否错误地只 dispatch caption，而把真正的附件本体完全忽略。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。
> 注：本段记录的是 SURVEY 阶段的 as-is 证据，其中部分问题已在 2026-03-15 修复；以本文档顶部的“已实现/已覆盖测试”与勾选项为准。

  - 代码证据：
  - `crates/telegram/src/bot.rs:123`：polling loop 通过 `tokio::spawn` 后与 `start_account()` 解耦，缺少存活监督。
  - `crates/telegram/src/bot.rs:172`：`offset = update.id.as_offset()` 在 handler 执行前推进。
  - `crates/telegram/src/bot.rs:180`：message handler 报错仅记录 `error!`，无最小用户回执或结构化 reason code。
  - `crates/telegram/src/bot.rs:229`：非预期 update kind 直接 ignore，但由于 offset 已提前推进，实际上等价于终态 ack 丢弃。
  - `crates/telegram/src/handlers.rs:329`、`crates/telegram/src/handlers.rs:385`：语音/图片下载都依赖 `download_telegram_file(...)`。
  - `crates/telegram/src/handlers.rs:2166`：`download_telegram_file(...)` 手工拼接 `https://api.telegram.org/file/bot{token}/...`，且使用独立 `reqwest::get`，无 timeout/大小限制/脱敏。
  - `crates/telegram/src/handlers.rs:355`、`crates/telegram/src/handlers.rs:367`、`crates/telegram/src/handlers.rs:427`：下载/转写失败日志直接输出 `%e`。
  - `crates/telegram/src/handlers.rs:3187`、`crates/telegram/src/handlers.rs:3389`、`crates/telegram/src/handlers.rs:3853`、`crates/telegram/src/handlers.rs:4034`、`crates/telegram/src/handlers.rs:4137`：handlers 测试明确依赖 `Bot::set_api_url(...)`，说明下载路径应继承 bot 的 API endpoint 语义。
  - `crates/telegram/src/handlers.rs:1862`、`crates/telegram/src/handlers.rs:1866`、`crates/telegram/src/handlers.rs:1908`：callback handler 传入的 `_bot: &Bot` 未使用，`data == None` 与 account missing 都可直接 `return Ok(())`，导致 spinner 无法保证被清掉。
  - `crates/telegram/src/handlers.rs:598`、`crates/telegram/src/handlers.rs:623`、`crates/telegram/src/handlers.rs:648`、`crates/telegram/src/handlers.rs:675`：command 错误回包使用 `ChatId(parse().unwrap_or(0))` 且吞掉 send error。
  - `crates/telegram/src/handlers.rs:601`、`crates/telegram/src/handlers.rs:626`、`crates/telegram/src/handlers.rs:651`、`crates/telegram/src/handlers.rs:678`、`crates/telegram/src/handlers.rs:692`、`crates/telegram/src/handlers.rs:1942`、`crates/telegram/src/handlers.rs:1960`：command/callback 多处直接把 `Error: {e}` 暴露给 Telegram 用户。
  - `crates/telegram/src/handlers.rs:1155`、`crates/telegram/src/handlers.rs:1202`、`crates/telegram/src/handlers.rs:1741`、`crates/telegram/src/handlers.rs:1799`：sessions/context/model/sandbox helper 都用 `unwrap_or(0)` 解析 chat_id。
  - `crates/telegram/src/handlers.rs:1180`、`crates/telegram/src/handlers.rs:1207`、`crates/telegram/src/handlers.rs:1780`、`crates/telegram/src/handlers.rs:1791`、`crates/telegram/src/handlers.rs:1847`：helper send 失败被 `let _ = ...await` 吞掉。
  - `crates/telegram/src/handlers.rs:556`、`crates/telegram/src/handlers.rs:560`：group 未 addressed 的 slash command、DM 中发给其他 bot 的 command 都是 silent ignore，目前未统一打 reason code。
  - `crates/telegram/src/handlers.rs:1896` 到 `crates/telegram/src/handlers.rs:1903`：callback query 在 `chat_id` 为空时直接 `return Ok(())`，未 `answer_callback_query`。
  - `crates/telegram/src/handlers.rs:1919`：callback reply target 固定 `message_id=None`，意味着后续用户可见回包依赖单独消息而非原地反馈。
  - `crates/telegram/src/handlers.rs:1929`、`crates/telegram/src/handlers.rs:1965`：callback 的 `answer_callback_query(...)` 失败被直接吞掉，当前没有结构化日志或 retry 语义。
  - `crates/telegram/src/handlers.rs:556`、`crates/telegram/src/handlers.rs:562`、`crates/telegram/src/handlers.rs:916`、`crates/telegram/src/handlers.rs:1074`、`crates/telegram/src/handlers.rs:1890`：存在多类 intentional silent return，目前未统一收敛为 reason-coded observability。
  - `crates/telegram/src/handlers.rs:915` 到 `crates/telegram/src/handlers.rs:917`：OTP pending 但用户发的不是 6 位码时直接 silent ignore。
  - `crates/telegram/src/handlers.rs:1073` 到 `crates/telegram/src/handlers.rs:1075`：`OtpInitResult::AlreadyPending | LockedOut` silent ignore。
  - `crates/telegram/src/handlers.rs:961`、`crates/telegram/src/handlers.rs:980`、`crates/telegram/src/handlers.rs:1000`、`crates/telegram/src/handlers.rs:1044`：OTP challenge / wrong code / locked out / expired 等主动发送路径同样用 `let _ =` 吞发送失败。
  - `crates/telegram/src/handlers.rs:513`：`event_sink == None` 时，候选入站消息会直接跳过 dispatch，当前没有显式 reason code 或用户反馈策略。
  - `crates/telegram/src/handlers.rs:500`：document/video/sticker 等 unhandled attachment 当前只记一条 info 日志。
  - `crates/telegram/src/handlers.rs:512`：只有 `body` 或 `attachments` 非空才会继续 dispatch，因此“无 caption 的 unsupported attachment”最终会静默结束。
  - `crates/telegram/src/handlers.rs:540`：slash command 拦截和普通消息 dispatch 逻辑混在同一大函数内，交互分类与后续副作用没有独立边界。
  - `crates/telegram/src/outbound.rs:813`：媒体发送路径直接构造 `send_photo/send_document/send_voice/send_audio`，没有沿用统一 retry helper。
  - `crates/telegram/src/outbound.rs:1007` 到 `crates/telegram/src/outbound.rs:1114`：URL 媒体与 location 路径均为单次 `req.await?`。
  - `crates/telegram/src/outbound.rs:1197` 到 `crates/telegram/src/outbound.rs:1201`：Telegram `send_typing(...)` 把 `send_chat_action(...).await` 结果直接 `let _ =` 吞掉，并始终返回 `Ok(())`。
  - `crates/telegram/src/outbound.rs:1131`：`reply_to` 解析失败会静默退化为“不 thread reply”，当前没有 reason code。
  - `crates/telegram/src/markdown.rs:238`：caption limit 常量已定义，但 `send_media_inner(...)` 未使用。
  - `crates/telegram/src/outbound.rs:1409` 到 `crates/telegram/src/outbound.rs:1447`：流式 edit 失败会降级记日志，但用户侧没有固定 fallback 语义。
  - `crates/telegram/src/outbound.rs:1455`：流式 `StreamEvent::Error(e)` 会把原始内部错误文本直接拼进 Telegram 用户可见消息。
  - `crates/telegram/src/outbound.rs:813`、`crates/telegram/src/outbound.rs:1078`：media/location 路径未和文本路径对齐 `silent` / typing / retry 语义。
  - `crates/gateway/src/channel_events.rs:287` 到 `crates/gateway/src/channel_events.rs:324`：`dispatch_to_chat(...)` 会在 `chat.send(params).await` 整个期间维持一个每 `4s` 触发一次的 typing loop，说明现网主链路的设计目标本来就是 run-scoped typing。
  - `crates/gateway/src/chat.rs:2730` 到 `crates/gateway/src/chat.rs:2753`：`LiveChatService::send()` 会同步等待 `run_streaming(...)` 或 `run_with_tools(...)` 整轮完成，而不是仅触发后立即返回。
  - `crates/gateway/src/chat.rs:5050`、`crates/gateway/src/chat.rs:5299`：带工具的 agent run 也是在 `run_agent_loop_streaming(...)` 完成后才统一 `deliver_channel_replies(...)`。
  - `crates/gateway/src/chat.rs:5987` 到 `crates/gateway/src/chat.rs:6010`：provider delta 当前只广播给 Web UI；Telegram 主回复并不是边收 delta 边回发。
  - `crates/gateway/src/chat.rs:6167`、`crates/gateway/src/chat.rs:6297`：Telegram 主回复当前是在整次 run 结束后，统一通过 `deliver_channel_replies(...)` 回发。
  - `crates/gateway/src/channel_events.rs:300`、`crates/gateway/src/channel_events.rs:887`：gateway 的 typing loop 依赖 `send_typing()` 返回 `Err` 才能记录失败，但 Telegram 实现当前永远返回 `Ok(())`，导致 typing 失败不可观测。
  - `crates/telegram/src/outbound.rs:1561`：stream 路径仅在发送占位消息前单次调用 `send_typing`，没有像 `dispatch_to_chat(...)` 那样维持持续 typing loop。
  - `crates/telegram/src/outbound.rs:1564`：stream 路径发送 `"…"` 占位消息后改用 edit 流更新文本；这会让“长推理期间持续呈现 typing”的用户体验依赖于额外保活机制，而当前实现未提供。
  - `crates/channels/src/plugin.rs:167`、`crates/channels/src/plugin.rs:211`：跨 channel 的 inbound attachment contract 只有 `media_type + data`，没有 asset kind / capability / fallback 语义。
  - `crates/gateway/src/channel_events.rs:715`：gateway `dispatch_to_chat_with_attachments(...)` 把 attachment 一律编码为 `image_url`，说明当前 attachment 边界实际是 image-only。
  - `crates/common/src/types.rs:45`：出站 `ReplyPayload` 只支持单个 `media`，没有多附件组合能力。
  - `crates/gateway/src/channel_events.rs:344`、`crates/gateway/src/channel_events.rs:906`：gateway dispatch 失败时把原始 `⚠️ {e}` 直接回给 channel 用户。
  - `crates/gateway/src/chat.rs:8082`：截图发送失败仍会把原始 `Failed to send screenshot: {e}` 回给 Telegram 用户。
  - `crates/telegram/src/plugin.rs:205` 到 `crates/telegram/src/plugin.rs:247`：`probe()` 只做 `get_me()`，不检查本地 polling runtime 的 `polling_liveness`。
  - `crates/telegram/src/plugin.rs:233`：probe details 直接拼接原始 `API error: {e}`，脱敏边界未冻结。
- 当前测试覆盖：
  - 已有：
    - 文本/流式文本 retry：`crates/telegram/src/outbound.rs:1692`、`crates/telegram/src/outbound.rs:1875`、`crates/telegram/src/outbound.rs:1927`
    - DM allowlist 安全回归：`crates/telegram/src/access.rs:195`
  - 缺口：
    - file download token 脱敏/timeout/大小限制
    - callback `data == None` / `message == None` / `chat_id == ""` / answer 失败 / account missing 场景
    - helper send 失败的结构化日志与 fallback
    - 用户可见错误文案脱敏，不回显原始内部错误
    - media/location retry 与 caption clamp
    - `polling_liveness`、batch stop 语义与 pre-ack recoverability

## 根因分析（Root Cause）
- A. ack 语义与 handler 语义耦合不清：
  - polling loop 把“收到 update”近似当成“成功处理”，导致 offset 推进过早；而现有文档与代码都还没有定义 `Err`、retry、quarantine 之间的边界。
- B. 错误处理能力分布不均：
  - 文本/流式文本路径已经进入“分类 + retry + 结构化日志”模式；
  - callback、OTP、helper、media/location 仍保留早期 `let _ =` 与单次 await 模式。
- C. 下载路径绕过了现有 Telegram client 抽象：
  - 手工拼接 file URL，既引入 token 泄露风险，也失去统一 timeout/base_url/client 配置能力，并与 `Bot::set_api_url(...)` 语义脱节。
- D. 健康探针口径偏“认证成功”而不是“通道可收发”：
  - `get_me()` 只证明 token 可用，不证明 polling loop 活着、offset 在推进、update 还在被处理。
- E. “静默”没有被显式建模：
  - 当前代码里存在“有意静默”（例如 OTP flood control、unknown callback data、发给其他 bot 的命令）与“错误被吞掉”的混合，但没有统一 reason code 与观测策略。
- F. pre-dispatch / post-dispatch 的失败分类没有冻结：
  - 当前实现既没有定义哪些失败可安全重试，也没有定义一旦进入 `dispatch_to_chat / dispatch_command` 或用户可见回执后是否还允许整条 update 重放。
- G. callback 生命周期依赖可变 account state 重建：
  - `handle_callback_query(...)` 已拿到 bot，但实际 answer/follow-up 又回头查 `accounts`，导致 account 缺失时既无法清 spinner，也没有统一的降级口径。
- H. typing indicator 语义实现了“调用点”，但没有实现“失败可观测”：
  - gateway 已在 `dispatch_to_chat` 前启动 typing loop，但 Telegram `send_typing()` 吞掉了真实发送错误，导致上层既无法感知失败，也无法解释为什么用户侧看不到“正在输入”。
- H1. 现网 typing 目标与实际体验之间缺少 run-scoped 口径冻结：
  - 现网 TG 主回复链路并不是边收 delta 边回 Telegram，而是由 gateway 等待整次 run 完成后统一回发。
  - 因此用户真正需要的不是“首个 delta 前 typing”或“stream 阶段 typing”，而是“从 run 开始到最终用户可见结果/失败回执完成为止”的 run-scoped typing。
  - 当前虽然已有 gateway typing loop，但它的失败观测、覆盖边界与手工验收口径都没有冻结清楚，所以出现“代码看起来有 typing，用户侧却觉得中途没反应”时很难判断是语义问题还是实现故障。
- H2. Telegram 原生 stream outbound 的 typing lifecycle 仍未闭环：
  - `send_stream_with_transport(...)` 只在占位消息发送前单发一次 typing。
  - 一旦 `"…"` 占位消息发出，后续只剩 edit 流，没有继续维持 typing；因此若未来接入这条路径，仍会复现“模型仍在推理，但客户端不再显示正在输入”。
- I. 适配层缺少稳定的中间表示：
  - 当前没有“先把 Telegram update 归一化成 interaction + parts + effect plan，再执行”的中间层；于是 access、`prepare`、command/callback、本地 helper 与 dispatch 全都耦合在 `handlers.rs` 的大函数里。
- J. 跨 channel 附件 contract 被命名成“通用附件”，但实际语义并未冻结：
  - `ChannelAttachment` 既没有 asset kind，也没有 capability/fallback 语义；gateway 又默认把它解释成 image part，导致只要未来有人把 document/audio/video 填进去，就会产生错误的下游行为。
- K. active/passive/unsupported 的决策没有集中规划：
  - 当前很多分支在叶子节点临时决定“继续 dispatch caption”“silent ignore”“给用户回原始错误”或“只打一条日志”，导致同类故障在不同媒体类型上的表现不一致。
- L. 出站能力模型没有统一归一：
  - 现有 text、media、location、stream 路径分别自行处理 `threading / silent / typing / raw error exposure / retry`，导致公共语义在不同内容类型上出现漂移与漏洞。
- M. `retry_barrier` 定义得还不够工程化：
  - 当前文档已经提出“pre-dispatch 才能 retry”，但没有进一步冻结“哪些前置动作不算越过 `retry_barrier`”；若不补这层，typing / callback answer / local feedback 仍会在实现时产生歧义。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

  - 必须：
  - 每个 Telegram update 在进入业务分支前，必须先被收敛到统一的三件套：
    - `InboundInteraction`：`interaction_kind / is_active_interaction / reply_target / callback_id? / source_message_id? / prepared_parts`
    - `ExecutionPlan`：`route_kind / before_barrier_effects / retry_barrier / after_barrier_effects / outbound_request?`
    - `UpdateOutcome`：`AckSuccess / AckTerminal(reason_code) / RetryableFailure(reason_code)`
  - `InboundInteraction.prepared_parts` 必须只收敛为少量稳定类型：`text_like / image / audio_for_stt / location / unsupported_attachment`。
  - Telegram file download 相关日志与错误链中，必须不包含 bot token、原始 file URL、完整正文。
  - Telegram file download 必须继承 bot 当前的 `api_url/client` 语义；实现上应优先复用 `Bot::download_file(...)` 或等价封装，而不是手工拼接下载 URL。
  - polling loop 必须在 handler 产出明确的“ack / retry / quarantine”结果之后，才推进或保留 update ack；不得再使用“进入 handler 前先 ack”的模式。
  - 若当前批次中的某条 update 产出 `RetryableFailure(reason_code)`，polling loop 必须立即停止处理该批剩余 updates；不得在同一批次中继续消费后续 update。
  - 一旦识别为 callback query，系统必须保证 `answer_callback_query` 在任何慢操作之前完成，且每个 callback 最多只走一次 answer 语义；`data == None`、`chat_id` 缺失、account state 缺失等分支也不得 silent return。
  - 所有 helper send 路径必须返回显式错误结果或结构化 reason code，不能继续 `let _ = ...` 静默吞错。
  - 所有主动发给 Telegram 用户的失败文案必须是脱敏、稳定的用户文案；不得直接回显原始 `Error: {e}` / anyhow / teloxide error string。
  - 会进入 chat/LLM 回复的主动路径，应尽力维持 typing indicator；`send_chat_action` 失败必须可观测，不得伪装成“typing 已发送”。
  - 对现网 TG 主回复链路，typing 的生命周期必须绑定到一次完整 run，而不是绑定到“首个 delta”“首轮推理”或某个局部 stream 阶段。
  - 这里的一次完整 run，指从 `dispatch_to_chat(...)` 进入 `chat.send(...)` 开始，到该次 run 的最终 Telegram 用户可见结果发送完成，或者最终失败反馈发送完成为止；其间无论经历多少轮“推理 -> 工具调用 -> 再推理 -> 再工具调用”，typing 都应尽力持续保活。
  - 所有“策略性静默”路径必须记录低噪声、带 reason code 的结构化日志，至少覆盖：OTP 非验证码、OTP already pending/locked out、unknown callback data、发给其他 bot 的命令、被策略拒绝但已命中候选的消息。
  - 通道健康状态必须区分“bot auth 可用”“polling loop 仍在运行”“最近轮询仍成功”“最近是否处理过 update”；`probe().details` 也必须是脱敏摘要，而不是原始 API 错误串。
  - 当前 gateway 边界上的 `ChannelAttachment` 语义必须冻结为“仅 LLM image attachment”；只有图片类部件可以进入 `dispatch_to_chat_with_attachments(...)`。
  - `document / video / sticker / animation / contact / poll / venue / game` 等当前未支持直接处理的附件类型，在主动交互路径上必须显式走 unsupported 策略，不得静默丢弃，也不得只 dispatch caption。
  - gateway 侧的 dispatch 失败若需要给 Telegram 用户回执，必须使用固定、脱敏的通用错误文案；不得直接回显 `⚠️ {e}`。
  - Telegram 出站在内部必须先收敛到统一的最小模型：
    - `thread_target`：是否 reply-thread，以及解析失败时的 reason-coded degrade
    - `delivery`：至少包含 `silent` 与 `typing_policy`
    - `content`：至少显式区分 `text / media / location / stream`
  - `StreamEvent::Error`、截图发送失败、流式编辑失败等出站故障，若需要对用户可见，必须映射成稳定、脱敏文案，不得直接拼接原始内部错误字符串。
- 不得：
  - 不得再把 `chat_id` 解析失败默默退化成 `ChatId(0)`。
  - 不得在媒体/位置路径继续绕过统一的 timeout/retry/error-classification 口径。
  - 不得把 token 泄露风险留给“上线后人工排查是否打到了日志”。
  - 不得把非图片附件偷偷塞进当前 `image_url` 多模态路径。
  - 不得让 `reply_to` 解析失败静默退化成“无 threading”而无 reason code。
- 应当：
  - 任何“主动触发 bot 工作”的路径（DM、addressed group message、slash command、callback）在失败且尚未产生用户可见副作用时，应尽力给用户一个最小、脱敏、可重试的错误提示。
  - 纯被动 listen-only/group transcript 摄入失败应保持低噪声，以 reason-coded log 为主，不强制用户可见回执。
  - caption 超长应采用 `TELEGRAM_CAPTION_LIMIT` 口径做 clamp，并把剩余文本走单独文本消息 fallback，而不是直接触发 opaque API error。
  - 入站媒体下载/转写失败后的语义必须冻结为显式策略，不能由实现临时决定。
  - 现网主链路的 typing policy 必须冻结为 `run_scoped_typing`：
    - typing loop 从 `dispatch_to_chat(...)` 调用 `chat.send(...)` 前开始；
    - 在整次 run 完成前持续每 `4s` best-effort 保活；
    - 只有在最终 Telegram 用户可见结果已经发送完成、或最终失败回执已经发送完成、或 run 被明确取消/超时终止后，typing loop 才能停止。
  - `run_scoped_typing` 不要求 Telegram 主回复必须接入原生 stream outbound；即使当前主回复仍是“run 结束后一次性回发”，typing 也必须覆盖整个 run。
  - `send_stream_with_transport(...)` 若未来接入主链路，其 typing 行为也必须兼容 `run_scoped_typing`，不得重新退化为“只在 placeholder 前单发一次 typing”。
  - 无论是 gateway typing loop 还是未来的 stream typing 变体，`send_chat_action` 失败都必须有结构化 reason code；不得继续 `let _ = ...` 吞掉。
  - 新增 Telegram 附件能力时，应当只新增对应的“`prepare` 规则 + capability row + 测试”，而不是再触碰 ack / callback / typing / 用户错误文案主逻辑。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 在现有架构上做分阶段 hardening，优先修掉安全事故与不可恢复故障，再统一 helper/media/location/OTP 的错误处理语义。
- 优点：
  - 影响面可控，和既有 `TelegramOutbound` 文本 retry 能力兼容。
  - 可以快速关闭 P0 风险，不要求先引入 durable queue。
- 风险/缺点：
  - 仍不提供跨进程 exactly-once 保证。
  - 需要统一 callback / helper / 主动交互错误的用户文案与回执方式，会带来小幅 UX 变化。

#### 方案 2（备选）
- 核心思路：
  - 直接把 inbound/outbound 都提升到 durable inbox/outbox 语义，再统一用户反馈。
- 优点：
  - 长期可靠性上限更高。
- 风险/缺点：
  - 复杂度显著更高，不适合作为本单的最小可落地修复。

### 最终方案（Chosen Approach）
#### 统一适配主干（Core Adapter Spine）
- 主干流程只保留 3 步：
  - `prepare`
    - polling loop 只负责取回原始 `Update`、调用适配层并接收 `UpdateOutcome`；
    - 适配层在这一步把原始 Telegram update 准备成 `InboundInteraction`，并准备好 `prepared_parts`；
    - 下载、转写、图片优化、unsupported 判定都在这里完成。
  - `plan`
    - 基于 `InboundInteraction + capability matrix` 只生成一份 `ExecutionPlan`；
    - `ExecutionPlan` 必须一次性决定：`route_kind`、`before_barrier_effects`、`retry_barrier`、`after_barrier_effects`、`outbound_request?`。
  - `execute`
    - 先执行 barrier 前动作（例如 callback 空 answer、typing、结构化日志）；
    - 再越过 `retry_barrier` 执行 barrier 后动作（例如 dispatch、用户可见消息、最终 edit/reply）；
    - 最后统一收敛到 `UpdateOutcome`。
- `ExecutionPlan` 的 effect 口径在本单中一并冻结：
  - `before_barrier_effects` 仅允许承载 `callback_answer / typing / observability` 这类 barrier 前动作；
  - `after_barrier_effects` 仅允许承载 `dispatch / helper_send / user_feedback / final_reply` 这类 barrier 后动作；
  - 不允许在 `execute` 过程中临时新增第三类 effect。

#### `polling_liveness`（账号运行态监督）
- `polling_liveness` 不是单条 update 三件套的一部分，而是 Telegram 账号运行态的并行监督机制。
- 它回答的是：
  - 本地 polling task 现在是不是还在跑；
  - 最近一次 `get_updates()` 是否仍成功；
  - 当前是否已 stale；
  - 若已退出，最后一次退出原因是什么。
- 它不回答“Telegram 服务器当前是否健康”；它检查的是 moltis 自己这侧的 polling runtime 是否还活着。

#### 能力矩阵（Capability Matrix）
- 入站：
  - `text_like`：支持；覆盖普通文本、caption、addressed slash command body。
  - `callback`：支持；不进入 `prepared_parts` 列表，而是作为 `InboundInteraction.interaction_kind=callback` 直接进入 `feedback_only` 或 `command` 路由。
  - `image`：支持；当前只允许图片进入现有 image-only multimodal 路径。
  - `audio_for_stt`：支持；仅在完成 STT 后才能继续进入 chat 路由。
  - `location`：支持；优先走本地 location 更新，必要时再转成规范化文本。
  - `unsupported_attachment`：覆盖 `document / video / sticker / animation / contact / poll / venue / game` 等当前未直接支持的附件；主动交互路径给固定提示，被动路径只记 reason-coded log。
- 出站：
  - `send_text / send_text_with_suffix / send_single_media / send_location / send_typing / answer_callback / stream_edit_reply`：支持，是当前 Telegram channel 的稳定能力面。
  - `thread_target`：支持，但 Telegram 解析失败必须变成 reason-coded degrade，不能继续 silent drop。
  - `silent`：应视为统一 delivery 语义；text、media、location 至少要有明确的一致行为或显式 no-op reason code。
  - `send_multiple_media_album / 单次回复携带多个异构附件`：本单不支持；后续若需要，必须先升级公共 contract，而不是在 Telegram 实现里偷偷扩能力。

#### 最小出站能力模型（Minimal Outbound Model）
- `thread_target`
  - 统一承载 reply-thread 目标；
  - Telegram 适配器内部负责把它解析成 `ReplyParameters`，解析失败要留下 reason-coded log，而不是静默退化。
- `delivery`
  - 至少包含 `silent` 与 `typing_policy`；
  - 任何内容类型若不支持其中某项，也必须显式 no-op 并留 reason code，不能只在 text 路径实现、其他路径悄悄忽略。
- `content`
  - 至少显式区分 `text / media / location / stream`；
  - `media` 继续由 Telegram 私有 MIME 路由决定是 `photo / document / voice / audio`；
  - `stream` 的内部错误不得直接变成用户可见的原始异常文本。

#### 核心契约（Minimal Contracts）
- Telegram 内部主干契约只保留 3 个：
  - `InboundInteraction`
    - 封装单条 update 的统一交互语义；
    - 最少字段：`interaction_kind`、`is_active_interaction`、`reply_target`、`callback_id?`、`source_message_id?`、`prepared_parts`。
  - `ExecutionPlan`
    - 封装本次交互的一次性执行决策；
    - 最少字段：`route_kind`、`before_barrier_effects`、`retry_barrier`、`after_barrier_effects`、`outbound_request?`。
  - `UpdateOutcome`
    - 作为 polling loop 唯一接受的处理结果；
    - 只允许 `AckSuccess / AckTerminal(reason_code) / RetryableFailure(reason_code)` 三种。
- `prepared_parts` 与 `outbound_request` 保留为 `InboundInteraction` / `ExecutionPlan` 的内部字段，不再与主干契约并列成第二层概念。

#### 行为规范（Normative Rules）
- 规则 0（统一流水线）：
  - Telegram 适配层必须先完成 `prepare -> plan`，再进入 `execute` 执行具体副作用；
  - 不允许继续以“进入某个 if 分支后即时下载/即时 send/即时 dispatch”的方式叠加新能力。
- 规则 1（下载安全）：
  - Telegram file download 必须复用 bot 当前的 client/base_url 或等价安全封装；
  - 所有下载错误必须做脱敏映射，不直接透传原始 URL，也不直接把原始 `%e` 写入日志。
  - 下载路径必须有显式有界超时与大小限制；若 `get_file` 元数据已知超限，应直接终态失败并打 `reason_code=file_too_large`。
  - 下载实现必须继承 `Bot::set_api_url(...)` 语义，确保 mock/self-hosted endpoint 与生产路径一致。
- 规则 2（ack 边界）：
  - polling loop 在单个 account 内必须按 `update_id` 顺序串行执行，并在拿到上一条 update 的 `UpdateOutcome` 后，才允许进入下一条 update；Phase 0/1 不引入同账号 update 并发。
  - handler 必须产出明确的 `UpdateOutcome` 或等价结果：
    - `AckSuccess`：已完成处理，推进 offset；
    - `AckTerminal(reason_code)`：失败已收敛为终态，推进 offset；
    - `RetryableFailure(reason_code)`：暂不推进 offset。
  - `RetryableFailure` 只适用于“尚未越过 `retry_barrier`”的失败；本单先冻结为固定总尝试 `3` 次（含首次执行），不在本单配置化。
  - update 级 retry budget 以 `account_id + update_id` 为 runtime-local key 进行 best-effort 追踪；进程重启后预算允许重新开始，不要求跨重启持久化。
  - callback 空 answer、typing、结构化日志属于 barrier 前动作；一旦执行了用户可见消息/edit、`dispatch_to_chat`、`dispatch_command` 或其他会提交业务语义的动作，就视为已经越过 `retry_barrier`，不得再返回 `RetryableFailure`。
  - 一旦某条 update 返回 `RetryableFailure`，当前 `get_updates()` 批次必须立刻停止；后续 update 留待下次轮询重新获取，不得继续在本批次内处理。
  - `RetryableFailure` 超过预算后必须转为 `AckTerminal(reason_code=quarantined_after_retries)` 并打强日志，避免单条 poison update 无限阻塞后续消息。
  - `AckTerminal` 的典型样例包括：access denied / OTP 已处理、unsupported update kind、callback malformed or unknown data、`event_sink` 缺失、downstream dispatch failure、已产生用户可见副作用后的 follow-up send failure。
  - 进程在 outcome 提交前崩溃时，该 update 应视为未 ack。
- 规则 3（callback 最小反馈）：
  - callback query 一旦进入 handler，必须优先使用调用方已经传入的 bot handle 执行一次 `answer_callback_query` 清 spinner，而不是依赖后续从 `accounts` 重查 bot。
  - `query.data == None`、`message == None`、`chat_id == ""`、account missing 等分支，也必须先尝试一次空 answer，再按 reason-coded ignore / fallback 口径收敛。
  - 对可能等待 `dispatch_command(...)` 或发送 follow-up message 的路径，默认先发空 answer；后续结果通过普通消息或 edit 呈现，不依赖第二次 callback answer。
  - callback 首次 answer 若因“明确未送出”的传输失败而未送出，必须记录 `telegram.callback.answer_failed` 并返回 `RetryableFailure`；不得一边吞掉 answer 失败、一边继续执行慢 dispatch。
  - callback 首次 answer 若属于“结果未知”或 `query already answered / too old / invalid` 这类非 retryable 失败，必须记 reason code 并按终态收敛，不得把它继续当成可安全重放的 pre-barrier 失败。
  - 若底层库无法证明“这次 answer 明确未送出”，默认按 `unknown_outcome` 处理，而不是乐观进入 update 级 retry。
  - callback follow-up 若存在来源 message，应优先复用该 `message_id` 作为 reply/edit 目标；只有 `inline_message_id` 或无可访问 message 时才退化为非 threaded 回执。
  - 仅对纯本地、即时可得且无需后续消息的结果，才允许用单次 toast answer 直接结束。
- 规则 4（helper 一致性）：
  - `send_sessions_keyboard / send_context_card / send_model_keyboard / send_sandbox_keyboard` 等 helper 必须返回 `Result`；
  - OTP challenge / wrong code / locked out / expired 等“主动发消息”路径也适用同一规则；
  - 调用方决定 fallback，且必须记录结构化日志；用户文案使用固定脱敏文案，不透传原始错误。
- 规则 5（策略性静默）：
  - OTP flood protection、already pending、locked out 等“故意不回消息”的路径允许继续静默对用户，但必须记录 reason code，例如 `telegram.otp.ignored.non_code`、`telegram.otp.ignored.already_pending`。
  - 除 OTP 外，unknown callback data、group 未 addressed 的 slash command、发给其他 bot 的命令、命中候选后被 gating 拦截的消息，也必须收敛到 `telegram.*.ignored.<reason>` 口径。
- 规则 6（附件能力边界）：
  - 在本单 Phase 0/1 范围内，`ChannelAttachment` 的实际语义必须显式收窄为 `llm_image_attachment`；
  - 只有图片类输入可以进入 `dispatch_to_chat_with_attachments(...)`；
  - 语音/音频必须先完成 STT，document/video/sticker 等 unsupported 附件必须在 Telegram 侧终态收敛，不能穿透到当前 gateway image-only contract。
  - `ReplyPayload.media` 的语义继续冻结为“单主媒体”；如未来需要多附件/album，必须另开 contract 升级单。
- 规则 7（媒体/位置一致性）：
  - 媒体/位置路径必须补齐 timeout、失败分类、与文本路径对齐的结构化 retry/give-up 日志；本单冻结为复用既有文本 retry config，而不是另开一套新配置。
  - 图片 `PHOTO_INVALID_DIMENSIONS / PHOTO_SAVE_FILE_INVALID` 继续保留“photo -> document fallback”语义，并视为成功等价收敛，不额外扩大 retry 面。
  - caption 超长必须在发送前按 `TELEGRAM_CAPTION_LIMIT` 对最终 Telegram HTML 文本做 UTF-8 边界 clamp；溢出部分走单独文本消息 fallback。
  - 入站语音/图片下载或转写失败，在“本来会触发 active dispatch”的路径上，若判定为可重试失败则先走 update 级 retry budget；终态失败或预算耗尽后回最小、脱敏错误提示并终止本次 dispatch，不再继续把 caption/占位文本静默送进 LLM。
  - 纯被动 listen-only / transcript ingestion 路径命中媒体失败时，允许不做用户可见反馈，但必须打 reason-coded log。
  - typing 只属于 best-effort 的 barrier 前动作；typing 发送失败本身不得单独把整条 update 升级为 `RetryableFailure`。
- 规则 7a（单一路由决定）：
  - 每条 update 在进入执行阶段前，必须先由 `ExecutionPlan.route_kind` 唯一决定其去向；
  - 不允许在 helper、callback、OTP 或媒体分支内部再临时改写为另一条执行路线。
- 规则 8（健康口径）：
  - `polling_liveness` 是账号运行态监督，不属于单条 update 的 `InboundInteraction / ExecutionPlan / UpdateOutcome` 主模型。
  - channel probe 应当至少暴露：
    - auth 可用性
    - `last_poll_ok_at`：每次 `get_updates()` 成功返回时更新，即使 updates 为空
    - `last_update_finished_at`：任一 update 完成 handler outcome 提交时更新
    - `polling_state`：至少区分 `running / stopping / exited`
    - `last_poll_exit_reason_code`
    - stale 判定阈值及其结果（本单固定为 `90s`）
  - polling task 的正常退出、冲突禁用退出、意外退出（包括 panic）都必须留下 `last_poll_exit_reason_code`，不能只依赖 stale 倒推。
  - `ChannelHealthSnapshot.connected` 在本单中应表示 `auth_ok && polling_state == running && !stale`。
  - `probe().details` 先以脱敏摘要字符串承载 `auth_ok / polling_liveness / polling_state / last_poll_ok_at / last_update_finished_at / stale_threshold_secs / last_poll_exit_reason_code`；更细外部字段拆分留作后续 issue。
  - `polling_liveness` 不得仅由“最近处理过 update”推断；空闲 bot 在没有新消息时也应保持 healthy。
  - runtime 内部优先保存 `polling_state / last_poll_ok_at / last_update_finished_at / last_poll_exit_reason_code / stale_threshold_secs` 等基础状态；`polling_liveness` 更适合作为 probe/UI 摘要字段按需派生，而不是再维护一套独立真值。

#### 分阶段实施（Phased Hardening）
##### Phase 0（必须先做，P0）
- 下载安全：
  - 移除手工拼接 `file/bot{token}` 下载 URL 的裸实现；
  - 引入脱敏错误映射、timeout、大小限制、统一 client/base_url。
- ack 可恢复性：
  - 调整 `offset` 推进时机；
  - 冻结固定 `3` 次 update 级 retry budget、当前批次 stop-on-retryable 语义，以及 `ignored_update_kind` / `event_sink_missing` 等终态 reason code；
  - 为 handler fail 路径补 `event=telegram.update.handler_failed` 与最小主动交互路径用户反馈策略。
- callback 不转圈：
  - 补齐 `data == None` / `message == None` / `chat_id == ""` / account missing 时的 callback answer 逻辑；
  - 冻结“先清 spinner，再执行慢操作”的一次性 answer 语义，并补 `answer_failed` 结构化日志与 retry 口径。
- 用户错误文案脱敏：
  - 去掉原始 `Error: {e}` 直通 Telegram 用户的路径；
  - 统一为固定、脱敏、可重试文案。

##### Phase 1（应做，P1）
- helper send 一致化：
  - 去掉 `ChatId(0)` fallback；
  - 所有 helper 与 OTP 主动发送路径改为显式 `Result` + structured warn。
  - typing indicator：
  - `send_typing()` 不得吞掉 `send_chat_action` 失败；
  - 现网 TG 主回复链路必须把 typing 生命周期绑定为 `run_scoped_typing`，并明确 stop 条件为“最终用户可见结果/失败反馈完成”；
  - typing loop 的失败必须具备结构化日志，避免“代码里有 typing、用户却看不到”时无从排障。
  - 该项可独立拆成一个很小的 P1 子任务，不依赖 ack/download 改造先落地。
- OTP 可观测性：
  - 保留策略性静默，但记录去重/限频日志与 reason code。
- 媒体/位置最小可靠性：
  - caption clamp（按 `TELEGRAM_CAPTION_LIMIT` 与最终 HTML 口径）
  - location/media 复用文本 retry config 与结构化 give-up 日志
  - 用户可见错误策略冻结
  - 入站 media download / STT failure 终止 dispatch 的语义冻结
- 健康探针：
  - 增加 `polling_state / last_poll_ok_at / last_update_finished_at / last_poll_exit_reason_code / stale_threshold_secs` 等 runtime state；
  - `probe().details` 输出脱敏摘要，并由上述基础状态派生 `polling_liveness`；`connected` 反映本地 polling runtime 的 `polling_liveness`，而非单纯 auth。

##### Phase 2（建议做，P2）
- 健康探针外部展示：
  - `polling_liveness` 摘要口径已在 P1 通过基础 runtime state 进入 `probe().details`；
  - 是否把 `auth_ok / polling_liveness / last_poll_ok_at` 拆成 gateway/UI 显式字段，留作后续独立演进。
- 更细的 failure taxonomy：
  - inbound download failures
  - helper send failures
  - callback answer / follow-up failures
  - OTP ignored reasons

#### 接口与数据结构（Contracts）
- API/RPC：
  - 本单优先不改外部 RPC surface；以 Telegram 内部 helper 返回 `Result`、新增状态字段或日志字段为主。
- 存储/字段兼容：
  - `polling_liveness` 优先作为 runtime summary/派生口径处理；基础状态优先放在 runtime state，不引入持久化迁移。
- Telegram 内部主干契约只保留 3 个：
  - `InboundInteraction`
  - `ExecutionPlan`
  - `UpdateOutcome`
  - `prepared_parts / outbound_request` 作为内部字段挂在前两者之下，不再并列成新的主干 struct。
- 可观测性字段契约：
  - `event` 用于表达动作类别，例如 `telegram.download.failed`、`telegram.callback.answer_failed`；
  - `reason_code` 用于表达直接原因，例如 `timeout`、`file_too_large`、`chat_id_missing`、`quarantined_after_retries`；
  - 同一个 `event` 可以对应多个 `reason_code`，但 `reason_code` 命名必须保持稳定，避免把原始错误文本塞进结构化字段。
- UI/Debug 展示（如适用）：
  - 本单先通过 `probe().details` 的脱敏摘要暴露：
    - `auth_ok`
    - `polling_liveness`
    - `polling_state`
    - `last_poll_ok_at`
    - `last_update_finished_at`
    - `last_poll_exit_reason_code`
    - `stale_threshold_secs`

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - `telegram.update.handler_failed`：
    - 主动交互路径：仅在未开始任何用户可见回执、且尚未进入 downstream dispatch 前，尽力回一条通用错误文案；
    - 纯被动路径：默认不广播内部错误，但记录 reason-coded log。
  - `telegram.callback.answer_failed`：
    - 必须记录 callback id、has_message、has_chat_id、reason_code；
    - 同一 callback 不得再尝试第二次 `answer_callback_query`。
    - 仅 `reason_code=transport_failed_before_send` 这类“明确未送出”的失败允许走 update 级 retry；`already_answered / query_too_old / invalid_query_id / unknown_outcome` 等原因必须终态收敛。
  - `telegram.callback.ignored.no_data` / `telegram.callback.ignored.no_chat`：
    - 先空 answer 清 spinner；
    - 再按 ignore reason 低噪声记日志。
  - `telegram.download.failed`：
    - 记录脱敏后的 error class、timeout、status；
    - 用户可见内容仅限“下载失败/转写失败”，不得透出 URL/token。
  - `telegram.attachment.unsupported`：
    - 记录 attachment kind、has_caption、active_or_passive、reason_code；
    - 主动交互路径给固定 unsupported 提示，被动路径只留低噪声日志。
  - `telegram.prepare.failed`：
    - 记录 `part_kind / retryable / reason_code`；
    - 不得继续把半成品 caption/placeholder 静默送入 downstream dispatch。
  - `telegram.helper_send.failed`：
    - 记录 helper 名称、chat_id parse 状态、payload 长度；
    - 不得无日志结束。
  - `telegram.typing.failed`：
    - 记录 active_path、chat_id、reason_code；
    - 不得在发送失败时继续记录“typing indicator sent”一类误导性成功语义。
    - typing 失败只影响可观测性与用户体验，不单独决定本条 update 的 `UpdateOutcome`。
  - `telegram.update.terminal.event_sink_missing`：
    - 命中候选入站消息但 `event_sink` 缺失时，必须显式记录终态 reason code；
    - 若 chat 侧仍可回最小错误文案，则按主动交互路径规则 best-effort 回执后再终态 ack。
  - `telegram.user_feedback.failed`：
    - 记录 feedback stage、active_path、chat_id 是否可用；
    - 不得把失败再降级成原始内部错误回显。
  - `telegram.dispatch.failed`：
    - 记录 `dispatch_mode / has_attachments / active_path / reason_code`；
    - 若需要给 Telegram 用户回执，必须发送固定脱敏错误文案，而不是 `⚠️ {e}`。
  - `telegram.outbound.thread_target_invalid`：
    - 记录 content kind、reply target parse 状态、reason_code；
    - 可以降级成非 threaded 发送，但不得 silent。
  - `telegram.outbound.content_failed`：
    - 记录 `content_kind / silent / typing_policy / reason_code`；
    - text/media/location/stream 统一进入这一层失败分类，而不是各自产生不同口径的裸错误。
  - `telegram.update.quarantined`：
    - 同一 update 多次 `RetryableFailure` 后转为终态 ack；
    - 必须记录 `update_id / retry_count / reason_code`。
  - `telegram.update.ignored.unsupported_kind`：
    - 明确作为终态 ack 的 ignore 分类，而不是隐式 debug 掩盖。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - update ack 之前，必须保留“尚未完成处理”的状态；
  - 一旦当前 update 返回 `RetryableFailure`，本批剩余 update 不得继续处理；
  - update 级 retry budget 状态必须在 `AckSuccess / AckTerminal` 后清理，并对长时间未再出现的 `update_id` 做有界过期，避免 runtime state 无界增长；
  - `polling_liveness` 必须在 loop 退出/卡死时暴露 stale 状态；
  - callback spinner 必须优先清理。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 默认全部脱敏。
- 禁止打印字段清单：
  - bot token
  - Telegram file URL 原文
  - 完整正文
  - 原始音频/图片内容
  - OTP code 之外的任何敏感 token/secret

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 构造 Telegram file download 失败时，日志中不出现 bot token 或 `file/bot<token>` URL。
- [x] 使用 `Bot::set_api_url(...)` 的测试或自定义 endpoint 场景下，Telegram file download 仍命中 bot 当前 API endpoint，而不是硬编码 `api.telegram.org`。
- [x] 构造 callback query `data == None`、仅有 `inline_message_id`、`chat_id` 为空、account state 缺失的场景时，Telegram 客户端 spinner 仍会被清掉，且 `answer_failed` 不再静默。
- [x] command/helper 的 chat_id 解析失败时，不再尝试发送到 `ChatId(0)`，并有结构化日志说明失败原因。
- [x] polling loop 中单条 update 的 handler 报错不会在“未完成处理前”提前推进 offset。
- [x] 同一 Telegram account 下的 updates 在本单 Phase 0/1 范围内保持按 `update_id` 顺序串行处理，不会因为并发执行而破坏 batch stop / retry 语义。
- [x] 单条 retryable poison update 不会无限阻塞整个 polling 流；超过预算后会被 quarantine 并留下结构化日志；在该 update 进入终态前，同批后续 updates 不会被提前处理。
- [x] 非预期 update kind 会显式收敛为 `event=telegram.update.ignored reason_code=unsupported_kind`，而不是仅靠隐式 debug ignore。
- [x] probe 能区分“token 认证可用”与“polling loop 已失活或长时间无成功更新”，且 `details` 为脱敏摘要而非原始 API error。
- [x] 空闲但健康的 bot 不会仅因为长时间没有新消息而被误判为 degraded。
- [x] 会进入 chat/LLM 回复的主动路径，现网 TG 主回复链路会按 `run_scoped_typing` 在整次 run 内 best-effort 保持 typing；若 `send_chat_action` 失败，会留下 `telegram.typing.failed`，而不是静默成功。
- [x] 发送媒体时最终 Telegram caption 超过 `TELEGRAM_CAPTION_LIMIT=1024` 字节不会直接触发 opaque API error，而是按冻结策略 clamp/fallback。
- [x] 入站 media download / STT failure 不再静默 dispatch caption/占位文本到 LLM；预算耗尽后会给主动交互路径用户最小错误提示并终止 dispatch。
- [x] command/callback/helper/OTP 主动失败路径不再把原始 `Error: {e}` 直接展示给 Telegram 用户。
- [x] OTP、unknown callback data、group 未 addressed 的命令、发给其他 bot 的命令等策略性静默路径都有 reason code，排障不再依赖猜测。
- [x] `document / video / sticker / animation` 等当前 unsupported attachment 在主动交互路径上不会再“无 caption 时静默丢弃”或“有 caption 时只 dispatch caption”；用户会得到固定 unsupported 提示，且日志保留 reason code。
- [x] 当前 gateway image-only attachment 边界被显式冻结，非图片附件不会再被错误编码成 `image_url`。
- [x] gateway `dispatch_to_chat(...)` / `dispatch_to_chat_with_attachments(...)` 失败回给 Telegram 用户的文案已脱敏，不再直接暴露 `⚠️ {e}`。
- [x] `reply_to` 解析失败不会再 silent degrade；要么成功 thread reply，要么留下 `telegram.outbound.thread_target_invalid` 等 reason code。
- [x] 流式 reply、截图发送失败等出站故障不再直接把原始内部错误文本暴露给 Telegram 用户。
- [x] text/media/stream 的失败脱敏、thread degrade 与 typing best-effort 口径已冻结；其中现网主链路以 `run_scoped_typing` 为准，未来 `send_stream_with_transport(...)` 接入时也不得偏离该口径。
- [x] update 级 retry budget 明确为 runtime-local、best-effort 语义：终态后会清理，进程重启后允许重新开始，不要求持久化继承。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/telegram/src/handlers.rs`：download error 脱敏映射与 token redaction
- [x] `crates/telegram/src/handlers.rs`：download path 继承 `Bot::set_api_url(...)`
- [x] `crates/telegram/src/handlers.rs`：callback `data == None` / `message == None` / `chat_id == ""` / account missing 仍会 answer callback
- [x] `crates/telegram/src/handlers.rs`：callback 慢路径先空 answer，再走 follow-up message
- [x] `crates/telegram/src/handlers.rs`：callback `answer_callback_query` 失败会产生 `telegram.callback.answer_failed`，且不会继续 silent dispatch
- [x] `crates/telegram/src/handlers.rs`：callback `answer_callback_query` 的 `invalid_query_id / too_old` 一类终态失败不会误判为可 retry 的 pre-barrier failure
- [x] `crates/telegram/src/handlers.rs`：helper chat_id parse 失败不会退化为 `ChatId(0)`
- [x] `crates/telegram/src/handlers.rs`：command/callback 主动失败路径不会回显原始内部错误给 Telegram 用户
- [x] `crates/telegram/src/handlers.rs`：`event_sink == None` 会进入显式终态 reason code，而不是 silent drop
- [x] `crates/telegram/src/handlers.rs`：unsupported attachment（document/video/sticker 等）在 active path 上不会静默丢弃，也不会只 dispatch caption
- [x] `crates/gateway/src/channel_events.rs`：只有图片类 attachment 会进入 `image_url` multimodal 路径；非图片类不会误入
- [x] `crates/gateway/src/channel_events.rs`：dispatch 失败给 Telegram 用户的文案为固定脱敏错误，而不是原始 `⚠️ {e}`
- [x] `crates/telegram/src/outbound.rs`：`send_typing()` 会传播 `send_chat_action` 失败，而不是始终返回 `Ok(())`
- [x] `crates/telegram/src/outbound.rs`：`reply_to` 解析失败会留下 reason-coded degrade，而不是静默丢 threading
- [x] `crates/telegram/src/outbound.rs`：media caption clamp / fallback 行为
- [x] `crates/telegram/src/outbound.rs`：流式 `StreamEvent::Error` 不会把原始内部错误直接拼给 Telegram 用户
- [x] `crates/gateway/src/channel_events.rs`：现网 TG 主回复链路的 typing lifecycle 具备明确且可验证的 `run_scoped_typing` 语义，直到整次 run 的最终结果/失败反馈完成才停止
- [x] `crates/telegram/src/outbound.rs`：`send_stream_with_transport(...)` 若未来接入主链路，其 typing lifecycle 也与 `run_scoped_typing` 兼容，不再只单发一次 `send_typing`
- [x] `crates/telegram/src/handlers.rs`：入站 media download / STT failure 不再继续占位文本 dispatch
- [x] `crates/telegram/src/plugin.rs` 或 `crates/telegram/src/state.rs`：`polling_state / last_poll_ok_at / last_update_finished_at / last_poll_exit_reason_code / stale_threshold_secs` 与派生 `polling_liveness` 的 `probe().details` 脱敏语义
- [x] `crates/telegram/src/bot.rs`：同一 update 的 `RetryableFailure` 预算、batch stop、unsupported kind 与 quarantine 语义
- [x] `crates/telegram/src/bot.rs`：同一 account 的 polling loop 保持按 `update_id` 顺序串行处理，且 retry budget state 在终态后会清理

### Integration
- 建议补充：模拟 Telegram file download timeout / HTTP 500，验证：
  - handler 行为符合预期
  - 日志脱敏
  - 用户可见反馈符合冻结策略
- 建议补充：模拟使用 `Bot::set_api_url(...)` 的 Telegram file download，验证下载与 send 都命中同一 mock endpoint
- 建议补充：模拟 callback answer 路径失败，验证 `telegram.callback.answer_failed` 可观测、spinner 处理语义符合冻结规则，且普通 follow-up 不会静默失败
- 建议补充：模拟 callback answer `unknown outcome` 或“已答过/已过期”场景，验证不会被错误升级为 update 级 retry
- 建议补充：模拟 polling loop handler error，验证 offset 推进时机、batch stop 与结构化日志
- 建议补充：模拟单条 update 连续 retryable failure，验证不会永久卡住后续 updates，且同批后续 update 不会在前一条终态前被处理
- 建议补充：模拟进程重启后重收同一 update，验证 retry budget 不要求跨重启持久化，但终态前仍保持 pre-ack recoverability
  - 建议补充：模拟 Telegram `send_chat_action` 失败，验证不会再被当成 typing 成功吞掉，且有 `telegram.typing.failed`
  - 建议补充：模拟一次长耗时、多轮工具调用的 agent run，验证现网 TG 主回复链路在 run 全程持续尝试 typing，并仅在最终结果/失败反馈发送完成后停止
  - 建议补充：模拟 run 超时/取消/失败场景，验证 `run_scoped_typing` 会正确停止且不会泄漏后台 loop
  - 建议补充：模拟未来 stream reply 长耗时场景，验证 `send_stream_with_transport(...)` 若被接入，也不会偏离 `run_scoped_typing`
- 建议补充：模拟 unsupported attachment（无 caption / 有 caption 两种），验证 active path 的固定提示、被动路径的低噪声日志，以及“不会只把 caption 当正文 dispatch”
- 建议补充：模拟 gateway dispatch 失败，验证 Telegram 用户收到的是脱敏通用错误，而不是原始内部异常字符串
- 建议补充：模拟 reply threading id 非法，验证会触发 reason-coded degrade，而不是 silent 退化成非 reply
- 建议补充：模拟 `StreamEvent::Error` / 截图发送失败，验证 Telegram 用户看到的是固定脱敏错误提示，而不是原始内部错误

### UI E2E（Playwright，如适用）
- 建议补充：Telegram channel health 在 Web UI 上能显示 polling stale/degraded（如本单涉及 UI）

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - “在 `get_updates` 已返回、handler 尚未完成时杀进程”的窗口较难通过纯单测稳定复现；
  - Telegram callback 的客户端 spinner 行为需要真实客户端或较高保真模拟。
- 手工验证步骤：
  1. 在测试 bot 上注入故障版 handler，制造 `download timeout / helper send failure / callback no-data/no-message`。
  2. 观察 Telegram 客户端是否得到最小反馈，观察日志是否含 reason code 且无 secret。
  3. 在 `get_updates` 返回后立刻 kill 进程，重启后确认 update 是否仍可恢复处理或至少没有被提前 ack 掉。
  4. 在空闲 bot 上长时间不发送消息，确认 health 不会被误报为 stale/degraded。
  5. 触发流式 reply 错误、截图发送失败或非法 `reply_to` 场景，确认用户侧不再看到原始内部错误，且日志含新增 outbound reason code。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - Phase 0 可直接以最小增量落地；
  - 若引入新的 polling runtime 摘要字段，可先只打日志，再在 UI 展示。
- 回滚策略：
  - 下载安全封装、helper `Result` 化、callback answer 补齐都可逐项回滚；
  - 若 ack 时机调整引发副作用，可临时回退到旧逻辑，但必须保留新增日志以便比较风险。
- 上线观测：
  - `event=telegram.update.handler_failed`
  - `event=telegram.download.failed`
  - `event=telegram.callback.answer_failed`
  - `event=telegram.helper_send.failed`
  - `event=telegram.typing.failed`
  - `event=telegram.user_feedback.failed`
  - `event=telegram.outbound.thread_target_invalid`
  - `event=telegram.outbound.content_failed`
  - `event=telegram.otp.ignored.*`
  - `event=telegram.update.ignored.*`
  - `event=telegram.polling.degraded`
  - `event=telegram.polling.recovered`

## 实施拆分（Implementation Outline）
- Step 1:
  - 替换 `download_telegram_file(...)` 的实现，补脱敏、timeout、大小限制、统一 client/base_url，并继承 `Bot::set_api_url(...)`
- Step 2:
  - 引入 `UpdateOutcome` 或等价结果，调整 polling loop 的 offset 推进边界、固定 `3` 次 retry budget、batch stop、quarantine 语义，并为 handler fail 增加结构化日志与最小主动交互路径反馈
- Step 2a:
  - 在 Telegram 侧冻结统一的 3 段主流程：`prepare -> plan -> execute`；
  - 同步冻结 3 个核心契约：`InboundInteraction / ExecutionPlan / UpdateOutcome`
- Step 3:
  - callback handler 改为使用传入 bot 提前 answer 一次，补齐 `data == None` / account missing 分支、`answer_failed` 可观测性与 reply-thread 语义
- Step 4:
  - helper / OTP 主动发送函数改 `Result` 化，清理 `ChatId(0)` fallback、`let _ =` 与原始内部错误回显
- Step 4a（可独立先做的 P1 小单元）:
  - 修复 `crates/telegram/src/outbound.rs` 的 `send_typing()`，让 `send_chat_action` 失败向上返回，而不是始终 `Ok(())`
  - 修复 `crates/gateway/src/channel_events.rs` typing loop 的失败观测，统一收口到 `event=telegram.typing.failed`
  - 冻结现网 TG 主回复链路的 `run_scoped_typing`：从 `dispatch_to_chat(...)` 进入 `chat.send(...)` 前开始，到整次 run 的最终结果/失败反馈发送完成后停止
  - 保持 typing 为 best-effort：typing 失败只记录和降级，不中断后续 `chat.send(...)` 主流程
- Step 5:
  - 媒体/位置路径补 caption clamp、复用文本 retry config、最小失败分类，并冻结入站 media/STT failure 的终止-dispatch 语义
  - 显式实现 unsupported attachment 策略，避免 document/video/sticker 等继续静默丢弃或 caption-only dispatch
- Step 5a:
  - 在 Telegram outbound 侧引入最小统一模型 `thread_target / delivery / content`；
  - 先统一 text/media/location/stream 的 `silent`、threading degrade、错误脱敏与失败分类，再保留 Telegram 私有 MIME 映射细节
- Step 5b（本次新增，供审阅后实施）:
  - 收口 `send_stream_with_transport(...)` 的 typing lifecycle，但明确其优先级低于现网主链路的 `run_scoped_typing`。
  - 本次筹备结论：
    - 现网先以 gateway `dispatch_to_chat(...) -> chat.send(...)` 的 run-scoped typing 为主规范；
    - `send_stream_with_transport(...)` 视为未来接入时必须服从该主规范的子实现，而不是反过来定义现网 typing 行为。
  - 推荐实施口径：
    - 若未来接入 Telegram 原生 stream outbound，typing 仍应从 run 开始持续到最终用户可见结果/失败反馈完成；
    - stream placeholder `"…"` 不能单独作为停止 typing 的依据；
    - 首个 delta、首轮推理完成、首个工具调用返回等局部事件，都不能结束 `run_scoped_typing`；
    - `send_chat_action` 失败统一记录 `event=telegram.typing.failed op=send_stream reason_code=...`，不影响 run 主流程。
  - 需要额外冻结的边界：
    - 若未来确实把 TG 主回复改成边收 delta 边 edit，typing loop 由 gateway 统一持有，还是下沉到 Telegram outbound 自己持有；
    - `send_stream_with_transport(...)` 在 run 已经结束但最后一条 edit/reply 尚未完成时，typing 应保活到哪个精确时点；
    - 如何避免 gateway 级 typing loop 与未来 stream 内部 typing loop 双重发送。
- Step 6:
  - 增加 `last_poll_ok_at / last_update_finished_at / polling_state / last_poll_exit_reason_code / stale_threshold_secs` 等 runtime state，并在 `probe().details` 中派生 `polling_liveness` 脱敏摘要
- Step 7:
  - gateway 侧统一收口 dispatch 失败的用户反馈文案，去掉原始 `⚠️ {e}` 暴露，并与 Telegram 用户错误文案策略对齐
- 受影响文件：
  - `crates/telegram/src/bot.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/outbound.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/telegram/src/state.rs`
  - `crates/telegram/src/markdown.rs`
  - `crates/channels/src/plugin.rs`（若需要把 image-only 语义显式写进注释/契约）
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/channel.rs`（如状态展示口径需要同步）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-telegram-outbound-retry-policy-and-send-failure-recovery.md`
  - `issues/issue-observability-llm-and-telegram-timeouts-retries.md`
  - `issues/issue-cron-system-governance-one-cut.md`
- Related commits/PRs：
- External refs（可选）：

## 遗留项（Remaining Work）
- P0（建议继续在本单完成）：
  - 无。
- P1（建议拆小单或作为 Phase 2a）：
  - 若未来把 TG 主回复切到 Telegram 原生 stream outbound，则将 `send_stream_with_transport(...)` 的 typing lifecycle 对齐到 `run_scoped_typing`，避免 gateway loop 与 outbound loop 双重发送或中途漏发。
  - 将 `InboundInteraction / ExecutionPlan / UpdateOutcome + retry_barrier` 从“规范”落为显式 struct + 单测，并把 `handlers.rs` 的分支式实现逐步迁移到统一流水线，降低新增媒体类型的耦合面。
  - 进一步固化 capability matrix（以 `prepared_parts` 的少量归一化类型为准），并明确每类 unsupported/active/passive 的用户反馈策略与限频口径。
- P2（建议后置）：
  - durable inbox/outbox / exactly-once 等强语义（本单已明确 out of scope）。

## 未决问题（Open Questions）
- Q1（非阻塞，后续可拆单）：`probe().details` 是否要在后续演进中拆成显式 JSON/RPC 字段，而不是继续承载脱敏摘要字符串？
  - 建议：Phase 0/1 保持摘要字符串，避免过早扩 RPC/UI surface；待前端确有稳定消费需求时再拆单结构化。
- Q2（非阻塞，后续可拆单）：在拿到一轮线上观测数据后，是否需要把 inbound update retry budget 从固定 `3` 次演进为配置项？
  - 建议：当前保持固定 `3` 次，不在本单引入新配置面；等拿到真实线上重试分布后再决定是否配置化。
- Q3（非阻塞，后续可拆单）：当 future model/provider 真正需要 document/audio/video 直达模型时，是否引入通用 `channel_input_part` / `channel_output_part` 公共 contract；本单先冻结 Telegram 内部 `prepare` 层与 image-only gateway boundary 是否足够？
  - 建议：当前明确后置，不为未来可能性提前泛化；待确有 document/audio/video 直达模型需求时，再单开 contract 升级单处理。
- Q4（非阻塞，但需在“切 TG 真流式出站”前拍板）：若未来把 TG 主回复从“run 结束后统一回发”改成“边收 delta 边 edit”，typing loop 的唯一 owner 是 gateway 还是 Telegram outbound？
  - 建议：当前不阻塞现网主链路实施；先把 gateway `run_scoped_typing` 做对。未来若接入真流式出站，再单独冻结 owner，避免双 loop 或 stop 条件分裂。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

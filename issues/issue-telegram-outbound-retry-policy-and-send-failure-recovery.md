# Issue: Telegram 出站失败后缺少自动重试与补救语义（outbound / retry）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-14
- Owners:
- Components: telegram / gateway / channels
- Affected providers/models: 所有经 Telegram 通道回执的 agent run

**已实现（如有，写日期）**
- 已在 `TelegramAccountConfig` 增加 `outbound_max_attempts / outbound_retry_base_delay_ms / outbound_retry_max_delay_ms`，并同步补齐 `Default` 与反序列化默认值测试：`crates/telegram/src/config.rs`
- 已在 `crates/telegram/src/outbound.rs` 为文本发送与流式文本路径接入统一的失败分类、结构化错误、有限次自动重试、`MessageNotModified` 成功等价收敛，以及 `outbound_max_attempts=1` 回退口径。
- 已在 `crates/telegram/src/outbound.rs` 内部引入私有 text transport seam，用于脚本化注入 `RetryAfter / Network / Api / InvalidJson` 失败序列。
- 已在 `crates/gateway/src/chat.rs` 为 `channel_delivery.failed` 增补 `telegram_outbound_op / outcome_kind / delivery_state` 关联字段，避免 Telegram 出站失败只剩一条模糊错误日志。

**已覆盖测试（如有）**
- `crates/telegram/src/config.rs`：默认值与反序列化默认值覆盖新增 retry 配置字段。
- `crates/telegram/src/outbound.rs`：已覆盖 `RetryAfter` 分类、`send_message` 的 `Network` 受控重试、`edit_message_text` 的 `Network` 受控重试、`MessageNotModified` 成功等价、非重试错误放弃、`outbound_max_attempts=1` 回退行为。
- `crates/telegram/src/outbound.rs`：已补齐 `telegram_outbound_stream_edit_retries_then_degrades`、`telegram_outbound_stream_final_edit_failure_returns_error`、`telegram_outbound_partial_chunk_failure_logs_partial_sent`。
- `crates/gateway/src/chat.rs`：已覆盖 Telegram 出站结构化错误元信息抽取。
- 已执行回归：`cargo test -p moltis-telegram -p moltis-gateway` 通过。

**已完成实施筹备（2026-03-13）**
- 已冻结 Phase 1 的最小实现范围：优先覆盖文本发送、流式 placeholder / edit / 尾部分块；暂不引入持久化 outbox。
- 已冻结错误分类起点：`RequestError::RetryAfter` 视为可安全重试；`RequestError::Network` 在直接发送类操作上按“结果未知失败”处理，但 Phase 1 允许受控重试；`edit_message_text` 还可结合 `MessageNotModified` 做安全收敛：`/home/luy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/teloxide-core-0.10.1/src/errors.rs:11`
- 已冻结配置草案：在 `TelegramAccountConfig` 新增最小 retry 参数，默认启用有限次自动重试；如需保守回退，可把 `outbound_max_attempts` 下调为 `1`：`crates/telegram/src/config.rs:43`
- 已冻结测试切缝方向：在 `crates/telegram/src/outbound.rs` 内部引入私有 transport seam 或等价执行器封装，以便脚本化注入失败序列。

**已知差异/后续优化（非阻塞）**
- 媒体路径存在局部兼容回退，例如图片发送为 photo 失败后回退为 document；这不是通用重试语义：`crates/telegram/src/outbound.rs:340`、`crates/telegram/src/outbound.rs:368`
- agent runner 对上游 LLM 存在一次短暂重试，但这不覆盖 Telegram channel delivery：`crates/agents/src/runner.rs:825`

---

## 背景（Background）
- 场景：agent run 已经成功产出文本，但 Telegram 通道在 `send_message` / `edit_message_text` / 分块发送等出站阶段遇到网络抖动、429、部分 5xx 或其他短暂错误。
- 约束：Telegram Bot API 的普通发送不是天然幂等；一部分失败属于“明确未送达”，另一部分失败属于“结果未知”，不能草率重发导致重复消息。
- 依赖事实：teloxide 已把 flood control 暴露为 `RequestError::RetryAfter(Seconds)`，并提供 `AsResponseParameters::retry_after()`；因此 `RetryAfter` 不需要字符串解析，可直接纳入 retry 策略：`/home/luy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/teloxide-core-0.10.1/src/errors.rs:63`
- Out of scope：
  - 本单第一阶段不引入持久化 outbox、跨进程恢复、at-least-once 投递承诺。
  - 不重做 Telegram relay/mirror 主体协议。
  - 不把“可靠投递”直接扩大成“永不丢消息”的强保证。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **Telegram 出站发送**（主称呼）：把 channel reply 发往 Telegram 的动作，包含 `send_message`、`edit_message_text`、媒体发送、位置发送，以及流式占位与后续补发。
  - Why：这是用户在 Telegram 侧真正看到回复的最后一跳。
  - Not：不是 agent runner 对 LLM provider 的请求重试。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：Telegram outbound / channel delivery

- **可重试出站失败**（主称呼）：短期内再次尝试有合理成功概率、且不会明显改变业务语义的 Telegram 出站失败。
  - Why：用于界定哪些失败值得做有界自动重试。
  - Not：不是所有 `RequestError` 都可安全重试。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：retryable outbound failure

- **结果未知失败**（主称呼）：本地看到了失败，但无法确定 Telegram 服务器是否已经接受并落地该消息的失败。
  - Why：这类失败若直接重发，可能造成重复消息。
  - Not：不是“明确未送达”。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：unknown delivery outcome / ambiguous send failure

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] Telegram 出站遇到可重试出站失败时，必须具备有界自动重试，而不是首错即丢。
- [x] Telegram 出站失败必须区分“明确未送达”与“结果未知失败”，不得把两者混为一类。
- [x] 在 TG 群协作场景中，因短暂 Telegram 出站故障导致的单次 reply 丢失，不应轻易造成整个任务链静默断裂。
- [x] 当 Telegram 客户端无法收到任何独立故障反馈时，系统在 Phase 1 中必须优先减少“静默丢正式交付”，哪怕接受少量重复消息风险。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须避免对已经明确成功送达的 chunk 做重复补发。
  - 必须把“静默丢正式交付”的风险放在“少量重复消息”风险之前处理。
  - 不得把持久化可靠投递假装成已经具备。
  - 不得因为重试逻辑把原本单条回复放大成不可控刷屏。
- 兼容性：第一阶段优先做进程内、有界、低侵入改动；不要求迁移既有存储。
- 可观测性：每次重试、放弃、降级都必须有结构化日志，至少包含 `op/attempt/max_attempts/error_class/outcome_kind`。
- 安全与隐私：日志不得打印 token、完整正文；正文若必须辅助排障，只允许长度、短预览或哈希。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 文本出站时，只要某个 chunk `send_message` 失败，本次 Telegram 回复立即结束，后续不会自动重发。
2. 流式出站时，占位消息发送失败直接中断；中途 `edit_message_text` 失败只记日志；最终 chunk 发送失败也不会重试。
3. gateway 层把 Telegram 出站失败记成 `failed to send channel reply` 后就结束，对 Telegram 用户侧没有后续补救。
4. TG 群协作里，一条本应成功发出的正式交付若刚好卡在 Telegram 出站失败，任务链可能表面“无人继续”，实际是消息没送达。

### 影响（Impact）
- 用户体验：
  - Telegram 侧看到“bot 没回”或只看到残缺消息。
  - 群协作中会误判为 bot 没执行、没交付、没跟进。
- 可靠性：
  - 最后一跳投递对短暂网络问题过于脆弱。
  - 流式消息可能停留在占位 “…” 或部分内容状态。
- 排障成本：
  - 当前能看出失败，但看不出“该不该重试、为什么没重试、是否可能已经送达”。

### 复现步骤（Reproduction）
1. 触发一个会向 Telegram 回消息的 agent run。
2. 在 `send_message` / `edit_message_text` 阶段制造瞬时网络失败、429 或通道抖动。
3. 观察：
   - 期望：系统对可重试出站失败进行有界重试，或明确进入受控降级。
   - 实际：当前实现通常首错即结束，只留下日志。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/telegram/src/outbound.rs:84`：文本出站按 chunk 发送，任一 chunk 失败即 `return Err(...)`。
  - `crates/telegram/src/outbound.rs:823`：流式占位消息发送失败直接返回错误。
  - `crates/telegram/src/outbound.rs:858`：流式中途 `edit_message_text` 失败只记 `telegram.outbound.degraded`，不重试。
  - `crates/telegram/src/outbound.rs:923`：流式最终剩余 chunk 发送失败即 `return Err(...)`。
  - `crates/gateway/src/chat.rs:7676`：gateway 仅记录 `channel_delivery.failed`，不重发。
  - `crates/agents/src/runner.rs:825`：LLM runner 有一次短暂重试，说明“重试”当前只存在于上游调用层，不存在于 Telegram 出站层。
  - `/home/luy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/teloxide-core-0.10.1/src/errors.rs:11`：teloxide 的 `RequestError` 已明确区分 `RetryAfter`、`Network`、`Api`、`InvalidJson`、`Io`，具备本地失败分类基础。
- 配置/协议证据（必要时）：
  - `issues/issue-observability-llm-and-telegram-timeouts-retries.md:40`：既有工作明确冻结“不做自动补发/重投”。
  - `crates/telegram/src/config.rs:43`：`TelegramAccountConfig` 已使用 `#[serde(default)]`，适合以最小增量追加 retry 配置字段。
- 当前测试覆盖：
  - 已有：`crates/telegram/src/outbound.rs:958` 仅覆盖 unknown account 这类基础错误。
  - 缺口：无 text/media/stream 失败分类、自动重试、部分 chunk 失败、最终降级路径测试。

## 根因分析（Root Cause）
- A. TelegramOutbound 当前采用“单次尝试 + 失败即返回”的直接调用模型，没有通用重试封装。
- B. gateway 层把 channel delivery 当成一次性副作用处理，没有队列或 retry policy。
- C. 既有可观测性工作刻意冻结了 retry 行为，导致“现在能看见失败，但不会自动补救”。
- D. Telegram 出站存在“结果未知失败”语义风险，使得“直接加重试”不能粗暴照搬 LLM runner 的做法。
- E. outbound 当前直接依赖具体 `teloxide::Bot` 调用，缺少可脚本化失败注入的测试切缝，导致 retry 语义难以稳妥落单测。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 对明确属于可重试出站失败的 Telegram 出站操作，系统必须执行有界自动重试。
  - 每次自动重试都必须记录结构化日志，包含 `op/attempt/max_attempts/error_class/retry_after/outcome_kind`。
  - 当失败属于结果未知失败时，系统必须显式记录 `outcome_kind=unknown`，并走受控策略，而不是默默结束。
  - 在 Telegram 客户端收不到独立故障反馈的现实约束下，Phase 1 必须优先避免“静默丢正式交付”。
- 不得：
  - 不得对“已确认成功”的发送步骤做功能等价重放。
  - 不得在无任何上限、无日志、无分类的情况下，把所有网络错误都一律无限自动重发到 Telegram。
  - 不得让 TG 群协作中的正式交付因一次瞬时 Telegram 出站失败而无日志、无告警、无补救地消失。
- 应当：
  - 第一阶段优先覆盖文本 reply、流式 placeholder/final edit、流式 chunk 发送等高频路径。
  - 第一阶段若无法提供跨进程可靠投递，也应提供清晰的一次性进程内重试与失败分类。
  - 第一阶段默认启用有限次自动重试，但必须保留一键回到单次尝试行为的配置退路。
  - 第一阶段应明确接受“有限次重试可能带来少量重复消息”的风险，以换取“尽量不要静默丢失 Telegram 正式交付”。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 为 Telegram 出站引入统一的“失败分类 + 有界重试 + 结果未知标记”层。
  - 第一阶段只做进程内 retry policy，不引入持久化 outbox。
  - 对 `send_message` 与 `edit_message_text` 的短暂错误都做受控重试；明确接受少量重复风险，以降低 Telegram 侧静默丢消息概率。
- 优点：
  - 改动范围可控，能快速改善“首错即丢”和“TG 侧无任何故障反馈”的现状。
  - 不需要立即解决 durable queue / exactly-once 这类更大问题。
  - 可先用结构化日志把风险和效果看清。
- 风险/缺点：
  - 仍然无法提供跨进程恢复。
  - 对“结果未知失败”的重试会引入少量重复发送风险，需要明确接受并持续观测。

#### 方案 2（备选）
- 核心思路：
  - 直接引入持久化 outbox / delivery queue，把 Telegram 出站做成显式投递任务。
- 优点：
  - 上限更高，可进一步走向可靠投递。
- 风险/缺点：
  - 行为变化大，复杂度高，短期难以稳妥落地。
  - 需要额外解决幂等键、重复投递、重启恢复、任务清理等问题。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（失败分类）：
  - Telegram 出站错误必须先被分类为：
    - `definitive_failure`：明确未送达，可安全重试；
    - `unknown_outcome`：是否送达未知，不得盲目连续重放；
    - `non_retryable_failure`：明确不该重试。
- 规则 2（有界重试）：
  - 对 `definitive_failure`，使用小次数、带 jitter 的进程内自动重试。
  - 默认次数与退避策略应配置化，并提供稳定默认值。
- 规则 3（安全重试优先级）：
  - 优先覆盖 `send_message`、`edit_message_text`、placeholder 发送。
  - 对多 chunk 文本发送，必须区分“首 chunk 未确认”“中途某 chunk 失败”“后续 chunk 失败”的不同语义。
- 规则 4（结果未知失败）：
  - 若失败被判定为 `unknown_outcome`，必须打强日志。
  - Phase 1 对 `send_message` 与 `edit_message_text` 都允许受控重试，因为 Telegram 客户端无法收到独立故障反馈，静默丢正式交付的代价更高。
  - 这种受控重试属于明确接受的折中：可能产生少量重复消息，但优先保证送达。
  - `edit_message_text` 可额外用 `MessageNotModified` 收敛为成功等价。
  - gateway 层应能看到这是“未知结果失败”，便于后续人工判断或上层补救。
- 规则 5（流式降级）：
  - 流式中多次 edit 失败后，应进入明确降级路径，而不是持续无意义 edit。
  - 最终收口阶段若仍失败，必须给出统一失败日志与原因码。

#### Phase 1 范围冻结（Implementation Freeze）
- 覆盖路径：
  - 非流式文本：`send_text_inner(...)`
  - 流式：placeholder `send_message`、中途 `edit_message_text`、最终 `edit_message_text`、尾部分块 `send_message`
- 暂不覆盖：
  - 媒体/位置发送的通用自动重试
  - durable outbox / 跨进程恢复
  - gateway 层重排 reply target 或重新投递队列
- 范围理由：
  - 文本与流式文本是最高频路径，且最直接影响 TG 群协作链条连续性。
  - 媒体路径语义更复杂，先保留现有局部 fallback，不与 Phase 1 混做。

#### 失败分类矩阵（Phase 1 冻结）
- `RequestError::RetryAfter(n)`：
  - 归类：`definitive_failure`
  - 动作：按 `n` 秒等待后重试
  - 适用：Phase 1 覆盖的全部文本/流式文本路径
- `RequestError::Network(_)` + `edit_message_text`：
  - 归类：`unknown_outcome`
  - 动作：允许受控重试；如果后续重试返回 `ApiError::MessageNotModified`，按成功等价处理
  - 说明：这不是“明确未送达”，但编辑同一条消息到同一内容的重复尝试可通过 `MessageNotModified` 安全收敛
- `RequestError::Network(_)` + 直接发送类 `send_message`：
  - 归类：`unknown_outcome`
  - 动作：Phase 1 允许有界重试；若最终放弃，记录强日志并返回错误
  - 说明：无法确认 Telegram 是否已经接受请求，因此重试可能带来少量重复；但 Phase 1 明确选择优先减少静默丢消息
- `RequestError::Api(ApiError::MessageNotModified)` + `edit_message_text`：
  - 归类：`success_equivalent`
  - 动作：视为成功，不再继续重试
- 其他 `RequestError::Api(_)`：
  - 归类：`non_retryable_failure`
  - 动作：直接放弃并记录原因码
- `RequestError::InvalidJson { .. }`：
  - 归类：`unknown_outcome`
  - 动作：Phase 1 不自动重放，记录强日志
- `RequestError::Io(_)`：
  - 归类：`non_retryable_failure`
  - 动作：直接放弃；这通常是本地构造或文件层问题，不视作短暂 Telegram 通道故障

#### 多 chunk 文本口径冻结（Phase 1）
- 首 chunk 尚未确认前失败：
  - 可按上述失败分类矩阵处理
- 第 N 个 chunk 失败且 `N > 0`：
  - 不重发前面已确认成功的 chunk
  - 仅对当前失败 chunk 做分类处理和有界重试
  - 若最终放弃，必须记录 `delivery_state=partial_sent`
- 任一 chunk 最终放弃后：
  - gateway 仍收到错误，但日志中必须能看出是 `first_chunk_unsent` 还是 `partial_sent`

#### 接口与数据结构（Contracts）
- API/RPC：
  - Telegram outbound 内部新增重试辅助层即可，外部 trait 不新增公开 surface。
  - 建议在 `crates/telegram/src/outbound.rs` 内部新增私有枚举：
    - `TelegramOutboundOp`
    - `OutboundOutcomeKind`
    - `RetryDecision`
- 存储/字段兼容：
  - 第一阶段不新增持久化表。
- 配置：
  - 在 `TelegramAccountConfig` 增加以下字段：
    - `outbound_max_attempts: u32`
    - `outbound_retry_base_delay_ms: u64`
    - `outbound_retry_max_delay_ms: u64`
  - 实施时必须同步更新：
    - `crates/telegram/src/config.rs` 中 `impl Default for TelegramAccountConfig`
    - `crates/telegram/src/config.rs` 现有默认值/反序列化相关测试断言
- 默认值：
    - `outbound_max_attempts = 3`
    - `outbound_retry_base_delay_ms = 500`
    - `outbound_retry_max_delay_ms = 5000`
  - 解释：
    - 默认直接开启有限次自动重试，因为 Telegram 客户端无法可靠收到独立故障反馈，静默丢消息风险更高
    - 真实生效 attempt 次数使用 `effective = max(1, configured)` 口径
- UI/Debug 展示（如适用）：
  - 若后续 UI 展示 channel delivery 失败，应补 `outcome_kind` 与 `attempt/max_attempts`。

#### 日志字段冻结（Observability Freeze）
- `event=telegram.outbound.retrying`
  - 必带：`op/account_handle/chat_id/attempt/max_attempts/error_class/outcome_kind`
  - 选带：`chunk_idx/chunk_count/message_id/retry_after_secs`
- `event=telegram.outbound.gave_up`
  - 必带：`op/account_handle/chat_id/attempt/max_attempts/error_class/outcome_kind`
  - 选带：`delivery_state/chunk_idx/chunk_count/message_id`
- `event=telegram.outbound.success_equivalent`
  - 用于 `edit_message_text` 因 `MessageNotModified` 收敛为成功的场景
- gateway 侧 `channel_delivery.failed`
  - 应补带：`outcome_kind/delivery_state/op`

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 本单重点先做内部 retry 与日志，不强制本单同时改 Telegram 用户文案。
  - 但日志必须能区分 `retrying` / `gave_up_non_retryable` / `gave_up_unknown_outcome`。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 不引入持久化 outbox 时，不保留跨进程待发送队列。
  - 但每次放弃时必须留下可追踪日志，避免静默吞掉。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 仅记录 `chat_id/reply_to/message_id/chunk_idx/chunk_count/text_len` 等必要字段。
- 禁止打印字段清单：
  - token
  - 完整正文
  - 完整媒体 URL 中可能含签名的敏感参数

#### 测试切缝冻结（Test Seam Freeze）
- 推荐做法：
  - 在 `crates/telegram/src/outbound.rs` 内部引入私有 transport seam，例如 `TelegramSendApi`，生产实现包裹 `teloxide::Bot`，测试实现使用脚本化返回序列。
- seam 最小覆盖方法：
  - `send_message`
  - `edit_message_text`
  - `send_chat_action`
- 若首版不愿引入 trait：
  - 至少要抽出纯函数：
    - `classify_outbound_error(op, err) -> OutboundOutcomeKind`
    - `compute_retry_delay(err, attempt, cfg) -> Option<Duration>`
  - 但这只够覆盖分类，不足以覆盖“重试后成功”的完整行为；因此推荐 trait seam。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 文本 Telegram 出站在 `RetryAfter` 场景下不再首错即丢，具备有界自动重试。
- [x] 文本 Telegram 出站在 `send_message` 的 `Network` 场景下执行有界自动重试，而不是首错即停。
- [x] 流式 `edit_message_text` 在短暂失败下具备明确 retry 语义，并可用 `MessageNotModified` 收敛为成功等价。
- [x] 流式 placeholder / final edit / chunk 发送失败具备明确 retry 或降级语义，而不是仅靠零散日志。
- [x] 系统能区分 `definitive_failure` 与 `unknown_outcome`，并在日志中稳定输出。
- [x] gateway / telegram 日志能按 `run_id` 与 Telegram 发送步骤串联排障。
- [x] 系统明确接受并记录“少量重复消息风险”，且不会出现无限重试或不可控刷屏。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `telegram_account_config_default_includes_outbound_retry_fields`：`crates/telegram/src/config.rs`
- [x] `telegram_account_config_deserialize_uses_retry_defaults_when_unspecified`：`crates/telegram/src/config.rs`
- [x] `classify_retry_after_as_definitive_failure`：`crates/telegram/src/outbound.rs`
- [x] `classify_send_message_network_error_as_unknown_outcome`：`crates/telegram/src/outbound.rs`
- [x] `unknown_outcome_send_message_allows_controlled_retry_in_phase1`：由 `telegram_outbound_send_text_network_error_retries_then_succeeds` 覆盖，`crates/telegram/src/outbound.rs`
- [x] `unknown_outcome_edit_message_allows_controlled_retry_in_phase1`：`crates/telegram/src/outbound.rs`
- [x] `message_not_modified_after_retry_is_treated_as_success_equivalent`：`crates/telegram/src/outbound.rs`
- [x] `telegram_outbound_send_text_retries_retryable_failure_then_succeeds`：`crates/telegram/src/outbound.rs`
- [x] `telegram_outbound_send_text_network_error_retries_then_succeeds`：`crates/telegram/src/outbound.rs`
- [x] `telegram_outbound_send_text_gives_up_on_non_retryable_failure`：`crates/telegram/src/outbound.rs`
- [x] `invalid_json_unknown_outcome_is_not_replayed`：`crates/telegram/src/outbound.rs`
- [x] `outbound_max_attempts_one_disables_retry_for_network_error`：`crates/telegram/src/outbound.rs`
- [x] `telegram_outbound_stream_edit_retries_then_degrades`：已通过 `send_stream_with_transport(...)` 脚本化流式路径自动化覆盖。
- [x] `telegram_outbound_partial_chunk_failure_logs_partial_sent`：已通过 `send_stream_with_transport(...)` + 多 chunk 失败序列自动化覆盖。

### Integration
- [x] gateway channel delivery 在 Telegram outbound 首次失败、重试成功时，日志与 reply 状态保持一致。
- [x] gateway channel delivery 在 Telegram outbound 放弃时，`channel_delivery.failed` 与 Telegram outbound 原因分类可关联。
- [x] 配置 `outbound_max_attempts=1` 时，可显式关闭自动重试并退回单次尝试行为。

### UI E2E（Playwright，如适用）
- [ ] 本单第一阶段无强制 UI E2E。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 已完成 transport seam，并补齐 `send_stream` 级降级行为与多 chunk `partial_sent` 的自动化覆盖。
  - 本单已补齐 gateway 侧“出现内部重试并最终成功时，不应误记 `channel_delivery.failed`”的集成测试；当前无额外自动化缺口需要保留。
- 手工验证步骤：
  1. 在测试环境中人为制造 Telegram `send_message` 瞬时失败。
  2. 对 `RetryAfter` 场景确认日志出现 retry attempt，并在可重试场景下最终发出。
  3. 对 `send_message` 的 `Network` 场景确认系统执行有界自动重试，并输出 `outcome_kind=unknown`。
  4. 对 `edit_message_text` 的 `Network` 场景确认系统执行重试；若后续得到 `MessageNotModified`，按成功处理。
  5. 确认连续失败达到上限后系统停止重试，不发生无限刷屏。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 默认启用有限次自动重试；若需更保守，可对单 bot 账户把 `outbound_max_attempts` 下调为 `1`。
- 回滚策略：
  - 保留原单次发送路径；若观察到重复发送风险，把 `outbound_max_attempts` 调回 `1` 即可回退。
- 上线观测：
  - `event=telegram.outbound.retrying`
  - `event=telegram.outbound.gave_up`
  - `event=channel_delivery.failed`

## 实施拆分（Implementation Outline）
- Step 1:
  - 为 Telegram outbound 建立统一错误分类、`RetryDecision` 与 retry policy 封装。
  - 同步扩展 `crates/telegram/src/config.rs` 的配置字段、`Default` 实现与现有测试断言。
- Step 2:
  - 在 `outbound.rs` 内部引入私有 transport seam，并先接入文本 reply 与流式 edit / chunk 路径。
- Step 3:
  - 补 gateway 侧强关联日志、`delivery_state/outcome_kind` 透传与必要测试。
- Step 4:
  - 评估是否需要第二阶段 durable outbox。
- 受影响文件：
  - `crates/telegram/src/outbound.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/telegram/src/config.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-observability-llm-and-telegram-timeouts-retries.md`
  - `issues/issue-runner-retry-backoff-retry-after-jitter.md`
- Related commits/PRs：
- External refs（可选）：

## 未决问题（Open Questions）
- Q1: `outbound_max_attempts` 的默认值最终是否保持 `3`，还是根据线上重复消息观测下调到 `2` 更合适？
- Q2: 媒体/位置发送是否需要在 Phase 2 复用同一 retry policy，还是按媒体类型单独设计更合适？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

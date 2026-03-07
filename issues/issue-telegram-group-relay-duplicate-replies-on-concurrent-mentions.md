# Issue: Telegram 群聊 relay 并发点名导致同一结果重复回复（并发触发 / reply targets）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-07
- Owners: <TBD>
- Components: telegram / gateway / sessions
- Affected providers/models: <N/A>

**已实现（相关基础能力，写日期）**
- relay（bot@bot 点名触发）：`crates/gateway/src/chat.rs:6626`
- per-session singleflight + message queue（run active 时排队）：`crates/gateway/src/chat.rs:2350`
- reply targets：按 `session_key + trigger_id` 暂存 + per-trigger drain 投递：`crates/gateway/src/state.rs:535`
- channel status log：按 `session_key + trigger_id` 缓冲 + per-trigger drain（避免并发串线）：`crates/gateway/src/state.rs:619`

**已覆盖测试（如有）**
- per-trigger drain（成功投递）：`crates/gateway/src/chat.rs:9469`
- per-trigger drain（失败回执）：`crates/gateway/src/chat.rs:9530`
- Followup：只重放 1 条，其余回填队列：`crates/gateway/src/chat.rs:10543`
- dispatch_to_chat 入口失败清理（reply targets + logbook）：`crates/gateway/src/channel_events.rs:1858`

**已知差异/后续优化（非阻塞）**
- 本单仅聚焦“并发点名导致重复回复”的机制缺陷；不讨论 V4 的 WAIT/RootMap/TaskCard/epoch。

---

## 背景（Background）
- 场景：Telegram 群里有多个 bot。bot A、bot B 在很短时间内都用**行首点名**把任务转派给 bot C（触发 relay）。
- 修复前现状：bot C 会在群里连续发出两条几乎相同的回复（分别 reply 到 A、B 的那两条消息下面）。
- 期望：并发点名时的行为应当可解释、可控，且不要出现“同一份推理结果被重复发送”的现象。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **并发点名**（concurrent mentions）：bot A 与 bot B 在 bot C 的一次推理尚未结束时，都触发了对 bot C 的 relay。
  - Why：这是群聊协作中非常常见的节奏（多个人/多只 bot 同时把活丢给某个执行 bot）。
  - Source/Method：effective（由当前 gateway 实现语义决定）

- **reply target（回复目标）**：bot C 推理完成后，系统要把回复发回 Telegram 的哪条消息下面（reply-to message_id）。
  - Source/Method：as-sent（以实际发送成功的 message_id 为准）

- **message queue（消息排队）**：当同一个 session 已经有推理在跑时，新触发不会并发跑，而是先排队（默认 Followup：逐条重放）。
  - Source/Method：effective（默认 Followup）：`crates/config/src/schema.rs:998`

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 并发点名 bot C 时，不应出现“同一份推理结果”被重复发送到群里两次的情况。
- [x] 行为必须可解释：要么明确“会跑两次，第二次上下文更全”；要么明确“合并成一次，只回一次”；不允许现在这种“看起来跑了两次但其实像复读”的体验。

### 非功能目标（Non-functional）
- 正确性口径：
  - 必须：每条触发（A 点名 / B 点名）与最终回复的关系应当明确（至少在日志/状态里可追溯）。
  - 不得：把“同一轮推理的同一段输出”无差别地投递给多个 reply target，造成重复回复。
- 可观测性：
  - 日志至少应能看出：该次回复是由哪个触发产生、投递给了哪些 reply target、是否发生了排队与重放。
  - 建议补齐的最小字段（实现时可用 tracing fields / 结构化日志）：
    - `session_key`、`run_id`、`queue_mode`（followup/collect）、`queued`（是否入队/重放）
    - `trigger_id`（每次触发的唯一 id；入队/重放/投递/失败都带上）
    - `reply_to_message_id`（本次触发绑定的 Telegram reply-to；Collect 合并时记录“最新触发”的 reply-to）
    - `delivery_target_count` + `delivery_targets`（投递到哪些 target，便于识别“同一输出投递多次”的异常）
    - Collect 合并时建议额外记录：
      - `merged_trigger_id`（合并后的触发 id）
      - `merged_from_trigger_ids`（本次合并包含了哪些触发）

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 修复前：bot A 与 bot B 同时（或很接近）行首点名 bot C 后，bot C 在群里会连续发两条几乎相同的回复。
2) 修复前：从用户视角看，就像 bot C “重复推理并复读了两次同样的结果”，但预期更像应当逐条吸收上下文后更新，或者合并处理。

### 结论（人话版结论，便于快速对齐口径）
- 修复前实现里，确实可能出现“推理跑了两次，但只有第一次的输出被投递了两次”的组合：
  - run2 往往会被 Followup/Collect 重放触发；
  - 但 run1 结束时已经把 session 的 reply targets 全桶 drain 掉，run2 结束时可能“无处投递”，最终用户只看到 run1 的同一份输出被 reply 两次。
- 修复前这不是 Telegram 群聊特有的问题：根因是 gateway 内部“reply targets 按 session 桶聚合 + run 结束 drain 全桶”的通用设计缺陷；群聊只是更容易触发并发。

### 影响（Impact）
- 用户体验：刷屏、显得 bot “不聪明/不自洽”。
- 成本：可能造成额外推理（并发触发进入队列后会被重放；但重放后的结果可能无处投递）。
- 排障成本：很难从群消息判断到底是“推理跑了两次”还是“投递跑了两次”。
- 语义错配：B 点名触发排队后，B 的 reply-to 目标可能被 run1 顺带消费；导致“run1 的结果（更像在回答 A）”被 reply 到 B 的线程下。
- 连带副作用（放大器）：
  - 同一份输出被投递多次（回归风险），会导致 `maybe_mirror_telegram_group_reply` 被多次执行，进而让其它 bot 的 session history 出现重复镜像（影响后续推理上下文）：`crates/gateway/src/chat.rs:6941`
  - 同一份输出被投递多次（回归风险），也会导致 `maybe_relay_telegram_group_mentions` 被多次执行，从而可能对下游 bot 造成重复 relay/重复推理甚至级联（每次投递都有不同的 Telegram outbound message_id，去重 key 不一定能挡住）：`crates/gateway/src/chat.rs:6626`

### 范围（Scope）
- 不仅是“群聊”才会有：这是一个**按 session 聚合 reply targets**的通用缺陷。
  - Telegram 群聊更容易触发（多 sender 并发点名同一 bot）。
  - Telegram DM（1 对 1）也可能触发：同一个人连续快速发两条消息、bot 还在推理时第二条进入队列；两条消息各自的 reply-to 目标会被塞进同一 session 桶，导致 run1 的同一份输出被 reply 两次，然后 run2 被重放时可能“没有 pending targets”。（是否显性表现为“复读”，取决于 Telegram 客户端对 reply threading 的展示。）

### 回归验证（Reproduction / Regression）
1. 在 Telegram 群里让 bot A 发出：行首 `@c ...`（触发对 bot C 的 relay）。
2. 在 bot C 还在推理时，bot B 很快发出：行首 `@c ...`（再次触发 relay）。
3. 观察：bot C **不再**出现“同一段回复文本被连续发送两次”的复读式现象。
4. 期望 vs 实际：
   - 期望（示例口径）：要么 C 跑两次（第二次上下文包含 A+B），要么合并成一次并只回复一次；
   - 实际（验收通过口径）：Followup 下每条触发各自回执、且 reply-to 不串线；Collect 下只合并 queued 的那批触发并只回一次（默认回到 merged 触发线程）。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/ids.rs:1`：统一生成 `trigger_id`（`trg_<ULID>`）。
  - `crates/gateway/src/channel_events.rs:160`：channel 入站触发会 `push_channel_reply(session_id, trigger_id, reply_to)` 并把 `_triggerId` 传给 `chat.send(...)`。
  - `crates/gateway/src/chat.rs:6611`：Telegram 群 relay 会为目标 bot 的 session 生成 `trigger_id`，并把 reply target 绑定到 `session_key + trigger_id`。
  - `crates/gateway/src/chat.rs:2025`：`chat.send` 入口会补全 `_triggerId` 并持久化 `triggerId/mergedFromTriggerIds` 到 session history。
  - `crates/gateway/src/state.rs:535`：reply targets 按 `session_key + trigger_id` 存储与 drain。
  - `crates/gateway/src/state.rs:619`：channel status log 也按 `session_key + trigger_id` 缓冲与 drain（避免并发触发时 logbook 串线）。
  - `crates/gateway/src/chat.rs:6169`：投递时只 drain “当前 trigger”的 reply targets + status log（不再 drain 全桶）。
  - `crates/gateway/src/chat.rs:2858`：Followup 会逐条重放 queued；Collect 会合并 queued 并转移“最新 queued trigger”的 reply targets。
- 修复后语义（人话版）：
  - reply target 可以理解为“回信地址”。修复前问题本质是“不同触发的回信地址被混到同一个桶里一起消费”；修复后每个触发都有自己的 `trigger_id`，回信地址绑定到该触发。
  - 因此并发点名时：run1 的输出只会 reply 到 trigger1 的地址；后续 run2（Followup）或 merged run2（Collect）的输出只会 reply 到各自绑定的地址，不会出现“同一段输出被投递两次/串线”的体验。
  - 这不是弱网重发导致的：弱网重发是幂等/去重问题；这里修复的是“触发 ↔ reply-to 绑定关系”与 drain 粒度。

### 时间线举例（修复后，Followup 最直观）
> 目标：说明并发点名时不会复读；run2 有自己 reply-to（不串线）。

1) t0：bot A 在群里发：行首 `@c 请做 X`（触发对 bot C 的 relay）
   - relay 生成 `trigger_id`，并把“回给 A 这条消息”的 reply-to 绑定到 `target_session_id + trigger_id`：`crates/gateway/src/chat.rs:6611`
   - 然后启动 C 的推理 run1（`chat.send`）
2) t1：run1 还在跑，bot B 很快又发：行首 `@c 请做 Y`（再次触发 relay）
   - 生成另一个 `trigger_id`，把“回给 B 这条消息”的 reply-to 绑定到同一 session，但**不同 trigger**：`crates/gateway/src/state.rs:535`
   - 由于 C 的 session 正在跑，B 这次触发进入队列（Followup）：`crates/gateway/src/chat.rs:2350`
3) t2：run1 结束，系统投递回复
   - `deliver_channel_replies` 只会 drain “本次 trigger”的 targets：`crates/gateway/src/chat.rs:6169`
   - 因此 run1 的输出只 reply 到 A 的线程，不会顺带 reply 到 B
4) t3：Followup 重放 B 的触发（run2）：`crates/gateway/src/chat.rs:2858`
5) t4：run2 结束 → 只 reply 到 B 的线程（Y 的结果；上下文通常更全）

### 代码核实：为什么“第二次推理不会无处投递”
- 投递 drain 粒度已从 “session 全桶” 改为 “session + trigger”：
  - `deliver_channel_replies(..., trigger_id, ...)`：`crates/gateway/src/chat.rs:6169`
  - `drain_channel_replies(session_key, trigger_id)`：`crates/gateway/src/state.rs:553`
- 失败回执同样按 trigger drain（避免把别的触发 targets 一起清掉）：`crates/gateway/src/chat.rs:4219`

### 与 MessageQueueMode（Followup/Collect）的关系（修复后）
- **根因已修复**：reply targets/status log 已绑定到 trigger；并发触发不再因 drain 粒度导致复读式重复投递或“重放无处投递”。
- **队列策略只影响‘额外推理次数’与‘合并语义’**：
  - Followup：逐条重放，每条触发各自回执（最直观）。
  - Collect：只合并 run active 期间 queued 的那批触发，生成 merged trigger，并转移“最新 queued trigger”的 reply-to 到 merged trigger。
  - 失败/超时/空输出：不会吞掉后续 queued；会继续 drain + replay（除非触发连续失败熔断）：`crates/gateway/src/chat.rs:2804`

### 明确回答（用于对齐 2 个确认点）
1) 上述问题是否只有群聊才存在？
   - 否。只要同一个 session 在 “run active” 期间再次被触发（并发或紧密连续），就可能发生；群聊更常见只是因为多人/多 bot 并发更高。
2) 上述问题是否由 Followup/Collect 策略导致？
   - 否。根因是 reply targets 作为 session 级别桶被 drain；队列策略只决定“后续触发怎么重放/合并”，并不修复串线。

## 根因分析（Root Cause）
- A. relay 触发路径把 reply target **立即**写入 session 的“待回复目标列表”。
- B. message queue 只对“触发消息”排队，但 reply targets 不随触发消息一起排队/绑定。
- C. 推理结束后按 session 维度 drain targets，并把同一份输出广播式投递给所有 targets → 造成重复回复与语义错配。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 并发点名时，系统必须明确选择一种策略，并保证输出不出现“复读式重复回复”。
  - 每个触发对应的 reply target 必须与该触发的处理结果绑定（至少在实现上不应被别的触发“顺带消费掉”）。
- 不得：
  - 不得把同一轮推理的同一段输出，无差别投递给多个触发目标，造成重复。

### 推荐口径（冻结建议）
> 明确两套口径：Followup（默认、最直观）与 Collect（去抖合并、节省推理/刷屏）。

#### Followup（默认）：逐条处理，逐条回复（每条触发都有自己的回执）
**规则（必须/应当）**
- 必须：当同一 session run active 时，后续触发进入队列；run 结束后按顺序逐条重放触发（Followup）。
- 必须：每次重放触发所产生的输出，只能投递到“该触发绑定的 reply-to 目标”（不可跨触发/跨线程投递）。
- 应当：后续触发的推理上下文会自然包含前序触发的对话与输出，因此第二轮回复通常应当比第一轮更“更新/更全”，而不是复读。

**时间线举例（Followup）**
1) t0：A：`@c 请做 X` → C 启动 run1，reply-to 绑定 A 这条
2) t1：run1 期间 B：`@c 请做 Y` → B 进入队列（Followup），reply-to 绑定 B 这条
3) t2：run1 结束 → 只回复到 A 线程（X 的结果）
4) t3：重放 B 触发 → 启动 run2（处理 Y）
5) t4：run2 结束 → 只回复到 B 线程（Y 的结果；上下文通常更全）

#### Collect（去抖合并）：只合并 run active 期间新增的 queued 触发
**规则（必须/应当）**
- 必须：Collect 合并的是“run active 期间新到的 queued 触发”，不包含已经启动 run1 的那条触发（避免语义混乱）。
- 必须：合并后的重放只触发一次推理（run2），并只投递一次回复（默认 reply 到“这批 queued 触发里最新那条”的线程）。
- 应当：在 run2 回复开头用一句很短的说明提示“已合并处理刚才的补充点名/触发（列出来源）”，避免被合并者误以为被吞。

**时间线举例（Collect）**
1) t0：A：`@c 请做 X` → C 启动 run1，reply-to 绑定 A 这条
2) t1：run1 期间 B：`@c 另外请做 Y` → B 入队（Collect），reply-to 绑定 B 这条
3) t2：run1 期间 D：`@c 再补充 Z` → D 入队（Collect），reply-to 绑定 D 这条
4) t3：run1 结束 → 只回复到 A 线程（X 的结果）
5) t4：把 queued 的 B:Y + D:Z 合并成一次重放 → 启动 run2
6) t5：run2 结束 → 只回复到 D 线程（合并后的结果；开头注明“合并处理：B、D”）

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：把 reply target 与 queued trigger 绑定（而不是挂在 session 桶上）
- 核心思路：
  - 当 run active 导致触发进入队列时，把“这次触发对应的 reply target”一起放进队列条目里；
  - 重放该 queued trigger 之前再 push 对应 reply target；
  - 推理结束后只投递“本次触发绑定的 reply target”（或按绑定集合投递），不再 drain 全桶。
- 优点：
  - 最符合直觉：A 的触发就回 A；B 的触发（重放后）就回 B。
  - 不需要改变 relay 的语义（仍可 followup/collect）。
- 风险/缺点：
  - 需要改动 reply targets 的存储与投递接口（从“按 session 暂存”走向“按触发绑定”）。
  - 需要定义触发标识（trigger_id）与 Collect 合并后的追溯字段，否则日志仍难对齐“哪条触发对应哪次投递”。

#### 方案 2（备选）：并发触发直接 Collect 合并，只跑一次并只回一次（固定回复到“最新触发”的线程）
- 核心思路：
  - 当同一 session 已在跑时，后续触发不排 Followup，而是 Collect 合并；
  - 最终只回复一次（例如回到最新触发的那条消息下面），避免重复。
- 优点：实现可能更少、成本更低。
- 风险/缺点：A 的触发可能得不到直接回执（被合并掉），需要产品口径接受。

### 最终方案（Chosen Approach）
- 采用“方案 1：reply target 与 queued trigger 绑定”（修复根因），并遵循上文冻结的 Followup/Collect 口径（队列策略不变，只修正 reply-to 绑定与投递语义）。

#### `trigger_id`（触发标识）建议（已采纳）
- 目标：让每次触发从“入队 → 重放/合并 → run → 投递/失败”全链路可追溯，且让 reply-to 绑定在触发上，而不是绑在 session 桶上。
- 生成位置（推荐）：在 `chat.send(...)` 入口统一生成（若 `params` 内没有则补一个）；若上游已显式提供则保留。
- 格式建议：ULID / UUIDv7 均可（例如 `trg_01...`），要求全局唯一、近似时间有序、可打印。
- 传播规则（建议落在 `params` 上，便于与队列/重放复用）：
  - `params["_triggerId"]`：单次触发 id（Followup 重放时保持不变）
  - Collect 合并重放：生成新的 `params["_triggerId"]` 作为 `merged_trigger_id`，并附带 `params["_mergedFromTriggerIds"] = [..]`

#### 失败/空输出时的队列处理口径（已采纳）
- `silence`（输出空）：视为“本触发已处理完但无需回复”。必须清理该触发绑定的 reply targets，然后继续处理后续 queued 触发（不能因为 silence 让队列卡死或被丢弃）。
- `失败/超时`：
  - 必须：对“本触发绑定的 reply targets”发一次错误提示（类似当前 `handle_run_failed_event` 行为，但不能 drain 其它触发的 targets）。
  - 应当：失败后仍继续重放后续 queued 触发（避免把后续触发直接吞掉）。
  - 应当：增加一个很小的“连续失败上限/熔断”（例如连续 2～3 次失败就暂停/清空队列，并对剩余触发各给出一次简短回执，提示稍后重试），避免错误刷屏与无限重放。

#### 默认参数（已冻结）
- `trigger_id`：默认使用 ULID（时间有序 + 可读性更好，便于排障）。
- 连续失败上限：默认 2。
- 可观测性展示：默认只写结构化日志，不要求 UI/Debug 面板展示（后续需要再加）。
- 连续失败上限触发时的回执文案（中文，1 句，建议先用此默认值）：
  - `⚠️ 我这边连续出错，已暂停处理后续请求；请稍后重试或重新 @我。`
- `trigger_id` 持久化：需要持久化到 session history（用于重启后排障与回溯），同时保留结构化日志字段。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] A、B 并发行首点名 C 时，不再出现“同一段回复文本被发送两次”的现象。
- [x] 若采用 Followup（逐条处理）口径：C 对 A 与 B 的回复应当可解释（至少不会出现完全相同且无法区分来源的复读）。
- [x] 日志可定位：一次回复对应哪个触发、投递给哪个 reply-to message_id。
- [x] 覆盖范围必须包含：Telegram 群聊 + Telegram DM（1 对 1）+ 其它 channel（所有经 `ChannelEventSink::dispatch_to_chat` 触发 `chat.send` 的渠道），均不得出现“串线/复读式多投递/重放无处投递”的问题。
- [x] `silence` 时不应吞队列：某次触发返回 silent 后，后续 queued 触发仍能按 Followup/Collect 口径继续推进。
- [x] `失败/超时` 时不应吞队列：某次触发失败后，后续 queued 触发仍能继续推进（或在达到连续失败上限后给出明确回执）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 新增覆盖：并发 relay 触发时 reply targets 不应被“全桶 drain”误消费：`crates/gateway/src/chat.rs`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：需要模拟 Telegram 群内并发 relay 的触发序列与 reply-to threading。
- 手工验证步骤：
  1) A 行首 `@c ...`，紧接 B 行首 `@c ...`；
  2) 观察 C 的回复是否仍然出现“复读式重复”；
  3) 观察是否符合选定口径（Followup 两轮更新 / Collect 单轮合并）。
  4) Telegram DM（1 对 1）：同一人快速连续发两条消息（X、Y），确保最终不会出现“run1 输出回复两次 / run2 无处投递”的串线现象；Followup 下应当各自有回执，Collect 下应当只合并 queued 的那批触发。
  5) 其它 channel（如已配置）：重复上述“紧密连续两条触发”的场景，确认行为与日志同样可追溯且无串线。

## 交叉引用（Cross References）
- Related docs：
  - `issues/discussions/telegram-group-at-rewrite-mirror-relay-as-is.md`
  - `issues/discussions/design-telegram-group-multi-bot-nl-collaborative-orchestration-v4.md`
- Related code：
  - `crates/gateway/src/chat.rs:2350`（message queue）
  - `crates/gateway/src/chat.rs:6169`（deliver_channel_replies drain）
  - `crates/gateway/src/chat.rs:6626`（relay）
  - `crates/gateway/src/channel_events.rs:160`（channel 入站触发 push reply target）
- History (git):
  - 引入 session-scoped `channel_reply_queue` + drain-all 投递设计：`153178b5f8b759fe6bcf746669364a5e11076e33`（Fabien Penso，2026-01-31，`feat(telegram): per-channel sessions, slash commands, and default model config`）
  - 引入 Telegram 群 bot-to-bot relay，并把 reply target 塞进目标 session 桶：`7e8e212dda7f314f10766025748899ab8247d4bd`（luy，2026-02-22，`feat(telegram): relay bot-to-bot @mentions in group chats`）
  - 并发 send 的队列重放策略（Followup/Collect）：`667acb386a8510b28e1c476c9f2e1d483e27d918`（`feat(chat): add message queue modes for concurrent send handling`）

## 发布与回滚（Rollout & Rollback）
- 发布策略：作为 bugfix 直接启用（当前无 feature flag；如需灰度可后续补）。
- 回滚策略：回滚相关 commit 恢复旧行为（会复现复读式重复回复，但保证可用性）。
- 上线观测：日志应能定位“触发 id → reply target → 投递结果”，并能看出是否发生合并/重放。

## 实施拆分（Implementation Outline）
- Step 1: 明确口径：Followup 两轮推理 vs Collect 合并一次（在本文 “Chosen Approach” 冻结）。
- Step 2: 调整 reply targets 的存储/投递边界（从 session bucket 改为 trigger-bound，或采用明确的合并策略）。
- Step 3: 补齐可观测性：触发 id、队列条目 id、投递的 reply-to message_id 列表。
- Step 4: 修正失败/空输出的队列策略：silence/失败不吞后续 queued；增加连续失败上限。
- Step 5: 持久化 `trigger_id`/`merged_from` 到 session history（至少覆盖 channel 入站与 relay 注入两条路径）。
- Step 6: 最小化测试与手工验收清单（覆盖并发触发 + 不复读 + 不吞队列）。
- 受影响文件（预估）：
  - `crates/gateway/src/chat.rs`
  - `crates/config/src/schema.rs`（若需要新增/调整队列模式或策略开关）

## 未决问题（Open Questions）
- Q1: `trigger_id/merged_from` 的持久化位置选哪一层（`params`/session JSONL message 字段/`channel` meta 字段）以最小化侵入且便于回溯？
  - Resolved: 写入 session JSONL message 顶层字段（`triggerId`、`mergedFromTriggerIds`），并在运行参数中继续使用 `params["_triggerId"]`/`params["_mergedFromTriggerIds"]` 贯穿入队与重放。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] 触发与回复关系可追溯（日志/状态可定位）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

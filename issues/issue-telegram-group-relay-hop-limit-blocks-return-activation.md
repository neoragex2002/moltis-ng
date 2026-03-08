# Issue: Telegram 群聊 relay 在 hop 超限时阻断派发，导致“员工 bot 行首点名 PM bot”无法激活（relay_hop_limit / return-to-PM）

## 实施现状（Status）【增量更新主入口】
- Status: IN-PROGRESS
- Priority: P1
- Updated: 2026-03-08
- Owners: 
- Components: gateway / telegram
- Affected providers/models: openai-responses::gpt-5.2（与根因无关，仅为现场日志上下文）

**已实现（如有，写日期）**
- 现状行为（hop_limit early-return + relay chain）：`crates/gateway/src/chat.rs:6757`
- 2026-03-08（短期止血）：Web UI 放宽 hop_limit 输入上限（至少到 65536）+ 后端类型链路升级为更大整数，避免 UI 能填但后端失败：`crates/gateway/src/assets/js/page-channels.js`、`crates/telegram/src/config.rs`
- 2026-03-08（短期可观测性）：当 hop_limit 阻断且存在“行首 directive 候选”时，输出 reason-code 日志 `relay_skip_reason=hop_limit_exceeded`：`crates/gateway/src/chat.rs`
- 2026-03-08（短期保险丝）：引入 `epoch_relay_budget`（默认 128，短期 epoch=relayChainId），预算耗尽后停止 relay 并输出 `relay_skip_reason=epoch_budget_exceeded`（同一 chain 仅一次）：`crates/telegram/src/config.rs`、`crates/gateway/src/state.rs`、`crates/gateway/src/chat.rs`

**已覆盖测试（如有）**
- relay 提取/strictness/基础覆盖：`crates/gateway/src/chat.rs:9150`
- hop_limit 超限时不做 loose labeling 且不派发：`crates/gateway/src/chat.rs`
- `epoch_relay_budget`：预算阻断/不消耗缺失 target/dispatch 失败退款：`crates/gateway/src/chat.rs`

**已知差异/后续优化（非阻塞）**
- `epoch_relay_budget` 的 UI 配置入口暂未提供（当前仅后端默认 128 + 可通过配置更新）；是否要在 Web UI 增加可视化配置见 Open Questions。
- 中期三原则（return-to-root 等语义）尚未实现，短期仍依赖用户把 `relay_hop_limit` 配置到足够大来覆盖常见 PM 串行点名工作流。

---

## 背景（Background）
- 场景：Telegram 群内 PM bot（`cute_alma_bot`）按顺序点名多个员工 bot；员工 bot 需要通过“行首点名 PM”把交付回传给 PM，从而激活 PM 继续推进。
- 约束：Telegram bot 无法 bot-to-bot 直接私聊；当前通过 gateway 的 **outbound relay 扫描**实现 bot-to-bot “派活/回执”注入。
- Out of scope：本单不讨论 transcript 文本协议（TG-GST v1）本身的格式优雅性；仅讨论 relay 链路为何未触发（导致 `-> you` 缺失/未激活）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **relay 注入**：gateway 扫描某 bot 的出站文本，识别出对其它 bot 的“行首点名指令”，将任务文本以 `channel.relay=true` 写入目标 bot 的 session，并触发目标 bot 推理排队。
  - Why：这是 bot-to-bot 协作的“激活/派活”路径。
  - Not：不是 mirror 旁观记录；mirror 不会触发推理。
  - Source/Method：as-sent（以实际写入 session 的 `channel.relay` 字段为准）
- **mirror 旁观记录**：将某 bot 的群内出站回复以 `channel.mirror=true` 写入其它 bot session，供“看见发生了什么”。
  - Why：让所有 bot 共享群内上下文。
  - Not：不触发推理/不代表被点名。
  - Source/Method：as-sent
- **relay hop / hop_limit**：relay 链路上的跳数与上限。当前实现中，如果当前入站属于 relay（带 `relayHop`），则该 bot 的下一次 relay 出站会把 hop 加 1；当 `next_hop > relay_hop_limit` 时会阻断 relay 派发（可选择仅做本地解析用于日志），从而不产生 relay 注入。
  - Why：用于防止 bot ping-pong/级联 relay。
  - Not：不是 Telegram 本身的“转发次数”。
  - Source/Method：effective（以 gateway 运行时 snapshot + `channel.relayHop` 为准）

- **authoritative**：来自 provider 返回或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件/DB 原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入 session / 实际派发出去的值

### hop 的明确定义（当前实现，冻结口径）
- **hop 的含义**：同一条 relay 链（`relayChainId`）里，已经发生过的 **relay 注入（`channel.relay=true`）** 次数计数。
- **单调性**：hop 是**单调递增计数器**；每发生一次 relay 注入就 `+1`。
- **不带策略**：当前实现**不存在** `hop--` / “深度（depth）” / “返程抵消”等语义；不论 A→B 还是 B→A，只要是 relay 注入就 `+1`。
- **不统计哪些事件**：
  - 不统计 mirror（`channel.mirror=true`）；
  - 不统计 Telegram 原生 update（人直接 @bot 触发的入站）；
  - 不统计任何未走 relay 注入的“旁观/转写”。
- **链路延续条件**：只有当“本次 run 的最近一条 user 消息本身是 relay 注入（`channel.relay=true`）”时，才会把这次出站当作同一条 relay 链继续计算 `next_hop=inbound_hop+1`；否则会新起链 `hop=1`。

### hop 示例（人话时间线）
假设 `relay_hop_limit=3`，且每一步都发生了“真实 relay 注入”：
1) A 在群里行首点名 B（A 出站被扫描）→ 注入到 B：`hop=1`
2) B 在群里行首点名 A 回执（B 出站被扫描）→ 注入到 A：`hop=2`
3) A 再行首点名 C（A 出站被扫描）→ 注入到 C：`hop=3`
4) C 再行首点名 A 回执（C 出站被扫描）→ 这一步计算 `next_hop=4`，由于 `4 > 3`，当前实现会阻断 relay 派发，导致 **不再注入 A**（只能剩 mirror 旁观记录）。

### epoch 与 budget（新口径，用于防“自激发”且不依赖 hop 语义）
> 背景：即使将 hop 改成“回到 root 清零”的语义，`root <-> 员工` 仍可能在 prompt/模型行为失控时形成无限往返自激发，因此需要一个“保险丝”。

- **epoch**：一段“可连续发生 relay 注入”的协作区间。
  - 中期目标（有 root 配置后）：以 “return-to-root（注入目标为 root）” 作为 epoch 边界；回到 root 即开启新 epoch。
  - 短期（无 root 配置时）：用 `relayChainId` 作为粗粒度 epoch（即每条 relayChainId 视为一个 epoch）。
- **epoch_relay_budget（每个 epoch 的最大 relay 注入数上限）**：
  - 含义：在同一个 epoch 内，允许发生的 **relay 注入（`channel.relay=true`）总次数**上限（budget），与 hop 的“深度/策略语义”无关。
  - 目的：阻断“无新信息也会无限互相点名触发”的自激发（例如 `root -> A -> root -> A -> ...`）。
  - 触发动作：当 budget 用尽时，停止继续派发 relay（仍允许 mirror 旁观），并必须打 reason 日志（脱敏）。

#### epoch_relay_budget 示例（人话）
场景：`R` 为 PM/root（或逻辑上的主持者），`A` 为员工。
1) `R -> A`（派活，触发 A 推理）
2) `A -> R`（回执，触发 R 推理）
3) `R -> A`（追问/补充，触发 A 推理）
4) `A -> R`（再次回执，触发 R 推理）
若两边 prompt 写得“永远追问/永远回执”，上述 2-cycle 会无限重复。此时设置 `epoch_relay_budget=128`：
- 在同一 epoch 中最多允许 128 次 relay 注入；第 129 次开始停止派发（但仍 mirror），并输出：
  - `relay_skip_reason=epoch_budget_exceeded`
  - `relay_chain_id=... budget=128 used=128`（以及 chat_id/source ids）

#### epoch_relay_budget 口径冻结点（必须写清，否则实现会飘）
> 说明：budget 是“保险丝”，要做到 **既能熔断自激发**，又不会因为误计数导致“正常协作被误伤”。

1) **budget 的计数单位是什么**
   - 建议单位：每一次“成功派发的 relay 注入（per target）”记为 1（一次注入 = 写入目标 session + 触发一次推理排队）。
   - 示例：
     - 出站文本：`@botB 做X；@botC 做Y` → 解析出 2 条 directive → budget 消耗 2。
     - 出站文本：`@botB 做X；@botB 做X` → 去重后只派发 1 条 → budget 消耗 1。

2) **budget 何时消耗（attempt vs success）**
   - 建议：仅在“去重之后且实际派发成功”时消耗（success-based），避免：
     - 目标 bot 不存在/不在群里/无 session 导致 dispatch 失败，却把 budget 烧光；
     - 重试/重复文本被 dedupe 掉，却把 budget 烧光。
   - 示例：
     - 出站包含 `@ghost_bot 做X`（无法 resolve / 无 session）→ dispatch 不发生 → budget 不应减少，但应记录一次 `relay_skip_reason=target_session_missing`（脱敏）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 当员工 bot 在群里以“行首点名 PM bot”的方式提交交付时，PM bot 必须能被激活（即 PM session 里应出现 `channel.relay=true` 的注入消息 / 或等价的触发信号）。
- [x] **短期**：当“回执到 PM/root”的 relay 因 hop_limit 等策略被拦截时，必须有明确可观测性（reason code 日志），避免用户感知为“行首点名不稳定/偶现失效”。
- [ ] **中期**：落地三原则语义后，“return-to-root”不应再被 hop_limit 这类计数口径误伤（除非触发 `epoch_relay_budget` 熔断）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：对“return-to-PM/回执”给出明确且可解释的 hop 行为（避免用户靠手工 `@PM 继续` 救场）。
  - 不得：为了解决回执而放开无限 relay，导致 ping-pong 自激发。
- 兼容性：默认配置不应让常见 PM→员工→PM→员工→PM 工作流“必然断链”。
- 可观测性：当 relay 因 hop_limit 被跳过时，必须显式记录到 log（不包含敏感文本/不打印全量正文），至少包含：
  - `relay_skip_reason=hop_limit_exceeded`
  - `chat_id` / `source_account_id`
  - `chain_id` / `inbound_hop` / `next_hop` / `hop_limit`
  - `source_outbound_message_id`（便于和 Telegram outbound send 的 message_id 对齐）
  - 建议仅在 `outbound_text` 含 `@` 时输出，避免过量噪声
  - 建议 log message：`telegram outbound relay skipped`（字段用结构化 fields）

#### 兼容性/迁移说明（冻结口径）
- `relay_hop_limit` 类型链路升级：由 `u8` → `u32`（端到端支持更大 hop_limit，例如 Web UI 允许到 `65536`）。
  - 迁移风险：低（SQLite/JSON 的数字字段可被 `u32` 接受；旧值 `3` 等保持不变）。
  - 回滚风险：若回滚到旧版本（`u8`），而 DB 中已写入 `>255` 的值，会导致反序列化失败或被 clamp（取决于旧实现）；建议回滚前先把值降回 <=255。
- 新增 `epoch_relay_budget`（`u32`，默认 128）：
  - 行为变化：会对“自激发无限 relay”链路产生熔断（停止 relay 派发），但不影响 mirror；并新增 reason-code 日志用于排障。
  - 回滚风险：回滚到旧版本会忽略该字段（若旧版严格反序列化则需核实）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 员工 bot 在群里发出“行首 @cute_alma_bot …”交付文本（看上去格式完全正确），但在 PM（alma）侧 session 中只出现 mirror 旁观记录：
   - `fluffy_tomato_bot(bot): @cute_alma_bot 我补 2 点...`
   - 缺少 `fluffy_tomato_bot(bot) -> you: ...` 这样的 relay 注入，因此 PM bot 未被激活继续推进。
2) 用户（Neo）不得不手工补发 `@cute_alma_bot 继续` 才能激活 PM，体验非常差。

### 影响（Impact）
- 用户体验：需要人肉介入“叫醒 PM”，破坏自动协作闭环。
- 可靠性：链路达到一定深度后，回执路径高概率被截断。
- 排障成本：若缺少 hop_limit 阻断的 reason 日志，很难从日志直接定位原因（现已补齐 reason-code 日志）。

### 复现步骤（Reproduction）
1. Telegram 群内，PM bot 行首点名员工 bot A：`@botA 做 X` → 触发 relay 注入给 A（hop=1）。
2. botA 行首点名 PM 回执：`@pm_bot 交付 ...` → 触发 relay 注入给 PM（hop=2）。
3. PM bot 再行首点名员工 bot B：`@botB 做 Y` → 触发 relay 注入给 B（hop=3）。
4. botB 行首点名 PM 回执：`@pm_bot 交付 ...`。
5. 期望 vs 实际：
   - 期望：PM session 出现 `channel.relay=true` 的注入（`-> you`），PM 被激活。
   - 实际：PM session 只看到 mirror（无 `-> you`），PM 未被激活；需要人手工 `@pm_bot 继续`。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/chat.rs:6757`：relay 扫描入口（在 Telegram 出站成功后调用 `maybe_relay_telegram_group_mentions(...)`）。
  - `crates/gateway/src/chat.rs:6822`：当 `next_hop > relay_hop_limit` 时执行“hop_limit 拦截分支”，不会继续派发 relay（仅记录 reason-code 日志）。
  - `crates/gateway/src/chat.rs:6405`：`load_telegram_relay_inbound_context(...)` 只要“最近一条 user 消息是 relay 注入”，就会把 `relayHop` 作为 inbound_ctx，影响后续 next_hop 计算。
- 配置/协议证据（现场采样，2026-03-08）：
  - 本机 `~/.moltis/moltis.db` 的 `channels` 表（telegram 三个账号）显示均为：
    - `relay_hop_limit = 3`
    - `relay_strictness = strict`
    - `relay_chain_enabled = true`
    - `chan_user_name = cute_alma_bot / lovely_apple_bot / fluffy_tomato_bot`
  - Web UI 配置面板（历史问题，已修复）：
    - 旧：`crates/gateway/src/assets/js/page-channels.js` 的 `Relay Hop Limit` 输入框设置了 `max="10"`，导致 UI 无法填入更大的 hop_limit
    - 新：已放宽到 `max="65536"`（并保持 `min="1"`）
  - 后端类型上限（历史问题，已修复）：
    - 旧：`crates/telegram/src/config.rs` 中 `relay_hop_limit` 为 `u8`（上限 255）
    - 新：已升级为更大整数类型（当前为 `u32`），端到端支持 `65536`
  - 员工（fluffy）被激活的入站本身已处在 hop=3（因此它后续回 @cute_alma_bot 时 next_hop 会变成 4）：
    - 某次会话片段中存在 `channel.relay=true` 且 `relayHop=3` 的入站（来自 `@cute_alma_bot` 对 `@fluffy_tomato_bot` 的派活）。
- 日志证据（现场摘录）：
  - `telegram outbound text sent account_handle="telegram:8576199590" chat_id="-5288040422" reply_to=Some("526") ...`（fluffy 已成功发到群里）
  - 同时缺少紧随其后的 `moltis_gateway::chat: telegram outbound relay: dispatched ... target_account_id="telegram:8704214186"`（说明 relay 未派发回 alma）
- 当前测试覆盖：
  - 已有：`crates/gateway/src/chat.rs:9150`（relay strictness/relay 提取相关）
  - 已新增：`epoch_relay_budget`（预算阻断/缺失 target 不消耗/dispatch 失败退款）：`crates/gateway/src/chat.rs`
  - 缺口：真实 Telegram 环境的集成验收（确认日志与 outbound message_id 的关联、以及 PM 串行点名闭环在不同 hop_limit 下的体验差异）。

## 根因分析（Root Cause）
- A. 触发条件：员工 bot 的本次出站回复属于一个已有 relay chain（inbound_ctx 存在），且该 inbound_ctx 的 `relayHop` 已经等于配置上限（例如 hop=3, limit=3）。
- B. 中间逻辑缺陷：`maybe_relay_telegram_group_mentions(...)` 在 `next_hop > relay_hop_limit` 时阻断 relay 派发（`next_hop = hop+1`），导致“回执到 PM”这类必要 relay 也会被截断（短期通过提高 hop_limit + reason 日志止血，中期用三原则语义解决）。
- C. 下游表现：
  - relay 注入未发生 → PM session 不会出现 `channel.relay=true` 的入站 → PM 不会入队推理。
  - 但 mirror 旁观记录仍会发生，所以 PM “能看见”员工发言，却“不会被叫醒”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - “员工 bot 行首点名 PM bot 的交付回执”不应出现**无声失败**：若出站文本里存在可派发的 relay directive，但因 hop_limit / budget 等策略被拦截，必须有明确 reason-code 日志可追溯。
  - 默认/推荐配置必须能覆盖最常见的 PM 工作流（至少支持 `PM->A->PM->B->PM` 这种两名员工的串行点名闭环）；否则必须在文档/日志中明确提示“当前 hop_limit 配置不足，需要提高”。
  - 必须具备“自激发保险丝”：以 `epoch_relay_budget` 形式限制同一协作区间内的 relay 注入总量，避免无限循环把系统跑爆。
- 不得：
  - 不得导致 bot 间 ping-pong 无限互相激活。
- 应当：
  - 当 hop_limit 阻断 relay 时，应当有明确 reason 日志（例如 `relay_skip_reason=hop_limit_exceeded`），用于排障。
  - 当 budget 阻断 relay 时，应当有明确 reason 日志（例如 `relay_skip_reason=epoch_budget_exceeded`），用于排障。

### 三条原则（目标语义，先讨论后实现）
> 说明：这三条原则是“我们想要的群聊协作语义”。中期需要配合 root/群聊配置落地；短期可先通过提升 hop_limit + budget 保险丝止血。

1) **回到 R 清零（Return resets epoch/hop）**
   - 含义：只要 relay 注入最终回到 R（PM/root），就视为“本轮闭环”，下一轮从 R 再出发不应被历史 hop/链路长度惩罚。
   - 示例（路径）：`R A B C A D B R`
     - 允许：`R->A, A->B, B->C, C->A, A->D, D->B, B->R`
     - 当 `B->R` 发生后，下一步 `R->A` 应被视为新一轮（不因上一轮走得“长”而被拦）。

2) **return-to-root 永远允许（Return-to-root must always deliver）**
   - 含义：无论当前“非 root 扩散跳数/深度”已经多大，只要目标是 R，就必须允许把回执 relay 注入送达 R（否则用户会被迫手工 `@R 继续`）。
   - 示例（超限也要能回 R）：
     - 设 `N=3`（非 root 扩散上限），已发生：`R->A (1), A->B (2), B->C (3)`。
     - 此时：
       - `C->D`（非 root 扩散第 4 跳）应当被拦：`relay_skip_reason=non_root_limit_exceeded`
       - 但 `C->R`（回到 root）必须放行并触发 R 推理（即使“已经很深”）。

3) **非 root 扩散要受限（Non-root expansion bounded by N）**
   - 含义：任何“从 R 出发、尚未回到 R”的 relay 扩散，目标不是 R 的那部分，跳数不得超过 N（用来控制员工间互相点名导致的级联扩散）。
   - 示例（N=2）：
     - 允许：`R->A (1), A->B (2)`
     - 拦截：`B->C (3)`（非 root 扩散超过 N），但 `B->R` 仍必须允许（见原则 2）。

## 方案（Proposed Solution）
### 短期（止血 + 排障可见 + 保险丝）
- 目标：在不引入“root 配置/群聊角色配置”的前提下，避免再次出现“员工行首点名 PM 但 PM 没被激活且无日志”的体验，同时加上自激发熔断保险丝。
- 工作项：
  0) **止血操作必须“可配置可落地”**（避免只写在文档里）
     - Web UI 允许用户把 `relay_hop_limit` 设置到足够大的数值（至少支持 `65536`），不要用 UI 的 `max` 把用户卡死；
     - 配置/存储/运行时类型口径要一致（避免 UI 能填但后端反序列化失败或 silently clamp）：
       - 现状：后端已升级为更大整数类型（当前为 `u32`）
       - 目标：端到端支持 `65536`（需升级类型并补齐测试/验收）
  1) **hop_limit 拦截必须打日志（低噪声）**
     - 仅在“确实存在 relay 候选（例如 `extract_relay_groups(...)` 非空）但被 hop_limit 拦截”时记录；
     - 说明：为做到低噪声且不误报，实现上需要在 hop_limit early-return 之前做一次**纯本地解析**（不调用 LLM）来确认“是否真的存在 relay directive”；
     - reason code：`relay_skip_reason=hop_limit_exceeded`
     - 建议仅记录一次/每条出站消息一次（按 `source_outbound_message_id` 去重即可）。
  2) **配置止血建议**：文档建议将 `relay_hop_limit` 从 3 提升到 4/5（现场可用）。
  3) **epoch_relay_budget 保险丝（epoch=relayChainId）**
     - 在同一 `relayChainId` 内限制 relay 注入总量，超限即熔断：停止派发 relay（仍 mirror）；
     - reason code：`relay_skip_reason=epoch_budget_exceeded`；
     - 降噪：只在“第一次耗尽 budget 的那一刻”记录 1 条日志（同一 chain 后续不重复刷屏）。

### 中期（随“群聊配置/Root/角色”一并实现三原则）
- 目标：实现“回到 R 清零、return-to-root 永远允许、非 root 扩散受限（N）”三条协作语义。
- 依赖：群聊配置能力（群内 bot 列表 + 谁是 PM/root），并在 relay 元数据中传播 root/epoch 信息。

### 最终方案（Chosen Approach）
- 冻结决策（2026-03-08）：
  - **允许 root ↔ 员工 往返**：`root -> employee -> root -> employee ...` 这类“协调者（PM）主持的往返协作”是合理工作流，不应被 loop guard 误伤为“危险环”而无声截断。
  - **短期接受 epoch=relayChainId**：在 root/群聊配置缺失的前提下，先用 `relayChainId` 作为 epoch 粗粒度边界落地 `epoch_relay_budget`（保险丝优先落地）。
- 分阶段计划（短期 / 中期）：
  - **短期（不引入 root 配置）**：
    - 止血：建议提升 `relay_hop_limit`（例如 3→4/5），避免 PM 串行点名被误伤断链。
    - 可观测性：补齐 hop_limit 拦截日志（`relay_skip_reason=hop_limit_exceeded`）。
    - 保险丝：引入 `epoch_relay_budget`，短期将 epoch=relayChainId（不依赖 root 配置），用于阻断自激发无限循环。
  - **中期（与“群聊配置：群内 bot 列表 + PM/root 指定”一并实现）**：
    - 定义并传播 root（例如 `root_account_id`），以“return-to-root”切分 epoch，实现“回到 root 清零”的 hop/epoch 语义。
    - 将 `epoch_relay_budget` 从“按 relayChainId”升级为“按 return-to-root 的 epoch”更符合协作语义（并保留脱敏日志）。

#### 行为规范（Normative Rules）
- 规则 1：mirror 永远不触发推理；本单只讨论 relay（`channel.relay=true`）激活链路。
- 规则 2：当出站文本存在 relay directive 但被 hop_limit 拦截时，必须输出结构化日志 `relay_skip_reason=hop_limit_exceeded`（低噪声、脱敏、一次/出站 message）。
- 规则 3：`epoch_relay_budget` 是最终“保险丝”，与 hop 的语义解耦；budget 耗尽后停止继续派发 relay（仍 mirror），并输出 `relay_skip_reason=epoch_budget_exceeded`（同一 chain 仅一次）。
- 规则 4：短期不引入“群聊 root 配置”；中期三原则落地时，return-to-root 的语义必须在 Spec 中冻结并写入测试/验收。

#### 接口与数据结构（Contracts）
- 现有 `channel` 元数据（as-sent）：
  - `relay`（bool）：是否为 relay 注入
  - `mirror`（bool）：是否为 mirror 旁观
  - `relayChainId`（string）：relay 链标识（当前为 `sha256:<hex>`）
  - `relayHop`（u32）：当前链 hop 计数
- 新增配置/护栏（短期）：
  - `epoch_relay_budget`（u32，建议可配置）：同一 epoch 内允许的 relay 注入总量上限（短期 epoch=relayChainId）
- 日志字段（结构化，脱敏）：
  - `relay_skip_reason`（enum-like string）：`hop_limit_exceeded` / `epoch_budget_exceeded` / `target_session_missing` / ...
  - `chat_id`、`source_account_id`、`relay_chain_id`
  - `inbound_hop`、`next_hop`、`hop_limit`（命中 hop_limit 时）
  - `source_outbound_message_id`（一次/出站 message 去重 key）

#### 失败模式与降级（Failure modes & Degrade）
- hop_limit 拦截：不派发 relay；仍允许 mirror；必须记录 `relay_skip_reason=hop_limit_exceeded`（仅当存在 relay directive）。
- budget 熔断：不派发 relay；仍允许 mirror；记录 `relay_skip_reason=epoch_budget_exceeded`（同一 chain 仅一次）。
- 目标 session 不存在：不派发 relay；不消耗 budget；记录 `relay_skip_reason=target_session_missing`（低噪声、一次/出站 message）。
- Telegram `getUpdates` 网络 warning：不应影响“本次出站后的 relay 扫描与派发”（两条链路逻辑上独立）；但会影响 bot 入站更新的实时性，应单独排障。

#### 安全与隐私（Security/Privacy）
- 日志不得打印：bot token、完整消息正文、完整用户名通讯录等敏感信息。
- 如需辅助排障，可在日志中添加 `outbound_text_len`、`has_at_sign`、或正文哈希（不记录原文）。

## 验收标准（Acceptance Criteria）【不可省略】
### 短期
- [x] 当 relay 因 hop_limit 被跳过时，有明确日志可定位原因（脱敏、不打印 token/全量正文），并能用 `chat_id + source_outbound_message_id` 关联到对应的 Telegram outbound send 记录。
- [x] `epoch_relay_budget` 熔断时有明确日志（同一 chain 仅记录一次，避免刷屏），且熔断后停止派发 relay 但仍保留 mirror 旁观记录。
- [x] Telegram DM（1:1）/非群聊 channel 的行为不受影响：不应因为本单新增预算/日志而改变 DM 的推理触发或产生刷屏日志。

### 中期
- [ ] 三条原则落地：`R A B C A D B R` 这类路径允许回到 R，并且“回到 R 清零”后下一轮从 R 再派活不被历史长度惩罚。
- [ ] return-to-root 在 non-root 超限情况下仍能送达（除非触发 `epoch_relay_budget` 熔断）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 短期：`epoch_relay_budget` 按“去重后 per target 派发”计数（缺失 target 不消耗、耗尽只记录一次）。
- [x] 短期：hop_limit 拦截时会产生 `relay_skip_reason=hop_limit_exceeded`，并且 hop_limit 超限时不会调用 loose labeling / 不会派发。

### Integration
- [ ] 本地三 bot 群：在 `relay_hop_limit>=4` 的配置下，PM→A→PM→B→PM 的回执链路不需要 Neo 手工 `@PM 继续`。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：Telegram 真实环境下的 update/出站回执、reply_to 等字段需要集成验证。
- 手工验证步骤：
  1) 设置 `relay_hop_limit=3`，复现一次断链场景（PM 串行点名两名员工）；
  2) 验证日志出现 `relay_skip_reason=hop_limit_exceeded`，且可关联到对应的 Telegram outbound send；
  3) 将 `relay_hop_limit` 提升到 4（或 5），再次复现同样流程；
  4) 打开 Web UI 或直接检查 PM session JSONL，确认出现 `channel.relay=true` 的回执注入（即 `-> you` 的激活信号）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：当前为默认启用（`epoch_relay_budget` 默认 128）；属于安全护栏类变更，目标是“拦截无限链路 + 提升排障可观测性”。
- 回滚策略：代码回滚；配置层面可通过提高 `epoch_relay_budget`（例如非常大）放宽熔断，但不建议禁用（避免自激发）。
- 上线观测：关注 `telegram outbound relay: dispatched` 与新增的 `relay_skip_reason=...` 日志频率。

## 实施拆分（Implementation Outline）
- 短期：
  - Step 1: hop_limit 拦截日志（低噪声，命中候选但被拦截才记）。
  - Step 2: `epoch_relay_budget`（epoch=relayChainId）+ 熔断日志（仅一次）。
  - Step 3: 补齐 Unit 测试（覆盖 budget 计数与日志降噪）。
- 中期：
  - Step 4: 随“群聊配置/Root”落地三条原则（return-to-root、回到 R 清零、non-root N）。
  - Step 5: 补齐 Integration/手工验收步骤与文档。
- 受影响文件：
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/chat.rs`（tests）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-telegram-group-relay-duplicate-replies-on-concurrent-mentions.md`（同属 relay 机制，但问题不同）
- Related commits/PRs：
  - 引入当前 hop/relay 扫描骨架的提交：`7e8e212dd`（2026-02-22, luy）`feat(telegram): relay bot-to-bot @mentions in group chats`

## 未决问题（Open Questions）
- Q1: **预算的配置入口**：`epoch_relay_budget` 已默认 128（并已落到后端配置），是否需要在 Web UI 中增加可视化配置入口（以及展示当前 effective 值）？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

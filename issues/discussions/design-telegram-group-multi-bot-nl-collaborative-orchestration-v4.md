# Telegram 群组多智能体自然语言协同调度系统设计方案（V4）

> V4 目标：在 **不把“看门大爷（后台调度器）”做成 Agent/LLM** 的前提下，用一套足够收敛的工程机制，做到：
> - **只在被需要时唤醒**（低成本）
> - **不抢话、不刷屏**（静默协议）
> - **任务不断链**（Reply 挂链 + WAIT 挂起 + 自动追赶）
> - **抗竞态/时序**（epoch 版本收敛：只认最新快照，过期输出丢弃并重跑）

Implementation tracking issue：`issues/issue-telegram-group-multi-bot-nl-collaborative-orchestration.md`

---

## 0. 背景、需求与核心约束

### 0.1 背景
- Telegram 工作群：1 个最高权限 Owner + 多个分工不同的 Bot（Agent）。
- 目标是用自然语言组织协作，不依赖“命令行式指令”，并把成本压到极低。

### 0.2 需求（你关心的点）
1. **精准唤醒**：未被点名时不消耗 LLM。
2. **语义防呆**：被点名也要判断“是派活还是被引用/条件不成熟”，避免抢话和死循环。
3. **任务不断链**：依赖任务交付后能自动推进，不靠“记得@/记得提醒”的侥幸。
4. **工程可落地**：后台调度器只按规则执行，不做“像人一样的总结/理解”。
5. **抗竞态**：时序乱序、慢/快 bot、Owner 插话、重复消息等情况下依然收敛到正确行为。

### 0.3 核心约束（必须明确，否则后面都会复杂）
- **后台调度器（看门大爷）= 纯程序**：不跑 LLM，不做语义推理。
- **Bot = 负责脑力活**：写计划/查资料/写代码/决策；但也必须遵守输出协议。
- **每个 Bot 默认单线程**：同一时间只允许 1 个推理在路上（singleflight），其余触发合并为“重跑最新”。

### 0.4 设计目标（可核验）
- **低成本**：只有命中 A/B/C（或被 D 约束影响的交接提醒）才触发推理；其余消息仅入库。
- **低噪声**：PASS/WAIT 绝不进群；Action 默认 Reply root，群内对话可追溯。
- **不断链**：WAIT 可挂起；交付以 Reply 挂链；忘记 @ 的隐式交接用 D 兜底。
- **抗竞态**：任何乱序/快慢差/插话都由 epoch 机制收敛到最新快照。
- **可演进**：权限、安全、审计、工具调用可在不破坏核心协议的前提下加固。

### 0.5 非目标/边界（说清楚反而更省事）
- 不追求“完全自动理解所有自然语言隐喻”，隐式提及只做严格匹配与交接约束。
- 不建议让同一个 bot 同时处理多个无关 root（多任务请交给 ManagerBot 做拆分/排队）。

---

## 1. 角色与术语（务必统一口径）

### 1.1 角色
- **Owner**：最高指挥官（唯一可下达高危指令的人）。
- **Bot/Agent**：执行者（Search/Coder/Manager/...）。
- **看门大爷（Router/Scheduler）**：后台调度器，负责接收消息、计算唤醒集合、维护状态、转发消息、拦截静默信号。

### 1.2 关键术语
- **Mention / @**：显式点名（Telegram mention entity）。
- **Reply（回复/引用）**：Telegram 的 “reply_to_message”。用于把交付/反馈挂回一条“根指令”。
- **Root（根消息/根指令）**：一条任务链的锚点消息（通常是 Owner 的派活消息）。
- **TaskCard（任务卡/锚点摘要）**：后台存的一条“任务便签”，用于跨上下文截断仍能找回任务锚点。
- **WAITING（挂起）**：Bot 表示“任务归我，但缺前置”，后台登记等待。
- **PASS（静默）**：Bot 表示“与我无关/只是被引用”，后台丢弃回复。
- **epoch（版本号）**：每个 Bot 的单调递增计数，用于收敛竞态：只认最新快照，过期结果丢弃并重跑。

---

## 2. 总体架构（信息流）

```text
Telegram Update
    |
    v
看门大爷（纯程序）
  - 去重/鉴权
  - Root 追溯
  - 计算唤醒集合（A/B/C + 交接约束 D）
  - bot.epoch++ 标脏 + singleflight 调度
    |
    v
Bot（LLM 推理）
  - 意图判断：Action / PASS / WAIT
  - 产出内容或静默信号
    |
    v
看门大爷（拦截/转发）
  - PASS/WAIT：丢弃 + 记账
  - Action：强制 Reply 到 Root 转发
  - 过期输出：丢弃并重跑
```

---

### 2.1 上下文读写分离（全员静默旁听）
- 群里每条消息都会进入**全局事件流/消息库**（用于审计、追溯、构建上下文）。
- 前提：看门大爷需要能接收到群里**未点名**的消息（例如关闭 Telegram bot privacy mode，或使用 1 个 sentinel account 负责 ingest + 后台 fan-out）；否则 listen-only 与 C 路由无法对“未点名交付”生效。
- 但 **只有被路由命中（A/B/C）** 的 bot 才会真正发起 LLM 推理。
- “看门大爷”可以实现为：
  - 方案 1：每个 bot 有独立上下文库（写入时 fan-out），读取时直接取自己的窗口
  - 方案 2：全局消息库 + 每 bot 的“视图查询”（按 chat_id/root_id 取片段）

> 核心点：**记录消息 ≠ 消耗推理**。记录是 I/O，推理是成本。

## 3. 数据结构（最小可落地版）

> 关键原则：**用结构化状态解决“上下文截断”和“竞态”，不要靠 Bot 记性/自觉。**

### 3.1 AgentsRegistry（Bot 注册表）
- `agent_id`（内部 ID）
- `username`（Telegram @username）
- `telegram_user_id`（用于 B 路由/鉴权）
- `capabilities`（可选：Search/Coding/...）
- `handoff_tokens`（用于 D 路由 token 识别）：canonical 名称 + aliases（例如 `bot2` / `CoderBot`）；由配置维护（纯程序匹配，不跑 LLM）

### 3.2 MessageRootMap（消息 -> Root 映射）
用途：解决“Reply 到非 root 也能追溯到 root”。
- `chat_id`
- `message_id`
- `root_message_id`

规则：
- root 自己映射到自己：`root_id -> root_id`
- 任何由系统发出的 bot/system 消息，都写入映射：`msg_id -> root_id`
- 分段/多条出站（chunked send）：**每一条**发到 Telegram 的消息都必须写 `msg_id -> root_id`；否则用户 Reply 到“第二段/后续补发”时无法追溯 root（断链）。建议：能 thread 到 root 的都 thread；但即使未 thread，也必须写 RootMap 保证可追溯。
  - 实现提醒：若出站接口只返回“primary message_id（首段）”，仍需在发送循环中采集每个 chunk 的 message_id 并写 RootMap。

### 3.3 TaskCard（任务卡/锚点摘要）
用途：把“根指令”变成可查询的结构化锚点，避免因 FIFO 截断导致 Bot 被叫醒后“忘了自己在等什么/要干什么”。

最小字段建议：
- `chat_id`
- `root_message_id`
- `owner_user_id`（谁下的单）
- `created_at`
- `original_text`（原始派活原文，不做语义总结也行）
- `involved_agents`（被 @ 到的 bot 列表；以及后续参与过该 root 的 bot）
- `expected_handoff`（可选：从隐式提及 D 得到的“预期交接对象”列表）
- `status`：`OPEN | DONE | CANCELED`（最小就够用）
- `updated_at`

V1 最小口径（数据面约束，配合控制面“不断链”）：
- `original_text` 必须是 root 原文（authoritative），不得由看门大爷做语义总结替换。
- `expected_handoff`（若启用 D）：只能来自 token 识别（见 4.1 的 D），不做语义推断。
- `buildContext()` 必须注入 `TaskCard.original_text`（至少一条固定块），避免 session compact/FIFO 截断导致“被唤醒但失忆”。
  - 注意：TaskCard 不改变 A/B/C 的唤醒规则；它是“被唤醒后能做对事”的数据锚点。

可选增强（不是必须，但有用）：
- `brief`：1 行摘要（可由某个 bot 在正常输出里顺手给，也可由 ManagerBot 做；**看门大爷不需要用 LLM 总结**）
- `risk_flags`：是否涉及敏感操作（用于二次确认/审计）

### 3.4 WaitingTable（挂起登记表）
用途：实现“我在等交付”的可恢复状态，让交付到来时自动续链。
- `chat_id`
- `root_message_id`
- `agent_id`
- `status`：`WAITING`
- `updated_at`
- `expires_at`（建议）：TTL（避免永久 WAITING；**默认 7d**，可配置）

V1 最小口径（必须冻结）：
- 写入时机：仅当 bot 在最新快照下输出严格全等的 `<SILENCE:WAIT>` 才写入 WAITING（过期输出不得写表）。
- 清理时机：当 bot 在该 root 下输出 Action 或 `<SILENCE:PASS>` 时，必须清理该 bot 的 WAITING（避免持续被 C 路由反复唤醒）。
- 过期行为：当 `now >= expires_at` 时，WAITING 必须视为无效（C 路由不得唤醒），并应当被清理（可在入站事件/定时器触发时清理）。

> 重要：WaitingTable 不要求记录“等什么”的语义（那会把语义压力转回后台/LLM）。  
> **等待的正确性靠 epoch 机制保证**：等早了/等晚了都能收敛到最新快照。

### 3.5 BotRuntimeState（每个 Bot 的运行态）
最小字段建议：
- `agent_id`
- `epoch`：单调递增版本号
- `running`：是否有 LLM 请求在路上（singleflight）
- `last_trigger_message_id`：最近一次触发该 bot 的消息（用于调试/可观测）
- `last_trigger_root_id`：最近一次触发的 root（用于把 bot 输出 Reply 到正确 root）
- `offline`（可选）：接口异常时标记

### 3.6 Idempotency（去重表）
用途：Telegram update 可能重放/重复投递，必须去重。
- `chan_account_key`（或 `account_handle`）：用于命名空间 `update_id`（Telegram 的 `update_id` 是 per-bot token 单调序列）
- `chat_id`
- `update_id` 或 `message_id`
- `processed_at`

建议：优先用 `(chan_account_key, update_id)` 做去重键；`message_id` 在 `edited_message` 场景下不变，单用它可能导致“编辑事件被误判为重复而丢失”。

---

## 4. 核心交互规则（A/B/C/D + 静默协议 + Reply 硬约束）

> 这里是“看门大爷只按规则办事”的核心：规则越硬，系统越省心。

### 4.1 唤醒路由：A / B / C / D

#### A：显式 @ 唤醒（成本开关）
- 条件：新消息里出现 `@botX`（Telegram mention entity 命中已注册 bot）。
- 动作：将 `botX` 加入唤醒集合。

#### B：定向 Reply 唤醒（追问/反馈闭环）
- 条件：新消息是 Reply；被 Reply 的那条消息作者是某 bot（根据 `telegram_user_id` 识别）。
- 动作：将该 bot 加入唤醒集合。

#### C：挂起溯源唤醒（WAIT 续链）
- 条件：新消息是 Reply；解析出 `root_id`；`WaitingTable[root_id]` 里存在 waiters。
- 动作：将所有 waiters 加入唤醒集合。

#### D：隐式交接约束（严格匹配 bot 名，但不直接唤醒）
- 条件：Owner 的根指令里出现了某个已注册 bot 名（严格匹配，例如 `bot2`），但没有 `@bot2`。
- 实现（纯程序，不跑 LLM）：基于 AgentsRegistry 的 `handoff_tokens`（canonical name + aliases）做 token 识别（边界匹配，避免 substring 误伤），并排除已显式 `@` 命中的 bots：`expected_handoff = matched_tokens - explicit_mentions`。
  - 示例（边界匹配）：
    - ✅ 命中：`bot2 等拿到结果后...`（后面接中文/空格/标点都算边界）
    - ❌ 不命中：`bot21` / `robot2`（substring 误伤）
- 动作（推荐做法）：
  1) 在 `TaskCard.expected_handoff` 里记录该 bot（例如 `bot2`）
  2) 要求当前负责人（被 @ 的 bot1）在交付时**必须显式 `@bot2`** 完成交接
  3) 若 bot1 忘了 @，看门大爷提醒 bot1 补发（或系统代发一次交接消息）

> 为什么 D 不直接唤醒？  
> 因为会破坏“只有 @ 才醒”的成本开关，也容易误伤（聊天里提到名字就把 bot 拉起来）。

### 4.2 静默协议（Silence Protocol）

Bot 被唤醒后，只允许三类输出：
1) **执行内容（Action）**：正常工作成果/计划/回复。
2) **`<SILENCE:PASS>`**：与我无关/只是被引用/我不该发言。
3) **`<SILENCE:WAIT>`**：任务归我，但缺前置/需要他人交付/条件未成熟。

看门大爷的拦截规则必须极严格（避免模型多输出一个字就刷屏）：
- 先 `trim()`，然后要求整条消息 **全等** 才算静默信号。
- 命中 `PASS`：丢弃，不转发。
- 命中 `WAIT`：丢弃，不转发；并登记 `WaitingTable[root]`。

### 4.3 Reply 硬约束（交付必须挂链）
为了让 C 路由稳定成立，本系统把“交付/反馈必须 Reply 到 root”当成硬约束：
- **Bot 发出的任何交付/反馈**：看门大爷在发送到 Telegram 时强制带 `reply_to_message_id = root_id`（不依赖 bot 自觉）。
- **Owner/人类的交付与追问**：强烈建议也用 Reply；否则看门大爷只能当作新 root（避免误判）。

### 4.4 交接协议（显式 @ 才是交接开关）
- 需要下游 bot 开始干活时，当前 bot 的输出末尾必须加一句：`@下游bot ...`。
- “隐式提及 D”只作为**约束与兜底**：如果预计要交接给 bot2，但输出里没出现 `@bot2`，看门大爷提醒补发/代发。

---

## 5. Prompt 工程（developer 角色固化）

> 目的：把“意图判断 + 输出约束”写死，让 Bot 自己懂得闭嘴、懂得 WAIT、懂得交接。

### 5.1 Developer Prompt 模板（每个 Bot 一份）

```markdown
# 身份与环境
你叫：{@AgentName}。你的职责：{RoleDescription}。
你在 Telegram 工作群中协作。Owner 是最高指挥官。

# 团队花名册（可呼叫）
{TeamRegistry}

# 最高优先级：意图识别与输出协议
当最新消息中出现对你名字的 @ 提及，或你被系统唤醒时，你必须结合上下文做意图判断：
1) [Action] 若任务明确指向你，且前置条件已满足：输出工作成果。
2) [Wait] 若任务指向你，但缺前置（资料/同事产出/确认/权限）：你必须且只能输出 `<SILENCE:WAIT>`。
3) [Pass] 若你只是被引用/举例/与任务无关：你必须且只能输出 `<SILENCE:PASS>`。

# 协作规则（避免断链）
- 需要下游同事接手时，在输出末尾用自然语言显式 `@对方` 交接。
- 避免社交废话：严禁输出“收到/好的/谢谢”等无业务信息。
- 严禁在静默信号后追加任何解释文本。
```

### 5.2 上下文组装建议（看门大爷侧）
- 静态前缀：Developer Prompt 固定，走 Prompt Caching（降低成本/延迟）。
- 动态消息：仅保留最近 N 条（FIFO），**但必须额外注入 TaskCard**：
  - 当前 root 的 `original_text`（根指令原文）
  - 当前 root 的关键参与者/状态（OPEN/DONE）
  - 该 bot 是否在该 root 上 WAITING

> 解释：FIFO 可以截断聊天，但 **TaskCard 必须长期存在**，否则 WAIT 续链会失忆。

---

### 5.3 关键问题与对应机制（把坑写死）

#### (1) 上下文爆炸 / 截断导致“失忆”
- 问题：只保留最近 N 条时，root 指令可能被截断；bot 被叫醒后不知道原任务。
- 机制：TaskCard 长期保存 root 原文；构建上下文时强制注入 TaskCard（而不是依赖聊天窗口里碰巧还在）。

#### (2) 噪声与死循环
- 问题：被引用也被唤醒，容易刷屏；bot 互相 @ 容易无限链式触发。
- 机制：
  - 静默协议：`<SILENCE:PASS>` / `<SILENCE:WAIT>` 严格拦截
  - 自唤醒过滤：发送者 == 被 @ 的对象时不触发
  - 严禁“收到/好的”等确认废话（写进 developer prompt）

#### (3) 任务断链（忘记 @ / 忘记 Reply）
- 问题：人类/上游 bot 不按习惯操作就断链。
- 机制：
  - Bot 交付统一由后台强制 Reply root（不靠自觉）
  - WAIT + WaitingTable + C 路由实现自动续链
  - 隐式交接用 D 做约束与提醒（不破坏“@ 才醒”）

#### (4) 竞态与时序（快慢差、乱序、插话）
- 问题：bot2 慢导致 WAIT 晚到；Owner 中途改需求；多路触发同时命中。
- 机制：singleflight + epoch 收敛（只认最新快照，过期输出丢弃并重跑）。

#### (5) 路由误判（正则扫文本不可靠）
- 问题：纯正则容易误判、漏判（尤其是特殊字符、text_mention、转发等）。
- 机制：优先使用 Telegram Update 的 `entities`（mention/text_mention）与 `reply_to_message` 字段作为“物理事实”。

#### (6) 权限与越权（建议至少做到 P0 的最小版）
- 机制：
  - L1：白名单 `user_id` 过滤（非法 @ 不触发唤醒）
  - L2：入库前注入身份标签（如 `[Owner]` / `[Agent-X]`），并在 prompt 里规定只听 `[Owner]` 的高危指令

---

## 6. 看门大爷后台算法（收敛版：singleflight + epoch）

> 设计目标：用一个统一机制吃掉大部分竞态，不在 C/D 上堆满特判。

### 6.1 总原则：只认最新快照
- 每个 bot 有一个 `epoch`。
- 任何“可能影响该 bot 决策”的事件发生，后台就对该 bot 执行：`epoch++`（标脏）。
- bot 在跑 LLM 时不打断；但它回来时，如果 `run_epoch != current_epoch`，说明输出基于旧快照 —— **直接丢弃并重跑**。

### 6.2 事件入口：收到一条新消息（伪代码）

```pseudo
onMessage(update):
  if !authOK(update): return
  if isDuplicate(update): return

  root = resolveRoot(update)
  upsertTaskCardIfNeeded(root, update)

  affected = union(
    routeA_mentions(update),
    routeB_replyAuthorBot(update),
    routeC_waiters(root, update),
    routeE_inflightSameRoot(root),   // 关键：让“慢 bot”的旧快照自动作废
  )

  for bot in affected:
    touch(bot, root, update.message_id)  // bot.epoch++
    schedule(bot)

  if isOwner(update) and isRootMessage(update):
    enforceHandoffConstraintD(root, update) // 只记约束/提醒，不直接唤醒
```

补充说明：`routeE_inflightSameRoot(root)`
- 返回集合：所有 `running=true` 且 `last_trigger_root_id == root` 的 bot。
- 作用：只做 **epoch 作废**，不额外制造新的并发请求。
- 解决的问题：当 bot 基于旧快照即将输出 WAIT/旧方案时，只要 root 线程里来了新消息（交付/插话），它的旧输出就会被判定过期并重跑，从而避免“lost wakeup / 发旧答案”。
- 注意（记录风险，不额外处理）：若某个 root 线程极度嘈杂（高频 Reply/插话），`routeE` 会导致旧快照频繁作废、出现多次重跑与延迟；本设计默认接受（本群预计不嘈杂）。如未来需要，可加 debounce/节流或“只对 Owner/授权人消息触发 routeE”。

### 6.3 Root 追溯：resolveRoot（关键，不然 Reply 链会断）

```pseudo
resolveRoot(update):
  if update.reply_to_message_id is null:
    return update.message_id

  replied = update.reply_to_message_id
  // 先查 MessageRootMap；查不到就退化为 replied 本身
  return MessageRootMap.get(replied) ?? replied
```

> 同时要求：你每发出去一条 bot/system 消息，都写入 `MessageRootMap[msg_id] = root_id`。

### 6.4 调度：singleflight + 合并触发（伪代码）

```pseudo
touch(bot, root, trigger_msg):
  bot.epoch += 1
  bot.last_trigger_root_id = root
  bot.last_trigger_message_id = trigger_msg

schedule(bot):
  if bot.running: return   // 合并触发：只标脏，不并发跑
  bot.running = true
  run_epoch = bot.epoch
  ctx = buildContext(bot, bot.last_trigger_root_id)
  callLLMAsync(bot, ctx, run_epoch)
```

### 6.5 LLM 返回：过期输出丢弃并重跑（伪代码）

```pseudo
onLLMResult(bot, run_epoch, text):
  bot.running = false

  if bot.epoch != run_epoch:
    // 期间来了新消息/交付/插话：旧输出一律作废
    schedule(bot)
    return

  handleBotOutput(bot, text)  // PASS/WAIT/Action
```

### 6.6 输出处理：handleBotOutput（要点）
1) `trim(text)` 后：
   - `== "<SILENCE:PASS>"`：丢弃（若曾 WAITING，则清理 WAITING，避免持续被 C 路由反复唤醒）。
   - `== "<SILENCE:WAIT>"`：在当前 `root_id`（通常为 `bot.last_trigger_root_id`）下写入 `WaitingTable[root_id, bot] = WAITING`，丢弃。
   - 否则：当作 Action（并清理该 bot 在该 root 下的 WAITING 记录）。
2) Action 转发规则：
   - **强制 Reply 到 `bot.last_trigger_root_id`**
   - 写入 `MessageRootMap`，保证后续 Reply 可追溯 root
3) 交接约束 D 检查（兜底）：
   - 若 `TaskCard.expected_handoff` 非空，但 Action 文本里未出现对应 `@botX`，提醒当前 bot 补发交接（或系统代发一次交接）

### 6.7 故障处理（Fail Fast & Loud）
目标：不让群里“死等”、不让任务链“悄悄断”。
- LLM 调用超时/5xx：
  - 记录错误（含 `root_id/agent_id/epoch/request_id`）
  - 以该 bot 名义代发标准化声明（可包含“是否会重试/需要 Owner 介入”），并 **Reply 到 root**、写入 `MessageRootMap`
  - 标记 `BotRuntimeState.offline=true`（可选）
- 重试策略（建议最小化）：
  - 指数退避 + 最大次数
  - 超过阈值就停止自动重试，改为提示 Owner 决策（避免无限重试烧钱）
- 探活恢复：
  - 后台定时 ping（或健康检查）
  - 恢复后代发“已恢复在线”

### 6.8 鉴权与防越权（建议至少做 L1）
- L1（网关白名单）：只有 `OWNER_ID` 与授权 bot 的 `user_id` 产生的消息才允许进入唤醒路由（A/B/C）；未授权用户消息最多 ingest-only，不得触发 run（含 C 路由的 waiter wakeup）。
- L2（身份贴标）：入库前将消息重写为带身份前缀（如 `[Owner]` / `[Agent-Coder]`），并在 prompt 中规定高危操作仅响应 `[Owner]`。

### 6.9 可观测性（强烈建议）
最小日志字段（不然调竞态会很痛）：
- `chat_id, message_id, root_id`
- `affected_bots`（A/B/C 命中原因）
- `agent_id, run_epoch, current_epoch, running`
- `output_type`（PASS/WAIT/ACTION/STALE_DROPPED）

### 6.10 最小控制面状态机（proof-oriented，便于实现/验收）
> 目的：把“不断链/低成本/低噪声/抗竞态/幂等”收敛为一个最小闭环，避免靠场景枚举堆特判。

#### 6.10.1 状态（最小集合）
> 记号只是为了说明最小性；实现时可映射到本章 3.x 的表/字段。

- `Seen(chat_id, update_id)`：去重表（3.6 Idempotency）
- `RootOf(chat_id, message_id) -> root_id`：Root 追溯映射（3.2 MessageRootMap）
- `Waiters(chat_id, root_id) -> set<agent_id>`：挂起登记（3.4 WaitingTable）
- `BotVer(agent_id) -> {ver, running, target_root}`：把 `epoch + singleflight + “只跑最新”` 合并成一个版本戳（3.5 BotRuntimeState 的最小子集）
- `TaskCard(chat_id, root_id)`：数据面锚点（3.3 TaskCard；不影响“唤醒正确性”，但影响“被唤醒后是否失忆”）

#### 6.10.2 入站事件（Router）管线（伪代码）

```pseudo
onInbound(update):
  if Seen.contains(chat_id, update_id): return
  Seen.insert(chat_id, update_id)

  root = resolveRoot(update)  // 6.3：RootOf(reply_to) ?? reply_to ; else message_id
  RootOf.insert(chat_id, message_id, root)

  upsertTaskCard(root, update)          // 写 root 原文/参与者等
  if isOwner(update) and isRootMessage(update):
    enforceHandoffConstraintD(root, update) // 仅 token 识别 -> TaskCard.expected_handoff；不唤醒

  wake = union(
    routeA_mentions(update),            // @ 唤醒
    routeB_replyAuthorBot(update),      // Reply 唤醒作者
    routeC_waiters(root, update),       // WAIT 续链唤醒
  )

  touch = wake ∪ routeE_inflightSameRoot(root) // 只作废旧快照（epoch++），不额外并发跑

  for bot in touch:
    BotVer[bot].ver += 1
    BotVer[bot].target_root = root
    if bot ∈ wake and BotVer[bot].running == false:
      startRun(bot, root, BotVer[bot].ver)
```

#### 6.10.3 Bot 回包处理（Latest-only side effects）

```pseudo
onLlmResult(bot, run_root, run_ver, text):
  BotVer[bot].running = false

  if run_ver != BotVer[bot].ver:
    discard(text)              // 旧快照不得产生任何副作用（含 WAIT 写表）
    startRun(bot, BotVer[bot].target_root, BotVer[bot].ver)  // 只跑最新
    return

  t = trim(text)
  if t == "<SILENCE:PASS>":
    Waiters[chat_id, run_root].remove(bot)   // 若曾 WAITING，则清理（避免持续被 C 路由反复唤醒）
    discard
  else if t == "<SILENCE:WAIT>":
    Waiters[chat_id, run_root].insert(bot)
    discard
  else:
    Waiters[chat_id, run_root].remove(bot)     // 建议：避免持续被 C 路由反复唤醒
    sent_id = sendToTelegram(reply_to = run_root, text = t) // 4.3：强制 Reply root
    RootOf.insert(chat_id, sent_id, run_root)  // 6.3：保证后续 Reply 可追溯
```

#### 6.10.4 不变量（最小验收口径）
- P1（线程闭包）：任意 Reply 都可通过 `RootOf` 追溯到 root；任意 Action 出站都强制 Reply root 并写 `RootOf(sent)=root`。
- P2（唤醒纪律）：只有 A/B/C 进入唤醒集合；D 只写 `TaskCard.expected_handoff`，不直接唤醒（不破坏“@ 才醒”成本开关）。
- P3（只认最新快照）：只有 `run_ver == current_ver` 的结果允许写表/发群；过期输出一律丢弃并只跑最新（解决乱序/慢 bot/lost wakeup/插话改需求）。
- P4（幂等）：同一 `update_id` 至多处理一次，避免重复触发与重复副作用。

最小性说明（直觉）：去掉 `RootOf/强制 Reply` 会断链；去掉 `Waiters` 无法续链；去掉 `BotVer` 无法收敛竞态；去掉 `Seen` 会被重复投递打穿。D 属于体验兜底项，可选但推荐。

---

## 7. 竞态/时序问题：用 epoch 机制统一收敛

### 7.1 典型竞态：bot2 本该 WAIT，但它很慢（你点名的场景）

时间线（旧方案会丢唤醒）：
1) `M100` 同时触发 bot1/bot2
2) bot1 很快 Reply 交付 `D101`；后台查 C 时 `WaitingTable[M100]` 还空
3) bot2 过会才回 `<SILENCE:WAIT>` → **这时已经错过 `D101`，bot2 等死**

V4（epoch 收敛）怎么解决：
- `D101` 到来时（它 Reply 在 root `M100` 线程里），后台通过 `routeE_inflightSameRoot(M100)` 对 bot2 执行 `epoch++`（touch），让 bot2 的“旧快照推理”自动作废。
- bot2 即便后来回了 WAIT，那也是基于旧快照：
  - 返回时发现 `run_epoch != current_epoch` → **WAIT 被判过期丢弃** → 立刻重跑
- 重跑后 bot2 能看到 `D101`，不会再 WAIT（或会基于最新情况做正确动作）。

### 7.2 Owner 中途插话/改需求（Steering）
- Owner 新消息到来：后台 touch bot（epoch++）。
- bot 之前那次推理返回：`run_epoch` 过期 → 丢弃输出 → 用最新上下文重跑。
- 结果：不用“杀进程”，也不会把旧答案发到群里污染现场。

### 7.3 多路触发同时命中（A/B/C 同时击中同一 bot）
- singleflight 保证同一 bot 不会并发跑多份 LLM。
- 触发越多，只会把 epoch 加大，最终收敛到“最后一次触发对应的最新快照”。

### 7.4 重复投递/重放
- Idempotency 去重确保同一 update 不会反复触发 epoch++ 与唤醒。

---

## 8. 场景枚举（逐例：解释 + 核验）

> 每个场景都回答三件事：谁会被唤醒？谁会闭嘴？为什么不断链？

### 8.1 场景总览（矩阵）

| ID | 场景 | 主要命中路由 | 典型输出 | 核验重点 |
| --- | --- | --- | --- | --- |
| S01 | 单点直派 | A | Action | 只唤醒被 @ 的 bot |
| S02 | 并行派发 | A | Action / WAIT | WAIT 不刷屏、交付后续链 |
| S03 | 噪声引用 | A | PASS | 被引用也不发言 |
| S04 | 显式依赖 | A + C | WAIT -> Action | Reply 挂链 + WaitingTable |
| S05 | 总包接力（显式） | A | Action + `@下游` | 交接靠 @ 开关 |
| S06 | 隐式交接（无 @） | A + D | Action + `@下游` | D 只约束不唤醒 |
| S07 | 定向追问/反馈 | B | Action | 不用 @ 也能叫回原作者 |
| S08 | 竞态：WAIT 晚到 | epoch | STALE 丢弃重跑 | 不会 lost wakeup |
| S09 | 动态转向/插话 | epoch | STALE 丢弃重跑 | 不会发旧答案 |
| S10 | 自唤醒防御 | A(过滤) | PASS/无触发 | 不会死循环 |
| S11 | 物理故障 | N/A | 代发告警 | 不死等、可恢复 |
| S12 | 越权防御 | L1/L2 | 不唤醒 | 不烧钱、不越权 |
| S13 | bot 间求助 | A | Action | bot 间协作可用 |
| S14 | 编辑更正 | epoch | STALE 丢弃重跑 | 改需求可追溯 |
| S15 | 多任务并发 | epoch | latest wins | 需要优先级/排队 |
| S16 | 人类不 Reply 交付 | N/A | N/A | 作为群规强制 Reply |

### 8.2 对话示例（覆盖最小闭环：Seen/RootOf/Waiters/BotVer + TaskCard）
> 目标：用最少的例子覆盖全部机制；每条示例都可用 6.10 的状态机推演。

#### E01 标准派活（A，强制 Reply root）
- Owner（root=M100）：`@bot1 你去干A`
- bot1（Action）：`A 的结果是...`
- 关键：看门大爷强制 `reply_to=M100` 发群，并写 `RootOf(sent)=M100`（保证后续 Reply 可追溯）。

#### E02 噪声引用（PASS 严格拦截）
- 成员：`@bot1 上次你干得不错`
- bot1：`<SILENCE:PASS>`
- 关键：`trim()==PASS` → 丢弃，不发群（低噪声；但仍消耗一次“被 @ 叫醒”的判定成本）。

#### E03 显式依赖不断链（WAIT -> Waiters -> C 唤醒）
- Owner（root=M200）：`@SearchBot 查资料；@CoderBot 等资料后写 patch`
- CoderBot：`<SILENCE:WAIT>`（不进群，但写 `Waiters[M200] += CoderBot`）
- SearchBot（Action，Reply root）：`资料如下...`
- 关键：交付消息在 root 线程内 → C 路由命中 `Waiters[M200]`，即使没再次 `@CoderBot` 也会唤醒并续链。

#### E04 Reply 到中间消息（RootOf 追溯 + B/C）
- 前提：SearchBot 的交付消息 `D201` 已被系统写入 `RootOf(D201)=M200`（出站必须写 RootOf）。
- Owner Reply `D201`：`补充：要兼容 Windows`
- 关键：`resolveRoot` 追溯回 `M200`；B 可唤醒 SearchBot（追问闭环），C 可唤醒 `Waiters[M200]`（等待续链）。

#### E05 隐式交接（D：token 识别，只记约束不唤醒）
- Owner（root=M300）：`@bot1 你去干A，bot2 等拿到结果后再去干B`（注意：没有 `@bot2`）
- 关键：
  - D：token 识别命中 `bot2` → `TaskCard.expected_handoff=[bot2]`（仅记约束）
  - bot2 不会被唤醒；必须靠显式交接完成激活：
    - bot1 交付时显式 `@bot2 ...`（A 路由唤醒），或
    - 系统按策略提醒/代发一次交接 ping（仍是纯程序，不做语义推断）。

#### E06 竞态收敛（BotVer：旧快照输出丢弃并只跑最新）
- 时间线：
  1) `M400` 触发 bot2 运行（`run_ver=10`）
  2) root 线程里来了新消息（交付/插话）→ `routeE_inflightSameRoot(M400)` touch bot2（`current_ver=11`）
  3) bot2 返回旧结果（可能是旧 WAIT/旧方案）→ `run_ver!=current_ver` → 丢弃，不得写 `Waiters`、不得发群；然后只跑 `ver=11` 的最新快照

#### E07 重复投递（Seen：幂等）
- Telegram 可能重放同一 update。
- 关键：命中 `Seen(chat_id, update_id)` 直接忽略；避免重复触发/重复发群/重复写表。

#### E08 人类交付不 Reply（默认视为新 root，不做高风险猜测）
- 前提：`Waiters[M500]` 里有 CoderBot（在等交付）。
- 人类成员新发一条“资料如下...”但未 Reply `M500`。
- 关键：默认当作新 root；C 不触发；需要把“交付必须 Reply root”当群规（或引入额外但更冒险的启发式，默认不推荐）。

#### E09 edited_message（更新 TaskCard + touch BotVer；去重键优先 update_id）
- Owner 编辑 root `M600`（message_id 不变，但 update_id 变）。
- 关键：
  - 不能用 message_id 做唯一去重键，否则会丢掉“编辑事件”。
  - edited_message 视为新事件：更新 `TaskCard.original_text`，并 touch 相关 bot 的 `BotVer`，让旧快照输出过期丢弃。

#### E10 上下文截断（TaskCard 注入，数据面兜底）
- 群聊很长导致 session FIFO/compact，root 原文被截断。
- 关键：`buildContext()` 必须注入 `TaskCard.original_text`（以及必要的 `expected_handoff/WAITING` 快照），否则 bot 被唤醒后容易“失忆”并输出错误动作。

#### E11 WAITING TTL（默认 7d，过期视为无效）
- 前提：`Waiters[M700]` 里有 CoderBot（之前输出过 `<SILENCE:WAIT>`）。
- 过了 7 天（超过 `expires_at`），群里有人 Reply root `M700`：`当时那个链接在哪？`
- 关键：过期 WAITING 不得触发 C 路由唤醒；并应被清理。若仍需继续任务，需要重新 `@CoderBot` 或让其再次输出 WAIT 记账。

#### E12 routeE 在“嘈杂 root”下的影响（记录风险）
- root 线程高频插话时，in-flight bot 会被频繁 touch（epoch++），旧输出大量变 STALE。
- 关键：旧输出不会污染群（P3 仍成立），但会出现延迟/重跑成本；本设计默认不额外处理（群不嘈杂），后续可加 debounce/节流。

#### E13 分段出站与 RootOf（每个 chunk 都写 Root）
- bot 输出很长被拆成两条 Telegram 消息：`S1`、`S2`（`S1` 通常 Reply root；`S2` 可能是续发 chunk，也可能同样 Reply root）。
- 用户 Reply `S2`：`第二段再解释下`
- 关键：必须写 `RootOf(S1)=root` 且 `RootOf(S2)=root`；否则 Reply 到 `S2` 会追溯失败导致断链（B/C 失效）。

### S01 标准派活（单点直派）
**Owner**：`@bot1 写个脚本`
- 触发：A 唤醒 bot1
- bot1：Action 输出脚本
- 大爷：强制 Reply root 转发
- 核验：只有 bot1 消耗；其他 bot 0 成本

### S02 平行派发（并行）
**Owner**：`@SearchBot 查资料，@CoderBot 写代码框架`
- 触发：A 唤醒两者（并行）
- 若 CoderBot 需要资料：回 `<SILENCE:WAIT>`，进入 WaitingTable
- SearchBot 交付时 Reply root：触发 C 唤醒 waiters（或直接 touch + epoch 收敛）
- 核验：依赖存在时不刷屏，交付后自动推进

### S03 噪声引用（提到名字但不该说话）
**Owner/成员**：`上次 @bot1 做得不错`
- 触发：A 仍会唤醒 bot1（成本开关没法避免）
- bot1：意图判断为引用 → `<SILENCE:PASS>`
- 大爷：丢弃，不发群
- 核验：群里无噪声；代价只有一次 bot1 的判定（可接受）

### S04 显式依赖（WAIT + 交付 Reply 续链）
**Owner(root=M100)**：`@SearchBot 查 X；@CoderBot 等资料后写代码`
1) 大爷：A 唤醒 SearchBot、CoderBot
2) CoderBot：缺前置 → `<SILENCE:WAIT>` → 进入 `WaitingTable[M100]`
3) SearchBot：交付资料（Action）
4) 大爷：强制 SearchBot 的交付 Reply 到 `M100`；触发 C 唤醒 waiters（CoderBot）
5) CoderBot：被唤醒后开始写代码
- 核验：不靠“SearchBot 记得 @CoderBot”，也不断链

### S05 总包接力（显式交接）
**Owner**：`@ManagerBot 你出计划，然后让 @CoderBot 实施`
- 触发：A 唤醒 ManagerBot、CoderBot
- CoderBot：如果前置未就绪 → WAIT
- ManagerBot：输出计划，末尾 `@CoderBot 按上述计划开始实现`
- 核验：交接靠显式 @；未就绪时 CoderBot 不刷屏

### S06 隐式交接（你点名的关键场景）
**Owner(root=M200)**：`@bot1 你先做个计划，然后请 bot2 实施`（注意 bot2 没 @）
- 触发：A 只唤醒 bot1
- D：大爷在 TaskCard 里记 `expected_handoff=[bot2]`（不唤醒 bot2）
- bot1：输出计划；末尾必须 `@bot2 ...` 完成交接
- 若 bot1 忘了：
  - 大爷提醒 bot1 补发交接（或系统代发一次 `@bot2 请按 bot1 计划实施`，并 Reply root）
- 核验：既保留“@ 才醒”的成本开关，又不因为“没 @bot2”断链

### S07 定向追问/反馈（Reply 唤醒原作者）
1) bot1 发了一条方案（系统已写入 `MessageRootMap` 并 Reply root）
2) Owner Reply 这条消息：`改一下第二点`
- 触发：B 唤醒 bot1（追问闭环）
- 核验：无需 @ 也能精准叫回原作者

### S08 竞态核验：WAIT 晚于交付（lost wakeup）
**核心验证点**：旧方案会“错过 C”；V4 必须收敛。
- 交付到来时 touch(bot2) → `epoch++`
- bot2 的 WAIT 返回若过期：丢弃并重跑 → 最终不再 WAIT/或基于最新动作
- 核验：无论 bot2 快慢，都不会“等死”

### S09 动态转向（Owner 插话改需求）
- Owner 新消息触发 touch → epoch++
- bot 旧输出过期丢弃 → 重跑最新 → 按新需求走
- 核验：不会把旧答案发出来污染群聊

### S10 自唤醒防御
**bot1** 幻觉发：`我是 @bot1`
- 大爷：路由时排除“发送者本人”命中（不唤醒自己）
- 核验：避免自触发死循环

### S11 物理故障（超时/5xx）与代发
- LLM 调用超时：大爷捕获异常，按 bot 名义代发：`[System Surrogate] 当前任务中止/稍后重试`
- 探活恢复：恢复后代发“已恢复在线”
- 核验：群里不会死等；信息对齐

### S12 安全：非白名单路人 @bot 删库
- 大爷 L1 鉴权失败：不触发唤醒，只做可选的上下文记录（看策略）
- 核验：不烧钱、不越权

### S13 Bot 间求助（bot 叫 bot）
**CoderBot**：`@SearchBot 帮我查一下 X 报错原因`
- 前提：白名单允许“授权 bot”触发唤醒（否则 bot 之间无法互叫）。
- 触发：A 唤醒 SearchBot
- SearchBot：Action 输出资料（后台强制 Reply 到当前 root）
- 核验：bot 间协作可用；依然不抢话、不乱醒

### S14 Owner 编辑/更正派活消息（edited_message）
**Owner**：编辑 root `M300`（补充条件/改需求）
- 大爷：将“编辑事件”视为一条新事件：
  - 更新 `TaskCard[M300].original_text`
  - touch `TaskCard.involved_agents`（epoch++），让相关 bot 重跑最新快照
- 核验：改需求不会被旧输出污染；不会死等

### S15 同一 bot 同时接到两个无关任务（多 root 并发）
**Owner**：
- `M400`: `@bot1 做 A`
- `M401`: `@bot1 做 B`（很快又来）
- V4 默认语义（设计选择）：bot1 singleflight + epoch 会“只认最新快照”（latest-wins），倾向先把 B 跑到稳定；不保证旧 root 一定被完成（便于快速 steer bot 工作方向）。
- 建议工作流（更稳也更像真实团队）：
  1) Owner 让 `@ManagerBot` 做排队与拆分
  2) 或 Owner 明确优先级：`@bot1 先 B，A 等会`
- 核验：系统不会并发炸成本；但多任务需要管理层（ManagerBot/Owner）明确优先级

### S16 人类交付不 Reply（线程不挂链）
**成员**：直接新发一条“资料如下…”，但没 Reply root
- 大爷：为避免误判，默认当作新 root；不会自动唤醒 WaitingTable 上的 waiters
- 解决：把“交付必须 Reply”当群规（最简单最稳）
- 核验：规则清晰，避免后台做高风险猜测

---

## 9. 最小实现清单（把复杂度锁死在后台）

如果你只做最小闭环，建议优先保证以下 10 条（顺序按重要性）：
1) A/B/C 路由 + D 交接约束（D 不直接唤醒）
2) Bot 输出协议：Action / `<SILENCE:PASS>` / `<SILENCE:WAIT>`（严格全等拦截）
3) Bot singleflight（同一 bot 只允许 1 个 LLM 在路上）
4) `epoch` 机制：触发即 `epoch++`；过期输出丢弃并重跑
5) Root 追溯：`MessageRootMap`（Reply 到非 root 也能找到 root）
6) Bot 发送强制 Reply root（系统层强制，不靠自觉）
7) WaitingTable（`WAIT` 记账 + C 唤醒）
8) TaskCard（至少存 root 原文；构建上下文时注入）
9) Idempotency 去重
10) 基础故障代发（timeout/5xx）与简单探活

---

## 10. 你可以怎么审阅（快速核验点）

你审阅时可以盯三条“硬指标”：
1) **成本指标**：没有 @ 的消息不会唤醒 bot（除 B 的追问闭环），D 不直接唤醒。
2) **噪声指标**：PASS/WAIT 不进群；Action 默认 Reply root，避免线程散落。
3) **竞态指标**：任何“慢 bot / 快交付 / 插话改需求”都能靠 epoch 收敛到最新，不会等死、不发旧答案。

### 10.1 特别关注事项（已冻结/已记录的默认口径）
- WAITING TTL：默认 7d；过期视为无效且应清理；Action/PASS 时必须清理该 bot 在该 root 下的 WAITING（见 3.4 / E11）。
- routeE：记录“嘈杂 root 下可能导致反复重跑/延迟”的风险；本群预计不嘈杂，默认不做额外节流（见 6.2 / E12）。
- 多 root 公平性：latest-wins 是设计选择（便于 steer），不保证旧 root 完成；需要排队请交给 ManagerBot/Owner（见 S15）。
- RootOf 完整性：分段/多条出站必须每条都写 RootOf，否则 Reply 到后续消息会断链（见 3.2 / E13）。
- “能看到未点名消息”的前提：
  - 人类未点名消息：需要 Telegram 平台把消息 update 投递给看门大爷（通常要求 privacy mode OFF 或 sentinel ingest）；Moltis 收到后可做 ingest-only（见 2.1 / E08）。
  - 其他 bot 的群消息：Telegram 平台不会投递给别的 bot，必须依赖 Moltis 侧 outbound mirror（见本仓库已落地的 mirror/relay 方案）。
- 本仓库现状核实（已落地能力，便于排障）：
  - 群聊未点名消息 listen-only ingest：`crates/telegram/src/handlers.rs:249`
  - ingest-only 写入入口（不触发 run）：`crates/gateway/src/channel_events.rs`
  - outbound mirror（bot 输出复制到其他 bot sessions）：`crates/gateway/src/chat.rs:6743`
  - outbound relay（bot@bot 指派必达触发）：`crates/gateway/src/chat.rs:6411`
  - 相关已关单实现说明：`issues/done/issue-telegram-group-ingest-reply-decoupling.md` / `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md` / `issues/done/issue-telegram-group-bot-to-bot-mentions-relay-via-moltis.md`

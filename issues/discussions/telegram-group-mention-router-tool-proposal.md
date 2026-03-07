# Telegram 群聊 mention → router tool → 激活多 bot 推理（设想方案）

> 讨论时间戳：2026-03-07T16:27:57Z（UTC）

> 目标：把当前 bot→bot relay 的“@ 格式解析（Strict/Loose）”收敛成一个统一的 **router（路由器）** 步骤：  
> **只要检测到 mention，就调用一次内部 router tool 做意图识别与分发抽取；需要激活的 bot 由服务器触发其 LLM 推理/排队。**
>
> 本文是“设想方案 / proposal”，不是现状（as-is）描述。

---

## 1. 背景：为什么要做 router tool？

当前机制对“@ 点名”的处理存在大量边角与不稳定来源（空行、标点、引用/代码块、行首/非行首、误触发与漏触发、去重与串线等）。  
本设想的核心是：把“从自由文本里猜哪些 @ 是指令”升级为“由 LLM 输出结构化分发结果”，服务器只做执行与兜底。

一句话：**让模型负责“理解意图 + 抽取结构”，让系统负责“幂等执行 + 投递 + 排队”。**

补充口径（重要）：router tool 的定位是 **意图抽取机器**，不是“会话智能本体”。  
像群聊里可能出现的 `@` ping-pong、互相拉扯、谁该收敛等行为，主要应由 **各 bot 自身的模型能力/提示词/角色约束（以及必要的 hop/节流限制）**来处理；router 只负责把“这一条消息里应该激活谁”抽取成结构化结果。

---

## 2. 冻结口径（本轮用户已定）

- 只要存在 mention（包含纯点名），router 都可以判定为需要激活（例如 `ping`），并且 **会触发目标 bot 的 LLM 推理**（忙则入队）。
- router 使用哪个小模型（例如 deepseek）与成本上限：后续再谈（本方案不绑定具体 provider）。

---

## 2.1 MVP 规格（极小化闭环，落地口径）

- **触发条件**：群聊消息里出现“候选 bot 的 mention”（优先 Telegram entities；见 3.1 的匹配口径）→ 调用 router 一次。
- **调用时机**：仅在“入站 update 解析完成”时调用（不是在 bot 生成回复内容时调用）。
- **router 输入**：`rawText + mentions(已过滤到候选 bot) + candidates + source`（默认不依赖群历史）。
- **router 输出**：`assignments[] = {targetAccountId, kind, taskText}`；server 强校验 `targetAccountId ∈ candidates`。
- **执行与幂等**：必须有两层去重（route_key + assign_key）；去重存储需要跨实例共享（见 5.2.4）。
- **失败策略**：默认“失败就不派活但要记录日志/指标”；是否 fail-open（例如对命中的候选 mention 统一 `ping`）作为可选配置（见 5.1.1）。
- **适用范围**：对“人类消息”和“bot 消息”一视同仁；只要命中触发条件都可路由一次（每条 Telegram 消息各自独立）。
- **编辑消息**：MVP 建议默认不对 `edited_message` 重新路由（只记录）；后续如需支持再明确语义（见 3.1）。
- **candidates 来源**：MVP 暂不展开（先假设 server 侧能拿到本群候选 bot 列表）；后续由“后台群成员列表功能”提供更可靠来源。

---

## 3. 关键思路：两层协议分离

### 3.1 外层：检测 mention（触发 router）

建议优先使用 Telegram 的 `MessageEntity`（`mention` / `text_mention`）来判断“是否存在点名”，而不是用 `text.contains('@')`：

- 好处：邮箱 `a@b.com`、代码里的 `@decorator` 通常不会被 Telegram 标为 mention entity，能降低误触发。
- 仍保留 fallback：当 Telegram 未提供 entities 时，再做边界安全的文本匹配（但这属于兜底，不作为主路径）。

触发标准（尽量收敛、讲人话）：
- **群聊场景**：只要当前消息里出现“候选 bot 的 mention”（优先用 Telegram entities 判定），就调用 router tool 做一次意图抽取。
- **DM 1:1 场景**：默认可以关闭 router（因为不需要“多 bot 分发”），除非你明确要在 DM 里做跨 bot 派活。

调用时机（必须说清楚）：
- router tool 的推荐调用点是：**系统收到一条 Telegram 消息 update（入站）并完成解析**的这一刻（ingest/relay 判断阶段）。
- router tool **不是**在“某个 bot 生成回复内容”的那一刻被调用；而是当该回复真正发到 Telegram（群里/私聊里）后，系统再收到它对应的入站 update 时，才会作为一条“新消息”按同样规则评估是否触发 router。

两个例子（讲人话）：
1) 人类发言触发路由：
   - 群里人发：`@b 做X @c 做Y`（假设 `chat_id=group42`，`message_id=100`）
   - 系统收到 `group42/100` 的入站 update → 解析 entities → 调用 router 一次 → 输出 assignments → 激活 b/c 推理（并做去重）
2) bot 回复本身也可能触发路由（但触发点仍是“入站 update”）：
   - b 在群里回复：`@c 我这边X完成了，你那边Y需要我配合吗？`（假设 `message_id=101`）
   - 这条回复被发到群里后，系统收到 `group42/101` 的入站 update → 同样按触发标准评估 → 若命中 mention，则对 **这条新消息** 再调用 router 一次

实现口径（建议写死，避免各处实现不一致）：
1) **优先从 Telegram entities 抽取 mention**：
   - 从 update 里读取 `entities` 与 `caption_entities`（媒体消息的 caption 也可能含 mention）
   - 仅抽取 `mention` / `text_mention` 两类 entity（不要用 `text.contains('@')` 做主路径）
2) **与 candidates 做匹配过滤**（只保留“候选 bot”）：
   - `mention(@username)`：按 username 做匹配（Telegram username 大小写不敏感，建议统一 lower-case 再比）
   - `text_mention(user_id)`：按 user_id 做匹配（如果你把 bot 的 user_id 也纳入 candidates）
3) **若匹配结果为空**：不调用 router（避免“有人提到 @ 但不是候选 bot”导致无谓路由）
4) **若匹配结果非空**：把“过滤后的 mentions + 完整 candidates 列表”一起交给 router 做意图抽取
5) **关于 edited_message**（MVP）：
   - 默认不重跑路由（否则会和 `route_key=chat_id+message_id` 的幂等语义打架）
   - 仅记录日志：`edited_message_seen=true`，用于后续决定要不要支持“编辑后重新派活”

### 3.2 内层：router tool 输出结构化分发（执行不猜测）

router tool 的职责：
1) 判断每个 mention 在当前上下文中是：
   - `directive`：明确派活（需要激活目标 bot）
   - `reference`：只是提到/举例/引用（不激活）
   - `ping`：纯点名/叫醒（需要激活目标 bot）
2) 对需要激活的对象，抽取出：
   - 目标 bot（稳定标识）
   - 对该 bot 的任务文本（或固定 ping 文本）

服务器职责：对 router 产出的 assignments 进行幂等执行（去重、触发 `chat.send`、排队、投递回执与可观测性）。

---

## 4. Router tool 的最小接口（建议）

### 4.1 输入（server → router）

- `rawText`：原始消息文本（不做“行首/空行”特殊规则，交给 router 理解）
- `mentions`：从 entities 解析出的候选 mention 列表（避免靠 regex 猜）
  - 例如：`[{ "username": "b" }, { "username": "c" }]` 或含 `userId`
- `candidates`：本群可被激活的 bot 列表（白名单）
  - 例如：`[{ "targetAccountId": "telegram:bot_b", "username": "b", "display": "@b" }, ...]`
- `source`：用于追溯/幂等的上下文
  - `chatId`, `messageId`, `senderId/username`, `chatType=group`
- `context`（可选）：用于提高意图识别准确度的额外上下文（见 4.3）

### 4.2 输出（router → server）

建议输出格式（严格 JSON）：
```json
{
  "assignments": [
    {
      "targetAccountId": "telegram:bot_b",
      "kind": "directive",
      "taskText": "你干xxxx"
    },
    {
      "targetAccountId": "telegram:bot_c",
      "kind": "ping",
      "taskText": "ping"
    }
  ],
  "notes": "optional"
}
```

约束建议：
- `targetAccountId` 必须来自 `candidates`（不允许 router 发明目标）
- `kind` 仅允许 `directive|reference|ping`
- router 内部解析失败：默认返回空 assignments（宁可不激活也不误激活）
- router 调用失败/超时：由 server 侧兜底策略决定（见 5.1.1）

---

## 4.3 Router 的上下文粒度（补充想法）

router tool 是否需要“吃完整群上下文”本质是 **准确率 / 延迟 / 成本** 的权衡。这里建议默认保持极简：router 的职责是抽取“这一条消息”的路由意图，不要把它做成“会话理解引擎”。

建议的三档输入粒度（从轻到重）：
- A) **最小上下文（默认）**：只给 `rawText + mentions + candidates + source`
  - 优点：最快、最省 tokens、最利于 prompt cache（系统提示词更稳定）
  - 适用：绝大多数“派活语句很明确”的场景
- B) **短窗口上下文**：附带最近 N 条群消息的“精简版”
  - 例如：仅保留 `role(人/机器人)+sender+text(截断)`，不带长历史
  - 优点：能解决“这句 @ 是在引用还是在派活”的歧义
  - 代价：延迟与 tokens 增加；也更容易被 history compact 影响
- C) **摘要上下文**：附带一段稳定的“频道摘要/状态”
  - 例如：“当前在讨论 X；未完成任务 Y；@b 负责… @c 负责…”
  - 优点：信息密度高、对 cache 相对友好
  - 风险：摘要过期会误导路由

建议默认走 A；B/C 仅作为未来可选增强（不作为本方案的前置条件）。

---

## 5. 执行语义（server 侧）

### 5.1 对 directive/ping：触发目标 bot 推理或排队

对每条 assignment：
- 生成 `trigger_id`
- 将其作为一个“relay 注入消息”写入目标 bot session，并调用 `chat.send(...)`
- 如果目标 session 正在跑：`chat.send` 会自动入队（Followup/Collect 由全局配置决定）

对 `ping`：
- `taskText` 固定为 `"ping"`（或 `"请确认收到"`），保证目标 bot 收到的输入可读、可解释
- 仍然触发 LLM 推理（本轮冻结口径）

### 5.1.1 router 失败/超时的兜底（极小化口径）

router 是外部/内部依赖，必须明确失败时的系统语义，否则实现会各自“猜”：

- **默认（推荐）**：`drop_and_log`
  - router 调用失败/超时 → 当次不产生 assignments（等价于“本条消息不派活”）
  - 但必须记录：`router_error=true`、错误原因、耗时、`route_key`
  - 优点：不误派活；机制最简单
  - 缺点：可能出现“用户明明 @ 了 bot，但这次没人被激活”的漏派（靠日志排障）
- **可选**：`ping_matched_mentions`（fail-open）
  - router 调用失败/超时 → 对“已匹配到候选 bot 的 mentions”逐个生成 `ping` assignment（`taskText="ping"` 或固定短句）
  - 优点：尽量不漏派（至少能叫醒被点名的 bot）
  - 缺点：会把引用/举例里的 mention 也可能当成 ping（误激活风险上升）

> 注：如果你还保留旧 Strict/Loose 解析，也可以把它作为另一个可选 fallback，但这会把复杂度带回主路径；MVP 不建议默认启用。

### 5.2 必须的两层去重（否则“所有 bot 都能路由”会重复派发）

因为同一条群消息可能被多个 bot 同时看到并尝试路由：

1) **router 去重**（同一入站消息只路由一次）
- key：`telegram.route|chat_id|message_id`
- 命中则跳过 router（避免 N 个 bot 各调用一次 router）

2) **assignment 去重**（同一入站消息对同一目标只激活一次）
- key：`telegram.assign|chat_id|message_id|target_account_id|task_hash`
- 命中则跳过执行（避免重复激活同一个目标 bot）

> 备注：此处不限定必须由“哪个 bot”做 router；允许所有 bot 都具备能力，但通过去重保证全局只执行一次。

---

#### 5.2.1 去重为什么关键（举例讲人话）

例子：同一个群里有 3 个 bot（a/b/c）。人类发了一条消息（假设 `chat_id=group42`，`message_id=100`）：
```text
@b 你处理下 X；@c 你处理下 Y
```
在“广播 + 多 bot 都能路由”的架构里，a/b/c 三个 bot **都会收到**这条入站消息，并且都可能尝试做路由：
- 没有 router 去重：router 可能被调用 3 次（浪费 tokens/延迟），并且每次都可能触发一轮派活执行（重复激活 b/c）。
- 有 router 去重：3 个 bot 都可以“尝试路由”，但只有第一个成功写入 `route_key` 的执行者会真正调用 router；其余实例看到命中就直接跳过（或复用已落地的路由结果）。
  - router 去重 key 示例：`telegram.route|group42|100`
- 仍需要 assignment 去重：即使 router 只跑一次，后面的“派活/激活推理”也可能因为并发竞争、重试、弱网重复投递、或进程重启而重复执行，所以要保证对同一 `target_account_id` 的激活是幂等的（不会重复激活、重复推理、重复回群）。

一句话：**去重不是“优化”，而是正确性前提**；否则“所有 bot 都能路由”必然会变成“重复派活/重复推理/重复回复”。

---

#### 5.2.2 assignment 去重（再举一个更直观的例子）

还是上面的那条人类消息（`group42/100`），router 只跑了一次并输出：
- 给 b：`directive`，`taskText="你处理下 X"`
- 给 c：`directive`，`taskText="你处理下 Y"`

系统随后会执行两次“激活推理”（写入目标 bot session 并触发 `chat.send(...)`）。假设在执行“激活 b”时发生了重试：
- 第一次执行：已经成功把“你处理下 X”注入到了 b 的 session，并触发了 b 推理
- 但调用方没有及时收到成功回执，于是触发了第二次重试（或另一个 worker 接手重复执行）

如果没有 assignment 去重，b 可能会被重复激活两次，导致：
- b 推理两次
- b 在群里回两条几乎相同的结果（用户体感就是“重复回复/刷屏”）

assignment 去重的作用就是把“激活 b 这件事”做成幂等：
- assignment 去重 key 示例：`telegram.assign|group42|100|telegram:bot_b|hash(你处理下 X)`
- 命中该 key 时直接跳过执行：保证 b 最多只会因为这条入站消息被激活一次（同理对 c 也成立）。

---

#### 5.2.3 （可选）按 `chat_id` 串行：减少并发踩踏与乱序体感

这不是“去重”，但在群聊高并发时很实用：把同一个群（同一 `chat_id`）的路由任务串行处理。

例子：同一个群里，短时间内连续来了两条消息：
1) `group42/100`：`@b 做X`
2) `group42/101`：`@c 做Y`

如果完全并行处理，可能出现“101 先路由完、100 还在路由”的情况（尤其当 router/执行在重试或排队时），用户体感会比较混乱。  
按 `chat_id` 分区串行后，`group42` 的 route job 会按顺序排队处理，至少能保证同群内处理顺序更稳定、可解释。

---

#### 5.2.4 去重存储、TTL 与重启语义（落地必须写清）

上面的 `route_key` / `assign_key` 如果只存在“某个 bot 自己的内存里”，在多进程/多实例/重启场景下会失效，导致重复派活复发。MVP 也建议把落地口径写清楚：

- **去重存储必须跨实例共享**
  - 推荐：Redis（带 TTL）或任何支持原子 `set_if_absent` 的 KV
  - 单机开发可用：进程内 TTL map（但一重启就会丢，重复派活概率会上升）
- **建议 TTL（可调）**
  - `route_key` TTL：`24h`（足够覆盖短期重放/重试/重复投递；也能控制存储大小）
  - `assign_key` TTL：`7d`（更偏向排障：避免“几天后重试/重启”又把同一任务派一遍）
  - 说明：TTL 不是“业务语义”，只是“幂等窗口”；可以按你的排障与成本偏好调整
- **重启语义**
  - 如果去重存储是进程内的：重启后会忘记已处理的 `route_key/assign_key` → 有概率重复激活（可接受但要在文档里明确）
  - 如果去重存储是 Redis/持久 KV：重启不影响幂等窗口 → 重复派活显著减少

---

### 5.3 Router tool：逻辑单例 vs 物理多实例（以及调用次序）

群聊里消息与各 bot 的回复存在先后顺序与并发竞争，因此 router 的正确打开方式建议是：
- **逻辑上单例**：对同一条入站消息（`chat_id + message_id`）只产生一次“路由决策”（靠 5.2 的去重保证）。
- **物理上可多实例**：router 服务/worker 可以水平扩展，吞吐靠扩容；去重让你不需要“只能一个进程”。

关于“返回了之后马上就要 router”的直觉，这里建议澄清调用单位（避免误解）：
- router 的调用单位是“一条 Telegram 消息”。**每条消息最多路由一次**（用 `chat_id + message_id` 幂等）。
- bot 的回复消息本身也是 Telegram 新消息：如果它也包含 mention（触发标准命中），那它就**作为一条新消息**再走一次 router；但不会因为“有人回复了”就回头反复重跑旧消息的 router。

---

### 5.4 是否需要排队：按 `chat_id` 分区串行（可选）

当 router 需要更强的一致性/顺序保证时（例如使用 B/C 档上下文，或者会更新“群摘要/状态”），才建议引入一个非常明确的执行语义：
- **同一群（同一 `chat_id`）的 route job 串行处理**
- **不同群之间可以并行**

实现上常见做法是“按 `chat_id` 分区队列/分区 worker”：
- 好处：同群内路由次序稳定、减少并发踩踏；跨群吞吐仍可扩展。
- 代价：同一群里极端高频时会排队（但这是“顺序一致性”的自然代价）。

如果 router 永远只用 A 档（不读历史、不写摘要），严格串行不是刚需；但即使如此，按 `chat_id` 串行仍能减少并发边界问题（尤其在重试/重启时）。

---

## 6. 示例（讲人话）

### 6.1 明确派活（directive）
输入：
```text
@b 你干xxxx  @c 你干XXX
```
router 输出（示例）：
- b: directive, `你干xxxx`
- c: directive, `你干XXX`
server 行为：
- 激活 bot b 推理（或入队）
- 激活 bot c 推理（或入队）

### 6.2 纯点名（ping）
输入：
```text
@b
```
router 输出（示例）：
- b: ping, `ping`
server 行为：
- 激活 bot b 推理（或入队）

### 6.3 引用/举例（reference，不激活）
输入：
```text
我举个例子：@b 你干xxxx
```
router 输出（示例）：
- b: reference（或 assignments 为空）
server 行为：
- 不触发 b

---

## 7. 与现有 Strict/Loose 的关系（迁移建议）

### 7.1 现状（简述）
- Strict：基本只认“行首点名”是指令
- Loose：非行首点名会额外调用一次 LLM 做 `directive|reference` 分类

### 7.2 proposal 的定位
- 用 router tool 统一替代 Strict/Loose 的“规则 + 边角”解析：
  - “是否是指令/引用/叫醒”交给 router
  - “是否触发/触发谁/触发什么文本”交给结构化输出

迁移策略建议：
- Phase 1：保留旧解析作为 fallback，router 先只在部分条件下启用（对照观测误触发/漏触发）
- Phase 2：router 覆盖所有 mention 触发路径，旧解析逐步退役

---

## 8. 可观测性（最少字段）

建议日志/事件字段：
- `chat_id`, `message_id`, `sender`
- `route_key`（例如 `telegram.route|chat_id|message_id`）, `route_dedupe_hit`（bool）
- `assign_dedupe_hit`（bool，至少在 debug 级别可见）
- `queue_partition`（例如 `chat_id`）, `queue_depth`（可选）
- `router_called`（bool）, `router_model`（string）
- `router_assignment_count`
- `assignments[]`（至少打印 targetAccountId + kind + taskText_hash，不打印全文避免隐私/刷屏）
- 对每个执行：
  - `target_session_id`
  - `trigger_id`
  - `queued`（是否入队）、`queue_mode`

---

## 9. 风险与注意点

- “任何 mention 都触发 LLM”会显著增加推理次数：需要后续讨论成本与小模型 router 的稳定性（本轮先不展开）。
- 路由器一旦判错可能引发连锁：必须依赖去重与 hop 限制（现有 relay chain/hop 语义可复用）。
- 当 `candidates` 列表不完整时，router 会漏派：需要一个可靠的“本群 bot 列表”来源（本文先假设候选列表可用；后续由“后台群成员列表功能”补齐）。

---

## 10. Open Questions（后续再谈）

- router 具体用哪个模型/成本上限/超时策略？
- `ping` 的注入文案用 `"ping"` 还是中文短句？（本轮冻结为“会触发推理”，文案可后议）
- 需要一个“router bot”还是“所有 bot 皆可路由 + 全局去重”（本文倾向后者）？
- 是否要引入 B/C 档上下文（或两段式路由）来提升“引用 vs 派活”的区分准确度？（MVP 默认不引入）

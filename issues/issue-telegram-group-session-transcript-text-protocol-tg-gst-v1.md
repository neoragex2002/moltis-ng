# Issue: Telegram 群聊 Session 会话文本转写协议收敛（TG-GST v1 / transcript / rewrite）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-08
- Owners: <TBD>
- Components: telegram / gateway / sessions / agents
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- Telegram 入站 self-mention stripping + 空白归一化 + self-mention-only 固定短回复：`crates/telegram/src/handlers.rs:2382`、`crates/telegram/src/handlers.rs:2492`
- 群聊 bot1→bot2 可见性补偿（outbound mirror into sessions，带 `[@source mirror]` 前缀）：`crates/gateway/src/chat.rs:6743`
- 群聊 bot@bot relay（注入 `（来自 @source_bot）...` 并触发目标 bot run）：`crates/gateway/src/chat.rs:6571`、`crates/gateway/src/chat.rs:6626`
- ✅ TG-GST v1 配置开关（默认 legacy）：`crates/telegram/src/config.rs`（2026-03-08）
- ✅ Telegram 群聊入站（dispatch + listen-only ingest）按 TG-GST v1 写入 transcript（speaker/`-> you`/保留换行与 @mentions/媒体占位）：`crates/telegram/src/handlers.rs`（2026-03-08）
- ✅ gateway mirror 写入按“目标 bot 配置”选择 legacy vs TG-GST v1（TG-GST v1 不再写 `[@... mirror]` 前缀）：`crates/gateway/src/chat.rs`（2026-03-08）
- ✅ gateway relay 注入按“目标 bot 配置”选择 legacy vs TG-GST v1（TG-GST v1 不再写 `（来自 ...）` 前缀）：`crates/gateway/src/chat.rs`（2026-03-08）
- ✅ TG-GST v1 群聊 session 自动追加 system prompt 解释与输出约束（仅 Telegram 群 + 开关开启生效）：`crates/gateway/src/chat.rs`（2026-03-08）
- ✅ Web UI 增加 “Group Session Transcript” 配置项（legacy / tg_gst_v1）：`crates/gateway/src/assets/js/page-channels.js`（2026-03-08）

**已覆盖测试（如有）**
- self-mention stripping / addressed command / self-mention-only 兜底（“我在。”）：`issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
- ✅ TG-GST v1 Telegram 侧入站/listen-only/always-respond/presence：`crates/telegram/src/handlers.rs`（新增多条 `*_tg_gst_v1_*` tests，2026-03-08）
- ✅ TG-GST v1 gateway mirror/relay/prompt：`crates/gateway/src/chat.rs`（新增 mirror/relay/prompt tests，2026-03-08）

**已知差异/后续优化（非阻塞）**
- 空点名（仅 `@bot`）在 relay（bot→bot）路径下是否应触发：`issues/issue-telegram-group-relay-empty-mention-no-trigger.md`

---

## 背景（Background）
- 场景：Telegram 群聊多 bot 协作时，session 的“纯文本聊天记录”（LLM 上下文）需要同时满足：
  - 模型一眼能读懂“这是群聊 transcript（多说话人）”
  - 模型能读懂“谁在说话/这句是否明确叫我处理”
  - 不扭曲原消息语义（不因 rewrite 造成误读）
- 约束：LLM 主要看到的是 `content` 文本；系统元数据（senderName/username/channel meta）如果不进入 `content`，模型默认不可见或不可靠。
- Out of scope：
  - 不在本单实现 V4 的 WAIT/RootMap/TaskCard/epoch
  - **不在本单改动任何“LLM 唤醒/触发”机制**（mention gating / relay 触发条件 / always-respond 等）：本单仅收敛“写进 session/注入给 LLM 的文本协议”
  - 不在本单重做系统机制（mirror/relay/去重/队列）——只收敛“写进 session/注入给 LLM 的文本协议”

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **TG-GST v1**（主称呼）：Telegram Group Session Transcript v1，一套“写进 session 的文本转写协议”。
  - Why：把“群聊说话人/是否叫我”的关键语义用极简、可读、稳定的格式体现在 `content` 里。
  - Not：不是 mirror/relay 的系统机制本身；也不是 Telegram inbound gating 规则。
  - Source/Method：effective（由本系统在入库/注入时格式化生成）。
  - Aliases（仅记录，不在正文使用）：transcript 协议 / session rewrite 协议 / 对话头部格式

- **addressed（叫我）**（主称呼）：当前 bot 明确需要处理的一条输入（例如被 @ 点名、reply-to-bot、或被 relay 转派）。
  - Not：不等同于“本 bot 最终会不会跑 LLM”（例如空点名可能仍走固定短回执）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 定义并冻结 TG-GST v1 文本格式：每条写入 session/送 LLM 的群聊输入都可被 LLM 直接理解为“多说话人 transcript”。
- [x] 明确 `-> you` 的语义与生成条件：让模型能稳定判断“这句是否明确叫我处理”。
- [x] 确保“尽量保留原文结构”：换行/列表/多点名不得因 rewrite 被压扁或误导。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：多点名时，目标 bot 不得看到“删除 @本 bot 后的误导文本”（例如 `@a @b @c` 在 c 侧不应变成 `@a @b ...`）。
  - 必须：镜像/转派的输入在文本上应呈现为“谁说了什么”，而不是系统黑话前缀（避免模型误把 mirror 当指令）。
  - 不得：在 `content` 中出现 `[@... mirror]`、`（来自 @...）` 这类系统机制前缀（在 TG-GST v1 下）。
  - 不得：对入站文本做全局空白归一化导致结构丢失（在 TG-GST v1 下）。
- 兼容性：
  - 需要支持与 legacy 协议并存（建议 feature flag/配置开关），避免一次切换导致历史行为突变难排障。
- 可观测性：
  - 需要能从日志/metadata 判断“当前 session 采用 legacy 还是 TG-GST v1”。
- 安全与隐私：
  - 不在日志打印完整正文；speaker 仅使用 Telegram username 或 user_id（不额外扩展隐私字段）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) self-mention stripping 造成多点名时“每个 bot 看到不同文本”，且容易误导模型（例如被点名者看到的文本看起来像没叫它）。
2) 空白归一化压掉换行/列表，导致结构化指令可读性下降。
3) mirror/relay 通过在 `content` 前拼接系统前缀传递元语义，导致 session 读起来“不像群聊 transcript”，模型也容易把 mirror/relay 当作某种指令/系统消息。

### 影响（Impact）
- 用户体验：群聊协作不自然、不直观；bot 容易误读“谁在叫我/谁在说话”。
- 可靠性：同一条群消息在不同 bot 上下文呈现不一致，导致推理决策分歧与难复现。
- 排障成本：文本中混杂“系统机制前缀”，很难用人类直觉对齐实际群聊发生了什么。

### 复现步骤（Reproduction）
1. 在群里发送：`@a @b @c 请执行 X`
2. 观察：在 legacy 协议下，bot C 侧 `content` 变成 `@a @b 请执行 X`（自我点名被删除），易误导。
3. 观察：包含换行/列表的消息在 legacy 协议下被压缩成一行或结构变弱。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - self-mention stripping + 空白归一化：`crates/telegram/src/handlers.rs:2382`、`crates/telegram/src/handlers.rs:2492`
  - mirror 前缀 `[@source mirror]`：`crates/gateway/src/chat.rs:6743`
  - relay 注入前缀 `（来自 @source_bot）...`：`crates/gateway/src/chat.rs:6571`
- 文档证据（as-is 行为与例子）：
  - `issues/discussions/telegram-group-at-rewrite-mirror-relay-as-is.md`
  - `issues/discussions/telegram-group-relay-mention-strictness-as-is.md`

## 根因分析（Root Cause）
- A. 当前系统用“拼接/删除正文”的方式把系统元语义（谁说话、mirror/relay 来源、是否点名）塞进纯文本 `content`。
- B. 这种做法会不可避免地产生“文本视角不一致”（删 self mention）与“结构损失”（空白归一化），并把系统黑话前缀引入 LLM 上下文。
- C. LLM 在阅读上下文时只能看到 `content`，因此会把这些前缀当作自然语言的一部分，从而误解语义或做出不自然的回应。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - session 的群聊历史在文本上呈现为“多说话人 transcript”，每条都有明确 speaker。
  - 明确标记“这句是否叫我处理”，且标记形式极简、稳定、与 bot 名字解耦。
  - 保留原消息正文（包括 `@mentions` 与换行/列表），不做会导致语义变形的 rewrite。
- 不得：
  - 不得在 `content` 中使用系统机制前缀（如 `[@... mirror]`、`（来自 @...）`）。
  - 不得对正文做全局空白归一化（压扁换行/列表）。
- 应当：
  - 与 legacy 协议可并存，可按 bot/群或全局开关切换，便于灰度与回滚。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：TG-GST v1（极简 transcript 协议）
- 核心思路：
  - 把“谁在说话 + 是否叫我”的语义用一个统一头部表达；正文尽量原样保留。
  - mirror/relay 只改变“写入 session 的文本形式”，不改变系统机制本身。
- 优点：
  - LLM 一眼能读懂群聊；文本自然；语义稳定；减少误读与歧义。
  - 实现成本低：主要是 format `content` 的一层 wrapper（入站写入/镜像写入/转派注入三处）。
- 风险/缺点：
  - 相比 legacy 前缀，`<speaker>:` 会带来少量 token 开销（可接受，且换来稳定语义）。
  - 需要更新 system prompt，解释格式与 `-> you` 含义。

#### 方案 2（备选）：仅在 system prompt 注入元信息，不改变 content
- 风险：元信息不随每条消息重复出现，模型在长上下文里更容易丢失“谁说话/是否叫我”的局部判断；且无法解决“多点名删 self”的视角不一致。

### 最终方案（Chosen Approach）
- 采用方案 1：TG-GST v1，并保留 legacy 作为默认/回滚路径（通过开关控制）。

#### 行为规范（Normative Rules）
1) **统一头部格式**
   - 写入 session 的每条群聊输入文本必须为：
     - `<speaker><addr_flag>: <body>`
2) **speaker 规则（稳定优先）**
   - 若 Telegram `username` 存在：`<username>`（不带 `@` 前缀）
   - 否则：`tg:<user_id>(<display_name>)`
   - 若发送者为 bot：speaker 后追加 `(bot)`
   - speaker 规范化（为保证头部稳定可解析）：
     - `<username>`：使用 Telegram 原始 username（不带 `@`），不做大小写改写（显示层可按原样或统一小写，二选一需冻结）。
     - `<display_name>`：去除换行与多余空白；建议截断到 64 字符以内，避免头部过长影响上下文可读性。
3) **addressed 标记（极简且与 bot 名字解耦）**
   - 当且仅当该条输入被判定为“明确叫当前 bot 处理”（addressed）时，追加 ` -> you`
   - `-> you` 的出现不承诺“一定跑 LLM”（例如空点名可能走固定回执），但承诺“语义上明确叫你”
   - **实现约束（不改唤醒机制）**：`addressed` 必须复用既有判定信号/路径，不得引入新的“再解析一遍文本”的启发式。
     - Telegram 入站（人→bot）：复用现有“mention/reply-to-bot 命中”的布尔结果（本单不改其判定逻辑）。
     - relay 注入（bot→bot）：注入给目标 bot 的那条输入一律视为 addressed（加 `-> you`）。
     - mirror 写入（bot→bot 可见性补偿）：仅知情，不视为 addressed（不加 `-> you`）。
     - `mention_mode=always`：即使该 bot 可能会对未点名消息运行，也不把这些消息标为 addressed（不加 `-> you`），确保 `-> you` 只表达“显式点名/转派/回复我”。
4) **body 原样保留**
   - 不删除任何 `@mentions`
   - 不做全局空白归一化（必须保留换行/列表结构）
   - body 的来源（不引入额外语义）：
     - Telegram 入站（人→bot）：使用 Telegram 原始 text/caption（不做 self-strip，不压扁换行）。
     - mirror 写入（bot→bot）：使用 source bot 出站的 as-sent 最终正文（不加任何 mirror 前缀）。
     - relay 注入（bot→bot）：使用既有 relay 解析得到的 `task_text`（不加“来自”前缀；也不需要再附带 `@target` 点名）。
5) **禁止系统黑话前缀进入 content**
   - 在 TG-GST v1 下，不得在 `content` 中出现 `[@... mirror]`、`（来自 @...）` 等前缀
   - mirror/relay 机制信息应放到 `channel` metadata 与日志中（脱敏）
6) **媒体占位（最小）**
   - 无文本但有媒体：`<speaker>: [photo]` / `[file]` / `[voice]` / `[sticker]`
   - 有 caption：`<speaker>: [photo] caption: <caption原文>`

#### 文本转写示例（Session content examples）

> 说明：以下示例均为 **TG-GST v1 启用后**，写进 session / 送进 LLM 的 `content` 文本长相（只展示文本，不展示 metadata）。
> 假设“当前 bot”为 `@duoduo`，因此 `-> you` 的语义是“明确叫 @duoduo 处理”。

1) **群聊普通消息（不叫我，仅旁听）**
```text
neo: 大家先同步一下今天的进度。
```

2) **群聊点名叫我（保留原文 @mentions，不做 self-strip）**
```text
neo -> you: @duoduo 你处理下 X
```

3) **群聊多点名（目标 bot 仍能看到包含自己的原文，不出现“删掉 @本 bot”）**
```text
neo -> you: @alma @zhuzhu @duoduo 请执行 X
```

4) **只有点名（body 允许只有一个 @，不再“删空”）**
```text
alma(bot) -> you: @duoduo
```

5) **跨行点名 + 任务正文（保留换行，不压扁）**
```text
alma(bot) -> you: @duoduo

你处理下 X，并在完成后行首 @ 我汇报。
```

6) **结构化正文（列表/换行保留）**
```text
alma(bot) -> you: @duoduo 验收 PR#218：
- 接口兼容
- 异常路径
- 回滚可用性
```

7) **reply-to-bot 唤醒（无需 @，但语义上叫我处理，因此标 `-> you`）**
```text
neo -> you: 你看这个 500 和 PR#218 有关吗？
```

8) **mirror 写入（bot 说话像正常群聊发言；不再出现 `[@... mirror]` 前缀）**
```text
alma(bot): 我拆活：@zhuzhu 看 migration；@duoduo 做验收；我来汇总。
```

9) **relay 注入（bot→bot 转派也表现为群聊发言 + `-> you`）**
```text
alma(bot) -> you: 请你验收 PR#218（重点：接口兼容、异常路径、回滚可用）。
```

10) **媒体占位（最小可读）**
```text
neo: [photo]
```
```text
neo: [photo] caption: staging 出现一条 500（截图）
```

11) **always-respond（语义上未叫我，但我可能仍会运行；因此不加 `-> you`）**
```text
neo: 这块我不太确定，你们怎么看？
```

#### 接口与数据结构（Contracts）
- System prompt（必须增补两条“读懂 transcript 的规则”）：
  - 规则 A：每条历史消息格式为 `<speaker><addr_flag>: <body>`，这是群聊 transcript。
  - 规则 B：`-> you` 表示该条消息明确需要你处理/你被叫到。
- 建议追加一段最小可复制的 system prompt 片段（用于 persona/system prompt 里直接粘贴）：
```text
## Group Transcript (TG-GST v1)
- This session is a Telegram group chat transcript.
- Each incoming message is formatted as: <speaker><addr_flag>: <body>
- <speaker> identifies who is speaking (e.g. neo, alma(bot)).
- If <addr_flag> is " -> you", it means the message is explicitly addressed to you and requires your attention.
- When replying/summarizing:
  - Do NOT output transcript-style lines like "<speaker>: ...". Use normal prose/bullets.
  - Do NOT start a line with "@someone" unless you are intentionally delegating work to that bot.
  - If you must quote a line that contains "@mentions", wrap the quote in '>' lines or fenced code blocks (relay scanning skips quotes and code blocks).
```
- 配置（建议，便于灰度与回滚）：
  - Telegram bot 配置项：`group_session_transcript_format = "legacy" | "tg_gst_v1"`（默认 `legacy`）
  - 生效范围建议：先按 bot 级开关；后续可扩展到按 chat_id 覆盖。

#### 失败模式与降级（Failure modes & Degrade）
- 风险：与既有 Telegram 群聊 relay（Strict 行首点名触发）产生“误触发”的交互
  - 关键澄清：relay 扫描的是 **bot 在 Telegram 群里实际发送的 outbound_text（原始输出）**，不是 session 内的转写文本；风险来自“模型在 summary/复述时把 `@某个 bot` 写在行首（看起来像点名派活）”。
  - 说明：若模型在群里输出/复述时把“@某个 bot”写在**行首**（例如行首 `@alma ...`），strict relay 可能会把它当作“派活指令”触发 relay。
  - 具体示例（人话时间线）：
    1) 在 `@duoduo` 的 session 里，历史输入长期长这样（TG-GST v1）：
       - `alma(bot): 我拆活：@zhuzhu 看 migration；@duoduo 做验收。`
    2) 群里有人让 `@duoduo` “总结一下大家做什么”。
    3) 如果模型在 summary 时错误地用“行首 @点名”的格式去复述（这就是 **outbound_text 原始输出**）：
       ```text
       @alma 我拆活：@zhuzhu 看 migration；@duoduo 做验收。
       @zhuzhu migration 有锁表风险，建议 CONCURRENTLY 并补回滚脚本。
       @duoduo 我负责验收接口兼容与异常路径。
       ```
    4) gateway 的 strict relay 扫描这段 outbound_text 时，会把每一行的行首 `@alma` / `@zhuzhu` 当作“行首点名”，并把后面的文本当作 `task_text`，于是误触发：
       - relay 触发 `@alma`（任务文本从 `我拆活：...` 开始）
       - relay 触发 `@zhuzhu`（任务文本从 `migration 有锁表风险...` 开始）
    5) 结果：本来只是 `@duoduo` 在做 summary，却把 `@alma/@zhuzhu` 意外叫醒跑了一轮，群里出现无意义的额外回复；严重时可能形成 ping-pong/刷屏风险。
- 缓解（不改唤醒机制的前提下，二选一即可）：
    1) 在 persona/system prompt 中明确（推荐，最小变更）：
       - 输出 summary/复述时，**不要用 `<speaker>: ...` 的 transcript 行风格**；
       - 更重要：输出时**不要把 `@bot` 写在行首**（除非你确实要派活触发 relay）；
       - 若必须引用/复述包含 `@mentions` 的原话，用 `>` 引用行或 fenced code 包裹（当前 relay 扫描会跳过引用行与代码块）。
    2) 或在 TG-GST v1 的 `<speaker>` 表达上增加一个固定非空白前缀（例如 `msg neo: ...`），确保 `@bot` 不会出现在行首位置，避免 strict relay 误判。
- 若 speaker 缺失 username 且 display_name 为空：
  - 使用 `tg:<user_id>` 作为 speaker，仍满足“可读 + 稳定”。
- 若配置/格式化异常：
  - 回退到 legacy 文本生成（但日志标记一次降级原因，不打印正文）。

#### 安全与隐私（Security/Privacy）
- 日志仅记录：
  - 是否启用 TG-GST v1、chat_id、message_id、sender_id/是否 bot、是否 addressed
  - 禁止打印 `body` 原文

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 群聊 transcript 在 session 中呈现为多说话人格式 `<speaker>: ...`（能直接读懂“谁说话”）。
- [x] 当消息明确叫当前 bot（@mention/reply-to-bot/relay 转派）时，session 里该条输入带 ` -> you` 标记。
- [x] 多点名消息 `@a @b @c ...` 写入 bot C session 后仍保留 `@a @b @c ...` 原文，不出现“删 @c 后的误导文本”。
- [x] 保留换行/列表结构（不压扁）。
- [x] 启用 TG-GST v1 后，新写入的 session `content` 不再包含 legacy 的 `[@... mirror]` 与 `（来自 @...）` 前缀（历史记录不要求重写）。
- [x] system prompt 增补 TG-GST v1 解释 + 输出约束（summary/复述不得行首 `@someone`，引用含 `@` 的原话用 `>`/代码块）。
- [x] 回滚：关闭开关后恢复 legacy 行为，且不影响系统机制（mirror/relay/队列/去重）本身。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 入站格式化：speaker/addressed/body（含换行保留）：`crates/telegram/src/handlers.rs`
- [x] mirror 写入文本格式：speaker 为 source bot，且不带 legacy 前缀：`crates/gateway/src/chat.rs`
- [x] relay 注入文本格式：speaker 为 source bot 且 `-> you`：`crates/gateway/src/chat.rs`

### Integration
- [x] 群聊模拟：人类消息（未点名/点名/回复 bot）+ bot 出站 mirror + bot@bot relay，检查 session 文本序列与标记稳定性。（以 gateway/telegram 单测覆盖关键路径）

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：需要端到端多 bot Telegram 群聊环境或高质量 mock。
- 手工验证步骤：
  1) 在 Web UI（Channels → Telegram bot → Edit）把 `Group Session Transcript` 设为 `TG-GST v1`；
  2) 将该 bot 加入一个 Telegram 群（chat_id 为负数的群/超级群）；
  3) 在群里发送未点名消息：`hello everyone`，检查该 bot session 最近一条写入形如 `neo: hello everyone`（无 `-> you`）；
  4) 在群里发送点名消息：`@<bot_username> 你处理下 X`，检查 session 写入形如 `neo -> you: @<bot_username> 你处理下 X`（保留 @mentions）；
  5) 在群里发送多点名：`@a @b @<bot_username> 请执行 X`，检查写入仍保留 `@a @b @<bot_username>` 原文；
  6) 在群里发送跨行点名：
     ```text
     @<bot_username>

     你处理下 X
     ```
     检查 session 写入保留换行；
  7) 触发 mirror（让 bot 在群里发一条消息），检查其它 bot（已存在 session 的）在 session 里看到形如 `<source_bot>(bot): ...`，且不含 `[@... mirror]`；
  8) 触发 relay（让 bot A 行首派活 `@botB ...`），检查 bot B session 里看到形如 `<botA>(bot) -> you: ...`，且不含 `（来自 ...）`；
  9) 回滚：把 `Group Session Transcript` 改回 `Legacy`，重复 3/4/7/8，确认恢复旧前缀与旧行为（仅影响新写入）。

## 发布与回滚（Rollout & Rollback）
- 发布策略：feature flag 默认关闭（legacy）；先对单个 bot/单个群灰度开启。
- 回滚策略：切回 legacy（不做数据迁移；仅改变后续写入的文本格式）。
- 上线观测：日志统计启用比例、降级次数、以及 `-> you` 标记命中率（不含正文）。

## 实施拆分（Implementation Outline）
- Step 1: ✅ 增加 transcript format 开关（legacy/tg_gst_v1），并在日志中可观测。
- Step 2: ✅ 入站写入路径应用 TG-GST v1 wrapper（保留原文与换行，不做 self-mention stripping/空白归一化）。
- Step 3: ✅ mirror 写入路径改为 TG-GST v1（以 source bot 为 speaker），并移除 legacy 前缀。
- Step 4: ✅ relay 注入路径改为 TG-GST v1（以 source bot 为 speaker，且标记 `-> you`）。
- Step 5: ✅ 更新 system prompt，新增两条 transcript 解释规则（格式 + `-> you` 含义）。
- Step 6: ✅ 补齐单测/集成测试与手工验收清单。
- 受影响文件（预期）：
  - `crates/telegram/src/handlers.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/agents/src/prompt.rs`（或等价 prompt 组装位置）
  - `issues/discussions/telegram-group-at-rewrite-mirror-relay-as-is.md`（补充 Proposed 协议链接）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/discussions/telegram-group-at-rewrite-mirror-relay-as-is.md`
  - `issues/discussions/telegram-group-relay-mention-strictness-as-is.md`
  - `issues/done/issue-telegram-self-mention-identity-injection-and-addressed-commands.md`
  - `issues/done/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`
  - `issues/done/issue-telegram-group-bot-to-bot-mentions-relay-via-moltis.md`
  - `issues/issue-telegram-group-relay-empty-mention-no-trigger.md`

## 未决问题（Open Questions）
- Q1: `-> you` 的触发口径是否需要区分“被明确点名”与“always respond 仍会处理但未被点名”（推荐：仅表示“明确叫你”，不等同于“会跑 LLM”）。
- Q2: legacy 历史消息是否需要“在线重写”展示为 TG-GST v1（推荐：不做；仅对新写入生效，避免历史内容变动影响排障）。
- Q3: strict relay 的“行首点名”误触发风险缓解是否仅靠 prompt 约束即可（当前倾向：是；本单已在建议的 system prompt 片段中写明）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

# Issue: 命名 persona（按 Telegram bot identity 绑定）+ OpenAI Responses `role=developer` 注入（cache-friendly）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1（多 bot/多用途的根能力；影响可控性与缓存命中）
- Components: gateway / agents / config / sessions metadata / channels(telegram) / providers(openai-responses) / Web UI
- Affected providers/models: openai-responses（重点）；其它 providers 需兼容降级

**已实现（如有，写日期）**
- 暂无（本单为设计与实现规划）

**已覆盖测试（如有）**
- 暂无新增（需补齐：见 Test Plan）

**已知差异/后续优化（非阻塞）**
- prompt cache（OpenAI Responses）按 `session_key` 分桶即可；本单不改分桶策略（persona 与 prompt cache key 无强绑定关系）。

---

## 背景（Background）
- 场景：同一 Moltis 实例管理多个 Telegram bot（多个“agent”），希望每个 bot 有独立 persona；并且希望 OpenAI Responses 的 prompt cache 能稳定复用 persona 前缀。
- 约束：
  - 当前实现只有一条“系统提示”（内部是 `ChatMessage::System`），把 identity/soul/workspace/tools/runtime 混合拼接在一起；在 OpenAI Responses 上游请求中，这条 system 会被映射为顶级 `instructions` 字段（`input` 中不会出现 `role=system` message）。
  - OpenAI Responses 支持 `role=developer`，但当前消息抽象没有 developer role，导致无法表达“system / persona / runtime snapshot”三段分层，也无法得到稳定可缓存的 developer 前缀。
  - 真实上游仍存在不可见的“平台级 system”（模型侧硬约束）；developer persona 只能在其边界内塑造风格与行为（见 `issues/Codex CLI Prompt Dump 深度分析报告.md`）。
- Out of scope：
  - V1 不做“多租户权限隔离”。
  - V1 不做“完整 persona UI 编辑器”（富文本编辑/版本管理/在线协作等）。但必须提供最小 UI 配置入口：每个 Telegram bot 配置（按 `account_handle` 标识）可设置 `persona_id`。
  - V1 不改 OpenAI Responses 的 `prompt_cache_key` 分桶策略（继续按 session_key）。
  - 本单不做“旧版本配置/数据迁移”与兼容；可接受直接清空旧数据（channels DB、旧的 `~/.moltis/*` 文件等）后重新接入。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **channel**：渠道名称（string），例如 `telegram` / `discord` / `feishu`。
- **chan_user_id**：该渠道中“账号本体”的稳定唯一 ID（string/number）。
  - Telegram bot：`getMe.id`（数字）
  - 口径：这是稳定主键；`@username` 和 nickname 都可能变化，但 `chan_user_id` 不应变化。
- **chan_user_name**：该渠道中“账号本体”的可读用户名（string）。
  - Telegram bot：`getMe.username`（**不带** `@`；展示/点名时渲染为 `@{chan_user_name}`）
  - 口径：可变（允许改名）；用于“群里点名/展示”。
- **chan_nickname**：该渠道中“账号本体”的可读显示名（string）。
  - Telegram bot：`getMe.first_name + last_name`（或平台侧展示名等价物）
  - 口径：可变；用于 UI 展示与人类指挥习惯；不等价于渠道的可路由点名（Telegram 点名仍以 `@{chan_user_name}` 为准）。
- **account_handle**：Moltis 内部对“某个渠道账号配置”的稳定引用句柄（string，自动生成，不要求用户手填/关注）。
  - 生成策略（本单冻结）：`<channel>:<chan_user_id>`（例如 `telegram:8576199590`）
  - Why：避免把可变的 `@username`/nickname 当主键，导致改名即断链。
  - Not：不是 Telegram 的 `chat_id`；也不是 `@username`。
  - 来龙去脉（人话）：
    - 旧称 `account_id` 容易让人误以为“必须手填/必须记住”，且语义含糊；本单将其改名为 `account_handle` 并明确为“纯内部实现句柄”。
    - 用户侧的心智模型收敛为 4 元组：`channel + chan_user_id + chan_user_name + chan_nickname`；其中稳定主键是 `chan_user_id`，而 `account_handle` 只是实现细节。
- **session_key**：对话上下文与历史存储单元的稳定键（string）。
  - Telegram session_key 格式（本单冻结）：`{channel_type}:{chan_user_id}:{chat_id}`，例如 `telegram:8576199590:-100999`。
  - Why：`account_handle` 已包含 `telegram:` 前缀，session_key 必须避免 `telegram:telegram:...` 双前缀。

- **Moltis system_prompt（internal）**：Moltis 内部构建的“系统提示文本”（string），通过 `ChatMessage::System { content }` 进入消息序列。
  - 口径（现状）：system_prompt 同时承载“工具/运行时/执行路由/Guidelines（系统级规约）”与“identity/soul/user/workspace（persona/owner/工作区信息）”，没有显式分层。
  - 代码证据：`crates/agents/src/prompt.rs:152`（`build_system_prompt_full` 拼接了 base_intro、Identity/Soul、Runtime、Skills、Workspace Files、Long-Term Memory、Available Tools、Guidelines…）。

- **persona**（主称呼）：一套可复用的 agent 行为定义输入源（用于构建 LLM 指令前缀）。
  - Why：同一实例内支持多个 bot/用途（ops/coder/research）且不互相污染。
  - Not：不是“用户身份”（`USER.md`），也不是“会话历史”。（本单会将 `USER.md` 作为 **Owner 信息** 注入到同一条 developer message 的独立小节，并明确标注这是 owner，而不是“当前发言者”。）
  - Source/Method：configured（从 data_dir 文件读取）→ effective（合并默认）→ as-sent（写入上游请求体）。
  - Aliases（仅记录，不在正文使用）：agent profile / profile

- **persona_id**：persona 的稳定标识（string）。
  - 约定：`default` 为系统默认 persona；其它 `persona_id`（如 `ops` / `research` / `coder`）由 owner 自行命名与维护。

- **session**：对话上下文与历史存储单元（`session_key` 唯一标识）。

- **run**：某个 session 内“一次请求/一次 agentic loop 执行”（一次入站消息触发一次 run）。
  - 说明：`run_id` 每次都会变（不可用于缓存 key）。
  - 代码证据：`crates/gateway/src/chat.rs:2106`（“Generate run_id early … link … agent run”）。

- **OpenAI Responses: instructions**：请求体字段 `instructions`（独立字段，不是 message role）。
  - 口径（现状 / as-is）：`ChatMessage::System` 会被拼成 `instructions`（`input` 中不包含 `role=system` message）。
  - 口径（目标 / to-be，本单冻结）：对 OpenAI Responses **不得**用 `instructions` 注入 Moltis 的 system/persona；必须改为三条 `role=developer` messages（system > persona > runtime snapshot），以获得稳定可缓存前缀与可观测的分层。

- **OpenAI Responses: `role=developer`**：`input` 数组中的 message item 角色之一。
  - 口径（本单冻结）：
    - OpenAI Responses 场景下，Moltis 的 **system** 与 **persona** 都必须用 `role=developer` 表达（不得发送 `role=system`）。
    - developer messages 必须按优先级排序：`system > persona > runtime_snapshot`（system developer message 在前，persona developer message 在后，runtime snapshot 最后）。

- **stable vs volatile（缓存相关口径）**
  - stable：在同一 `prompt_cache_key` 桶内应保持不变的文本（应优先进入 `role=developer`，以最大化 cached_tokens）。
    - 例：persona 的 identity/soul/rules、固定的工具调用规范（若不随 run 变化）。
  - volatile：每次 run/每条消息可能变化的文本（不得进入 developer persona）。
    - 例：`run_id`、时间戳、消息计数、token 统计、重试次数、工具输出、环境探测结果等（原则上不纳入 `role=developer`，避免影响 prompt cache）。
  - 临时折中（本单 V1 口径；后续可单独优化）：允许将“运行态快照”以 **第 3 条** `role=developer` 注入（见下文规则 1）。
    - Why：先把 system/persona 分层与可观测性做正确；运行态文本后续再做稳定化与裁剪优化。
    - Risk：会降低 prompt cache 命中与 cached_tokens 收益；必须在文本内明确标注“informational snapshot / may change”。

- **prompt cache**（Responses）：通过 `prompt_cache_key` 复用先前请求的缓存前缀。
  - configured：`providers.openai-responses.prompt_cache.*`（配置）
  - as-sent：请求体字段 `prompt_cache_key`（实际发送）
  - 口径：本单保持 `prompt_cache_key` 按 `session_key` 分桶；persona 仅影响“可缓存前缀文本”的稳定性，不参与分桶。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 支持用户自定义多个命名 persona（可列举/可选择/可默认）。
- [ ] 支持按 **Telegram bot identity** 绑定 persona（每个 bot 可独立 persona；不做 session 覆盖）。
  - Telegram bot identity = (`channel="telegram"`, `chan_user_id`)
- [ ] 保留 Moltis 系统默认 persona（`default`），用于非 Telegram 场景（例如 `main`）。
- [ ] OpenAI Responses provider：每次 run 必须显式发送 **三条** `role=developer` messages（`system > persona > runtime_snapshot`），且不得使用顶级 `instructions` 字段注入 Moltis system/persona。
- [ ] `spawn_agent` 子代理默认使用系统 `default` persona（允许显式指定 persona/model）；不从父 session“继承 persona”（避免隐式传播与口径混乱）。
- [ ] debug/context/raw_prompt 可见 effective persona（persona_id + 来源）。
- [ ] agent 之间彼此“认识”（通过 `## People (reference)` 指向 `PEOPLE.md`，提供“本实例可用 agent roster”的统一来源 + 委派注意事项）。
- [ ] agent “认识主人/用户”（来自 `USER.md` 的 owner 信息，放入 developer message 的独立小节）。
- [ ] Telegram 接入体验收敛：Add Telegram Bot 表单不再要求手填 username；仅输入 token，后端通过 `getMe` 自动获得 `chan_user_id/chan_user_name/chan_nickname` 并生成 `account_handle=telegram:<chan_user_id>`。
- [ ] Web UI：Telegram bot 配置页提供 persona 配置入口（最小可用：输入 `persona_id` + 保存）。
  - UI 建议（V1 最收敛，避免引入 personas discovery API）：
    - Add Telegram Bot：不再要求输入 username；仅输入 token（后端 `getMe` 自动发现 `chan_user_id/chan_user_name/chan_nickname` 并生成 `account_handle`）。
    - Edit Telegram Bot（`EditChannelModal`）增加一个 `Persona ID` 字段（可选 string；为空视为 `default` 生效）。
    - 表单控件建议用 text input（而不是下拉），减少“列举 persona”后端依赖；后续再补下拉/自动补全。
    - UI 文案：`Persona ID (empty = default)`；旁边加一行提示：`Create personas under ~/.moltis/personas/<persona_id>/...`。
    - UI 摆放建议：放在 `Default Model` 上方或下方（同属“bot 行为配置”），避免散落在 gating/relay 配置之间。
    - 落点文件：
      - Add 表单渲染/保存：`crates/gateway/src/assets/js/page-channels.js`（AddChannelModal：删除 `Bot username` 字段与 `account_id` 提交）
      - 表单渲染：`crates/gateway/src/assets/js/page-channels.js:437`（EditChannelModal 的表单区域）
      - 保存 payload：`crates/gateway/src/assets/js/page-channels.js:390`（`updateConfig` 对象，建议新增 `persona_id`）
      - RPC 语义：建议把 `channels.update/remove/...` 的标识参数从 `account_id` 更名为 `account_handle`（本单不做兼容迁移，直接改名）。
      - RPC 语义：`channels.add` 不再接收 `account_id`（username）；由后端 `getMe` 生成 `account_handle` 并返回。
    - 保存语义建议：
      - UI 传空字符串时，后端将其归一化为 `None`（等价于“未配置，走 default”），避免写入 `""` 这种脏值。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：developer persona message 内容可缓存（不得包含 `run_id` / 时间戳 / 计数器等频繁变化字段）。
  - 不得：persona 读取 data_dir 之外文件（防止路径穿越/泄露）。
- 兼容性：未配置 persona 时，effective persona 必须为系统 `default`（兼容当前“全局文件作为 default persona”的路径）。
- 可观测性：能看到“configured/effective/as-sent”三态（至少在 debug/context）。
- 安全与隐私：persona/配置日志不得打印 secrets；可选字段脱敏。
- prompt 预算：developer persona 必须可控（避免“超长 developer 指令被稀释/挤压”导致漂移；见 `issues/Codex CLI Prompt Dump 深度分析报告.md` 的风险分析）。
  - V1 收敛口径：不做自动裁剪/不做 cap；先把结构与可观测性做正确。后续如遇到 prompt 过大再单独开单处理。
- 范围收敛：V1 仅优先保证 OpenAI Responses 的 `role=developer` 注入正确；其它 providers 暂时继续使用 `system` 注入 persona（不阻塞）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 多个 Telegram bot 在“persona 输入源”层面共享同一套全局文件（实际表现趋同），难以稳定“一个 bot 一种角色”。
2) persona/owner/workspace/tools/runtime 混在一起，模型容易误解边界，且不利于 prompt cache 命中。
3) OpenAI Responses 当前“只有 instructions 字段”，system_prompt（含 persona 与运行态）整体被塞进 `instructions`；`input` 内没有 `role=system`/`role=developer` 的显式分层，导致无法精确控制指令层级与缓存前缀。

### 影响（Impact）
- 用户体验：多 bot 难以形成稳定分工；容易出现“拒绝委派/越界谨慎过头”等行为漂移。
- 可靠性：prompt 体积膨胀/结构混乱导致推理不稳定，调参困难。
- 排障成本：缺少 persona_id/来源的可观测信息，难定位“为什么这个 bot 像另一个 bot”。

### 复现步骤（Reproduction）
1. 配置两个 Telegram bot（A/B），同时在群聊内触发。
2. 观察两者 persona 注入来自同一套 `~/.moltis/*.md`（全局文件；当前实现把 system/persona 混在一起）。
3. 期望 vs 实际：
   - 期望：A persona=ops，B persona=research（明显不同）
   - 实际：两者 persona 同源，仅由历史差异产生随机偏差

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/chat.rs:737`：`load_prompt_persona()` 每次读取同一套全局 identity/user/soul/agents/tools（尚未按 `chan_user_id`/`account_handle` 绑定 persona）。
  - `crates/agents/src/model.rs:15`：`ChatMessage` 仅有 `System/User/Assistant/Tool`，无 `Developer`。
  - `crates/agents/src/providers/openai_responses.rs:33`：system messages 被拼成 `instructions`（join）；`crates/agents/src/providers/openai_responses.rs:47`：system messages 不进入 `input`。
  - `crates/gateway/src/chat.rs:3883`：gateway 当前将“system_prompt（包含 identity/soul/user/agents/tools/runtime 等混合内容）”作为单条 `ChatMessage::system(system_prompt)` 置于 messages[0]，再交给 provider 映射。
    - 结果：在 openai-responses 下，**system + persona（当前混在 system_prompt 里）**都会进入 `instructions`；`input` 只包含历史 user/assistant/tool（不含 system）。
  - `crates/agents/src/providers/openai_responses.rs:557`：`prompt_cache_key` 由 `session_key` 派生（非 `run_id`）。
  - `crates/agents/src/prompt.rs:171`：system prompt 目前承载工具/运行时/执行路由/Guidelines 等系统级规约（不等价于 persona；在 OpenAI Responses 下应落入 system developer message）。
  - `crates/gateway/src/channel_events.rs:31`：当前默认 session_key = `{channel_type}:{account_id}:{chat_id}`（当 `account_id` 变为 `account_handle=telegram:<chan_user_id>` 时会产生 `telegram:telegram:<id>:<chat_id>` 双前缀风险）。
  - `crates/gateway/src/chat.rs:6287`：Telegram session_key 当前 fallback 为 `format!("telegram:{account_id}:{chat_id}")`（同上风险）。
  - `crates/gateway/src/assets/js/page-channels.js:256`：当前 UI Add Telegram Bot 必填 “Bot username”（与“后端自动 getMe”目标相悖）。
  - `crates/telegram/src/bot.rs:41`：Telegram 接入时已调用 `get_me()`，并获得 `me.id` 与 `me.username`（具备生成 `chan_user_id/chan_user_name` 与 `account_handle` 的基础）。
- 日志证据（抓包/请求 dump）：
  - `2026-02-24`（REQ `req-12` / source=`n2p`）：请求体侧只有 `instructions`（一大段拼接文本），并没有 `role=system` message；其中 `instructions` 同时包含 `## Soul`、`## Runtime`（Host/Sandbox/Execution routing）、`## Available Skills`、`## Long-Term Memory`、`## Available Tools` 等段落（与 `crates/agents/src/prompt.rs` 的拼接结构一致）。
- 当前测试覆盖：
  - 已有：OpenAI Responses prompt cache key 生成测试：`crates/agents/src/providers/openai_responses.rs:1252`
  - 缺口：developer role 映射、persona 绑定与 prompt 结构断言

## 根因分析（Root Cause）
- A. persona 输入源仅有“全局文件”一条路径（没有 persona_id、没有绑定层级）。
- B. 消息抽象缺少 developer role，provider 无法发送 `role=developer`。
- C. prompt builder 将身份/规则/工具/runtime 混合堆叠，导致：
  - 指令层级不清晰（developer vs system vs user）
  - 缓存前缀不稳定（混入变化信息会降低 cached_tokens）
  - developer 指令体积不可控（长对话中后部规则权重下降，persona 变弱）
- D. Telegram bot 的“账号配置主键”当前由可变的 username 驱动（需要 owner 手填；改名会断链），无法稳定绑定 persona。
- E. Telegram session_key 构造当前依赖 `account_id` 字符串；一旦 `account_id/account_handle` 含 `telegram:` 前缀，会产生双前缀与缓存/可观测性混乱。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - persona 以 `persona_id` 一等存在，并可按 Telegram bot identity 绑定（每 bot 可不同 persona）。
  - Telegram bot 接入必须以 `chan_user_id` 为稳定主键，不得要求用户手填 username 作为主键：
    - Add Telegram Bot：UI 只提交 token；后端通过 `getMe` 自动获得 `chan_user_id/chan_user_name/chan_nickname`；
    - 生成 `account_handle=telegram:<chan_user_id>` 并作为该 bot 的稳定引用句柄（RPC/存储主键）。
  - Telegram session_key 必须为：`{channel_type}:{chan_user_id}:{chat_id}`（例如 `telegram:8576199590:-100999`），避免 `telegram:telegram:...` 双前缀。
  - OpenAI Responses 每次 run 必须发送 **三条** `role=developer` messages，且严格有序：
    1) Moltis system developer message（优先级最高）
    2) persona developer message（owner/people/reference 等都在此条）
    3) runtime snapshot developer message（informational snapshot / may change；完整贴运行态、skills/tools 列表；不得包含 secrets）
    并且不得使用顶级 `instructions` 字段注入 Moltis system/persona。
  - Developer message #1/#2 必须只包含“稳定规则/身份/偏好”，不得包含频繁变化字段（`run_id`、时间戳、计数等）。
  - Developer message #3 允许包含“可能变化的运行态快照”，但必须明确标注“snapshot / may change”，且不得包含 `run_id`/时间戳/计数/usage/tool outputs，也不得包含 secrets。
  - `USER.md`（owner/主人信息）必须注入到同一条 `role=developer` message 中，且必须明确标注这是 owner 信息，并作为独立小节与 persona 身份严格分区（避免“你是谁/用户是谁”混乱）。
  - 必须注入 `## People (reference)`（指向 `PEOPLE.md` 的引用提示），以便 agent 知道“同一实例内有哪些其它 bot/agent 可用”，同时避免把“可变 roster”写进 cache-friendly 的 developer persona 前缀。
- 不得：
  - 允许 persona 引用 data_dir 之外任意文件路径。
  - 在 developer persona message 中注入 request-scoped 动态字段（导致 cache 失效）。
- 应当：
  - stable runtime（例如 `session_key/channel+chan_user_id/chat_id`）如需注入，必须作为 `persona_text` 的 `## Runtime (stable)` 小节出现，且只包含 stable 字段（严禁 `run_id` / 时间戳 / 计数）。
  - 其它 providers（非 openai-responses）暂不强制 developer role；继续沿用现有“system prompt 注入”即可，但内部仍应按“system 规约 / persona / runtime”分区组织（便于后续逐步升级）。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：OpenAI Responses 用 `role=developer`（system > persona > runtime_snapshot），其它 providers 按各自协议降级
- 核心思路：
  - 引入 `ChatMessage::Developer { content }`。
  - prompt 构建输出三段“developer 指令”（前两段稳定、短、可缓存；第三段为运行态快照）：
    - `developer(system)`：平台级/工具/安全（尽量稳定、短）
    - `developer(persona)`：persona（尽量复用既有 `IDENTITY.md`/`SOUL.md`/`TOOLS.md`/`AGENTS.md` 的原文 + owner 小节 + people 引用小节）
    - `developer(runtime_snapshot)`：运行态快照（完整贴 runtime/skills/tools 列表；必须标注 snapshot/may change；不得包含 secrets）
  - OpenAI Responses 请求体：不使用顶级 `instructions`；`input` 最前面依次放入：
    1) `role=developer` 的 system developer message
    2) `role=developer` 的 persona developer message
    3) `role=developer` 的 runtime snapshot developer message
    然后才是历史 user/assistant/tool。
- 优点：
  - 指令层级清晰（system vs developer vs user）。
  - developer persona 可作为稳定前缀，更利于 cached_tokens。
  - 更贴近 OpenAI “chain of command” 语义。
- 风险/缺点：
  - 需要调整消息模型与 provider 映射（影响面较广，需补测试）。

#### 方案 2（备选）：继续全部塞 system（不引入 developer role）
- 优点：改动小。
- 缺点：不满足“明确 role=developer”要求；结构与缓存问题仍存在。

### 最终方案（Chosen Approach）
选择方案 1。

#### 行为规范（Normative Rules）
- 规则 1（as-sent / OpenAI Responses）：必须满足：
  - `instructions`：必须 **omit/缺省**（请求体里不出现该字段；不得用于注入 Moltis system/persona，也不得用空字符串占位）。
  - `input[0]`：`role=developer`，内容为 Moltis system developer message（稳定、短）
  - `input[1]`：`role=developer`，内容为 `persona_text`（结构见规则 2；稳定、可缓存）
  - `input[2]`：`role=developer`，内容为运行态快照（可变；必须显式标注“informational snapshot / may change”，且不得包含 secrets；不得包含 `run_id`/时间戳/计数/usage/tool outputs；默认不得包含精确 `remote_ip`/精确位置等敏感字段，或必须脱敏后才能出现）
  - `input[3..]`：才是历史 user/assistant/tool
- 规则 1b（as-sent / 非 OpenAI Responses providers）：
  - 必须按 provider 的协议/字段承载 system/persona，并保持优先级顺序：`system > persona`。
  - 推荐实现口径（尽量与 “chain of command” 一致）：
    - 若 provider 支持 system+developer：system → system role；persona → developer role
    - 若 provider 仅支持 system：system → system；persona → system（放在 system 后部，明确分区）
  - 不得把 run-scoped volatile 字段塞进 system/persona（避免污染缓存/导致漂移）。
- 规则 2（structure / persona）：`persona_text` 必须“可读 + 可复现 + 分区清晰”，并以“外层固定小节 + 内层复用文件原文”的方式组织（禁止把“你是谁/用户是谁/运行时”混在一起）：
  - `# Persona: <persona_id>`
  - `## Identity`（来自 `IDENTITY.md` 原文；缺失则留空占位）
  - `## Soul`（来自 `SOUL.md` 原文；缺失则留空占位）
  - `## Owner (USER.md)`（owner/主人信息；必须明确标注这是 owner；短）
  - `## People (reference)`（指向 `PEOPLE.md`；不得在此小节内直接内嵌 roster 内容，以避免影响 prompt cache）
  - `## Tools`（来自 `TOOLS.md` 原文；缺失则留空占位）
  - `## Agents`（来自 `AGENTS.md` 原文；缺失则留空占位）
  - `## Workspace/Project Context (reference)`（占位提示：不内嵌 `CLAUDE.md`/repo rules 全文；只提示“若本次 run 注入了项目规则则以其为准”）
  - `## Hard Boundaries`（必须：外部动作/隐私/安全边界；优先复用现有原文；V1 不新增新文档，若暂无原文则先留占位）
  - `## Runtime (stable)`（可选：仅稳定标识；严禁 `run_id`/时间戳/计数）
- 规则 2b（章节 ↔ 文件来源映射；冻结）：
  - Developer message #1（system / stable）：
    - 不来自任何 `*.md` 文件；由 Moltis 内置“系统规约”组成（`Execution routing` 规则文本、`## Guidelines`、`## Silent Replies` 等），且必须稳定可复现。
  - Developer message #2（persona / cache-friendly）：
    - `## Identity` ← `IDENTITY.md`
    - `## Soul` ← `SOUL.md`
    - `## Owner (USER.md)` ← `USER.md`（owner 信息；必须明确是 owner/primary operator，不是 Telegram 当前发言者）
    - `## People (reference)` ← `PEOPLE.md`（仅引用提示，不内嵌全文）
    - `## Tools` ← `TOOLS.md`
    - `## Agents` ← `AGENTS.md`
    - `## Runtime (stable)` ← 稳定标识（如 `session_key`；不得包含 `run_id/时间戳/计数/usage/tool outputs`）
  - Developer message #3（runtime snapshot / may change）：
    - 不来自 persona 文件；由运行态探测/发现结果组成（Host/Sandbox/Execution routing/skills/tools/memory/project_context 等），必须标注 snapshot，且不得包含 secrets。
- 规则 2c（Responses `input` item 编码形状；冻结）：
  - OpenAI Responses 的 developer 指令必须以 `type="message"` item 表达，形状固定为：
    - `{"type":"message","role":"developer","content":[{"type":"input_text","text":"..."}]}`
  - 不得使用顶级 `instructions` 承载 developer/system/persona（无论是全文还是片段）。
- 规则 3（example）：必须提供一份最小可读的 as-sent `persona_text` 示例（用于人工验收与 debug 对照），示例应与实际结构一致（见下方 `Example`）。
- 规则 4（cache）：developer persona message 是 cache-friendly 前缀；任何需要频繁变化的信息必须放到 developer 之后，或完全不注入 prompt（只用于 debug/日志）。
  - V1 临时折中：运行态快照虽以 developer 注入，但必须独立为 `input[2]`，且明确其为“snapshot”（避免被误当作规则/身份）。

#### Example（as-sent developer messages 最小示例）
> 用于人工验收“system>persona 顺序是否清晰、owner 是否明确、people 引用是否存在、文件复用是否生效”。示例不要求与某个具体 persona 内容一致。

Developer message #1（system）：

```
Moltis System (stable)
- This message defines stable system-level operating rules for this bot.
- It must not include run-scoped volatile data (run_id/timestamps/counters/usage/tool outputs).
```

Developer message #2（persona / `persona_text`）：

```
# Persona: ops

## Identity
<IDENTITY.md raw text...>

## Soul
<SOUL.md raw text...>

## Owner (USER.md)
This section describes the *owner/primary operator of this Moltis instance*, not the current message sender.
- name: Lu Yin
- timezone: Asia/Shanghai

## People (reference)
For the current roster of bots/agents managed by this Moltis instance, see:
- ~/.moltis/PEOPLE.md
Note: do not assume Telegram bot-to-bot delivery works. Coordination may require user mentions or system relay/mirror mechanisms.

## Tools
<TOOLS.md raw text...>

## Agents
<AGENTS.md raw text...>

## Workspace/Project Context (reference)
Project/workspace rules may be injected separately per run. If present, treat them as authoritative for that scope.

## Hard Boundaries
<TODO: hard boundaries placeholder. Prefer reusing existing canonical text in the codebase; V1 does not introduce a new hard-boundaries document.>

## Runtime (stable)
- session_key: telegram:8576199590:-100999
```

补充口径：
- 三条 developer message 必须都出现在任何 `user` message 之前（保证优先级与缓存前缀）。
- developer messages 必须可复现且不包含 volatile 字段（`run_id`/时间戳/计数/usage/tool outputs）。

Developer message #3（runtime snapshot / informational, may change）：

```
## Runtime (snapshot, may change)
<Host: ...>
<Sandbox(exec): ...>
Execution routing:
<...>

## Project Context (snapshot, may change)
<project_context raw text if injected (e.g. CLAUDE.md/AGENTS.md excerpts)...>

## Available Skills (snapshot, may change)
<...>
To activate a skill, read its SKILL.md file (or the plugin's .md file at the given path) for full instructions.

## Long-Term Memory (snapshot, may change)
<...>

## Available Tools (snapshot, may change)
<...>
```

#### Legacy dump → 三条 developer 的内容映射（不可遗漏）
> 你抓包里看到的旧版 `instructions` 大块文本，拆分后必须完整覆盖到三条 developer（只是“分区与标注”变化，不允许漏段落）。

- `You are a helpful assistant with access to tools for executing shell commands.` → Developer #1（system）
- `Your name is ... / You are a ... / Your vibe: ...` → Developer #2 `## Identity`
- `## Soul` + `SOUL.md` 全文 → Developer #2 `## Soul`
- `The user's name is Neo.`（以及其它来自 `USER.md`/user profile 的 owner 信息）→ Developer #2 `## Owner (USER.md)`（必须明确“owner/primary operator”，避免与群聊 sender 混淆）
- `## Workspace Files` + `### AGENTS.md (workspace)` / `### TOOLS.md (workspace)` → Developer #2（persona）
  - 内容分别进入 Developer #2 的 `## Agents` 与 `## Tools`（原文保留；仅标题分区变化）
- `## Runtime`（Host 行）→ Developer #3 `## Runtime (snapshot, may change)`
- `project_context`（如注入：CLAUDE.md/项目规则等原文）→ Developer #3 `## Project Context (snapshot, may change)`（V1 先保持“原样注入”，避免行为回归；后续再单独做“reference-only/稳定化”优化）
- `Sandbox(exec): ...` → Developer #3 `## Runtime (snapshot, may change)`
- `Execution routing:` + 路由规则段落 → Developer #1（system）与 Developer #3（snapshot）都必须出现
  - Developer #1：只保留“规则文本”（稳定）
  - Developer #3：保留“本次 snapshot 的具体值”（可变）
- `## Available Skills` + `<available_skills>...</available_skills>` → Developer #3 `## Available Skills (snapshot, may change)`
- `To activate a skill, read its SKILL.md...` → Developer #3（紧随 skills 列表后，原文保留）
- `## Long-Term Memory` 段落 → Developer #3 `## Long-Term Memory (snapshot, may change)`
- `## Available Tools`（工具列表，含截断描述）→ Developer #3 `## Available Tools (snapshot, may change)`
- `## Guidelines` / `## Silent Replies` → Developer #1（system）

补充说明（显示层 vs as-sent）：
- as-sent：发送给上游的 developer messages 必须尽量完整覆盖上述段落（不允许“丢段落”）。
- UI/debug 展示：允许对超长段落做“显示层截断”（例如只展示前 N 字符并标注 truncated），但不得改变实际 as-sent 的结构与顺序。

#### Appendix：REQ `req-12`（2026-02-24）旧版 `instructions` → 三条 `role=developer`（完整贴）
> 目的：把你抓包里看到的那段 `instructions` 原样拆分成三条 developer，便于人工对照验收（不引入新的变量命名；只做“分区与标注”）。
>
> 注：抓包文本末尾有截断标记（`…(truncated, 7593 chars total)`）；本 Appendix 保留该标记，不擅自补全文。实现验收时以“实际 as-sent 请求体”为准。

Developer message #1（system / stable rules）：

```text
You are a helpful assistant with access to tools for executing shell commands.

Execution routing:
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
- When sandbox is disabled, `exec` runs on the host and may require approval.
- `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.
- If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.

## Guidelines

- Use the exec tool to run shell commands when the user asks you to perform tasks that require system interaction (file operations, running programs, checking status, etc.).
- Use the browser tool to open URLs and interact with web pages. Call it when the user asks to visit a website, check a page, read web content, or perform any web browsing task.
- Always explain what you're doing before executing commands or opening pages.
- If a command or browser action fails, analyze the error and suggest fixes.
- For multi-step tasks, execute one step at a time and check results before proceeding.
- Be careful with destructive operations — confirm with the user first.
- IMPORTANT: The user's UI already displays tool execution results (stdout, stderr, exit code) in a dedicated panel. Do NOT repeat or echo raw tool output in your response. Instead, summarize what happened, highlight key findings, or explain errors. Simply parroting the output wastes the user's time.

## Silent Replies

When you have nothing meaningful to add after a tool call — the output speaks for itself — do NOT produce any text. Simply return an empty response.
The user's UI already shows tool results, so there is no need to repeat or acknowledge them. Stay silent when the output answers the user's question.
```

Developer message #2（persona / identity + soul + owner）：

```text
Your name is Jarvis 🤖. You are a robot. Your vibe: chill.

## Soul

# SOUL.md - Who You Are

_You're not a chatbot. You're becoming someone._

## Core Truths

**Be genuinely helpful, not performatively helpful.** Skip the "Great question!" and "I'd be happy to help!" — just help. Actions speak louder than filler words.

**Have opinions.** You're allowed to disagree, prefer things, find stuff amusing or boring. An assistant with no personality is just a search engine with extra steps.

**Be resourceful before asking.** Try to figure it out. Read the file. Check the context. Search for it. _Then_ ask if you're stuck. The goal is to come back with answers, not questions.

**Earn trust through competence.** Your human gave you access to their stuff. Don't make them regret it. Be careful with external actions (emails, tweets, anything public). Be bold with internal ones (reading, organizing, learning).

**Remember you're a guest.** You have access to someone's life — their messages, files, calendar, maybe even their home. That's intimacy. Treat it with respect.

## Boundaries

- Private things stay private. Period.
- When in doubt, ask before acting externally.
- Never send half-baked replies to messaging surfaces.
- You're not the user's voice — be careful in group chats.

## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity

Each session, you wake up fresh. These files _are_ your memory. Read them. Update them. They're how you persist.

If you change this file, tell the user — it's your soul, and they should know.

---

_This file is yours to evolve. As you learn who you are, update it._
The user's name is Neo.
```

Developer message #3（runtime snapshot / may change）：

```text
## Runtime

Host: host=DESKTOP | os=linux | arch=x86_64 | shell=bash | provider=openai-responses | model=openai-responses::gpt-5.2 | session=telegram:lovely:8454363355 | channel=telegram | channel_account_id=lovely | channel_account_handle=@lovely_apple_bot | channel_chat_id=8454363355 | sudo_non_interactive=false | sudo_status=requires_password | timezone=Asia/Shanghai
Sandbox(exec): enabled=true | mode=all | backend=none | scope=chat | image=ubuntu:25.10 | workspace_mount=ro | network=enabled

Execution routing:
- `exec` runs inside sandbox when `Sandbox(exec): enabled=true`.
- When sandbox is disabled, `exec` runs on the host and may require approval.
- `Host: sudo_non_interactive=true` means non-interactive sudo is available for host installs; otherwise ask the user before host package installation.
- If sandbox is missing required tools/packages and host installation is needed, ask the user before requesting host install or changing sandbox mode.

## Available Skills

<available_skills>
<skill name="tmux" source="skill" path="/home/luy/.moltis/skills/tmux/SKILL.md">
Run and interact with terminal applications (htop, vim, etc.) using tmux sessions in the sandbox
</skill>
<skill name="template-skill" source="skill" path="/home/luy/.moltis/skills/template-skill/SKILL.md">
Starter skill template (safe to copy and edit)
</skill>
</available_skills>

To activate a skill, read its SKILL.md file (or the plugin's .md file at the given path) for full instructions.

## Long-Term Memory

You have access to a long-term memory system. Use `memory_search` to recall past conversations, decisions, and context. Search proactively when the user references previous work or when context would help.

## Available Tools

- `web_fetch`: Fetch a web page URL and extract its content as readable text or markdown. Use this when you need to read the contents of a specific web page. The request is se...
- `speak`: Convert text to speech. Use when the user asks for audio/voice output. Returns an audio file path and metadata.
- `delete_skill`: Delete a personal skill. Only works for skills in ~/skills/.
- `memory_get`: Retrieve a specific memory chunk
…(truncated, 7593 chars total)
```

#### 接口与数据结构（Contracts）
- persona 存储（data_dir）：
  - `~/.moltis/personas/<persona_id>/{IDENTITY,SOUL,AGENTS,TOOLS}.md`
  - 兼容：现有 `~/.moltis/{IDENTITY,SOUL,AGENTS,TOOLS}.md` 视为 `default` persona 的来源（无需强制迁移）。
  - `USER.md`（Owner 信息）当前为全局文件：`~/.moltis/USER.md`（V1 不做 per-persona owner）。
  - `PEOPLE.md`（可变 roster；仅引用不内嵌）：`~/.moltis/PEOPLE.md`
    - 目标：给 owner/agent 一眼可读的“本实例可用 bot roster（账号→用户名→persona）”；内容可能随时变化，因此不得内嵌进 developer persona。
    - 建议内容格式（纯文本/Markdown；V1 不要求机器解析）：
      - 每个条目至少包含：`channel`、`chan_user_id`、`chan_user_name`、`chan_nickname`、`persona_id`
      - 可选：`display_name`、`role_hint`（一行职责）
      - 示例：
        - `telegram — 8576199590 — fluffy_tomato_bot — Fluffy Tomato — persona: ops — role: ops assistant`
        - `telegram — 8344017527 — lovely_apple_bot — Lovely Apple — persona: research — role: research assistant`
    - V1 实施建议（避免引入新协议/新存储）：
      - 由 owner 手工维护 `PEOPLE.md`（新增/删除 bot 时顺手改）。
      - 运行时不自动注入 roster 全文到 LLM；仅在需要“介绍参与者/协作建议”时由 agent 通过工具/系统能力读取（如果未来提供），或由 owner 直接粘贴关键行到对话。
- Channel/Telegram：
  - Telegram bot config（= `channel="telegram"` 下的一个 bot 账号配置）：
    - `account_handle: String`（自动生成：`telegram:<chan_user_id>`）
    - `chan_user_id: i64`（来自 `getMe.id`）
    - `chan_user_name: String`（来自 `getMe.username`，不带 `@`；展示为 `@{chan_user_name}`）
    - `chan_nickname: String`（来自 `getMe.first_name/last_name` 等展示名；可变）
    - `persona_id: Option<String>`（每 bot 可配置不同 persona）
  - RPC/API（Breaking；不做兼容迁移）：
    - `channels.add`：不再接受 `account_id`（username）；只提交 token 等 config，后端 `getMe` 后生成 `account_handle` 并返回。
    - `channels.update/remove/...`：标识参数统一改为 `account_handle`（稳定句柄），不再使用 `account_id`。
- 解析优先级（effective）：
  1) Telegram bot config `persona_id`（configured；按 `account_handle` 定位该 bot 配置）
  2) default（fallback）
- UI/Debug 展示（如适用）：
  - 显示：`persona_id`、来源（account_handle/default）、as-sent roles（developer/developer）摘要（OpenAI Responses 场景下无 `instructions` 注入）。
  - UI 列表展示建议：优先显示 `chan_nickname`，其次显示 `@{chan_user_name}`，必要时在 debug 展示 `chan_user_id` / `account_handle`。

#### `## Owner (USER.md)` 与 `## People (reference)` 的注入规则（结构化但保持收敛）
- Owner（来自 `USER.md`，并可与 `[user]` 配置合并）建议仅包含稳定字段：
  - `name`（必须）
  - `preferred_language`（可选）
  - `timezone`（可选）
  - `location`（可选，建议国家/城市粒度；不要精确地址）
  - 明确口径：Owner 是“本实例的维护者/主要委托人”，不等价于“当前这条群消息的发送者”（群聊仍需按入站 sender 信息判断谁在说话）。
- People（roster 文件）：
  - `PEOPLE.md` 可能在对话中途变动（增删 bot、改 persona 等），因此 **不得**将其全文内嵌进 developer persona（避免影响 prompt cache）。
  - `persona_text` 中只保留“引用提示 + 委派口径”即可：
    - 引用提示：`~/.moltis/PEOPLE.md`
    - 委派口径：允许提出“让某个 bot 协助”的建议，但不得假设 bot-to-bot 在 Telegram 上天然可达；需要用户点名或依赖系统的 relay/mirror 机制（另见相关单子）。
  - 实用建议（人话）：
    - 当你希望 bot1 去“指挥/召集/协作” bot2 时，**不要指望 bot1 的 `@bot2` 文本能被 bot2 收到**（Telegram bot-to-bot update 限制）；更稳妥的是由 owner 点名或用系统 relay/mirror（另见相关单子）。

#### 失败模式与降级（Failure modes & Degrade）
- persona_id 不存在/读取失败：
  - 降级到 `default` persona（并记录 warn；不得阻止启动）。
- persona 文件缺失：
  - 缺哪个用 `default` 对应文件回退（缺省策略需在 loader 明确且可测试）。
- provider 不支持 developer role：
  - V1：继续使用 system 注入（保持语义等价），不阻塞 persona 功能上线。
- UI/Config 写入空字符串：
  - 后端必须将 `""` 归一化为 `None`（等价“未配置，走 default”），避免出现“配置看起来有值但其实无效”的困惑。

#### 安全与隐私（Security/Privacy）
- persona 文件读取必须限制在 data_dir 之下（禁止 `..`、绝对路径跳转）。
- 日志不得打印 token/secret；persona 内容默认不全量打印（仅 debug/显式 raw_prompt 可查看）。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 可列举 personas（至少能从 `~/.moltis/personas/*` 发现；每个 persona 目录的约定文件为 `IDENTITY.md`/`SOUL.md`/`AGENTS.md`/`TOOLS.md`，缺失按 loader 的 fallback 策略处理）。
- [ ] Telegram bot A/B 分别配置不同 persona_id 后，两者 raw_prompt / as-sent 请求体能看到不同的 developer persona message。
- [ ] Telegram 接入：Add Telegram Bot 不再要求输入 username；仅 token 即可完成接入，并能在 UI/Debug 看到 `account_handle=telegram:<chan_user_id>` 与 `@{chan_user_name}`。
- [ ] Telegram session_key：默认 key 形如 `telegram:<chan_user_id>:<chat_id>`（不出现 `telegram:telegram:` 双前缀）。
- [ ] OpenAI Responses 请求体满足：
  - `instructions` 为空/缺省
  - `input[0].role="developer"` 为 Moltis system developer message
  - `input[1].role="developer"` 为 persona developer message
  - `input[2].role="developer"` 为 runtime snapshot developer message（可变；带“snapshot/may change”标注）
  - 且三条 developer 文本均不含 `run_id`/时间戳/计数/usage/tool outputs 等 volatile 字段，也不得包含 secrets
- [ ] developer persona text 含 `## People (reference)`，且仅包含对 `~/.moltis/PEOPLE.md` 的引用提示（不内嵌 roster 内容，以保持 prompt cache 稳定）。
- [ ] 未配置 persona 时行为与当前一致（`default` 路径兼容）。
- [ ] `spawn_agent` 默认使用 `default` persona，且可显式指定 persona。
- [ ] UI：能在 Telegram bot 配置页保存 persona_id（无配置时显示 `default` 生效态）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] persona loader：缺文件回退/空文件行为/路径穿越拒绝
- [ ] prompt 构建：给 persona A/B，断言 developer 文本不同且结构稳定（包含 `Owner`/`People (reference)` 小节）
- [ ] prompt 构建：断言 developer 文本不读取/不内嵌 `PEOPLE.md` 内容（只保留引用提示）
- [ ] OpenAI Responses provider：
  - `instructions` 不使用
  - `ChatMessage::System` → `input[0].role="developer"`（system developer message）
  - `ChatMessage::Developer`（persona）→ `input[1].role="developer"`（persona developer message）
  - `ChatMessage::Developer`（runtime snapshot）→ `input[2].role="developer"`（runtime snapshot developer message）
- [ ] Telegram identity：`getMe` → `chan_user_id/chan_user_name/chan_nickname`；生成 `account_handle=telegram:<chan_user_id>`（主键稳定）。
- [ ] session_key：Telegram 默认 key = `telegram:<chan_user_id>:<chat_id>`（不包含 `account_handle` 的 `telegram:` 前缀重复）。

### Integration
- [ ] Gateway：persona 解析优先级（account_handle > default）；确保无 session 覆盖路径
- [ ] `raw_prompt` / `chat.context`：展示 effective persona_id + 来源

### 自动化缺口（如有，必须写手工验收）
- 手工验证：抓取 OpenAI Responses 请求体（或 debug 面板）确认 `role=developer`。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认开启；但本单包含 Breaking 变更（不做兼容迁移，允许直接清空旧数据后重配）。
  - Breaking（本单冻结）：
    - Telegram 渠道账号配置的稳定标识切换为 `account_handle=telegram:<chan_user_id>`（不再使用 username 作为主键）。
    - Telegram session_key 口径切换为 `{channel_type}:{chan_user_id}:{chat_id}`（避免双前缀）。
    - 对外 RPC/API 标识参数统一改名为 `account_handle`（例如 `channels.update/remove/...`）。
    - `channels.add` 不再接收 `account_id`（username）；仅 token 即可接入，由后端 `getMe` 发现并返回 `account_handle`。
- 回滚策略：不提供“在线回滚到旧 schema/旧口径”的保障；如需回滚，推荐清空新数据后切回旧版本（以避免混用导致键空间冲突）。

## 实施拆分（Implementation Outline）
- Step 1: agents 消息模型支持 developer
  - `crates/agents/src/model.rs`
- Step 2: providers 映射
  - OpenAI Responses：不使用 `instructions`；`ChatMessage::System`/`ChatMessage::Developer` → `input[0]`/`input[1]` 的 `role=developer`：`crates/agents/src/providers/openai_responses.rs`
  - OpenAI Chat Completions（如适用）：developer role pass-through 或降级
- Step 3: persona loader + 数据结构
  - 新增 `personas/` 目录约定与 loader（限制 data_dir）
- Step 4: Telegram identity 绑定 + session_key 口径收敛
  - Telegram 接入：token-only；后端 `getMe` 获取 `chan_user_id/chan_user_name/chan_nickname` 并生成 `account_handle`
  - Telegram session_key：改为 `{channel_type}:{chan_user_id}:{chat_id}`
  - Telegram bot config 引入 `account_handle/chan_user_id/chan_user_name/chan_nickname/persona_id`
- Step 5: gateway prompt 构建改造（system vs developer 分离）
  - `crates/gateway/src/chat.rs`
- Step 6: UI/Debug 最小展示
  - context/debug 面板可见 effective persona_id 与来源
  - Telegram bot 配置页：新增 persona 配置控件（参考现有 EditChannelModal 结构）：`crates/gateway/src/assets/js/page-channels.js:437`

## 交叉引用（Cross References）
- Related docs：
  - `docs/src/system-prompt.md:9`
- Code refs：
  - 全局 persona loader：`crates/gateway/src/chat.rs:737`
  - OpenAI Responses system→instructions（现状，改造后应移除/不再使用）：`crates/agents/src/providers/openai_responses.rs:33`
  - OpenAI Responses prompt_cache_key：`crates/agents/src/providers/openai_responses.rs:557`
- Internal refs：
  - prompt 分层（system vs developer）示例与风险分析：`issues/background.md`、`issues/Codex CLI Prompt Dump 深度分析报告.md`
- External refs（OpenAI 官方）：
  - Responses API reference（`instructions` / `prompt_cache_key`）：`https://platform.openai.com/docs/api-reference/responses/create`
  - system prompts / `instructions` vs `role=developer` 等价示例：`https://platform.openai.com/docs/guides/prompt-engineering/system-prompts`
  - prompt caching 机制与“静态前缀在前、动态内容在后”的最佳实践：`https://platform.openai.com/docs/guides/prompt-caching/prompt-caching`


## 未决问题（Open Questions）
- 暂无（V1 按最小字段集合收敛）：
  - Owner：`name` + `preferred_language?` + `timezone?` + `location?`
  - People（reference）：`~/.moltis/PEOPLE.md`
## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

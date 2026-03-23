# Issue: V3 Telegram 群聊 speaker 与 link identity one-cut（TG-GST v1 / display_name）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-23
- Owners: TBD
- Components: telegram/gateway/ui/docs
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-20：TG-GST v1 文本包装已存在，群聊最终文本在 TG 侧生成：`crates/telegram/src/handlers.rs:3485`、`crates/telegram/src/adapter.rs:211`
- 2026-03-20：PEOPLE 配置与设置页已经具备 Telegram identity 相关字段：`crates/gateway/src/people.rs:183`、`crates/gateway/src/assets/js/page-settings.js:723`
- 2026-03-22：群聊 speaker / `(bot)` / `-> you` / link identity 规则已在设计文档里冻结：`docs/src/refactor/telegram-record-dispatch-boundary.md:150`
- 2026-03-23：实施计划已二次收敛，明确本单只做“Telegram 专属 identity 快照 + TG adapter speaker_match/render 闭环”，不扩成通用 identity 框架改造。
- 2026-03-23：gateway 已把 PEOPLE Telegram 字段提炼成 Telegram 专属只读 identity 快照，并在启动、`workspace.people.updateEntry`、`workspace.people.sync` 三条现有路径刷新到 Telegram runtime：`crates/gateway/src/people.rs:139`、`crates/gateway/src/methods.rs:49`、`crates/gateway/src/server.rs:1826`、`crates/gateway/src/channel.rs:626`、`crates/gateway/src/services.rs:389`。
- 2026-03-23：TG adapter 已统一收口 `speaker_match` + `speaker_render`，handlers 与 outbound 共用同一 `tg_gst_v1_render_text()`，并补齐降级可观测日志：`crates/telegram/src/adapter.rs:392`、`crates/telegram/src/handlers.rs:3453`、`crates/telegram/src/outbound.rs:858`。
- 2026-03-23：本地受管 bot 真值与 PEOPLE link identity 已进入同一 Telegram runtime 快照：`crates/telegram/src/config.rs`、`crates/telegram/src/plugin.rs:113`、`crates/telegram/src/state.rs`。

**已覆盖测试（如有）**
- TG-GST v1 群聊转写已有基本测试：`crates/telegram/src/handlers.rs:5463`
- `tg_gst_v1_format_inbound_text()` 已被群聊路径实际使用：`crates/telegram/src/handlers.rs:3491`
- TG speaker 渲染单测已覆盖 link display_name、managed bot `chan_nickname`、human display_name、technical short-id：`crates/telegram/src/adapter.rs:1316`、`crates/telegram/src/adapter.rs:1346`、`crates/telegram/src/adapter.rs:1370`、`crates/telegram/src/adapter.rs:1395`
- handlers / outbound 已覆盖 identity 快照参与群聊入站与 group-visible 文本渲染：`crates/telegram/src/handlers.rs:5605`、`crates/telegram/src/outbound.rs:3019`
- gateway 已覆盖 PEOPLE -> Telegram identity link 提取：`crates/gateway/src/people.rs:405`
- 2026-03-23：已执行 `cargo test -p moltis-telegram`、`cargo test -p moltis-gateway`、`cargo check -p moltis --bin moltis` 全绿。

**已知差异/后续优化（非阻塞）**
- 本单不重做 Telegram 设置页整体产品形态，只补齐 link identity 字段的真正消费与必要文案说明。
- session label、debug label 已纳入本单按同口径一并收口，但不应扩成新的命名体系改造项目。

---

## 背景（Background）
- 场景：Telegram 群聊正文必须在 TG adapter 内一次性生成最终 TG-GST v1 文本，其中 speaker、本体名显示、`(bot)`、`-> you` 都在这一层冻结，不再回流给 gateway/core 二次修补。
- 约束：
  - 群聊正文必须继续由 TG adapter 负责生成，gateway/core 不得再二次改写 speaker、`(bot)`、`-> you`。
  - 严格遵守 v3 one-cut，不保留“先按旧 speaker 输出，再在 gateway 偷偷 rewrite”的兼容尾巴。
  - 不允许内部 key（如 `telegram:1234567890`、`session_key`）进入 TG-GST v1 正文。
- Out of scope：
  - 不改 DM 输入文本协议。
  - 不引入新的 identity 存储格式；优先复用现有 PEOPLE 配置字段。
  - 不重做 Telegram account key / session label 的持久化主键语义。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`speaker_match`**（主称呼）：TG adapter 判定“这个 Telegram 发言者对应哪个逻辑身份”的过程。
  - Why：显示给模型看的名字，必须先建立在稳定匹配上。
  - Not：不是最终输出给模型看的文本。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：identity resolution

- **`speaker_render`**（主称呼）：TG adapter 把已识别的发言者渲染成 TG-GST v1 头部 speaker 的过程。
  - Why：群聊上下文可读性与本体名优先显示，取决于这一步。
  - Not：不是系统内部 account key 或路由主键。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：speaker label rendering

- **`link_identity`**（主称呼）：由 PEOPLE 配置以及本地受管 Telegram bot 账户身份共同组成的“逻辑身份 <-> Telegram 账号标识”映射。
  - Why：它是“优先显示本体名 `display_name`”的基础。
  - Not：不是 Telegram 原生 username，也不是内部 account key；本单也**不新增**独立于现有 PEOPLE 字段之外的新 identity schema。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：people link / identity link

- **`managed_bot_identity`**（主称呼）：来自本地 Telegram bot 账户配置与 agent 绑定关系的身份真值。
  - Why：本地 bot speaker 不能只靠 PEOPLE 猜；需要消费 `agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname` 这类账户真值。
  - Not：不是额外的新 identity schema，也不是群聊正文里可直接暴露的内部账户字段。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：local bot identity

- **`display_name`**（主称呼）：本体的人类可读名称。
  - Why：一旦命中 link identity，应优先显示给模型看。
  - Not：不是 Telegram 内部 ID，也不是系统路由 key。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：identity display

- **`telegram_user_id`**（主称呼）：Telegram 稳定用户 ID。
  - Why：它应是 TG speaker 匹配的第一优先级。
  - Not：不应直接原样泄露到群聊 speaker。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：chanUserId

- **authoritative**：来自 Telegram 事件本身或 bot 配置/PEOPLE 已冻结的真实值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给下游执行器的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] TG adapter 真正消费现有 PEOPLE 中的 Telegram identity 字段，以及本地 Telegram bot 账户配置中的 `agent_id` / `chan_user_id` / `chan_user_name` / `chan_nickname`，用于群聊 speaker 匹配与渲染。
- [x] 群聊 TG-GST v1 speaker 渲染优先显示本体 `display_name`，而不是 `tg:<id>` 或 `telegram:...`。
- [x] 冻结并实现群聊 speaker 规则：
  - 匹配优先级：本地受管 bot 使用 `agent_id` / `chan_user_id` / `chan_user_name`，其他 Telegram 发言者使用 `telegram_user_id` > `telegram_user_name`
  - 渲染优先级：`display_name` > `telegram_display_name` > Telegram `sender_name` > `username` > `tg-user/tg-bot-<short_id>`
- [x] 群聊 `(bot)` 与 `-> you` 规则全部收口在 TG adapter；DM 不适用 `(bot)` 规则。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：一旦命中 link identity，群聊 speaker 必须优先显示本体 `display_name`。
  - 必须：speaker 匹配和 speaker 渲染严格拆成两步，不得混用“显示名即匹配主键”的脏逻辑。
  - 不得：把 `telegram:123...`、`session_key`、`bucket_key`、binding blob 写进 TG-GST v1 speaker。
  - 不得：在 gateway/core 侧再做一层“补丁式 rewrite”来修 speaker。
- 兼容性：
  - 本单复用现有 PEOPLE 字段，不引入新的 legacy alias。
  - 未命中 link identity 时允许按冻结好的回退顺序渲染，但这属于正式规则，不属于 fallback 尾巴。
- 可观测性：
  - 仅在 speaker 匹配失败、碰撞或降级到技术兜底名时记录结构化日志。
  - 日志至少包含 `event`、`reason_code`、`decision`、`policy`，可补 `match_method`、`sender_id_hash`。
- 安全与隐私：
  - 不打印完整正文。
  - 若记录 sender 标识，优先记录 hash 或短 ID，不打印完整内部 binding。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) TG-GST v1 已经存在，但当前 speaker 仍以 `username` 或 `tg:<id>(<display>)` 为主，导致模型看到的是渠道脏名而不是本体名。
2) PEOPLE 里已经有 `telegram_user_id`、`telegram_user_name`、`telegram_display_name` 字段与设置页入口，但它们没有真正进入 TG adapter 的群聊转写链路。
3) 本地受管 Telegram bot 的账户真值（`agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname`）也尚未被纳入统一 speaker 真值来源。
4) `(bot)` 与 `-> you` 虽已有部分实现，但口径仍不完整，尚未与 link identity、渲染优先级一起冻结成单一实现源。

### 影响（Impact）
- 用户体验：
  - 群聊上下文仍会出现“几个字母 + 一串数字”的 speaker，模型可读性差。
  - 同一个 bot 若有多个 Telegram 账号，模型无法稳定看到统一的本体名。
- 可靠性：
  - speaker 匹配和渲染未分层，后续实现容易把 `display_name`、`username`、内部 key 混用。
- 排障成本：
  - 当前要同时看 PEOPLE、TG adapter、handlers 和 gateway label helper，边界不清。

### 复现步骤（Reproduction）
1. 配置 PEOPLE 条目，填写 `display_name`、`telegram_user_id`、`telegram_user_name`、`telegram_display_name`。
2. 在群聊中让对应 Telegram 账号发言。
3. 期望 vs 实际：
   - 期望：TG-GST v1 speaker 优先显示本体 `display_name`，例如 `风险助手(bot): ...`。
   - 实际：当前常见输出仍是 `risk_bot_cn(bot): ...` 或 `tg:1234567890(风险助手中文)(bot): ...`。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/telegram/src/adapter.rs:175`：`tg_gst_v1_format_speaker()` 当前优先使用 `username`，否则落到 `tg:<id>(<display>)`。
  - `crates/telegram/src/adapter.rs:205`：`(bot)` 目前只是基于 `sender_is_bot` 追加。
  - `crates/telegram/src/adapter.rs:211`：TG-GST v1 最终文本由 `tg_gst_v1_format_inbound_text()` 生成。
  - `crates/telegram/src/handlers.rs:3491`：群聊入站实际调用上述格式化函数。
  - `crates/gateway/src/people.rs:183`：现有 PEOPLE 已支持 `telegram_user_id`、`telegram_user_name`、`telegram_display_name` 字段。
  - `crates/gateway/src/assets/js/page-settings.js:785`：设置页已经暴露对应字段，但当前未形成 TG speaker 的闭环消费。
  - `crates/telegram/src/config.rs:135`：本地 Telegram bot 账户配置还持有 `agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname` 真值。
  - `crates/telegram/src/plugin.rs:106`：当前 account snapshot 仅向外围暴露 `chan_user_name`，不足以支撑本地 bot 身份统一显示。
  - `crates/gateway/src/session_labels.rs:3`：现有 helper 仍只按 Telegram bot username 解析 label，没有本体 display_name 口径。
- 当前测试覆盖：
  - 已有：TG-GST v1 转写基础路径已覆盖。
  - 缺口：缺少“命中 link identity 显示本体名”“未命中时的冻结回退顺序”“内部 key 不得进入 speaker”“群聊 bot 才追加 `(bot)`”等关键测试。

## 根因分析（Root Cause）
- A. TG-GST v1 已上线，但 speaker 渲染仍沿用“直接从 Telegram sender 字段拼名字”的旧思路。
- B. PEOPLE 中的 Telegram identity 字段目前主要停留在配置与 UI 层，没有进入 TG adapter 的运行时转写路径。
- C. 本地受管 Telegram bot 的账户真值也没有进入统一的 speaker 身份快照，导致 bot speaker 仍容易退化到渠道名。
- D. speaker 匹配、speaker 渲染、session label 三类问题尚未明确分层，导致实现很容易在多个地方打补丁。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - TG adapter 在群聊转写时先做 `speaker_match`，再做 `speaker_render`。
  - `speaker_match` 对本地受管 bot 必须优先消费账户真值（`agent_id`、`chan_user_id`、`chan_user_name`），对其他 Telegram 发言者必须按 `telegram_user_id` 优先、`telegram_user_name` 次之。
  - 一旦命中 link identity，`speaker_render` 必须优先显示本体 `display_name`。
  - TG-GST v1 群聊 bot speaker 必须追加 `(bot)`；人类 speaker 永不追加。
  - `-> you` 只在“当前 bot 视角下明确指向你”的群聊消息中出现。
- 不得：
  - 不得把内部 account key、session key、bucket key、binding key 输出到群聊正文 speaker。
  - 不得在 gateway/core 侧追加第二套 speaker 修正链路。
  - 不得为了兼容旧显示口径保留“若匹配不到就直接显示 `telegram:...`”的尾巴。
- 应当：
  - 应尽量复用现有 PEOPLE 字段与设置页，不另起新的 identity schema。
  - 应在仅必要时输出 speaker 退化日志，避免噪声。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 由 gateway 提供一份 Telegram identity 只读快照给 TG runtime。
  - TG adapter 在生成 TG-GST v1 文本时本地完成 `speaker_match` 与 `speaker_render`。
- 优点：
  - 满足“TG adapter 负责最终群聊正文”的边界口径。
  - 不需要 gateway/core 再次改写 TG-GST 文本。
- 风险/缺点：
  - 需要打通 PEOPLE -> TG runtime 的只读快照注入路径。

#### 方案 2（备选）
- 核心思路：
  - 保持 TG adapter 只做粗糙 speaker，gateway/core 再在入站后补改 speaker。
- 优点：
  - 表面上改动较少。
- 风险/缺点：
  - 与设计文档和 one-cut 口径直接冲突。
  - 会再次制造“谁负责最终群聊正文”的双轨实现。

### 最终方案（Chosen Approach）
- 采用方案 1。

#### 收敛实施约束（Implementation Constraints）
- 必须保持“TG adapter 产出最终 TG-GST v1 文本，gateway/core 不 rewrite”这一单一边界；**不得**再开第二条 gateway/core speaker 修补链。
- 本单只复用现有 PEOPLE 字段：`display_name`、`telegram_user_id`、`telegram_user_name`、`telegram_display_name`；**不得**新增新的 identity schema、alias 字段或通用 identity DSL。
- identity 快照只允许做成 **Telegram 专属、只读、最小字段集**；**不得**借题发挥抽象出跨渠道通用 identity bus / profile service / naming framework。
- session label / debug label 仅在能够直接复用同一 `speaker_render` 结果或同一命名优先级时顺手收口；**不得**因为本单扩成独立的 label 系统改造。
- PEOPLE 字段变更若需要让 Telegram 运行时立即生效，优先采用**单点共享只读快照**或**现有更新路径上的直接刷新**；**不得**为此引入新的全局 watcher、事件总线或泛化配置热更新框架。
- 测试只覆盖 4 类关键面：identity 匹配、speaker 渲染、handlers 实际入站转写、必要的 label 一致性；**不得**堆大量 UI/legacy/fallback 测试。

#### 明确不做（Non-goals for This Implementation）
- 不重做 PEOPLE 编辑页整体产品形态。
- 不做跨渠道统一 speaker/identity 大重构。
- 不把 account key / session key / binding key 暴露为任何正式显示规则。
- 不为了“兼容旧显示效果”保留 gateway/core rewrite 或旧 `tg:<id>(name)` 兜底格式。

#### 行为规范（Normative Rules）
- 规则 1：speaker 匹配
  - 对本地受管 bot，优先使用账户真值（`agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname`）命中本地 bot identity。
  - 对普通 Telegram 发言者，优先使用 `telegram_user_id` 命中 link identity。
  - 若拿不到稳定 ID 或未命中，再使用 `telegram_user_name`。
  - `display_name`、Telegram `sender_name` 只用于显示，不用于稳定匹配主键。
- 规则 2：speaker 渲染
  - 命中 link identity -> 使用本体 `display_name`
  - 本地受管 bot 未命中 link identity -> `chan_nickname` -> `telegram_display_name` -> Telegram `sender_name` -> 裸 `username`
  - 其他 Telegram 发言者未命中 -> `telegram_display_name` -> Telegram `sender_name` -> 裸 `username`
  - 仍无可用显示名 -> `tg-user-<short_id>` 或 `tg-bot-<short_id>`
- 规则 3：`(bot)`
  - 仅群聊 TG-GST v1 speaker 追加 `(bot)`。
  - DM 不适用该规则。
- 规则 4：`-> you`
  - 只由 TG adapter 根据当前 bot 视角下的 addressed 结果决定。
  - gateway/core 不得再推断或重写。
- 规则 5：禁止泄露内部 key
  - `telegram:123...`、`session_key`、`bucket_key`、binding blob、内部 account key 都不得进入群聊正文 speaker。

#### 接口与数据结构（Contracts）
- 类型归属约束：
  - 若需要新增运行时快照类型，应定义为 **Telegram 专属最小只读类型**（位于 `moltis-telegram` 可被 gateway 直接调用的位置），而不是定义在 gateway 私有模块里再向 Telegram 反向泄漏。
- Telegram identity 只读快照最小字段：
  - `display_name`
  - `telegram_user_id`
  - `telegram_user_name`
  - `telegram_display_name`
  - 对本地受管 bot 还必须包含：`agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname`
  - `chan_nickname` 只参与显示优先级，不作为稳定匹配主键。
- 运行时边界：
  - gateway 负责把 PEOPLE identity 与本地 Telegram bot 账户 identity 合并成只读快照并注入 TG runtime。
  - TG adapter 负责消费快照并输出最终 TG-GST v1 文本。
  - gateway/core 消费的仍然只是最终 `text`，不再修 speaker。
- 收敛实现要求：
  - 如需把 gateway 构造出的 Telegram identity 快照送入运行中 Telegram 插件，优先在**现有** `ChannelService` 上增加一个 Telegram 专用刷新 hook 复用启动/更新路径；不要为此新开独立服务或事件分发层。
  - 若需要运行时刷新 PEOPLE -> Telegram identity 快照，优先采用“gateway 启动时初始化 + `workspace.people.updateEntry` / `workspace.people.sync` 完成后直接刷新 Telegram 插件持有的同一份只读快照”；不要新增 watcher、事件总线或通用服务层抽象。
  - 若某一 label/helper 需要同口径名称，优先直接复用同一渲染 helper 或其冻结优先级，不再复制一套命名规则。
- UI/Debug 展示（如适用）：
  - 设置页沿用现有 PEOPLE 字段；如需补文案，只解释“这些字段用于 TG speaker 匹配/显示优先级”。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若未命中 link identity，不报错；按冻结好的显示顺序降级。
  - 若出现冲突匹配或非法快照，记录结构化日志并拒绝脏数据进入 speaker。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 不涉及额外队列。
  - 删除任何试图在 gateway/core 侧二次修 speaker 的临时补丁路径。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 记录匹配来源时优先使用 `match_method` 与短 ID/hash。
- 禁止打印字段清单：
  - 完整正文
  - 完整 binding
  - 内部 account key / session key

## 验收标准（Acceptance Criteria）【不可省略】
- [x] TG-GST v1 群聊 speaker 命中 link identity 时优先显示本体 `display_name`。
- [x] 未命中 link identity 时，speaker 按冻结好的回退顺序渲染。
- [x] 群聊 bot speaker 统一追加 `(bot)`；人类 speaker 不追加。
- [x] 本地受管 bot 若未命中 link identity，speaker 优先使用 `chan_nickname`，再进入通用 Telegram 显示回退链。
- [x] `-> you` 只出现在当前 bot 明确被指向的群聊视角消息中。
- [x] 群聊正文 speaker 不再出现 `telegram:...`、`session_key`、binding key 之类内部标识。
- [x] PEOPLE 现有 Telegram 字段真正进入 TG adapter 的群聊转写闭环。
- [x] 本地 Telegram bot 账户身份真值（`agent_id`、`chan_user_id`、`chan_user_name`、`chan_nickname`）也进入统一 identity snapshot。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `tg_gst_v1_format_speaker`：命中 link identity 时使用 `display_name`。
- [x] `tg_gst_v1_format_speaker`：本地受管 bot 未命中时按 `chan_nickname` -> `telegram_display_name` -> `sender_name` -> `username` -> `tg-bot-short_id` 回退。
- [x] `tg_gst_v1_format_speaker`：其他 Telegram 发言者未命中时按 `telegram_display_name` -> `sender_name` -> `username` -> `tg-user/tg-bot-short_id` 回退。
- [x] `tg_gst_v1_format_speaker`：内部 key 不得进入最终输出。
- [x] 群聊 bot 才追加 `(bot)`；DM 或人类发言不追加。

### Integration
- [x] `crates/telegram/src/handlers.rs`：群聊入站实际使用 identity 快照参与 TG-GST v1 渲染。
- [x] `crates/gateway/src/people.rs` / 相关 provider：PEOPLE 字段能被构造成 TG runtime 所需快照。
- [x] `crates/telegram/src/config.rs` / `crates/telegram/src/plugin.rs`：本地 bot 账户真值也被并入同一 identity snapshot。
- [x] 本单未触达 `crates/gateway/src/session_labels.rs`；按冻结范围不将 label 系统改造作为阻塞项。

### UI E2E（Playwright，如适用）
- [x] PEOPLE -> Telegram runtime 热刷新已接入启动、`workspace.people.updateEntry`、`workspace.people.sync`；真机展示一致性仍按下方手工验收执行。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 真机 Telegram 多 bot、多账号 link 同一本体的展示一致性仍需人工验收。
- 手工验证步骤：
  1. 配置一个 bot 本体对应两个 Telegram 账号，均 link 到同一 `display_name`。
  2. 在群内分别用两个账号发言。
  3. 确认 TG-GST v1 上下文中两者都显示相同本体名，且无内部 key 泄露。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 与 TG 群聊 planner one-cut 同期交付，不保留 gateway 侧 speaker rewrite 双轨。
- 回滚策略：
  - 代码级回滚；不提供“继续用旧 speaker 文本”的运行时 alias。
- 上线观测：
  - 关注 `telegram.speaker_resolution.*`、`telegram.speaker_degraded`、`telegram.identity_snapshot_invalid` 类日志。

## 实施拆分（Implementation Outline）
- 前置约束：
  - 本单建立在 `issues/issue-v3-telegram-group-execution-plan-record-dispatch-one-cut.md` 已稳定收口的前提上实施。
  - 本单不得重新打开 Issue 1 中关于 planner / record / dispatch 边界的讨论或实现面。
- Task A（最小 identity 快照注入）：
  - gateway 仅负责从现有 PEOPLE 数据提取 Telegram 专属只读 identity 快照，并与本地 Telegram bot 账户真值合并。
  - 不新开通用 identity 服务；如需热刷新，仅沿用 gateway 启动、`workspace.people.updateEntry`、`workspace.people.sync` 这三条现有路径做最小注入，并通过现有 `ChannelService` 的 Telegram 专用刷新 hook 下发到插件。
- Task B（TG adapter 单点收口 speaker 规则）：
  - 在 `crates/telegram/src/adapter.rs` 里完成 `speaker_match` + `speaker_render`，替换当前 `username` / `tg:<id>(display)` 直出。
  - `crates/telegram/src/handlers.rs` 只负责把当前 bot 视角、sender 原始字段和 identity 快照喂给同一套 helper；不在 handlers 再长出第二套命名逻辑。
- Task C（有限一致性收口）：
  - 仅在 `crates/gateway/src/session_labels.rs` 确认是否能直接复用同一冻结优先级；若不能低成本复用，则保持本单只修 TG-GST speaker，不扩展 label 系统，也不把 label 作为阻塞项。
  - UI 侧只补最小说明或必要接线，不改页面结构。
- Task D（聚焦验证与文档收口）：
  - 先补最小失败测试，再跑 `moltis-telegram` 相关定向测试与全量测试。
  - 仅在确有改动时补 gateway 相关单测；完成后同步本 issue 的实施现状/勾选项。
- 受影响文件：
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/gateway/src/people.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/session_labels.rs`（仅在低成本复用同一规则时）
  - `docs/src/refactor/telegram-record-dispatch-boundary.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/refactor/telegram-record-dispatch-boundary.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `issues/issue-telegram-group-session-transcript-text-protocol-tg-gst-v1.md`
  - `issues/issue-v3-telegram-group-execution-plan-record-dispatch-one-cut.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- Q1:
  - 无阻塞性未决问题；`session_labels` 若未被低成本复用，不构成本单阻塞项。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

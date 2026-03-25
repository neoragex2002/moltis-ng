# Issue: V3 一刀切收敛会话标识与渠道边界（删 `chan_chat_key/persona*`，定 `reply_target_ref/channelTarget`，统一 `session_id/session_key` 与 sandbox 分桶）

> SUPERSEDED BY:
> - 设计真源：`docs/src/refactor/session-key-bucket-key-one-cut.md`
> - 治理主单：`issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
> - 本单仅保留历史背景与实施证据，不再定义当前实现口径或规范优先级。

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-22
- Owners: TBD
- Components: gateway/agents/tools/common/channels/telegram/config/ui/onboarding/docs
- Affected providers/models: openai-responses::(prompt_cache_key), all (tool context)

**建议实施顺序（Sequence / 记录，避免上下文遗忘）**
1) 先把“基础坐标系”切干净：`persona* -> agent_id`、`people/ -> agents/`、`tools.exec.sandbox.scope -> scope_key`、tool context `_chanChatKey -> _sessionKey`（确保编译与测试稳定）。
2) 再改 hooks 载荷：删除 `chanChatKey/chanAccountKey`，统一 `channelTarget`（来源仅 `channel_binding`），并同步修复 plugins + docs。
3) 再做运行时 turn bridge 一刀切：`ChannelTurnContext`/WS/status/tool payload 移除 `chan_chat_key/ChannelReplyTarget`，统一 `session_id/session_key + reply_target_ref/channelTarget`。
4) 最后收敛 legacy parse：`channel_binding` 的 JSON 解析归口到 adapter/helper 单点；全仓兜底 `rg` 清扫 + 打勾验收项。

**已决策（Decision log）**
- 2026-03-20：V3 一刀切删除 `persona*` 术语与字段，统一收敛为 `agent_id`（对外字段 `agentId`），并将 Type4 模板目录从 `people/<id>/...` 改为 `agents/<agent_id>/...`；运行时与有效文档统一切到 `agents/`，命中 legacy `people/` 目录直接结构化拒绝并记录 `reason_code = legacy_people_dir_rejected`。
- 2026-03-20：Q6（Legacy 模块）决策：保留 `crates/auto-reply` 与 `crates/routing` 作为 V3 占位子系统（接口保留、功能后补），输入统一改为 `InboundContextV3`；不保留/不兼容 V2 `MsgContext/chan_chat_key` 路径（breaking）。
- 2026-03-20：Q5（Sandbox 分桶）决策：正式口径切到 `tools.exec.sandbox.scope_key=session_id|session_key`，默认模板与文档只写 `scope_key`；legacy `tools.exec.sandbox.scope` 不再保留兼容别名，命中即硬错误：
  - `session_id`：沙盒与会话实例绑定（推荐默认）
  - `session_key`：沙盒与逻辑桶绑定（按 dm/group scope 分桶口径共享）
  - legacy `scope=chat`：不再映射；若出现 `scope`（无论是否同时配置 `scope_key`）都显式报错
- 2026-03-20：渠道信息暴露边界决策：TG 适配层之外默认不暴露投递细节（`chat_id/thread_id/message_id/account_key/...`），任何渠道交互能力与回投统一走 `session_id -> channel_binding -> adapter`；UI 只拿展示字段，hooks 默认只拿跨渠道语义，必要时可选展开 `channelTarget`（可空、来源仅 `channel_binding`）。实施口径见：`docs/src/refactor/channel-info-exposure-boundary.md`。
- 2026-03-20：Q2（Inbound meta）决策：删除跨层 `ChannelMessageMeta.telegram`（以及 `ChannelTelegramMeta/TelegramChatKind/ChannelTranscriptFormat` 这类 TG 私有 meta），gateway/core 不再依赖 Telegram 私有字段拼装群聊入站文本；群聊入站文本（TG-GST v1/现有格式）由 TG adapter 产出。跨层仅保留/新增通用字段：`chat_kind/addressed/mode/message_kind/text`（以及 `session_id/session_key`），并在 hooks/UI 中按“channel-info exposure boundary”口径分别提供必要的最小集合。
- 2026-03-20：Q1（Reply 投递契约）决策：回投/回复 threading 走 adapter opaque 引用，不再把渠道投递细节暴露为跨层结构体字段：
  - `ChannelReplyTarget` 逐步退场（不再作为跨层契约承载 `chat_id/thread_id/message_id/account_key` 等渠道坐标）。
  - 新增 `reply_target_ref`（opaque，建议为带 `v=1` 的 JSON 字符串/字节），由 TG adapter 生成并负责解析与执行 reply-to/thread/topic 规则。
  - gateway 的职责仅限：选择对应 adapter（例如 telegram outbound）并转交 `reply_target_ref`；core 不识别也不持有任何 TG 投递字段。
  - 任何工具的“渠道交互能力”与正常回复回投，统一走 `session_id -> channel_binding/reply_target_ref -> adapter`（与 `docs/src/refactor/channel-info-exposure-boundary.md` 一致）。
- 2026-03-20：Q4（Tools 渠道识别）决策：tools 不再识别渠道类型（不解析 `_chanChatKey`/不靠 `_sessionKey` 前缀判断 TG/Discord）；凡需“渠道交互能力”（如 TG 位置请求）统一下放到渠道适配层（adapter/outbound），tools 仅用 `_sessionId` 发起请求：
  - tool：有 `_connId` 走浏览器；否则调用 `request_channel_location(_sessionId)`，不再做渠道判断
  - core/gateway：用 `session_id -> channel_binding` 判断是否支持，unsupported 立即 `NotSupported`（带 `reason_code`）
  - adapter：实现“请求位置”的渠道交互（发送提示/按钮），并确保回传按 V3 `bucket_key` 命中对应 `session_id`，禁止 fallback 到 chat-wide active session（避免多桶串线）
- 2026-03-20：Q3（Hooks 渠道信息）决策：hooks 仍可获得“不少于现状”的渠道细节，但不再在 hook payload 里传播 V2 `chanChatKey/chanAccountKey`：
  - hook payload 改为提供结构化 `channelTarget` 对象（camelCase），字段包含 `type/accountKey/chatId/threadId`（thread 可空）
  - `channelTarget` 的数据来源必须是 `session_id -> session metadata -> channel_binding -> adapter/helper 本地解析 helper` 的结果；允许调用本地 helper，但不得在 hook 热路径内做额外网络/IPC 查询
  - `channelTarget` 应出现在所有“带 sessionId 的事件”中（统一 schema，字段可空）
  - 若 session 无 binding：`channelTarget=null`，并且 hook 逻辑应仅基于 `sessionId/sessionKey` 做策略
- 2026-03-20：`group_session_transcript_format` 决策：C 阶段 one-cut 不保留该跨层配置项；TG adapter 直接产出当前固定口径的群聊入站文本（TG-GST v1/现有格式），gateway/UI/API/snapshot 不再暴露 transcript format 选择器。
- 2026-03-20：`channel-adapter-generic-interfaces.md` 决策：本轮实施前必须对齐当前 one-cut 口径；在文档完成对齐前，`issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`、`docs/src/refactor/channel-info-exposure-boundary.md`、`docs/src/refactor/telegram-adapter-boundary.md` 具有更高优先级。

**已实现（如有，写日期）**
- 2026-03-20：TG 主路径已具备 `bucket_key` 分桶，并且会话定位真值优先 bucket 映射：`crates/gateway/src/channel_events.rs:215`
- 2026-03-20：OpenAI Responses provider 的 prompt cache bucketing 已读取 `LlmRequestContext.session_id`：`crates/agents/src/providers/openai_responses.rs:570`
- 2026-03-20：worktree branch 已绑定在 sessions metadata 的 `key` 字段上（当前口径等价“会话实例 id”）：`crates/sessions/src/metadata.rs:465`
- 2026-03-20：V2 `MsgContext` 已从 `moltis-common` 删除，legacy `auto-reply` / `routing` 改为 V3 占位输入 `InboundContextV3`：`crates/common/src/types.rs:26`、`crates/auto-reply/src/reply.rs:1`、`crates/routing/src/resolve.rs:1`
- 2026-03-21：gateway 已把 `session_key/channelTarget` 作为 hook-only 上下文显式传入 runner，`BeforeLLMCall/AfterLLMCall/BeforeToolCall/AfterToolCall` 不再退化为 `None`：`crates/gateway/src/chat.rs`、`crates/agents/src/runner.rs`
- 2026-03-21：`docs/src/session-branching.md` 已完成 `chanChatKey/chanReplyTarget` → `sessionKey/channel binding / reply target state` 的文档口径收敛，`docs/src`（排除冻结历史文档）关键词清扫归零：`docs/src/session-branching.md`
- 2026-03-21：Telegram 旧配置字段 `persona_id` 已改为显式拒绝，不再静默丢失 bot 绑定并回退 default agent：`crates/telegram/src/config.rs`
- 2026-03-22：loader 在命中新 `agents/` 路径缺失但旧 `people/` 路径存在时，会直接拒绝读取并记录 `reason_code=legacy_people_dir_rejected`：`crates/config/src/loader.rs:586`
- 2026-03-22：sandbox 配置校验与模板已收口到 `scope_key`：默认模板仅输出 `scope_key`，legacy `tools.exec.sandbox.scope` 为硬错误，不再映射：`crates/config/src/template.rs:227`、`crates/config/src/validate.rs:1024`
- 2026-03-22：`scope_key=session_key` 在普通 web/main session 缺少 `_sessionKey` 时直接失败，并记录 `reason_code=missing_session_key_for_scope_key_session_key`；不再回退 `session_id`：`crates/tools/src/sandbox.rs:2433`

**已覆盖测试（如有）**
- `prompt_cache_key` 在 debug overrides 里会标注来源 `sessionId`：`crates/agents/src/providers/openai_responses.rs:1641`
- runner hook payload 回归：`_sessionKey` 会透传到 `BeforeLLMCall`，且不再出现 legacy `chanChatKey/chanAccountKey`：`crates/agents/src/runner.rs`
- gateway ↔ runner hooks 端到端回归：有 `channel_binding` 的会话会把 `sessionKey/channelTarget` 贯穿到 `BeforeLLMCall/BeforeToolCall/AfterToolCall`：`crates/gateway/src/chat.rs`
- Telegram config 回归：旧 `persona_id` 会被显式拒绝：`crates/telegram/src/config.rs`
- loader 回归：默认与 named agent 命中 legacy `people/` 文档时直接拒绝：`load_soul_rejects_legacy_people_default_path`、`load_agent_identity_md_raw_rejects_legacy_people_path`（`crates/config/src/loader.rs`）
- sandbox config 回归：legacy `scope=chat` 为硬错误，`scope` 与 `scope_key` 同时配置仍显式报错，默认模板只输出 `scope_key`：`legacy_sandbox_scope_chat_is_a_hard_error`、`sandbox_scope_and_scope_key_conflict_still_errors`、`default_config_template_uses_scope_key`（`crates/config/src/validate.rs`）
- sandbox runtime 回归：`scope_key=session_key` 且缺少 `_sessionKey` 时直接失败；legacy `scope=chat` 不再映射：`test_effective_sandbox_key_scope_key_session_key_missing_errors`（`crates/tools/src/sandbox.rs`）

**已知差异/后续优化（非阻塞）**
- 本单目标是“切干净”，不接受保留 V2 尾巴；在清理 `chan_chat_key` 的同时，也确认了一组“概念/命名”层面的额外收敛项需要一并收口：
  - `persona_id` 概念目前横跨 gateway/tools/telegram 配置与 docs/UI，与 V3 的 `agent_id` 概念并存；已决策一刀切删除 persona（见 Decision log）。
- `issues/issue-v3-one-cut-readiness-gaps.md` 中记录的实施前补缺已并回本单；该单保留为当时那轮复核记录。
- 本轮历史口径已冻结；当前实现与规范已迁移到 `docs/src/refactor/session-key-bucket-key-one-cut.md` 与 `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`。
- 2026-03-21：本轮建议实施顺序（减少返工，逐步可编译/可测）：
  1) Step 1：先贯穿 `session_id/session_key` 两条主链数据流（先修 `LlmRequestContext.session_id` 与 prompt-cache/worktree 绑定口径）
  2) Step 3：tool context 一刀切：只注入/读取 `_sessionId/_sessionKey`
  3) Step 9 + Step 4：sandbox `scope_key`（先 schema/validate/UI，再改 router key 取值）
  4) Step 2：清运行时旧桥（`ChannelTurnContext`/pending reply/status/WS payload 退场 `chan_chat_key/ChannelReplyTarget`）
  5) Step 5 + Step 11：hooks 统一 `channelTarget`（gateway 单点解析并传入 runner；禁止热路径网络/IPC）
  6) Step 6 + Step 12：reply 投递与渠道坐标 opaque 化（`reply_target_ref`，并收口 legacy parse 到 adapter/helper）
  7) Step 10：`location` 渠道交互下放 adapter（基于 `session_id -> channel_binding` 判定支持）
  8) Step 7 + Step 13：TG meta/transcript 职责回收与桥接尾巴清理
  9) Step 8：persona/Type4 目录 `people/` → `agents/` 的大面扫尾（breaking 收口放最后）

---

## 背景（Background）
- 场景：V3 已将 Telegram 与 core 边界收敛，但代码与工具/钩子层仍残留 V2 的跨域桥概念 `chan_chat_key/_chanChatKey/chanChatKey`，导致概念坐标系混乱、命名混用、并在部分路径上把 `session_key` 当 `session_id` 使用（影响 prompt cache、hooks、sandbox/router 等）。
- 约束：
  - V3 口径冻结：`session_id` 是会话实例唯一 id，用于 LLM 南向缓冲（prompt cache）与 worktree 绑定；`session_key` 是跨域桥/逻辑桶名（替代 V2 `chan_chat_key`）。
  - 代码内部统一 `snake_case`；对外 JSON/RPC/WS/hooks/tool-context 维持 `camelCase`（仅在边界处 serde 显式映射）。
  - V2 的 `chanChatKey` 口径禁止继续保留，必须删除而不是“继续兼容旧路径”。
- Out of scope：
  - 最终落盘替换（`session_event` 持久化替换 / 历史迁移）不在本单做。
  - `docs/src/concepts-and-ids.md` 的 V2 冻结内容暂不在 C 阶段收口（后继单独处理），但不得继续作为运行时口径/实现依据。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`session_id`**（主称呼）：唯一指代一个“会话实例”（同一逻辑桶可在时间维度上产生多个会话实例）。
  - Why：用于 prompt cache 分桶、worktree 绑定、LLM 南向缓冲等“实例级”行为。
  - Not：不是跨域桥；不应承载“渠道坐标/聊天坐标”语义。
  - Source/Method：authoritative（由系统创建/持久化的 id）；as-sent（写入 LLM 请求上下文）。
  - Aliases（仅记录，不在正文使用）：V2 `sessionId`（外部字段名仍可叫 `sessionId`，但语义必须等价本条）

- **`session_key`**（主称呼）：跨域桥/逻辑会话桶名（用于“这条输入属于哪个逻辑桶”）。
  - Why：替代 V2 `chan_chat_key`，作为跨层/跨域一致的会话桶坐标。
  - Not：不是会话实例 id；不得用于 prompt cache 分桶与 worktree 绑定。
  - Source/Method：effective（由 type/scope + adapter bucket 结果装配而成）。
  - Aliases（仅记录，不在正文使用）：V2 `chan_chat_key` / `chanChatKey` / `_chanChatKey`（必须删除）

- **`bucket_key`**（主称呼）：适配层返回的分桶结果编码（黑盒字符串，可持久化/可比对）。
  - Why：adapter 负责实现 scope 语义；core 用它装配 `session_key` 并命中会话。
  - Not：不是 `session_id`。
  - Source/Method：effective（adapter 解析 + degrade 规则后的生效值）。
  - Aliases：`dm_subkey` / `group_subkey`

- **`channel_binding`**（主称呼）：session 级渠道绑定载体（当前落盘保持不变）。
  - Why：当前阶段所有 `session_id -> adapter/outbound` 的稳定落点仍依赖它。
  - Not：不是 per-turn reply/thread/topic 引用；不得继续在 gateway/core/tools/hooks/ui 中到处直接展开成 `ChannelReplyTarget`。
  - Source/Method：authoritative（session metadata / 旧落盘真实载体）。

- **`reply_target_ref`**（主称呼）：adapter 私有的 per-turn/per-delivery opaque 引用。
  - Why：用于 reply-to / typing / edit / topic/thread 等“具体发回哪里”的运行时投递。
  - Not：不是 `channel_binding` 的别名；不应被 core 解释为公共字段集合。
  - Source/Method：effective（adapter 生成并回收）。

- **`sender_id`**（仅限通用 actor 场景）：可保留为“命令/策略/hook 的外层 actor 标识”。
  - Why：某些 hook / tool policy 仍需要表达“是谁触发了命令/策略”。
  - Not：不是 TG 原生 `sender_id`；不能再作为 `ChannelMessageMeta.telegram.sender_id` 那种平台私有字段继续跨层流转。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 运行时链路删除 V2 `chan_chat_key` 概念：`crates/*`、`docs/src/refactor/*`、hooks/tools/tests/UI 不再出现 `chan_chat_key` / `chanChatKey` / `_chanChatKey`（不包含 `docs/src/concepts-and-ids.md`，该文档后续单独收口）。
- [x] 工具上下文里用 `session_key` 表达跨域桥：新增并统一 `_sessionKey`（camelCase），并停止注入/透传 `_chanChatKey`。
- [x] `LlmRequestContext.session_id` 必须传入真实 `session_id`（会话实例 id），不得再误传 `session_key`。
- [x] prompt cache 分桶与 worktree 绑定统一使用 `session_id`（不使用 `session_key`）。
- [x] hooks 载荷必须同时能表达 `session_id` 与 `session_key`（如需要跨域桥），且字段语义不混用。
- [x] 删除 `persona*` 术语与字段：统一到 `agent_id`；对外 JSON/RPC/WS 字段统一为 `agentId`（仅边界处显式映射）。
- [x] Type4 模板目录从 `people/<id>/...` 迁移为 `agents/<agent_id>/...`，并更新所有 loader / docs / UI 引用；运行时统一以 `agents/` 为主，命中旧 `people/` 目录直接拒绝。
- [x] Sandbox 分桶口径改为 `tools.exec.sandbox.scope_key=session_id|session_key`；默认模板与文档统一输出 `scope_key`，legacy `tools.exec.sandbox.scope` 不再保留 alias，命中即报错。
- [x] Tools 不再识别渠道（删除 `_chanChatKey` 依赖与 `_sessionKey` 前缀判断）；渠道交互能力下放 adapter（以 `location` 为首个落点）：`crates/tools/src/location.rs`、`crates/gateway/src/server.rs`、`crates/telegram/src/outbound.rs`
- [x] Hooks payload 删除 V2 `chanChatKey/chanAccountKey`，改为结构化 `channelTarget`（来源于 `channel_binding`），能力不弱于现状：`crates/common/src/hooks.rs`、`crates/agents/src/runner.rs`、`crates/gateway/src/chat.rs`
- [x] 运行时 turn bridge 也必须一刀切：`ChannelTurnContext`、pending reply/status、WS/status payload 不再保存或广播 `chan_chat_key/ChannelReplyTarget` 这类旧桥字段，统一改为 `session_id/session_key + reply_target_ref/channelTarget` 口径：`crates/gateway/src/state.rs`、`crates/gateway/src/channel_events.rs`、`crates/gateway/src/chat.rs`
- [x] `channel_binding` 的 legacy 解析必须收敛到 adapter/helper 单点；gateway/core 不再散落 `serde_json::from_str::<ChannelReplyTarget>(channel_binding)` 这种直接解析。
- [x] 删除 `group_session_transcript_format` 全部 config/UI/API/snapshot 暴露面；TG adapter 直接产出固定口径的群聊入站文本：`crates/telegram/src/config.rs`、`crates/telegram/src/plugin.rs`、`crates/gateway/src/channel.rs`、`crates/gateway/src/assets/js/page-channels.js`
- [x] 删除 `persona*` / `people/` 的 UI/onboarding/loader/runtime 尾巴，收敛到 `agent_id` / `agents/<agent_id>/...`：`crates/gateway/src/assets/js/page-settings.js`、`crates/config/src/loader.rs`、`crates/onboarding/src/service.rs`

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：内部变量/字段命名统一 `snake_case`，不混用驼峰与下划线（JSON key 例外）。
  - 不得：保留任何 V2 `chan_chat_key` 的“兼容输入解析”尾巴；一律切到 V3 术语体系。
  - 必须：core 不需要知道的渠道投递细节，后续应继续向 adapter 私有对象（opaque ref）收敛（本单优先清 `chan_chat_key` 坐标系）。
- 兼容性：本单为“切干净”任务，允许 breaking change（但必须写清升级说明与回滚策略）。
- 可观测性：任何 strict reject / policy block 必须结构化日志 + `reason_code`（不打印敏感正文）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 代码与文档同时存在两套坐标系：V2 `chan_chat_key` 与 V3 `session_key/session_id` 并存，导致开发与排障时语义混淆。
2) 部分链路把 `session_key` 当成 `session_id` 传入 LLM 请求上下文，导致 prompt cache bucketing 不符合“实例级”口径（同桶多实例时可能错误共享/错误隔离）。
3) core/gateway 的通用跨层契约仍显式暴露渠道投递细节（`chan_account_key/chat_id/thread_id/message_id`），无法满足“渠道细节对 core 不透明”的 V3 信息隐藏口径。
4) hooks / tools 仍直接携带或解析 V2 `chan_chat_key`（以及关联的 `chan_account_key`），导致 V2 概念继续向外扩散。
5) `persona_id/persona*` 与 `agent_id` 两套术语并存（配置/UI/工具 schema/docs），且 Type4 模板目录仍为 `people/<id>/...`，不符合 V3 “agent” 口径并增加维护成本。

### 影响（Impact）
- 用户体验：prompt cache 命中不稳定（表现为延迟/成本/缓存读写 tokens 漂移），hook/工具上下文歧义导致行为难以解释。
- 可靠性：当 V3 scope 分桶导致同 chat 多桶并行时，继续依赖 `chan_chat_key` 会引入隐式 chat 级耦合。
- 排障成本：日志/调试面板/文档口径不一致，必须反向猜测“这个 session_id 字段里装的到底是什么”。

### 复现步骤（Reproduction）
1. 触发一次 TG group 多桶（per_sender/per_branch）对话，使同 chat 下存在多个 `bucket_key`。
2. 观察 tool context 注入仍包含 `_chanChatKey`，并且部分路径把 `session_key` 传入 `LlmRequestContext.session_id`。
3. 期望 vs 实际：
   - 期望：`session_id`（实例）用于 prompt cache/worktree；`session_key`（跨域桥）用于工具/路由。
   - 实际：V2/V3 概念混用，且 `_chanChatKey` 仍在主链出现。

## 现状核查与证据（As-is / Evidence，实施前历史记录）【不可省略】
> 说明：本节记录的是 2026-03-20 实施前的 inventory，用于解释根因与范围；截至 2026-03-21 已按本单 one-cut 全部清理并补齐测试。最新证据见下节“实施完成证据”。
- 代码证据：
  - `crates/gateway/src/state.rs:92`：`ChannelTurnContext` 仍保存 `chan_chat_key` 与 `Vec<ChannelReplyTarget>`，旧运行时桥仍在。
  - `crates/gateway/src/channel_events.rs:437`：WS/chat payload 仍输出 `chanChatKey`；`crates/gateway/src/channel_events.rs:1476`：状态卡片仍展示 `ChanChatKey`。
  - `_chanChatKey` 工具上下文仍被注入：`crates/gateway/src/chat.rs:5102`
  - tools 仍读取/透传 `_chanChatKey`：`crates/tools/src/location.rs:368`、`crates/tools/src/process.rs:408`、`crates/tools/src/sandbox_packages.rs:444`、`crates/tools/src/spawn_agent.rs:184`
  - `chan_chat_key` 仍被用于 sandbox/router key：`crates/gateway/src/chat.rs:4658`
  - `HookPayload::BeforeAgentStart.session_id` 当前传入的是 `session_key`：`crates/gateway/src/chat.rs:4686`
  - `LlmRequestContext.session_id` 在 streaming/compaction 等路径被设置为 `session_key`：`crates/gateway/src/chat.rs:5860`
  - V2 identity 辅助仍存在：`crates/common/src/identity.rs:41`
  - `request_channel_location()` 仍接受 `_chanChatKey` / `parse_chan_chat_key()` 归一，并直接把 `channel_binding` 反序列化成 `ChannelReplyTarget`：`crates/gateway/src/server.rs:218`
  - gateway prompt/debug/runtime 仍直接解析 `channel_binding -> ChannelReplyTarget`：`crates/gateway/src/chat.rs:899`
  - sandbox router 仍从 `channel_binding` 反推 `chan_chat_key`，并保留旧 `scope` 判断与提示文案：`crates/gateway/src/session.rs:122`、`crates/gateway/src/session.rs:404`、`crates/tools/src/sandbox.rs:606`、`crates/gateway/src/assets/js/sandbox.js:46`
  - core 的通用回复目标仍显式暴露 `chan_account_key/chat_id/thread_id/message_id`（渠道投递细节未 opaque）：`crates/channels/src/plugin.rs:248`
  - core 的通用 message meta 仍显式带 Telegram 专项字段块（`ChannelMessageMeta.telegram`）：`crates/channels/src/plugin.rs:190`
  - hooks payload 仍显式携带 `chan_chat_key/chan_account_key`：`crates/common/src/hooks.rs:111`
  - `group_session_transcript_format` 仍暴露在 TG config/snapshot/UI/API：`crates/telegram/src/config.rs:112`、`crates/telegram/src/plugin.rs:113`、`crates/gateway/src/channel.rs:80`、`crates/gateway/src/assets/js/page-channels.js:343`
  - `persona_id` 配置/字段仍存在并与 `agent_id` 并存：`crates/telegram/src/config.rs:151`、`crates/gateway/src/chat.rs:2499`、`crates/tools/src/spawn_agent.rs:117`、`crates/gateway/src/assets/js/page-settings.js:329`、`crates/onboarding/src/service.rs:496`
- 文档证据：
  - V2 文档冻结 `chanChatKey` 作为跨域桥：`docs/src/concepts-and-ids.md:25`
  - V3 refactor 文档已转向 `session_key/session_id` 与 bucket 语义：`docs/src/refactor/v3-design.md:1`
  - persona/type4 文档仍使用 `people/<persona_id>/...` 口径：`docs/src/system-prompt.md:18`
- 当前测试覆盖：
  - 已有：OpenAI Responses prompt cache 读取 `ctx.session_id`：`crates/agents/src/providers/openai_responses.rs:1507`
  - 缺口：缺少“同 chat 多桶、多实例”下 prompt cache key 必须按 `session_id` 的回归；缺少“不再注入/解析 `_chanChatKey`”的回归。

## 实施完成证据（Post-fix / Evidence）【2026-03-21】
- `rg -n \"_chanChatKey|chanChatKey|chan_chat_key\" crates docs/src/refactor` 结果为 0
- `rg -n \"\\bpersona_id\\b|\\bpersonaId\\b\" crates docs/src` 的命中仅剩“显式拒绝旧字段”的测试断言；运行时代码与有效文档已清零：`crates/telegram/src/config.rs`
- `rg -n \"people/\" crates docs/src` 的命中仅剩“legacy 拒绝日志 / 历史说明”的 loader 文案与测试；运行时代码与有效文档已清零：`crates/config/src/loader.rs`
- `rg -n \"group_session_transcript_format\" crates docs/src/refactor` 结果为 0
- runner 触发的 hooks 已携带 gateway 预解析的 `sessionKey/channelTarget`，不再退化为 `None`：`crates/gateway/src/chat.rs`、`crates/agents/src/runner.rs`
- `TelegramAccountConfig` 使用严格反序列化，旧 `persona_id` 输入会直接报错而不是静默掉字段：`crates/telegram/src/config.rs`
- `agents/` 缺失但旧 `people/` 文档存在时，loader 会直接拒绝读取并给出 `reason_code=legacy_people_dir_rejected` 的结构化告警：`crates/config/src/loader.rs`
- Telegram 私有 meta 跨层字段已删除：`crates/channels/src/plugin.rs` 不再含 `ChannelMessageMeta.telegram` / `ChannelTelegramMeta` / `ChannelTranscriptFormat` / `TelegramChatKind`
- TG-GST v1 入站文本由 adapter 产出（gateway 不再二次拼装）：`crates/telegram/src/handlers.rs`、`crates/telegram/src/adapter.rs`
- gateway 不再散点解析 `channel_binding`：解析/兼容性判断集中在 adapter helper：`crates/telegram/src/adapter.rs`
- gateway 入站与用户反馈不再依赖 `ChannelReplyTarget` 跨层透传，统一走 `reply_target_ref`：`crates/gateway/src/channel_events.rs`、`crates/channels/src/plugin.rs`
- `request_channel_location(session_id)` 不再在 gateway 解析 `channel_binding -> ChannelReplyTarget`，统一通过 adapter helper 生成 `reply_target_ref` 并由 outbound 执行：`crates/gateway/src/server.rs`
- workspace 单测：`cargo test -q` 通过

## 根因分析（Root Cause）
- A. V2 时代把 `chan_chat_key` 当作跨域桥，工具/hook/sandbox/router 围绕它建立；V3 切分后未同步替换。
- B. 历史上把 `session_key`（旧名）当成“会话实例 id”在代码中长期使用，导致 `session_id/session_key` 语义未彻底拆开。
- C. 边界处既要对外输出 camelCase，又要对内保持 snake_case，缺少统一“命名与语义映射”的硬规则与清理单。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 全系统只保留 V3：`session_id`（实例）与 `session_key`（跨域桥/桶名）。
  - `LlmRequestContext.session_id` 一律填 `session_id`，不得填 `session_key`。
  - prompt cache 分桶与 worktree 绑定只使用 `session_id`。
  - tool context 仅允许 `_sessionId` 与 `_sessionKey`（camelCase），禁止 `_chanChatKey`。
- 不得：
  - 不得再出现 `chan_chat_key/chanChatKey/_chanChatKey` 任意形态（含 docs 与测试）。
  - 不得把渠道坐标（chat/thread 等）当作跨域桥主键传播到 core（后续继续向 adapter opaque 收敛）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（命名边界）：Rust 内部一律 `snake_case`；对外 JSON 一律 `camelCase`；仅在 serde 显式映射处出现 camelCase 字面量。
- 规则 2（概念替换）：所有 V2 `chan_chat_key` 用途改为 V3 `session_key`；并删除 `chan_chat_key` 相关类型/工具上下文/identity helper。
- 规则 3（实例 id 统一）：所有“LLM 南向缓冲/会话实例绑定”使用 `session_id`，禁止使用 `session_key`。
- 规则 4（Sandbox 分桶口径）：sandbox router 的 key 一律来自 `tools.exec.sandbox.scope_key` 指定的来源（`session_id` 或 `session_key`），不得再隐式混用 `_chanChatKey`/`chan_chat_key` 或“从 channel_binding 推导确定性 key”。
- 规则 5（Tools 渠道识别禁止）：tools 不得解析/识别渠道类型（不得读取 `_chanChatKey`，不得用 `_sessionKey` 前缀判断 TG/Discord）；需要渠道交互的能力必须通过 core→adapter 契约实现（以 `session_id` 定位绑定）。
- 规则 6（Hooks 渠道信息来源）：hooks 如需渠道细节，只允许来自 `channel_binding` 经 adapter/helper 本地解析后的结构化 `channelTarget` 对象；允许本地 helper，禁止网络/IPC“现查现算”（避免引入外部依赖与失败面）。
- 规则 7（运行时旧桥必须退场）：`ChannelTurnContext`、pending reply/status、WS/status/tool payload 不得再把 `chan_chat_key` 或 `ChannelReplyTarget` 当真值结构；运行时只允许 `session_id/session_key` + `reply_target_ref/channelTarget`。
- 规则 8（legacy parse 归口）：`channel_binding` 的 legacy JSON 解析只允许留在 adapter/helper 单点；gateway/core 不得继续散点 `serde_json::from_str::<ChannelReplyTarget>(...)`。
- 规则 9（transcript 配置一刀切删除）：`group_session_transcript_format` 不再允许作为 bridge tail 留在 config/UI/API/snapshot；TG adapter 直接产出当前固定口径的群聊文本。
- 规则 10（`sender_id` 口径澄清）：只删除平台私有 `sender_id/sender_is_bot` 跨层暴露；若 hook/tool policy 仍有通用 actor `sender_id`，可保留，但必须与渠道私有 sender 字段彻底脱钩。

#### 接口与数据结构（Contracts）
- 渠道信息暴露边界（实施级口径）：`docs/src/refactor/channel-info-exposure-boundary.md`
  - TG-GST v1（群聊文本拼装/转写）职责回收至 TG adapter；gateway/core 不再根据 Telegram 私有 meta 二次拼装入站文本
  - UI 仅保留展示字段（`channel.type/senderName/username/messageKind/model`），不暴露投递细节
  - hooks 默认只拿跨渠道语义字段；`channelTarget` 作为可选展开字段（默认 `null`），来源仅 `channel_binding`
- `channel_binding`（session 级）：
  - 继续保留当前落盘载体；本单不改落盘格式
  - 只允许通过 adapter/helper 的集中 helper 读成外围可用信息（例如 `channelTarget` / outbound target）
  - 不允许 gateway/core/tools/hooks/ui 继续把它当“公开 `ChannelReplyTarget` JSON”到处直接解析
- Reply 投递（跨层）：
  - 新增：`reply_target_ref`（opaque；由 adapter 生成/解析；建议携带 `v=1`）
  - 约束：gateway 只转交，不解析；core 不持有任何 `chat_id/thread_id/message_id/account_key` 等投递字段
- 运行时 turn context：
  - `ChannelTurnContext` 只保留 turn 级运行时真值（`session_id/session_key`、status、opaque reply ref）
  - 不再保存 `chan_chat_key`，也不再把 `Vec<ChannelReplyTarget>` 当作跨层 reply 队列
- Tool context（JSON）：
  - 新增：`_sessionKey`（必填/在有会话桶语义时）
  - 保留：`_sessionId`（必填/会话实例）
  - 删除：`_chanChatKey`
- Tool ↔ gateway（能力）：
  - `request_channel_location(session_id)`：输入只允许 `session_id`，gateway 以 `session_id -> channel_binding` 判断与路由，禁止再接受/解析任何渠道坐标 key。
- Hooks（JSON）：
  - 必须至少包含 `sessionId`（实例）；如需要跨域桥再加 `sessionKey`。
- Hooks（渠道信息）：
  - 删除：`chanChatKey` / `chanAccountKey`
  - 新增：`channelTarget`（可空）：
    - `type`：`"telegram" | "discord" | ...`
    - `accountKey`：例如 `"telegram:123456789"`
    - `chatId`：例如 `"-100123..."`
    - `threadId`：例如 `"42"`（无则省略或 null）
- Agent（对外）：
  - 删除：`personaId` / `persona_id`（所有对外 schema/config/docs）
  - 统一：`agentId`（camelCase）；内部使用 `agent_id`（snake_case）
  - Type4 模板目录：`agents/<agent_id>/{IDENTITY,SOUL,AGENTS,TOOLS}.md`
- Sandbox（配置）：
  - 新增：`tools.exec.sandbox.scope_key = "session_id" | "session_key"`
  - 删除：`tools.exec.sandbox.scope`（不再支持 `chat|bot|global`）

#### 失败模式与降级（Failure modes & Degrade）
- 若某路径拿不到 `session_id`（理论上不应发生），必须：
  - 直接失败并记录结构化日志（`reason_code = "missing_session_id"`），不得偷偷用 `session_key` 兜底冒充。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `rg -n \"_chanChatKey|chanChatKey|chan_chat_key\" crates docs/src/refactor` 结果为 0（template/历史说明除外；issue 跟踪文档允许作为历史说明存在）。
- [x] 运行时 turn bridge 不再保存 `chan_chat_key` / `Vec<ChannelReplyTarget>`，并且 reply/status 只按 `session + turn` 与 opaque ref 管理：`crates/gateway/src/state.rs`
- [x] WS/chat/status/tool/hook payload 不再输出 `chanChatKey`，状态卡/UI 也不再展示 `ChanChatKey`：`crates/gateway/src/channel_events.rs`、`crates/common/src/hooks.rs`、`crates/agents/src/runner.rs`
- [x] `LlmRequestContext.session_id` 在 streaming/compaction/tools 路径均为真实 `session_id`，不会再等于 `session_key`。
- [x] OpenAI Responses 的 `prompt_cache_key` 在“同一 `session_key` 但不同 `session_id`”场景下不会共享 bucket（以自动化测试证明）。
- [x] tool context 只包含 `_sessionId`/`_sessionKey`，且所有 tools 能正确读取并通过测试。
- [x] `rg -n \"\\bpersona_id\\b|\\bpersonaId\\b\" crates docs/src` 的命中仅允许出现在“显式拒绝旧字段”的测试断言中；运行时代码与有效文档不再保留该字段。
- [x] `rg -n \"people/\" crates docs/src` 的命中仅允许出现在“legacy 拒绝日志 / 历史说明”的 loader 文案与测试中；运行时代码与有效文档已切到 `agents/<agent_id>/...`。
- [x] `rg -n \"group_session_transcript_format\" crates docs/src/refactor` 结果为 0（template/历史说明除外）。
- [x] Config schema/validate 的正式口径已切到 `tools.exec.sandbox.scope_key=session_id|session_key`，legacy `tools.exec.sandbox.scope` 直接报错，不再兼容：`crates/config/src/schema.rs`、`crates/config/src/validate.rs`
- [x] 所有 sandbox router 调用点使用“同一条 key 口径”（不再读取 `_chanChatKey`，也不再从 channel_binding 推导 router_key）：`crates/gateway/src/session.rs`、`crates/tools/src/exec.rs`
- [x] `location` tool 不再读取 `_chanChatKey`/不再判断 `telegram:` 前缀；无 `_connId` 时仅用 `_sessionId` 调用 channel location 请求：`crates/tools/src/location.rs`
- [x] gateway 对“无 channel_binding / 渠道不支持定位”的情况立即返回 `NotSupported`，并记录结构化日志 `reason_code`（不等待 60s timeout）：`crates/gateway/src/server.rs`
- [x] hooks payload 不再出现 `chanChatKey/chanAccountKey`，并提供结构化 `channelTarget`（来源 `channel_binding`）：`crates/common/src/hooks.rs`、`crates/agents/src/runner.rs`
- [x] TG 适配层之外不再依赖 Telegram 私有 meta 去拼装群聊入站文本（TG-GST v1 由 adapter 产出），且跨层对象不再包含 `sender_id/sender_is_bot/transcript_format` 等字段：`crates/gateway/src/channel_events.rs`、`crates/channels/src/plugin.rs`
- [x] 跨层运行时契约不再暴露渠道投递坐标：gateway↔core 入站/turn bridge/WS/hooks/tool payload 不再使用/序列化 `ChannelReplyTarget`，统一改为 opaque `reply_target_ref`（adapter 内仍可保留 `ChannelReplyTarget` 作为 legacy binding/typing helper）：`crates/channels/src/plugin.rs`、`crates/gateway/src/channel_events.rs`
- [x] gateway/core 不再散点解析 `channel_binding -> ChannelReplyTarget`；只允许 adapter/helper 集中解析：`crates/gateway/src/server.rs`、`crates/gateway/src/chat.rs`、`crates/gateway/src/session.rs`
- [x] UI/WS payload 的 `channel` 仅包含展示字段（`type/senderName/username/messageKind/model`），不包含任何 `accountKey/chatId/threadId/messageId/senderId` 等投递细节：`crates/gateway/src/chat.rs`、`crates/gateway/src/assets/js/chat-ui.js`

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] prompt cache：同 `session_key` 不同 `session_id` → `prompt_cache_key` 不同：`crates/agents/src/providers/openai_responses.rs`
- [x] tool context：构造 tool_context 时不再注入 `_chanChatKey`，且包含 `_sessionKey`：`crates/gateway/src/chat.rs`
- [x] runtime turn bridge：不同 session 复用同一 turn_id 时，reply/status 不串线；`ChannelTurnContext` 不再持有 `chan_chat_key/ChannelReplyTarget`：`crates/gateway/src/state.rs`
- [x] WS/status payload：chat 事件、ingest-only 事件、状态卡序列化结果中不再含 `chanChatKey/ChanChatKey`：`crates/gateway/src/channel_events.rs`
- [x] hooks payload：BeforeAgentStart 等 hook 的 `sessionId/sessionKey` 语义正确：`crates/agents/src/runner.rs`
- [x] agentId 替换：gateway/tools/telegram config 运行时解析后不再出现 `persona_id` 字段；旧 `persona_id` 输入会被显式拒绝，且 UI settings/schema 显示 `agentId`：`crates/gateway/src/channel.rs`、`crates/telegram/src/config.rs`、`crates/tools/src/spawn_agent.rs`
- [x] Type4 loader：优先从 `agents/<agent_id>/...` 读取并渲染；若只存在 legacy `people/<id>/...`，则直接拒绝并记录结构化告警：`crates/config/src/loader.rs`、`crates/gateway/src/chat.rs`
- [x] Sandbox scope_key：当 `scope_key=session_id` 时 sandbox 以 `_sessionId` 分桶；当 `scope_key=session_key` 时以 `_sessionKey` 分桶：`crates/tools/src/sandbox.rs`、`crates/tools/src/exec.rs`
- [x] sandbox config/UI：UI 文案与 override 限制改为 `scope_key` 口径；legacy `tools.exec.sandbox.scope` 直接报错，冲突配置仍显式报错：`crates/config/src/validate.rs`、`crates/gateway/src/session.rs`、`crates/gateway/src/assets/js/sandbox.js`
- [x] `location` tool：当没有 `_connId` 时，不需要 `_chanChatKey` 也能发起 channel location 请求（参数仅 `_sessionId`）；当 session 未绑定 channel 时返回 `NotSupported`：`crates/tools/src/location.rs`、`crates/gateway/src/server.rs`
- [x] hooks payload：所有带 `sessionId` 的事件均使用 `channelTarget` 结构化字段（可空），且 `chanChatKey/chanAccountKey` 不再序列化：`crates/common/src/hooks.rs`、`crates/agents/src/runner.rs`
- [x] reply 投递：gateway/outbound 只转交 `reply_target_ref`，并且在“同 chat 多桶/多会话实例”下不会串线（最少用 mock adapter 断言目标一致性）：`crates/gateway/src/channel_events.rs`、`crates/telegram/src/outbound.rs`
- [x] UI channel meta：序列化出的 `channel` 对象不包含投递细节字段（结构断言/快照）：`crates/gateway/src/chat.rs`
- [x] transcript config 删除：TG config/snapshot/channel settings 不再接受或展示 `group_session_transcript_format`：`crates/telegram/src/config.rs`、`crates/telegram/src/plugin.rs`、`crates/gateway/src/channel.rs`、`crates/gateway/src/assets/js/page-channels.js`
- [x] page-settings/onboarding：UI、loader、onboarding、prompt 装配统一改成 `agent_id` / `agents/<agent_id>/...`：`crates/gateway/src/assets/js/page-settings.js`、`crates/config/src/loader.rs`、`crates/onboarding/src/service.rs`、`crates/agents/src/prompt.rs`

### Integration
- [x] TG group 多桶场景：follow-up/tool/sandbox 不再依赖 `chan_chat_key`（以 turn bridge/session_key + reply_target_ref 的单测/回归覆盖）

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若 sandbox/router 依赖外部环境难以自动化
- 手工验证步骤：
  1. 在 TG group 开启 per_sender/per_branch 产生多个桶
  2. 触发一次 tool（location/shell）并检查 tool_context 中只出现 `_sessionId/_sessionKey`
  3. 检查 prompt cache debug 面板（或 provider debug overrides）显示来源 `sessionId` 且不同会话实例不共享

## 发布与回滚（Rollout & Rollback）
- 发布策略：一次性切换 V3 主口径（`session_id/session_key`、`agent_id`、`scope_key`）；不保留 legacy `people/`、legacy sandbox `scope`、缺失 `_sessionKey` 回退等兼容尾巴。
- 回滚策略：回滚到上一版本二进制；本版本不保留 `_chanChatKey` 兼容路径。
- 上线观测：新增/复用日志关键词 `reason_code`，重点关注 `missing_session_id`、`missing_session_key_for_scope_key_session_key`、`legacy_people_dir_rejected`。
- 迁移提示：建议将本地 `data_dir/people/<id>/...` 手工迁移为 `data_dir/agents/<agent_id>/...`；当前版本不会读取旧目录。
- 迁移提示（breaking）：Telegram account config 中旧 `persona_id` 已删除；必须改为 `agent_id`，否则配置会被显式拒绝。
- 迁移提示：`tools.exec.sandbox.scope` 已删除；请改用 `tools.exec.sandbox.scope_key=session_id|session_key`。
- 说明：`tools.exec.sandbox.scope_key` 仅在“exec 处于沙盒模式”时生效；若 exec 运行在主机模式（非沙盒），该分桶配置不产生实际隔离效果。

## 实施拆分（Implementation Outline）
- Step 1: 定义并贯穿 `session_id/session_key` 两条主链数据流（gateway → agents → providers → tools/hooks）。
- Step 2: 清掉运行时旧桥：`ChannelTurnContext`、WS/status/tool payload、`channel_events`/`chat` 中的 `chan_chat_key` 与 `ChannelReplyTarget` 真值角色退出。
- Step 3: 替换 tool context `_chanChatKey` → `_sessionKey`，并更新所有 tools 读取点与测试。
- Step 4: 替换 sandbox/router key 的 `chan_chat_key` 依赖为 `session_key`，删除 `identity::format/parse_chan_chat_key` 相关代码与测试，并把旧 `scope` UI/校验/提示一并切掉。
- Step 5: 修正 hooks payload：显式携带 `session_id`（实例）并按需增加 `session_key`（跨域桥），同时统一 `channelTarget` 结构。
- Step 6: 收敛“渠道细节对 core 不透明”的跨层契约（已决策，按决策实施）：
  - reply 投递：`ChannelReplyTarget` → adapter opaque `reply_target_ref`（gateway 转交，adapter 执行）
  - `channel_binding` legacy parse 收口到 adapter/helper；gateway/core 不再散点直接解析
  - legacy 子系统：`MsgContext` 依赖的 auto-reply/routing 保持 V3 占位输入，不回退到旧桥
- Step 7: 删除 transcript format 桥接尾巴（已决策）：
  - 删除 `group_session_transcript_format` 的 config/snapshot/UI/API 暴露面
  - TG adapter 直接产出固定群聊文本；gateway/core 不再保留“最终文本格式切换器”
- Step 8: 删除 persona 口径（已决策）：
  - 全仓库字段/变量/文档从 `persona*` → `agent*`
  - Type4 模板目录从 `people/<id>/...` → `agents/<agent_id>/...`，loader 不再读取 legacy `people/`
  - 更新 UI/settings schema、工具 schema（spawn_agent 等）、TG config 字段、page-settings/onboarding、prompt 装配与 loader
- Step 9: Sandbox scope_key（已决策）：
  - 正式口径切到 `tools.exec.sandbox.scope_key=session_id|session_key`（默认 `session_id`）
  - legacy `tools.exec.sandbox.scope` 已删除，命中即报错
  - gateway/tools 一律从 `_sessionId/_sessionKey` 注入与读取，不再使用 `_chanChatKey`，也不从 channel_binding 推导 router key；若 `scope_key=session_key` 但缺少 `_sessionKey`，则直接失败并记录 `missing_session_key_for_scope_key_session_key`
- Step 10: Tools 渠道交互下放（已决策）：
  - `location`：删除 tool 内渠道判断；无 `_connId` 时仅用 `_sessionId` 走 gateway→adapter
  - gateway：基于 `session_id -> channel_binding` 判定支持/不支持，并将 TG 交互下放到 adapter/outbound
  - adapter：实现请求位置的具体交互，并保证回传按 `bucket_key` 命中 `session_id`（多桶不串线）
- Step 11: Hooks 渠道信息结构化（已决策）：
  - `crates/common/src/hooks.rs`：删除 `chanChatKey/chanAccountKey`，新增 `channelTarget` 结构（统一所有带 sessionId 的事件，字段可空）
  - `crates/agents/src/runner.rs`：改为从上游传入的“已解析 channelTarget 信息”填充 payload
  - gateway：在触发 runner 前，用 `session_id -> channel_binding -> adapter/helper 本地解析 helper` 算出 `channelTarget`（可空）并传入 runner；不得在 hook 触发链路内做网络/IPC 查询
- Step 12: Reply 投递 opaque 化（已决策）：
  - adapter 生成并解析 `reply_target_ref`（带 `v=1`），实现 reply-to/thread/topic 等投递规则
  - gateway 仅转交 `reply_target_ref`，core 不持有渠道投递字段
  - `ChannelReplyTarget` 从跨层契约退场（仅在 adapter 内部必要时保留）
- Step 13: Inbound meta 与转写职责回收（已决策）：
  - 删除跨层 `ChannelMessageMeta.telegram`（以及 `ChannelTelegramMeta/TelegramChatKind/ChannelTranscriptFormat`）
  - 群聊入站文本（TG-GST v1/现有格式）由 TG adapter 产出；gateway/core 不再拼装 TG transcript
- Step 14: 对齐实施文档：
  - `docs/src/refactor/channel-adapter-generic-interfaces.md` 明确标成 future-facing，当前 C 阶段以本单 + `channel-info-exposure-boundary.md` + `telegram-adapter-boundary.md` 为准
  - `docs/src/refactor/telegram-adapter-boundary.md` 同步删除“`group_session_transcript_format` 可继续保留为 bridge tail”的歧义表述

## 实施细化（Detailed Implementation Notes）【用于开工前对齐口径】
> 这里记录“怎么改、改到哪、出错怎么表现”的细节，避免边做边拍脑袋。

### Q5：Sandbox `scope_key`（替代旧 `scope`）
**目标（人话）**
- 你可以显式选择“沙盒跟会话实例走”还是“沙盒跟逻辑桶走”，并且全链路只存在一种口径，不再“有时用 chat 坐标、有时用 session uuid”。

**配置变更**
- 删除：`tools.exec.sandbox.scope`（原有 `session|chat|bot|global` 不再支持）
- 新增：`tools.exec.sandbox.scope_key`：
  - `"session_id"`：沙盒按 `_sessionId` 分桶（默认）
  - `"session_key"`：沙盒按 `_sessionKey` 分桶

**实现要点**
- schema/validate：
  - `crates/config/src/schema.rs`：新增字段、移除旧字段
  - `crates/config/src/validate.rs`：校验 enum 值；出现旧字段直接报错（breaking，避免静默退化）
- router key 计算：
  - gateway 注入到 tool context：必须同时注入 `_sessionId` 与 `_sessionKey`（便于 scope_key 二选一）
  - tools 读取：根据配置选择从 `_sessionId` 或 `_sessionKey` 取 sandbox key
  - 禁止：读取 `_chanChatKey`；禁止：从 `channel_binding` 推导确定性 key（否则又回到 V2 坐标）
- observability：
  - 若 scope_key 需要的上下文字段缺失（例如缺 `_sessionKey`），必须 fail-fast + 结构化日志 `reason_code`（例如 `missing_session_key_for_scope_key_session_key`），不得退化到任何其他 key。

### Q4：`location` 工具（渠道识别下放 adapter）
**目标（人话）**
- `location` 不再“猜我是不是 TG”，也不再依赖任何渠道坐标 key；它只请求“对当前 session 获取位置”，支持就做，不支持就明确告诉你不支持。

**期望的行为分支（按优先级）**
1) 有 `_connId`（Web UI）→ 浏览器定位（现状保持）
2) 没 `_connId` → 调用 `request_channel_location(_sessionId)`：
   - session 没绑定渠道（无 `channel_binding`）→ 立即返回 `NotSupported`（不等 60 秒）
   - session 绑定 TG 且支持 → TG 发起一次“请分享位置”的交互，然后等待回传（超时返回 `Timeout`）

**跨层职责（正式契约）**
- tools：
  - 只依赖 `_sessionId`（必需）与可选 `_connId`；不读取 `_chanChatKey`，也不解析 `_sessionKey` 前缀
- gateway/core：
  - 以 `session_id -> channel_binding` 做路由与判定
  - 若不支持：返回 `LocationError::NotSupported`，并记录 `reason_code = "channel_location_not_supported"`
  - 若支持：调用 adapter/outbound 发出请求，再在 core 内等待 `channel_location:<session_id>` 的 pending invoke
- telegram adapter/outbound：
  - 负责把“请求位置”的消息/按钮发到正确的 chat/thread
  - 必须保证回传 fulfillment 命中的是“这次发起请求的 `session_id`”（多桶不会串线）：
    - 适配层 inbound 处理回传时，必须按 V3 `bucket_key` 定位到正确 `session_id`
    - 禁止：fallback 到 chat-wide active session（否则 per_sender/per_branch 会串）

**测试建议（最小闭环）**
- unit：`location` tool 在缺 `_connId` 时不再需要 `_chanChatKey` 也能走到“请求 channel location”的分支（mock requester 断言入参是 `_sessionId`）。
- unit：gateway `request_channel_location` 对“无 binding”立即返回 `NotSupported`（避免等 timeout）。

### Q3：Hooks 渠道信息（保能力、去 V2 术语、避免热路径查询）
**目标（人话）**
- hooks 仍能按“TG 群/话题”等渠道坐标做审计/策略（能力不弱于现状），但不再依赖 V2 字段名，也不把渠道细节默认扩散到其它层。

**payload 结构（对外契约）**
- 保留：`sessionId`、`sessionKey`（如需要跨域桥）
- 替换：`chanChatKey/chanAccountKey` → `channelTarget`（结构化对象）

**`channelTarget` 的数据来源与边界**
- 唯一来源：`session_id -> session metadata -> channel_binding -> adapter/helper 本地解析 helper`
- 禁止：hook 触发热路径内做网络/IPC 查询（避免 hook 拖慢主链、增加失败面）
- 若无 binding：`channelTarget=null`（hook 应按 session 维度做策略）
- 字段覆盖（统一 schema，便于 shell hook 编写）：
  - `channelTarget` 必须出现在所有“带 `sessionId` 的事件”中（可空），避免“有的事件有 chat/thread、有的没有”的不确定性。
  - 现有 `MessageReceived.channel`（string，来源标签）可保留，但不得与 `channelTarget` 混用；hook 策略若要精确坐标必须读 `channelTarget`。
- 实现落点（避免 drift）：
  - 建议在 gateway 提供一个小 helper：`resolve_hook_channel_target(session_id) -> Option<ChannelTarget>`，内部只负责取 `channel_binding` 并调用 adapter/helper 解析。
  - gateway 触发的事件（Message*/AgentEnd/Compaction/Command 等）直接调用 helper 填充 payload。
  - agents runner 触发的事件（BeforeLLMCall/AfterLLMCall/ToolCall 等）禁止在 runner 内部解析或猜渠道；由 gateway 先通过 helper 算出 `channelTarget` 并作为上下文传入 runner，再由 runner 填充 payload。
- 受影响文件（初步）：
  - `crates/gateway/src/chat.rs`
  - `crates/agents/src/runner.rs`
  - `crates/tools/src/*`
  - `crates/common/src/identity.rs`
  - `crates/common/src/hooks.rs`
  - `crates/channels/src/plugin.rs`
  - `crates/common/src/types.rs`
  - `crates/auto-reply/src/*`
  - `crates/routing/src/*`

## 交叉引用（Cross References）
- V3 设计与 gap：
  - `docs/src/refactor/v3-design.md`
  - `docs/src/refactor/v3-gap.md`（背景参考；若与本单冲突，以本单和专项边界文档为准）
- 渠道信息暴露边界（实施级）：
  - `docs/src/refactor/channel-info-exposure-boundary.md`
- Telegram 适配层专项边界：
  - `docs/src/refactor/telegram-adapter-boundary.md`
- 通用接口草案（future-facing，若冲突以本单与专项边界文档为准）：
  - `docs/src/refactor/channel-adapter-generic-interfaces.md`
- 实施前补缺 / 可信性收口：
  - `issues/issue-v3-one-cut-readiness-gaps.md`
- 已完成的 TG/core 边界收敛（前置）：
  - `issues/issue-v3-c-telegram-core-boundary-and-context-bridge.md`

## 未决问题（Open Questions）
> 这些是“渠道细节是否应对 core 不透明”的硬决策点；你确认选项后再落实现/删改。

（当前无；后续若拆分子 issue，在此登记）

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（breaking）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

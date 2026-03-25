# Issue: Session Key / Bucket Key 一刀切治理主单（runtime / sandbox / telegram canonical）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-25
- Owners: TBD
- Components: gateway/telegram/tools/sessions/channels/agents/ui/common/config/docs
- Affected providers/models: all（prompt cache / worktree / sandbox / tool context / channel runtime）

**已实现（如有，必须逐条写日期）**
- 2026-03-24：已将本专项设计真源移入 refactor 文档树，并注册到站内目录：`docs/src/refactor/session-key-bucket-key-one-cut.md:1`
- 2026-03-24：已冻结“新真源优先级”口径：旧 refactor 文档与旧 V3 issue 降为历史背景，不再与主设计并列定义规则：`docs/src/refactor/session-key-bucket-key-one-cut.md:10`
- 2026-03-25：已补充 `prompt_cache_key` 南向缓存桶合同，并冻结 Agent / system heartbeat / non-Agent explicit bucket 三类口径：`docs/src/refactor/session-key-bucket-key-one-cut.md:1613`
- 2026-03-25：已补充持久化/运行时合同冻结目标，明确 `active_sessions(session_key -> session_id)`、`sessions(session_id, session_key -> metadata)`、`session_state(session_id)` 三层真值与硬切口径：`docs/src/refactor/session-key-bucket-key-one-cut.md:245`
- 2026-03-25：已进一步冻结 `sessions.session_key`、`SessionStore(session_id)`、Web/session management 实例视角合同，以及 `metadata.json` 禁止自动导入口径：`docs/src/refactor/session-key-bucket-key-one-cut.md:283`
- 2026-03-25：已在 `crates/sessions/src/key.rs` 收口 canonical `SessionKey` builder / parser，只允许 `agent:<agent_id>:<bucket_key>` 与 `system:cron:<bucket_key>`，并补齐拒绝旧 shape 的单元测试
- 2026-03-25：已在 `crates/agents/src/model.rs` / `crates/agents/src/runner.rs` / `crates/agents/src/providers/openai_responses.rs` 建立 caller-owned `prompt_cache_key` 合同；provider 不再伪造 `moltis:*:no-session`
- 2026-03-25：已将 provider setup / model probe / model test / TTS probe 改为不再伪造语义型 `session_id`，仅传递 `run_id` / 显式缺省态：`crates/gateway/src/provider_setup.rs`、`crates/gateway/src/chat.rs`、`crates/gateway/src/methods.rs`
- 2026-03-25：已将 Telegram canonical atom / bucket grammar 收口到 `crates/telegram/src/adapter.rs`，并把 gateway channel boundary 改为“adapter `bucket_key` 与系统 `session_key` 分层”
- 2026-03-25：已将 `ChannelInboundContext.session_key` 硬切更名为 `bucket_key`，并在 `crates/gateway/src/channel_events.rs` 按 Telegram account → `agent_id` 构造 runtime `session_key`
- 2026-03-25：已修正 Telegram follow-up / callback / edited-location / outbound typing-loop 相关链路，使 canonical bucket、root lineage 与非阻塞 typing 行为在当前关键路径成立
- 2026-03-25：已硬切 gateway/web 会话合同：新增 `sessions.home` / `sessions.create`，前端不再本地生成 `session:uuid`，首屏/回落/删除后回落都不再伪造 `"main"`，展示与权限只消费服务端 `displayName` / `sessionKind` / `can*` 字段：`crates/gateway/src/session.rs`、`crates/gateway/src/methods.rs`、`crates/gateway/src/assets/js/sessions.js`
- 2026-03-25：已硬切 chat/runtime/tool 合同：`send` / `send_sync` / `exec` / `process` / `sandbox_packages` / `spawn_agent` 缺失 `_sessionId` 时直接拒绝，不再默认回退 `"main"`：`crates/gateway/src/chat.rs`、`crates/tools/src/exec.rs`、`crates/tools/src/process.rs`、`crates/tools/src/sandbox_packages.rs`、`crates/tools/src/spawn_agent.rs`
- 2026-03-25：已硬切 sandbox / cron / debug 合同：`system:cron:<bucket_key>` + opaque `session_id` 生效，debug 同时展示 `effectiveSandboxKey` / `containerName`，sandbox runtime naming 固定为 `msb-<readable-slice>-<short-hash>`：`crates/gateway/src/server.rs`、`crates/gateway/src/chat.rs`、`crates/tools/src/sandbox.rs`
- 2026-03-25：已移除用户配置层 `tools.exec.sandbox.container_prefix`，配置模板/校验/运行时口径收敛为单一路径：`crates/config/src/schema.rs`、`crates/config/src/template.rs`、`crates/config/src/validate.rs`
- 2026-03-25：已将启动期 legacy `metadata.json` 改为直接拒绝，不再自动导入：`crates/gateway/src/server.rs`
- 2026-03-25：已补齐 channel `/new` 主路径，新的 channel session 实例只会创建 `sess_<opaque>`，不再本地生成 `session:<uuid>`：`crates/gateway/src/channel_events.rs`
- 2026-03-25：已移除 channel reply delivery 对 `session_key.starts_with(\"telegram:\")` 的 legacy 前缀依赖，交付日志改为实例视角：`crates/gateway/src/chat.rs`
- 2026-03-25：已恢复运行时对 legacy `tools.exec.sandbox.container_prefix` 的硬拒绝，并补齐单测，避免未显式跑 `config check` 时被静默吞掉：`crates/tools/src/sandbox.rs`
- 2026-03-25：已修补收尾 review 发现的两处 runtime regression：`sessions.switch` 改为先 `resolve` 成功再写 `active_sessions/active_projects`；channel-bound `send/send_sync` 与 web echo 的 tool/runtime `_sessionKey` 全部只取 session metadata 中的 canonical `session_key`，不再泄漏 Telegram `bucket_key`：`crates/gateway/src/methods.rs`、`crates/gateway/src/chat.rs`
- 2026-03-25：已修复 clean install 启动阻断：`crates/sessions/migrations/20240205100001_init.sql` 移除被后续迁移重复引入的 branch / mcp / preview / last_seen / version 列，fresh DB 现在能顺序跑完整套 sessions migrations：`crates/sessions/migrations/20240205100001_init.sql`、`crates/sessions/src/lib.rs`

**已覆盖测试（如有）**
- `cargo test -p moltis-sessions key::tests:: -- --nocapture`
- `cargo test -p moltis-sessions --lib run_migrations_succeeds_on_fresh_database_with_branch_columns_present_once -- --nocapture`
- `cargo test -p moltis-sessions --lib -- --nocapture`
- `cargo test -p moltis-agents providers::openai_responses::tests:: -- --nocapture`
- `cargo test -p moltis-agents runner::tests::before_llm_call_hook_payload_includes_channel_keys -- --nocapture`
- `cargo test -p moltis-telegram --lib`
- `cargo test -p moltis-gateway channel_events::tests::resolve_channel_bridge_session_builds_agent_scoped_session_key -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::dispatch_to_chat_run_is_not_blocked_by_slow_typing_request -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::ingest_only_persists_text_as_is -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::dispatch_to_chat_does_not_format_text_in_core -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::bucket_session_mapping_takes_precedence_over_active_session -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::resolve_channel_session_id_rejects_legacy_active_session_for_matching_bucket -- --nocapture`
- `cargo test -p moltis-gateway channel_events::tests::resolve_channel_session_id_does_not_reuse_active_session_from_other_bucket -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::run_streaming_passes_session_key_via_llm_request_context -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::run_with_tools_passes_session_key_via_llm_request_context -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::run_with_tools_emits_message_and_tool_persist_hooks -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::resolve_telegram_session_id_rejects_legacy_active_session_for_matching_bucket -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::resolve_telegram_session_id_does_not_reuse_active_session_from_other_bucket -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::web_channel_echo_uses_session_binding_even_when_another_bucket_is_active -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::send_sync_channel_bound_tool_calls_use_canonical_runtime_session_key -- --nocapture`
- `cargo test -p moltis-gateway chat::tests::ensure_channel_bound_session_rejects_existing_binding_without_bucket_route -- --nocapture`
- `cargo test -p moltis-gateway methods::tests::sessions_switch_does_not_poison_active_session_when_resolve_fails -- --nocapture`
- `cargo test -p moltis-config --lib -- --nocapture`
- `cargo test -p moltis-tools --lib -- --nocapture`
- `cargo test -p moltis-gateway --lib -- --nocapture`
- `timeout 15s env HOME=<tmp> XDG_CONFIG_HOME=<tmp> RUSTUP_HOME=<current> CARGO_HOME=<current> cargo run --bin moltis -- --port 3000 --bind 127.0.0.1`（验证 clean startup 已越过 sessions migrations 并成功监听）

**已知差异/后续优化（非阻塞）**
- 旧 issue 保留为历史证据与交叉引用，但不再参与本主题规范定义
- `crates/common/src/types.rs`、`crates/routing/src/resolve.rs`、`crates/auto-reply/src/reply.rs` 若未来触达，必须继续对齐本单 canonical 口径；本次不扩 scope

---

## 背景（Background）
> 说明：下方“问题陈述 / 现状核查 / 根因分析”主要保留为本单开工前的冻结审计快照与设计依据。  
> 当前实现结论、测试证据与关单口径，以上方 `Status` / “已实现” / “已覆盖测试” / Close Checklist 为准。

- 场景：当前代码库对 `bucket_key`、`session_key`、`session_id`、`run_id` 的职责划分仍未彻底硬化；gateway、telegram adapter、sandbox、cron、probe 代码路径各自长出了半套命名和复用规则。
- 约束：
  - `docs/src/refactor/session-key-bucket-key-one-cut.md` 是本专项唯一设计事实来源。
  - 本单按 strict one-cut 执行：不做 alias、不做 fallback、不做自动迁移、不做静默兼容。
  - Telegram 侧 canonical 对象族必须收敛到最小集合，不允许继续新增 `tgacct.*` 这类重复原子。
  - sandbox、prompt cache、worktree、hooks、tool context、channel runtime 都必须服从同一层级模型，不能各自解释 key。
- Out of scope：
  - `session_event` 最终持久化替换与历史迁移
  - Telegram 账号配置来源统一（DB vs config）专项
  - skills / sandbox mount / bot config 等其他独立治理主题
  - 其他渠道各自的 bucket grammar 设计

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`bucket_key`**（主称呼）：桶语义拥有者产出的本地逻辑分桶键。
  - Why：回答“这条输入属于哪个逻辑桶”。
  - Not：不是 `session_key`，不是 `session_id`，不是 `run_id`。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：adapter subkey / bucket

- **`session_key`**（主称呼）：全系统唯一的逻辑会话桶名。
  - Why：用于 active-session truth、逻辑桶级 sandbox、bucket-scoped policy / routing 的桶级命名。
  - Not：不是会话实例 id；不得承载一次执行实例。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：logical session bucket name

- **`session_id`**（主称呼）：会话实例 id。
  - Why：用于实例级字段（如 `LlmRequestContext.session_id`）、实例级历史/metadata、worktree、branching、`session_state`、实例级 sandbox。
  - Not：不得承载 `main` / `cron:*` / `provider_setup:*` / `tts.generate_phrase:*` 这类业务语义字符串。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：session instance id

- **`run_id`**（主称呼）：一次执行实例 id。
  - Why：用于 transient run、stream state、run-scoped metrics。
  - Not：不是 session，不得冒充 `session_id`。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：execution id

- **`scope_key`**（主称呼）：sandbox 复用边界选择器。
  - Why：决定 sandbox 是按 `session_id` 还是按 `session_key` 复用。
  - Not：不是新的会话概念；只是消费 canonical key 的策略位。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：sandbox scope key

- **`prompt_cache_key`**（主称呼）：provider 南向 prompt cache 的缓存桶名。
  - Why：决定一次 LLM 请求与哪类请求共享 provider cache。
  - Not：不是 `session_key`，不是 `session_id`，不是 `run_id`，不是 active-session truth。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：provider cache bucket

- **Telegram canonical 对象族**（主称呼）：Telegram 适配层允许进入 canonical key 的最小对象集合。
  - Why：防止 TG 适配层继续膨胀重复前缀和重复对象类型。
  - Not：不是任意 Telegram 字段拼出来的字符串族。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：TG atoms
  - 冻结集合：
    - `person.<person_id>`
    - `tguser.<telegram_user_id>`
    - `tgchat.<chat_atom>`
    - `topic.<topic_id>`
    - `reply.<message_id>`

- **authoritative**：由系统创建的真实 id、真实持久化记录或上游权威返回。
- **effective**：按当前设计规则求值后的生效口径。

## 需求与目标（Requirements & Goals）
### 历史开工目标（Functional / frozen pre-implementation snapshot）
> 说明：本节保留开工前冻结目标，供审计“原始问题面 / 目标面 / 实现覆盖面”之用。  
> 已完成项必须同步勾选；未勾选项表示当前仍未落地或未补足证据，不能再用 `DONE` 口径掩盖。
- [x] 将 `bucket_key / session_key / session_id / run_id` 的分层、命名和消费规则，在代码里一刀切对齐到 `docs/src/refactor/session-key-bucket-key-one-cut.md`
- [x] 治理所有语义型 `session_id` 用法：`main`、`cron:*`、`probe:*`、`models.test:*`、`provider_setup:*`、`tts.generate_phrase:*` 等全部退场
- [x] 治理系统层 `session_key`：只允许 `agent:<agent_id>:<bucket_key>` 与 `system:<service_id>:<bucket_key>`
- [x] 冻结当前 `system` 命名空间：本轮只允许 `service_id = cron`，新增其他 `service_id` 必须另开单
- [x] 治理 channel boundary 合同：adapter raw `bucket_key` 不得再以 `session_key` 字段名跨层传递
- [x] 治理 runtime/tool fallback：缺失 `session_id` / `session_key` 时不得隐式回退到 `main`
- [x] 治理 active-session truth：`session_key -> active session_id` 成为唯一真值，`channel_sessions` / `session_buckets` 退出系统真值路径
- [x] 治理实例级命名债：`session_state` / `parent_session_key` 等实例语义字段、列、参数硬切到 `session_id`
- [x] 治理 transient probe：model probe / model stream test / provider setup / tts 只保留 `run_id`，不得再伪造会话
- [x] 治理 sandbox 对 key 的应用：`scope_key` 只消费 canonical `session_id/session_key`，不得再自行拼装、回退或猜测
- [x] 治理 prompt cache：`prompt_cache_key` 必须改为调用方拥有；Agent 会话默认取 `session_id`，稳定 system lane 默认取 `session_key`，其他非 Agent 路径要么显式提供稳定 bucket、要么直接省略；provider 不得伪造 `no-session` bucket
- [x] 治理 Telegram adapter 的 canonical 命名：统一 `tguser.*` / `tgchat.*` / `person.*` / `topic.*` / `reply.*`，切掉重复前缀和旧 `:` grammar
- [x] 治理 Telegram binding helper：不得再用 `session_key_from_binding` 这类误导性命名返回 raw `bucket_key`
- [x] 治理 Telegram identity link、branch 判定、legacy reject、degrade log，全部收口到单一规则和固定 `reason_code`
- [x] 治理 key 应用链路：prompt cache、worktree、hooks、tool context、channel runtime、active session truth、sandbox 全部按层使用正确 key
- [x] 建立单点 owner：系统层 key builder / validator 收口到 `crates/sessions/src/key.rs`；Telegram atom / bucket builder 只能在 Telegram adapter 内部

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：一个概念只能有一个名字；一个名字只能指一个概念
  - 必须：同一规则只能有一个 owner 实现点
  - 必须：所有 key 消费方只消费自己那一层该看的 key
  - 不得：新增 `tgacct.*`、`channel.*`、execution-only session id、语义型 session id 等重复或漂移概念
  - 不得：对重命名后的持久化字段/列保留 serde alias、SQL fallback、silent ignore
  - 不得：为 legacy shape 保留 alias、compat shim、silent degrade
- 兼容性：本单是 breaking one-cut；旧 shape 直接拒绝，不做迁移桥
- 可观测性：legacy reject、identity conflict、branch/sender degrade、missing session key 必须结构化日志 + 固定 `reason_code`
- 可观测性：当 prompt cache 已启用、但因“无 canonical 默认值且调用方未显式提供”而被省略时，必须有结构化日志，禁止 silent omission
- 安全与隐私：日志不得打印 token、secret、完整消息正文；正文最多允许短预览或摘要

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) persistence 主模型还没有硬切成 `active_sessions(session_key -> session_id)` + `sessions(session_id, session_key -> metadata)`，实例层与逻辑层仍混在一起。
2) gateway/web runtime 仍把 `session_key` 当 `session_id` 用，history / metadata / hooks 没有彻底切到实例视角。
3) `SessionStore` / media / search 仍以 legacy “key”语义工作，并使用 `:` ↔ `_` 文件名伪编码，会直接破坏 `sess_<opaque>`。
4) `session_state` / `parent_session_key` / branch parent 这类实例语义命名仍未硬切到 `session_id`。
5) tools/runtime 仍有多处在缺失上下文时默认回退到 `"main"`，会把缺失态伪装成合法实例态。
6) Web/session management 仍本地生成 `session:uuid`，并依赖 `main` / `telegram:*` / `cron:*` 这类 legacy 字面规则。
7) hooks / observability 仍缺少从 session metadata 直接读取 canonical `session_key` 的能力，当前还在靠 Telegram `channel_binding` 反推。
8) 启动期仍保留 `metadata.json` 自动导入尾巴，与 hard-cut / no-migration 原则冲突。
9) Web 启动 / 空本地态 / 删除后回落仍缺服务端 owner 的“默认主会话”合同，多个前端入口仍直接伪造 `"main"`。
10) sandbox runtime artifact name 仍基于 lossy sanitize 与字面 `"main"` 判定：容器名 / `.sandbox_views` 目录名有碰撞风险，`non-main` 语义也会在 opaque `session_id` 下失真。
11) cron 持久路径仍在生成 `cron:*` 语义型 `session_id`，与 `system:cron:<bucket_key> + opaque session_id` 的冻结模型冲突。

### 影响（Impact）
- 用户体验：
  - sandbox 复用边界、prompt cache、worktree、group collaboration 容易出现错桶、串桶、误隔离
  - TG DM / Group 的会话归属、identity 命中、branch 归属容易出现不可解释行为
- 可靠性：
  - 任何一个消费者继续把 `session_id` 当语义 key，都会污染整个 runtime
  - Telegram adapter 如果继续输出旧 grammar，会让系统层和适配层长期双轨
- 排障成本：
  - 当前同一问题要同时追 gateway、telegram、tools/sandbox、cron/probe 多处逻辑
  - 没有单一主 issue 时，review 会在历史 issue 之间来回跳

### 复现步骤（Reproduction）
1. 观察 persistence 层：`crates/sessions/src/metadata.rs` 仍保留 `SessionEntry.key/id`、`parent_session_key`、`channel_sessions`、`session_buckets`
2. 观察 history/store 层：`crates/sessions/src/store.rs` 仍按“key”读写，并用 `:` ↔ `_` 做文件名反解
3. 观察 runtime：`crates/gateway/src/chat.rs` / `crates/gateway/src/session.rs` 仍按 `session_key` 读 history、写 hooks、管理会话
4. 观察 tools：`crates/tools/src/spawn_agent.rs` / `crates/tools/src/exec.rs` / `crates/tools/src/process.rs` / `crates/tools/src/sandbox_packages.rs` 仍默认回退 `"main"`
5. 观察 Web UI：`crates/gateway/src/assets/js/sessions.js` / `session-header.js` / `session-list.js` 仍本地生成 `session:uuid` 并硬编码旧前缀
6. 期望 vs 实际：
   - 期望：实例层、逻辑桶层、前后台合同、持久化存储全部按单一模型收口
   - 实际：当前仍有多处沿用 legacy key-as-id 假设

## 现状核查与证据（As-is / Evidence）【不可省略】
> 本节只记录当前仍需治理的现状证据；历史治理成绩不在这里分散展开，统一放到 Cross References。

- 代码证据：
  - `crates/sessions/src/metadata.rs:15`：`SessionEntry` 仍保留 `key` + `id` 双 id 模型
  - `crates/sessions/src/metadata.rs:39`：`parent_session_key` 仍以 key 命名承载实例父子关系
  - `crates/sessions/src/metadata.rs:555`：`channel_sessions(channel_type, account_handle, chat_id) -> session_id` 仍在承担 active-session truth
  - `crates/sessions/src/metadata.rs:598`：`session_buckets(channel_type, bucket_key) -> session_id` 仍在承担 active-session truth
  - `crates/sessions/src/store.rs:16`：`SearchResult` 仍暴露 `session_key`
  - `crates/sessions/src/store.rs:80`：`SessionStore::key_to_filename()` 仍做 `:` → `_`
  - `crates/sessions/src/store.rs:313`：search 路径仍把 `_` 全量反解成 `:`
  - `crates/gateway/src/chat.rs:2299` / `crates/gateway/src/chat.rs:2345` / `crates/gateway/src/chat.rs:2613`：chat runtime 仍按 `session_key` 读 metadata / history / append
  - `crates/gateway/src/chat.rs:346`：`session_key_from_session_entry()` 目前只能从 Telegram `channel_binding` 反推出 bucket，说明 metadata 还没显式持久化 `session_key`
  - `crates/gateway/src/chat.rs:6502`：Telegram reply cleanup 仍依赖 `session_key.starts_with("telegram:")`
  - `crates/gateway/src/session.rs:172`：session list 仍会隐式补 `main`
  - `crates/gateway/src/session.rs:446`：delete 仍硬编码 `cannot delete the main session`
  - `crates/gateway/src/session.rs:629`：fork 仍本地生成 `session:<uuid>`
  - `crates/gateway/src/session.rs:760`：clear_all 仍按 `main` / `telegram:*` / `cron:*` 字面判断
  - `crates/sessions/src/state_store.rs:1`：`SessionStateStore` 文档和 SQL 列名仍写 `session_key`
  - `crates/tools/src/session_state.rs:67`：`session_state` 工具实际消费的是 `_sessionId`
  - `crates/tools/src/branch_session.rs:48`：branching 逻辑实际把 `_sessionId` 当 parent id 使用
  - `crates/tools/src/branch_session.rs:79`：branch tool 仍本地生成 `session:<uuid>`
  - `crates/tools/src/spawn_agent.rs:208`：sub-agent 缺 `_sessionId` 时仍默认回退到 `"main"`
  - `crates/tools/src/exec.rs:273`：exec 工具缺 `_sessionId` 时仍默认回退到 `"main"`
  - `crates/tools/src/process.rs:426`：process 工具缺 `_sessionId` 时仍默认回退到 `"main"`
  - `crates/tools/src/sandbox_packages.rs:482`：sandbox_packages 缺 `_sessionId` 时仍默认回退到 `"main"`
  - `crates/gateway/src/chat.rs:3134`：gateway runtime 路径缺 `_sessionId` 时仍默认回退到 `"main"`
  - `crates/agents/src/runner.rs:729`：hook/tool context 仍依赖 `_sessionId` / `_sessionKey` 注入，说明工具上下文链路必须一起治理
  - `crates/gateway/src/assets/js/sessions.js:171`：前端“新会话”仍本地生成 `session:${crypto.randomUUID()}`
  - `crates/gateway/src/assets/js/sessions.js:195`：clear_all 仍按 `main` / `cron:` / `telegram:` 旧前缀判断
  - `crates/gateway/src/assets/js/components/session-header.js:18`：session header 的 fallback 仍返回 `main`
  - `crates/gateway/src/assets/js/components/session-list.js:27`：session list icon 仍按 `telegram:` / `cron:` 前缀判断
  - `crates/gateway/src/assets/js/app.js:43`：root redirect 仍在本地缺省时回退 `"main"`
  - `crates/gateway/src/assets/js/page-chat.js:1093`：chat page 入口仍在 URL 缺失时回退 `"main"`
  - `crates/gateway/src/assets/js/state.js:10`：全局 active session state 初始化仍默认 `"main"`
  - `crates/gateway/src/assets/js/stores/session-store.js:117`：session store reactive state 初始化仍默认 `"main"`
  - `crates/gateway/src/assets/js/onboarding-view.js:76`：onboarding redirect 仍默认 `"main"`
  - `crates/tools/src/sandbox.rs:2280` / `crates/tools/src/sandbox.rs:2407`：sandbox runtime id 仍由 `sanitize_sandbox_key()` 派生，属于 lossy runtime naming
  - `crates/tools/src/sandbox.rs:2388`：`SandboxMode::NonMain` 仍以 `session_id != "main"` 判定
  - `crates/gateway/src/chat.rs:3827`：debug/UI 目前只展示派生后的 `containerName`，未并列暴露 canonical `effectiveSandboxKey`
  - `crates/gateway/src/server.rs:1424`：cron agent turn 仍生成 `session_id = cron:{name}` 或 `cron:{uuid}`
  - `crates/gateway/src/server.rs:1314`：启动时仍会自动把 `metadata.json` 导入 SQLite
- 配置/协议证据（必要时）：
  - `docs/src/refactor/session-key-bucket-key-one-cut.md:1`：新规范已经冻结，但代码尚未对齐
- 当前测试覆盖：
  - 已有：
    - `crates/tools/src/sandbox.rs` 已覆盖 `scope_key=session_key` 缺失时拒绝
    - `crates/sessions/src/key.rs`、`crates/agents/src/providers/openai_responses.rs`、`crates/telegram/src/adapter.rs`、`crates/gateway/src/channel_events.rs` 已覆盖本轮已落地的 key / prompt cache / TG canonical 关键路径
  - 缺口：
    - 缺 persistence owner 合同回归（`active_sessions` / `sessions(session_key)` / `SessionStore(session_id)`）
    - 缺 Web/session management 硬切回归（`sessions.create`、不再本地生成 `session:uuid`、不再依赖 `main` / `telegram:` / `cron:`）
    - 缺启动期 legacy `metadata.json` reject 回归
    - 缺 chat runtime / hooks / compaction 全链路实例视角回归

## 根因分析（Root Cause）
- A. 历史上没有先把 key 分层钉死，导致系统层、适配层、执行层都拿字符串各自编码语义。
- B. 历史 issue 虽然各自解决了一部分问题，但没有形成“最新治理主单 + 唯一设计真源”的执行面。
- C. TG adapter 的 bucket grammar、identity link、branch 判定、sender/peer/account 命名，长期处在“代码先长、规则后补”的状态。
- D. sandbox、prompt cache、worktree、tool context 都是 key 的消费者；只要上游 key 模型没硬切，消费方再严也会继续继承错误。
- E. active-session truth 没有退到单一 `session_key -> active session_id`，导致 legacy 映射表一直在和新设计并存。
- F. 一批实例级组件历史上用 `session_key` 命名字段/列，但运行时却喂 `session_id`，形成“名字和语义相反”的持续污染。
- G. 历史上为了“先跑起来”，大量 session-scoped 消费者在缺上下文时直接补 `"main"`；这会把缺失态伪装成合法实例态。
- H. Telegram 旧 grammar 不只体现在 builder；还有 helper 命名和 handlers 里的 grammar 反解析，若只改生成侧会留下半套旧语义消费者。
- I. prompt cache 的 owner 边界从来没钉死：调用方没有显式 `prompt_cache_key` 槽位，provider 就开始代替上游猜 bucket，最后长出了 `no-session` fallback。
- J. 持久化层没有冻结成单一实例模型：`SessionEntry.key/id` 双写、`parent_session_key`、`session_state.session_key`、`channel_sessions` / `session_buckets` 并存，导致 gateway/runtime/tool 即便局部改名，也会被底层旧模型重新污染。
- K. 会话实例存储与管理面合同也还没切干净：`SessionStore` 仍把实例 id 当“key”存文件，Web UI 仍本地生成 `session:uuid` / 识别 `main` / `telegram:*` / `cron:*`，说明前后台仍共享一套 legacy 主标识假设。
- L. Web 管理面没有被服务端明确下发“home session / display / capability”合同，所以前端只能继续猜 `"main"`、猜前缀、猜能否 rename/delete/fork。
- M. sandbox 事实源与 runtime artifact name 没有被分开：当前直接把 sanitize 后的 key 当容器名 / view dir 名，会把“可运行的派生名”错误混成“可回推的真值”，还会留下碰撞风险。
- N. `non-main` 其实是逻辑桶语义，但实现历史上把它偷懒写成了 `session_id == "main"`，这在 opaque `session_id` 下必然失效。
- O. cron 持久 lane 还在偷用 `cron:*` 语义型 `session_id`，说明系统会话那条主路径也还没完全切到“`session_key` 承载语义、`session_id` 保持 opaque”。
- P. sandbox runtime artifact name 仍暴露 `container_prefix` 这类可配置噪音；如果不一并切掉，`msb-...` 命名合同就不是确定规则，而只是“默认值”。

## 剩余历史债专项 Review 结论（2026-03-25）
> 本节是本轮“继续实施前”的冻结审查结论。目的不是扩 scope，而是划清剩余边界，避免继续边改边返工。

### A. gateway / web runtime 仍是剩余主战场
- `crates/gateway/src/chat.rs:1927`：`session_key_for()` 仍默认返回 `"main"`，说明 web runtime 还没完成“缺失态不得伪装”为 canonical 口径
- `crates/gateway/src/chat.rs:3137`：`send_sync()` 仍把 `_sessionId` 读到一个名叫 `session_key` 的变量里，并默认 `"main"`
- `crates/gateway/src/chat.rs:3697`、`crates/gateway/src/chat.rs:5950`、`crates/gateway/src/chat.rs:6160`：`LlmRequestContext.session_key/session_id/prompt_cache_key` 仍在混用同一 legacy 变量
- `crates/gateway/src/chat.rs:6502`：仍以 `session_key.starts_with("telegram:")` 检测 Telegram，会在 canonical `agent:<agent_id>:<bucket_key>` 下失真
- `crates/gateway/src/session.rs:172`、`crates/gateway/src/session.rs:760`：session service / clear_all 仍按 `main`、`telegram:*`、`cron:*` 这类旧字面做保留逻辑，说明管理面也还没切到 canonical 分层
- `crates/gateway/src/assets/js/app.js:43`、`crates/gateway/src/assets/js/page-chat.js:1093`、`crates/gateway/src/assets/js/state.js:10`、`crates/gateway/src/assets/js/stores/session-store.js:117`、`crates/gateway/src/assets/js/onboarding-view.js:76`：Web boot / redirect / reactive state 仍把 `"main"` 当默认实例 id，说明当前 issue 还缺“服务端 owner 的 home session 合同”
- `crates/gateway/src/assets/js/components/session-header.js:31`、`crates/gateway/src/assets/js/components/session-list.js:30`：header / list 仍靠 `sessionId` 前缀和 fallback 做展示与 capability 判断，说明还缺服务端显式 `displayName` / `sessionKind` / capability 合同

### B. tool consumer 仍保留 session-required fallback
- `crates/tools/src/spawn_agent.rs:206`
- `crates/tools/src/exec.rs:271`
- `crates/tools/src/process.rs:424`
- `crates/tools/src/sandbox_packages.rs:480`

以上路径都还在缺 `_sessionId` 时回退到 `"main"`。  
这不是“小尾巴”，而是直接把缺失态伪装成合法实例态，必须硬切。

### C. 持久化 / schema 才是下一阶段的真正 blocker
- `crates/sessions/src/metadata.rs:15`：`SessionEntry` 仍保留 `key` + `id` 双 id 模型
- `crates/sessions/src/metadata.rs:39`：`parent_session_key` 仍以 key 命名承载实例父子关系
- `crates/sessions/src/metadata.rs:555` / `crates/sessions/src/metadata.rs:598`：`channel_sessions` / `session_buckets` 仍在承担 active-session truth
- `crates/sessions/src/state_store.rs:1`、`crates/tools/src/session_state.rs:67`：实例级 state 仍以 `session_key` 命名
- `crates/tools/src/branch_session.rs:48`：branching 仍把 `_sessionId` 装进 `parent_key` / `new_key = session:*` 这套旧命名里

结论：

- 在这层没有先冻结成 `session_key -> active session_id` / `session_id -> metadata` 之前，
  继续零散改 gateway / tools，只会继续返工
- 所以下一阶段 broad cut 必须先从持久化与 owner API 合同开始，而不是继续补外围 if/else

### D. 仍有少量 legacy placeholder / 兼容尾巴，但不在当前关键路径
- `crates/common/src/types.rs:35`
- `crates/routing/src/resolve.rs:1`
- `crates/auto-reply/src/reply.rs:1`

这些文件还保留旧 V3 placeholder 语义，但当前不决定本专项 runtime 主路径。  
本单要求是：它们不能再定义 canonical 规则；若后续 implementation 触达，必须同步对齐或直接删除。

### E. 还有两处必须记账的管理面/兼容债
- `crates/gateway/src/channel_events.rs:124` / `crates/gateway/src/channel_events.rs:143`：channel runtime 仍会从 `bucket_session` / `active_session(chat)` 两套 legacy 真值做回填，这一段必须在持久化主键治理后一起收口
- `crates/sessions/src/metadata.rs:646`：`list_channel_sessions()` 的注释仍残留 legacy `account_id` 口径，说明 metadata 周边仍有历史表述漂移，需要在 schema 硬切时一起清理

### F. `SessionStore` / hooks / Web UI 仍共享 legacy key-as-id 假设
- `crates/sessions/src/store.rs:80` / `crates/sessions/src/store.rs:296`：`SessionStore` 与 `SearchResult` 仍以“key”命名实例历史，并使用 `:` ↔ `_` 文件名伪编码；这会直接破坏设计里的 `sess_<opaque>`
- `crates/gateway/src/chat.rs:2345` / `crates/gateway/src/chat.rs:2613` / `crates/gateway/src/chat.rs:3176`：chat runtime 仍按 `session_key` 读写 history / metadata / hooks，说明实例层还没从逻辑桶层剥离
- `crates/agents/src/silent_turn.rs:188`：silent memory turn 的参数名仍叫 `session_key`，但实际注入的是工具上下文 `_sessionId`；这条 compaction 辅助链路若不一起硬切，会继续把实例语义和逻辑桶语义搅混
- `crates/gateway/src/chat.rs:346`：`session_key_from_session_entry()` 目前只能从 Telegram `channel_binding` 反推出 bucket；Web/system session 并没有被显式持久化 `session_key`
- `crates/gateway/src/session.rs:629` / `crates/tools/src/branch_session.rs:79`：fork/new branch 仍本地生成 `session:<uuid>`，说明实例 id 生成与旧前缀仍耦合在管理面
- `crates/gateway/src/assets/js/sessions.js:171`：前端“新会话”仍本地生成 `session:${crypto.randomUUID()}`
- `crates/gateway/src/assets/js/components/session-header.js:18`、`crates/gateway/src/assets/js/components/session-list.js:27`、`crates/gateway/src/assets/js/sessions.js:195`：前端 session management 仍硬编码 `main` / `telegram:*` / `cron:*`

结论：

- 下一阶段不能只改数据库层；必须把 `SessionStore`、hooks/session metadata、SessionService RPC、Web session management 一起纳入 persistence-first cut
- 否则后端刚切完 `session_id`，前端和搜索/媒体路径又会把实例 id 重新污染回旧 key 语义

### G. 启动期仍有 legacy 自动导入尾巴
- `crates/gateway/src/server.rs:1314`：启动时仍会自动把 `metadata.json` 导入 SQLite 并改名备份

这与本单的 hard-cut/no-migration 原则直接冲突。  
本轮必须明确：命中旧 `metadata.json` 时不得自动导入，必须直接失败并给 remediation。

### H. sandbox runtime naming / policy 也还有一刀
- `crates/tools/src/sandbox.rs:2280` / `crates/tools/src/sandbox.rs:2407`：container name 与 `.sandbox_views` 目录仍由 sanitize 后的 key 派生，尚未与 canonical `effective_sandbox_key` 分层
- `crates/tools/src/sandbox.rs:2388`：`SandboxMode::NonMain` 仍靠 `session_id == "main"`，这在 opaque `session_id` 下没有可持续性
- `crates/gateway/src/chat.rs:3827`：debug 面只展示 `containerName`，未并列展示 `effectiveSandboxKey`，排障时无法确认“同名派生物对应的真实逻辑桶”
- `crates/config/src/schema.rs:1300` / `crates/config/src/template.rs:246` / `crates/tools/src/sandbox.rs:951`：`tools.exec.sandbox.container_prefix` 仍允许用户改 runtime name 前缀；这会直接破坏本单刚冻结的 `msb-<readable-slice>-<short-hash>` 命名确定性

结论：

- sandbox 不只是“吃 `session_id/session_key` 就完事”，还必须把“事实源”和“运行时派生名”彻底分开
- `container_prefix` 这种纯 artifact 命名开关也必须一起切掉，否则容器名、view dir、debug/UI 合同仍然不稳定

### I. Web home / display contract 目前仍未冻结
- `crates/gateway/src/assets/js/app.js:43`、`crates/gateway/src/assets/js/page-chat.js:1093`、`crates/gateway/src/assets/js/onboarding-view.js:76`：没有服务端 home path 时，前端只能继续伪造 `"main"`
- `crates/gateway/src/assets/js/components/session-header.js:34` / `crates/gateway/src/assets/js/components/session-list.js:31`：没有显式 display/capability 字段时，前端只能继续猜主会话 / 渠道会话 / cron 会话

结论：

- 在 persistence-first cut 之后，Web 还需要同步拿到：
  - 服务端 owner 的 home session path
  - 显式 `displayName`
  - 显式 `sessionKind`
  - 显式 capability flags
- 否则 `"main"` fallback 和前缀猜测只会从旧代码迁到新代码

### J. cron 持久 lane 仍有语义型 `session_id` 历史债
- `crates/gateway/src/server.rs:1424`：cron agent turn 仍生成 `session_id = cron:{name}` / `cron:{uuid}`

结论：

- 本单不能只把 probe / provider / Telegram 路径改干净
- cron 持久 lane 也必须一起切到：
  - `session_key = system:cron:<bucket_key>`
  - `session_id = sess_<opaque>`

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 本单不复制第二份详细 grammar。完整规范只以 `docs/src/refactor/session-key-bucket-key-one-cut.md` 为准；本节只冻结实施级硬要求。

- 必须：
  - 系统只保留 `bucket_key / session_key / session_id / run_id` 四层核心标识
  - 系统层 `session_key` 只允许：
    - `agent:<agent_id>:<bucket_key>`
    - `system:<service_id>:<bucket_key>`
  - 当前冻结的 `system` 服务集合只有：
    - `cron`
  - `bucket_key` 只允许停留在 adapter 局部；跨出 adapter 边界后必须立刻由系统层 owner 组装成 `session_key`
  - `session_key -> active session_id` 必须成为唯一 active-session truth
  - `session_id` 与 `run_id` 必须 opaque
  - `LlmRequestContext.session_id`、worktree、branching、`session_state` 这类实例级消费者必须只看 `session_id`
  - `LlmRequestContext`（或等价 provider context）必须增加显式可选 `prompt_cache_key`；provider 只能消费这个字段，不得自行发明 fallback
  - `prompt_cache_key` 的默认派生 owner 必须是 provider 边界外的调用侧请求构造者；不得把默认派生逻辑放进 provider 实现内部
  - `prompt_cache_key` 不是 core runtime key；Agent 会话默认取 `session_id`，稳定 system lane 默认取稳定 `session_key`，其他非 Agent 路径若业务上想吃 cache，必须由调用方显式提供稳定 bucket
    - 例：普通 Agent 对话默认是 `prompt_cache_key = <session_id>`
    - 例：heartbeat 默认是 `prompt_cache_key = system:cron:heartbeat`
  - provider setup / model probe / model stream test / tts 等 transient probe 必须只有 `run_id`，没有 `session_key` / `session_id`；若业务上确实需要 provider prompt cache，只允许额外显式给南向 `prompt_cache_key`
  - Web 默认主会话必须走服务端 owner path（例如 `sessions.home`），不得由前端伪造 `"main"`
  - Web session list / resolve / search 返回给 UI 的实例数据必须显式携带：
    - `displayName`
    - `sessionKind`
    - `canRename`
    - `canDelete`
    - `canFork`
    - `canClear`
  - sandbox 的事实源必须是 `effective_sandbox_key`；container name / `.sandbox_views` 目录名只能是稳定派生物，不得用 lossy sanitize 冒充真值
  - sandbox container / view dir 的可读派生格式固定为：
    - `msb-<readable-slice>-<short-hash>`
  - `readable-slice` 推荐收敛到：
    - `agent-<agent_id>-main`
    - `agent-<agent_id>-chat`
    - `agent-<agent_id>-dm`
    - `agent-<agent_id>-group-<chat_id>`
    - `system-cron-heartbeat`
    - `system-cron-job`
  - sandbox debug / observability 必须同时能看到：
    - `effectiveSandboxKey`
    - `containerName`
  - `SandboxMode::NonMain` 必须按 canonical `session_key` 是否为 `agent:<agent_id>:main` 判定，不得再看 `session_id`
  - cron 持久 lane 必须使用 `system:cron:<bucket_key>` + opaque `session_id`
  - 任何 session-scoped 消费者在缺失 `session_id` / `session_key` 时，必须按职责分流：session-required consumer 直接 reject，session-optional consumer 保持缺失；不得隐式补 `"main"`
  - 若调用方既没有 canonical 默认 derivation，也没有显式 `prompt_cache_key`，则必须直接省略；provider 不得生成 `moltis:*:no-session`
  - prompt cache 被省略时，若 prompt cache 功能处于启用态，必须留下结构化日志，至少包含 `event`、`reason_code`、`decision`
  - 持久化里的旧实例级字段/列形状（如 `parent_session_key`、`session_state.session_key`）必须直接 reject，不得 alias / 自动迁移 / silent ignore
  - Telegram canonical 对象族只允许：
    - `person.*`
    - `tguser.*`
    - `tgchat.*`
    - `topic.*`
    - `reply.*`
  - sandbox、prompt cache、worktree、hooks、tool context、active session truth 都必须按层消费正确 key
  - identity link / branch / legacy reject / degrade log 必须完全服从设计真源中冻结的合同和 `reason_code`
- 不得：
  - 不得把 Telegram raw bucket 直接当系统层最终 `session_key`
  - 不得再让 `channel_type + bucket_key` 或 `channel_type + account_handle + chat_id` 继续承担系统级 active-session truth
  - 不得把任何业务语义字符串继续塞进 `session_id`
  - 不得在任何 runtime / tool / hook 路径把缺失的 `session_id` 默认补成 `"main"`
  - 不得在 provider prompt cache 上生成 `no-session` 这类伪 session bucket
  - 不得让 provider 根据 `provider/model` 自行合成 `prompt_cache_key`
  - 不得保留 `ChannelInboundContext.session_key`、`parent_session_key`、`session_state.session_key` 这类“名字是 key、语义是 id”的实例级尾巴
  - 不得保留 `session_key_from_binding` 这类名字和实际语义不一致的 TG binding helper
  - 不得在 TG 适配层之外新增或解析 TG canonical atom grammar
  - 不得新增历史 issue 并列定义本主题规则
- 应当：
  - 系统层 key builder / validator 集中在单一 owner 模块
  - TG atom / bucket builder 继续集中在 `crates/telegram/src/adapter.rs` 或其同责任模块
  - 旧 issue 与旧文档仅作为 evidence / history / related refs

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：以 `docs/src/refactor/session-key-bucket-key-one-cut.md` 为唯一规范，建立一个最新治理主单，按“系统层 key 模型 -> key 消费方 -> Telegram adapter canonical 命名 -> legacy reject + tests”顺序集中实施。
- 优点：
  - 规则只认一个源头，review 不再在历史 issue 之间跳转
  - sandbox、runtime、TG adapter 一起对齐，不会出现“局部修好、整体继续歪”的情况
  - 便于做关键路径测试收口
- 风险/缺点：
  - breaking 范围明确，必须接受旧 shape 直接失败

#### 方案 2（拒绝）
- 核心思路：继续按模块拆散推进，分别在 cron、sandbox、TG adapter、runtime 上各自修 key
- 风险/缺点：
  - 会重新长出多份 grammar、多份 reject 口径、多份命名
  - 容易把新设计再次写散

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（唯一真源）：本主题详细规则只认 `docs/src/refactor/session-key-bucket-key-one-cut.md`
- 规则 2（唯一主单）：本单是最新治理主单；旧 issue 不再并列定义规则
- 规则 3（唯一 owner）：
  - 系统层 key builder / validator 只能有一个 owner，并收口到 `crates/sessions/src/key.rs`
  - Telegram canonical atom / bucket builder 只能在 Telegram adapter 内
- 规则 3.5（冻结的 system 服务集合）：
  - 当前只允许 `service_id = cron`
  - 若未来要新增 `system:<service_id>`，必须另开 issue 先冻结，不得在本单实现时顺手放开
- 规则 4（消费者不发明语义）：
  - sandbox / worktree / hooks / tool context / runtime 只能消费 canonical runtime key
  - provider prompt cache 只能消费调用方给出的 `prompt_cache_key`
  - 不允许任何消费者自行拼 key、改写 key、猜 key
- 规则 4.5（prompt cache 调用方拥有）：
  - `prompt_cache_key` 是调用方拥有的 provider cache bucket，不是 provider 自己发明的 runtime key
  - Agent 会话默认取 `session_id`
  - 稳定 system lane 默认取稳定 `session_key`
  - 非 Agent 且没有 canonical session 的路径，若想吃 cache，必须由调用方显式提供稳定 `prompt_cache_key`
  - transient probe 可以是 `run_id only`，但仍允许额外显式传入南向 `prompt_cache_key`
  - 若调用方既没有默认 derivation，也没有显式提供，则必须省略
  - provider 不得生成 `moltis:*:no-session` 或其他 fallback bucket
- 规则 4.6（prompt cache omission 可观测）：
  - 当 prompt cache 已启用、但因缺少 canonical 默认 derivation 且调用方未显式提供而被省略时，必须记录结构化日志
  - 固定字段至少包含：
    - `event = "provider.prompt_cache_key.omitted"`
    - `reason_code = "prompt_cache_key_missing"`
    - `decision = "omit"`
  - provider debug/source 字段只允许：
    - `session_id`
    - `session_key`
    - `explicit`
    - `omitted`
  - 不得再出现 `fallback`
- 规则 5（硬切 legacy）：
  - legacy `session_key` / `session_id` / TG bucket 直接拒绝
  - invalid PEOPLE identity link 直接检查失败 / 启动失败
- 规则 6（边界命名硬切）：
  - adapter 向 gateway 传 raw route 时，字段名必须是 `bucket_key`
  - 不得继续以 `session_key` 字段名承载 adapter raw bucket
- 规则 6.5（缺失态不得伪装成 `main`）：
  - 只有显式的 agent main bucket 选择，才允许出现 `bucket_key = main` / `session_key = agent:<agent_id>:main`
  - 任何下游 runtime / tool / hook 消费者都不得把缺失的 `session_id` 默认补成 `main`
- 规则 7（active-session truth 单点化）：
  - 系统只允许保留 `session_key -> active session_id`
  - `channel_sessions` / `session_buckets` 若短期内仍保留表结构，也只能退出主路径、仅供排障/清理，不得继续参与 active-session 判定
- 规则 8（实例语义只叫 `session_id`）：
  - `session_state`、branching parent/child、实例 metadata 字段与列，凡承载实例语义者都必须改叫 `session_id`
  - 旧 `parent_session_key` / `session_state.session_key` 命名必须硬切
  - 命中旧持久化字段/列时必须直接 reject，不得做 serde alias、列名回退或 silent ignore
- 规则 8.5（compaction helper 也按实例语义）：
  - silent memory turn / compaction helper 传给工具的上下文只能是 `session_id`
  - 不得再把实例参数命名成 `session_key`
- 规则 8.6（sandbox runtime 前缀固定）：
  - sandbox runtime artifact name 前缀固定为 `msb`
  - 旧配置 `tools.exec.sandbox.container_prefix` 必须直接 reject
- 规则 9（transient probe run-only）：
  - provider setup / model probe / model stream test / tts probe 只允许产出 `run_id`
  - 不得提升为 `system:*`，也不得伪造 execution-only session id

#### 接口与数据结构（Contracts）
- 设计真源：
  - `docs/src/refactor/session-key-bucket-key-one-cut.md`
- 文档入口：
  - `docs/src/SUMMARY.md`
- 系统层治理范围：
  - `crates/sessions/src/key.rs`
  - `crates/sessions/src/lib.rs`
  - `crates/sessions/src/metadata.rs`
  - `crates/sessions/migrations/20240205100001_init.sql`
  - `crates/sessions/migrations/20260205120000_session_state.sql`
  - `crates/sessions/migrations/20260205130000_session_branches.sql`
  - `crates/sessions/src/store.rs`
  - `crates/sessions/src/state_store.rs`
  - `crates/common/src/hooks.rs`
  - `crates/agents/src/model.rs`
  - `crates/agents/src/providers/openai_responses.rs`
  - `crates/agents/src/silent_turn.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/services.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/provider_setup.rs`
  - `crates/agents/src/runner.rs`
  - `crates/config/src/schema.rs`
  - `crates/config/src/template.rs`
  - `crates/config/src/validate.rs`
  - `crates/tools/src/sandbox.rs`
  - `crates/tools/src/exec.rs`
  - `crates/tools/src/process.rs`
  - `crates/tools/src/spawn_agent.rs`
  - `crates/tools/src/sandbox_packages.rs`
  - `crates/tools/src/session_state.rs`
  - `crates/tools/src/branch_session.rs`
  - `crates/channels/src/plugin.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/assets/js/sessions.js`
  - `crates/gateway/src/assets/js/app.js`
  - `crates/gateway/src/assets/js/page-chat.js`
  - `crates/gateway/src/assets/js/state.js`
  - `crates/gateway/src/assets/js/stores/session-store.js`
  - `crates/gateway/src/assets/js/onboarding-view.js`
  - `crates/gateway/src/assets/js/components/session-header.js`
  - `crates/gateway/src/assets/js/components/session-list.js`
- Telegram 治理范围：
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/telegram/src/outbound.rs`
- 原则：
  - 系统层 owner 只定义 system-layer key contract，并以 `crates/sessions/src/key.rs` 为唯一 builder / validator 落点
  - Telegram owner 只定义 TG atom / bucket / identity / branch contract
  - `prompt_cache_key` 默认派生逻辑只允许发生在 provider 边界外的调用侧；provider crate 只能读取、透传、记录 observability
  - Web/session management 采用实例视角：现有 `sessions.*` 管理接口只认 `session_id`
  - Web 新建会话必须走服务端 owner path；冻结新增 `sessions.create`，禁止继续让 `sessions.resolve` 同时承担“创建 + 读取”双语义

#### 开工前冻结（必须先满足）
- 冻结 1：`docs/src/refactor/session-key-bucket-key-one-cut.md` 已补齐持久化/运行时目标模型；后续实现必须严格按该模型落地，不再现场拍脑袋决定 schema
- 冻结 2：下一阶段 broad cut 先做 persistence owner 收口，再做 gateway / tools runtime 清理；禁止继续“外围先补一圈，再回来改 schema”
- 冻结 3：当前持久化目标固定为：
  - `active_sessions(session_key PRIMARY KEY, session_id, updated_at)` 负责 `session_key -> active session_id`
  - `sessions(session_id PRIMARY KEY, session_key, ...)` 负责 `session_id -> metadata`
  - `session_state(session_id, namespace, key, value, updated_at)` 负责实例级状态
  - `parent_session_id` 负责实例父子关系
- 冻结 4：`channel_sessions` / `session_buckets` 退出主路径真值；若短期保留，也只能作为排障/清理残留，不能再参与 runtime 判定
- 冻结 5：JSON/file helper、SQLite metadata、gateway runtime API，三者必须同构；禁止“测试 helper 一套语义、正式存储另一套语义”
- 冻结 6：`SessionStore` / media / search 必须全部按 `session_id` 工作；文件名与目录名直接使用原始 `session_id`，禁止 `:` ↔ `_` 伪编码
- 冻结 7：Web/session management 走实例视角；`sessions.*` 现有管理 RPC 只接受 `session_id`，客户端不得本地生成 `session_id`
- 冻结 8：启动期不得自动导入 legacy `metadata.json`
- 冻结 9：新增 `sessions.create` 作为唯一 Web 新建入口；`sessions.resolve` 硬切回“只按 `session_id` 读取”
- 冻结 10：Web 默认主会话必须走服务端 owner path（例如 `sessions.home`）；`app.js` / `page-chat.js` / `state.js` / `session-store.js` / `onboarding-view.js` 不得再伪造 `"main"`
- 冻结 11：Web 展示与交互合同固定由服务端显式下发 `displayName` / `sessionKind` / capability flags；前端不得再从 `sessionId` 或旧前缀猜
- 冻结 12：sandbox runtime artifact naming 只允许由完整 `effective_sandbox_key` 稳定派生；`containerName` / `.sandbox_views` 目录名都不得再用 lossy sanitize 充当真值
- 冻结 12.1：sandbox runtime 可读命名固定采用 `msb-<readable-slice>-<short-hash>`
  - `msb = moltis sandbox`
  - `readable-slice` 只做辅助识别，不承担真值职责
  - `short-hash` 必须基于完整 `effective_sandbox_key` 稳定生成，用于防撞
- 冻结 12.2：`tools.exec.sandbox.container_prefix` 属于本单要切掉的 legacy artifact 命名开关；配置检查与启动期都不得再接受它
- 冻结 13：`SandboxMode::NonMain` 固定按 canonical `session_key` 是否等于 `agent:<agent_id>:main` 判定，不得再读取 `session_id == "main"`
- 冻结 14：cron 持久 lane 与 heartbeat 一样，必须走 `system:cron:<bucket_key>` + opaque `session_id`

#### 失败模式与降级（Failure modes & Degrade）
- legacy `session_key`：
  - 直接 reject
- legacy `session_id`：
  - 直接 reject
- legacy Telegram bucket：
  - 直接 reject
- boundary 字段仍把 raw bucket 叫 `session_key`：
  - 直接修正，不保留双字段兼容
- missing session context 仍默认回退到 `"main"`：
  - session-required consumer 直接 reject
  - session-optional consumer 保持缺失
  - 不得 silent fallback
- prompt cache 缺少 canonical 默认 derivation，且调用方也没显式提供：
  - 直接省略 `prompt_cache_key`
  - provider 不得合成 fallback bucket
  - 若 prompt cache 功能已启用，必须记录 `provider.prompt_cache_key.omitted` / `prompt_cache_key_missing`
- legacy persisted shape（如 `parent_session_key`、`session_state.session_key`）：
  - 直接 reject
  - 启动/加载失败
- `scope_key=session_key` 且缺 `session_key`：
  - 直接 reject
- `sandbox mode=non-main` 但缺判定所需的 canonical `session_key`：
  - 直接 reject
  - 不得回退去猜 `session_id`
- 旧配置 `tools.exec.sandbox.container_prefix`：
  - 直接 reject
  - `config check` / 启动期失败
- identity link 重复：
  - `config check` 失败
  - 启动失败
- group scope 缺 sender / branch：
  - 只允许按设计真源既定 degrade 规则退化
  - 不得伪造 sender / branch

#### 安全与隐私（Security/Privacy）
- 默认日志只打结构化字段与短上下文
- 禁止打印：
  - token
  - secret
  - 完整消息正文
  - 未脱敏的外部凭据

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `docs/src/refactor/session-key-bucket-key-one-cut.md` 成为唯一规范真源，站内目录可直接访问
- [x] 本单成为最新治理主单；旧 issue 只保留历史和证据角色
- [x] 运行时不再存在语义型 `session_id`
- [x] 系统层 `session_key` 全部对齐 `agent:` / `system:` grammar
- [x] 当前 `system` 命名空间只开放 `cron`；不存在“实现时顺手支持更多 service_id”
- [x] `ChannelInboundContext` 等跨层合同不再把 raw `bucket_key` 叫成 `session_key`
- [x] runtime / tool / hook 路径缺失上下文时不再默认回退到 `"main"`
- [x] Telegram channel delivery / reply cleanup 不再依赖 `session_key.starts_with("telegram:")` 这类 legacy 前缀判断
- [x] active-session truth 只剩 `session_key -> active session_id`；`channel_sessions` / `session_buckets` 不再参与主路径
- [x] 持久化目标已落地为 `active_sessions(session_key -> session_id)` + `sessions(session_id, session_key -> metadata)`，不存在 `SessionEntry.key/id` 双 id 模型
- [x] gateway session management 路径（list / clear_all / active 标记）不再依赖 `main` / `telegram:*` / `cron:*` 这类 legacy 字面判断
- [x] Web 默认主会话改为服务端 owner path（例如 `sessions.home`）；app/chat/onboarding/state 不再伪造 `"main"`
- [x] Web session list / resolve / search 返回显式 `displayName` / `sessionKind` / capability flags；前端不再从 `sessionId` 前缀推断展示与交互
- [x] `SessionStore` / media / search 全部改为 `session_id` 视角，`sess_<opaque>` 文件名与 search 结果可无损 round-trip
- [x] Web/session management 不再本地生成 `session:uuid`，现有 `sessions.*` 管理 RPC 全部按 `session_id` 语义工作
- [x] `sessions.create` 成为唯一新建 Web 会话入口；`sessions.resolve` 只按 `session_id` 读取实例
- [x] hooks / channel observability 可从 session metadata 读取显式持久化的 `session_key`，不再只靠 Telegram `channel_binding` 反推
- [x] `session_state` / branching parent / metadata parent 等实例语义命名全部对齐 `session_id`
- [x] provider setup / model probe / model stream test / tts 等 transient probe 全部改为 `run_id only`
- [x] provider prompt cache 不再生成 `no-session` fallback bucket；Agent 会话默认取 `session_id`，稳定 system lane 默认取 `session_key`，其他非 Agent 路径只允许“显式提供”或“直接省略”
- [x] 非 Agent / transient 路径显式传入的 `prompt_cache_key` 能原样到达 provider，不会被 provider 改写、覆盖或忽略
- [x] prompt cache omission 具备固定结构化可观测性；debug/source 不再出现 `fallback`
- [x] 命中旧持久化字段/列形状时直接 reject，不存在 alias / silent ignore
- [x] sandbox 只消费 canonical `session_id/session_key`，不再承接历史 shape
- [x] sandbox runtime artifact naming 由完整 `effective_sandbox_key` 稳定、无碰撞派生；`containerName` 不再等于 lossy sanitize 结果
- [x] sandbox runtime artifact naming 采用 `msb-<readable-slice>-<short-hash>`，既可读又防撞
- [x] `tools.exec.sandbox.container_prefix` 被硬切移除/拒绝；runtime artifact naming 前缀固定为 `msb`
- [x] sandbox `non-main` 语义改按 canonical `session_key` 判定，不再依赖 `session_id == "main"`
- [x] debug/UI 可同时看到 `effectiveSandboxKey` 与 `containerName`
- [x] silent memory turn / compaction helper 的工具上下文不再混用 `session_key` / `session_id`
- [x] Telegram adapter 全部对齐 canonical atom / bucket grammar，并切掉重复前缀与旧 `:` grammar
- [x] Telegram binding helper / callback helper 不再以旧语义命名或反解析旧 grammar
- [x] identity link / branch / legacy reject / degrade log 全部对齐设计真源中的固定合同
- [x] 关键路径测试覆盖齐备，legacy reject 与核心 degrade 均有自动化回归

## 历史测试计划（Test Plan / frozen pre-implementation snapshot）【不可省略】
> 说明：本节保留开工前测试计划，供审计“计划测试面是否覆盖到最终落地面”之用。  
> 当前已落地的测试证据，以文首 `已覆盖测试` 为唯一准绳；已完成且有证据的条目必须同步勾选，未勾选项表示当前仍缺自动化证据或尚未在本轮复核中重新确认。
### Unit
- [x] 系统层 key builder / validator：允许 `agent:zhuzhu:main` / `agent:zhuzhu:dm-peer-person.neoragex2002` / `system:cron:heartbeat`，拒绝 `main` / `cron:heartbeat` / `dm:main`：`crates/sessions/src/key.rs`
- [x] active-session truth 只按 `session_key -> active session_id`：`crates/sessions/src/metadata.rs`
- [x] `sessions` metadata / active mapping API 已按 `session_id` / `session_key` 分层收口；`SessionEntry.key/id` 双写消失，metadata 显式持久化 `session_key`：`crates/sessions/src/metadata.rs`
- [ ] gateway session management 不再使用 legacy 字面前缀做 list / clear_all / active 判定：`crates/gateway/src/session.rs`
- [ ] Web home/default path 存在且唯一；前端 root/chat/onboarding/state 初始化不再回退 `"main"`：`crates/gateway/src/session.rs`、`crates/gateway/src/assets/js/app.js`、`crates/gateway/src/assets/js/page-chat.js`、`crates/gateway/src/assets/js/state.js`、`crates/gateway/src/assets/js/stores/session-store.js`、`crates/gateway/src/assets/js/onboarding-view.js`
- [ ] Web session row 显式返回 `displayName` / `sessionKind` / capability flags，前端 header/list 只消费这些字段：`crates/gateway/src/session.rs`、`crates/gateway/src/assets/js/components/session-header.js`、`crates/gateway/src/assets/js/components/session-list.js`
- [x] `SessionStore` / media / search 按 `session_id` 工作且 `sess_<opaque>` round-trip 正确：`crates/sessions/src/store.rs`
- [ ] `session_state` / branching parent / metadata parent 实例语义：`crates/sessions/src/state_store.rs`、`crates/tools/src/session_state.rs`、`crates/tools/src/branch_session.rs`
- [x] 旧持久化字段/列形状直接 reject：`crates/sessions/src/lib.rs`、`crates/sessions/src/metadata.rs`、`crates/sessions/src/state_store.rs`
- [x] legacy `session_key` / `session_id` validator 固定 `reason_code`：`crates/sessions/src/key.rs`
- [ ] missing `_sessionId` / `_sessionKey` 不再默认回退到 `"main"`：`crates/tools/src/spawn_agent.rs`、`crates/tools/src/exec.rs`、`crates/tools/src/process.rs`、`crates/tools/src/sandbox_packages.rs`
- [x] Telegram channel delivery / reply cleanup 不再依赖 `session_key` 旧前缀识别：`crates/gateway/src/chat.rs`
- [x] chat runtime / hooks / compaction 不再把 `session_key` 当 `session_id` 用：`crates/gateway/src/chat.rs`
- [ ] silent memory turn / compaction helper 只注入真实 `_sessionId`，不再保留误导性的 `session_key` 参数命名：`crates/agents/src/silent_turn.rs`
- [ ] Web session management 不再本地生成 `session:uuid` 或硬编码 `main` / `telegram:*` / `cron:*`：`crates/gateway/src/assets/js/sessions.js`、`crates/gateway/src/assets/js/components/session-header.js`、`crates/gateway/src/assets/js/components/session-list.js`
- [ ] sandbox runtime artifact naming 不再使用 lossy sanitize 作为真名；同/异 `effective_sandbox_key` 的派生结果 deterministically correct：`crates/tools/src/sandbox.rs`
- [ ] sandbox runtime artifact naming 产物符合 `msb-<readable-slice>-<short-hash>`，且 `readable-slice` 只做显示辅助、不参与真值判定：`crates/tools/src/sandbox.rs`
- [ ] 旧配置 `tools.exec.sandbox.container_prefix` 在 `config check` / 启动期直接拒绝：`crates/config/src/validate.rs`、`crates/config/src/schema.rs`、`crates/config/src/template.rs`
- [ ] `SandboxMode::NonMain` 按 canonical home bucket 判定：`crates/tools/src/sandbox.rs`
- [ ] `SandboxMode::NonMain` 在缺判定所需 `session_key` 时直接 reject：`crates/tools/src/sandbox.rs`
- [ ] debug panel 同时展示 `effectiveSandboxKey` 与 `containerName`：`crates/gateway/src/chat.rs`、`crates/gateway/src/assets/js/page-chat.js`
- [ ] `SessionService` / RPC 新增 `sessions.create`，并硬切 `sessions.resolve` 为“仅实例读取”：`crates/gateway/src/services.rs`、`crates/gateway/src/methods.rs`
- [ ] `SessionService` / RPC 增加服务端 owner 的 home/default path（例如 `sessions.home`）：`crates/gateway/src/services.rs`、`crates/gateway/src/methods.rs`
- [ ] 启动命中 legacy `metadata.json` 直接 reject，不自动导入：`crates/gateway/src/server.rs`
- [x] `LlmRequestContext`（或等价 provider context）显式承载可选 `prompt_cache_key`：`crates/agents/src/model.rs`
- [x] provider prompt cache 只消费调用方给出的 key；Agent 会话取 `session_id`，heartbeat 取 `system:cron:heartbeat`，无 canonical default 且未显式提供时省略，debug 不再报 fallback：`crates/agents/src/providers/openai_responses.rs`
- [x] provider prompt cache 对显式 caller bucket 保持透传：`crates/agents/src/providers/openai_responses.rs`
- [x] prompt cache omission 结构化日志 / debug source 固定值：`crates/agents/src/providers/openai_responses.rs`
- [x] Telegram DM bucket 生成：`dm-main` / `dm-peer-person.*` / `dm-peer-tguser.*` 三条主路径：`crates/telegram/src/adapter.rs`
- [x] Telegram Group bucket 生成：`group-peer-*` / `group-peer-*-branch-*` / `group-peer-*-branch-*-sender-*` 与 shared chat `tgchat.n*` 编码：`crates/telegram/src/adapter.rs`
- [x] Telegram identity link 命中优先级：有 `telegram_user_id` 时只认 `user_id`，缺 `user_id` 时才允许 username fallback：`crates/telegram/src/adapter.rs`
- [x] Telegram identity link 重复与冲突 `reason_code`：`identity_link_duplicate_user_id` / `identity_link_duplicate_user_name` / `identity_link_username_conflicts_with_user_id`：`crates/telegram/src/adapter.rs`
- [x] Telegram branch 判定优先级：`topic > reply > none`：`crates/telegram/src/adapter.rs`
- [x] Telegram branch / sender 缺失降级 `reason_code`：`branch_missing` / `sender_missing`：`crates/telegram/src/adapter.rs`
- [x] Telegram binding helper 命名和 callback sender 逻辑不再依赖旧 bucket grammar：`crates/telegram/src/adapter.rs`、`crates/telegram/src/handlers.rs`
- [ ] sandbox effective key / reject：`crates/tools/src/sandbox.rs`

### Integration
- [ ] web main / cron / heartbeat 不再产出语义型 `session_id`
- [ ] Web boot / redirect / clear_all fallback 都通过服务端 home path 获取实例 `session_id`，不会再伪造 `"main"`
- [ ] Web 展示与交互只依赖服务端下发的 `displayName` / `sessionKind` / capability flags，不再依赖 `sessionId` 前缀
- [ ] model probe / model stream test / provider setup / tts probe 仅产出 `run_id`，`LlmRequestContext` 不再伪造 session
- [x] prompt cache 在 transient / context-less 请求中不再合成 `no-session` bucket；heartbeat 等稳定 system lane 默认使用 `session_key`
- [x] prompt cache 在 non-Agent transient 显式提供 bucket 时可正常命中 provider；未提供时只会 `omit`，不会 `fallback`
- [x] gateway/channel boundary 只上传 `bucket_key`，系统层统一生成完整 `session_key`
- [x] gateway active-session truth 只按完整 `session_key`
- [x] hook / tool context 只透传真实 `_sessionId` / `_sessionKey`，缺失时不伪造 `"main"`
- [ ] sandbox runtime artifact name / debug display 对同一 `effective_sandbox_key` 稳定、对不同 key 不串桶
- [ ] `non-main` sandbox policy 在 `agent:<agent_id>:main` / other agent bucket / `system:cron:*` 三类路径上判定正确
- [x] Telegram DM / Group canonical routing、legacy reject、degrade 日志回归

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/sessions.spec.js`：Web 新建/切换/清理会话不再依赖 `main`、`session:*`、`telegram:*`、`cron:*` 旧字面规则
- [ ] `crates/gateway/ui/e2e/specs/sessions.spec.js`：Web header/list 的展示名、图标和按钮权限只依赖服务端下发合同；首屏与回落路径通过 `sessions.home`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：如实施时出现无法自动化覆盖的外部 Telegram 真网络差异，再单独登记
- 手工验证步骤：
  - 验证 `scope_key=session_key` 下同桶复用与异桶隔离
  - 验证 heartbeat 南向 `prompt_cache_key` 使用 `system:cron:heartbeat`
  - 验证 non-Agent transient 显式 bucket 会透传；未显式提供时只 `omit` 且留下固定结构化日志
  - 验证 Web 新会话不再由前端本地生成 `session:uuid`，而是由服务端返回 `sess_<opaque>`
  - 验证 Telegram DM / Group 在 canonical grammar 下的 session 归属
  - 验证 invalid PEOPLE identity link 启动失败

## 发布与回滚（Rollout & Rollback）
- 发布策略：direct hard-cut；不加 feature flag，不保留双轨
- 回滚策略：只能整体 revert 本专项提交；不得在代码里加 compat path 作为“回滚”
- 上线观测：
  - `canonical_key.reject`
  - `telegram.identity_link.reject`
  - `telegram.identity_link.conflict`
  - `telegram.route.degrade`
  - `missing_session_key_for_scope_key_session_key`
  - `provider.prompt_cache_key.omitted`

## 历史实施拆分（Implementation Outline / frozen pre-implementation snapshot）
> 说明：本节保留开工前拆分路径，供 review 追溯实施顺序与边界收敛思路；当前真实落地结果以上方状态区为准。
- Step 1: 以 `crates/sessions/src/key.rs` 收口系统层 key builder / validator，并冻结 `agent:` / `system:` grammar（已完成）
- Step 2: 先硬切 persistence owner 合同：`active_sessions(session_key -> session_id)`、`sessions(session_id, session_key -> metadata)`、`parent_session_id`、`session_state(session_id)`、`SessionStore(session_id)`
- Step 3: 再硬切 gateway/channel runtime 与 hooks/search/session management 对主真值的依赖，移除 `channel_sessions` / `session_buckets` 主路径依赖与 `telegram:*` / `main` / `session:*` 字面判断
- Step 3.5: 冻结并实现 Web home/default + display/capability 合同，切掉 app/chat/onboarding/state/header/list 对 `"main"` 与前缀的猜测
- Step 4: 硬切实例级命名债，把 `session_state` / `parent_session_key` / branching parent 全部改到 `session_id`
- Step 5: 清除 runtime / tool / hook 链路里的 `"main"` 隐式 fallback
- Step 6: 治理 runtime 语义型 `session_id`；持久会话路径切到 `session_key + opaque session_id`，transient probe 切到 `run_id only`
- Step 7: 治理 sandbox / prompt cache / worktree / hooks / tool context 的 key 消费；补齐调用方拥有的 `prompt_cache_key` 合同
- Step 7.5: 治理 sandbox runtime artifact naming / `non-main` 判定 / debug 展示，明确 `effective_sandbox_key` 与 `containerName` 分层
- Step 8: 治理 Web/session management 前后端合同，切掉本地 `session:uuid` 与 `main` / `telegram:*` / `cron:*` 前缀假设
- Step 9: 治理 Telegram canonical atom / bucket grammar / identity / branch / reject log
- Step 10: 补齐关键路径测试，完成 legacy reject 与 degrade 回归
- 受影响文件：
  - `docs/src/refactor/session-key-bucket-key-one-cut.md`
  - `docs/src/SUMMARY.md`
  - `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
  - `crates/sessions/src/key.rs`
  - `crates/sessions/src/lib.rs`
  - `crates/sessions/src/metadata.rs`
  - `crates/sessions/migrations/20240205100001_init.sql`
  - `crates/sessions/migrations/20260205120000_session_state.sql`
  - `crates/sessions/migrations/20260205130000_session_branches.sql`
  - `crates/sessions/src/store.rs`
  - `crates/sessions/src/state_store.rs`
  - `crates/common/src/hooks.rs`
  - `crates/agents/src/providers/openai_responses.rs`
  - `crates/agents/src/silent_turn.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/services.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/provider_setup.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/agents/src/runner.rs`
  - `crates/config/src/schema.rs`
  - `crates/config/src/template.rs`
  - `crates/config/src/validate.rs`
  - `crates/channels/src/plugin.rs`
  - `crates/tools/src/sandbox.rs`
  - `crates/tools/src/exec.rs`
  - `crates/tools/src/process.rs`
  - `crates/tools/src/spawn_agent.rs`
  - `crates/tools/src/sandbox_packages.rs`
  - `crates/tools/src/session_state.rs`
  - `crates/tools/src/branch_session.rs`
  - `crates/gateway/src/assets/js/sessions.js`
  - `crates/gateway/src/assets/js/components/session-header.js`
  - `crates/gateway/src/assets/js/components/session-list.js`
  - `crates/telegram/src/adapter.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/telegram/src/outbound.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/refactor/session-key-bucket-key-one-cut.md`
  - `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`
  - `issues/issue-v3-telegram-adapter-and-session-semantics.md`
  - `docs/src/refactor/session-scope-overview.md`
  - `docs/src/refactor/dm-scope.md`
  - `docs/src/refactor/group-scope.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - None

## 未决问题（Open Questions）
- Q1: 当前无新增开放规范问题；本单按既有设计真源实施
- Q2: 若未来新增 `system:<service_id>` 或新增其他渠道 canonical grammar，必须另开单独 issue，不得在本单内顺手扩展

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative / effective 边界清晰
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（本单为 hard-cut reject）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

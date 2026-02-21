# Issue: Sandbox 分桶粒度不支持 chat/bot/global（多 bot 群聊导致容器数膨胀）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P2
- Owners: <TBD>
- Components: tools / sandbox / gateway / sessions
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- 2026-02-20：支持 `tools.exec.sandbox.scope=session|chat|bot|global` 并按 effective sandbox key 复用容器：`crates/tools/src/sandbox.rs:2283`
- 2026-02-20：新增 `tools.exec.sandbox.idle_ttl_secs`（TTL 回收；=0 时按 session-delete 引用归零回收）：`crates/tools/src/sandbox.rs:405`
- 2026-02-20：共享 scope 下禁止 session 级 `sandbox_image` override，并在 UI 禁用 image selector：`crates/gateway/src/session.rs:337`、`crates/gateway/src/assets/js/sandbox.js:1`
- 2026-02-20：Session delete 逻辑避免误删共享容器（仅 `idle_ttl_secs=0` 且 remaining refs=0 才 cleanup）：`crates/gateway/src/session.rs:463`
- 2026-02-20：后台定时 prune idle sandboxes（TTL>0）：`crates/gateway/src/server.rs:1460`

**已覆盖测试（如有）**
- effective key 派生与 DM 回退：`crates/tools/src/sandbox.rs:2694`
- prune idle respects leases：`crates/tools/src/sandbox.rs:2935`

**已知差异/后续优化（非阻塞）**
- TTL 回收为 best-effort：当前通过后台周期性 `prune_idle()` 实现；未实现跨进程/跨实例的强一致 lease（足以满足本仓库场景，但不适合强一致租约需求）。

---

## 背景（Background）
- 场景：Telegram 群里同时启用多个 bot（例如 bot1/bot2/bot3），且每个 bot 都有自己的 `(bot, chat)` session（如 `telegram:lovely:-5288040422`）。
- 现状：工具执行（`exec`/`browser`/等）按 session 绑定 sandbox，导致“一个群 + 多 bot”可能带来多个 sandbox 容器并发（资源/启动开销）。
- Out of scope：
  - 不改变“会话上下文（session 历史）按 `(bot, chat)` 分桶”的口径（本单只讨论工具 sandbox 的分桶/复用）。
  - 不在本单实现完整会议/编排能力（仅提供可复用的底层能力：sandbox 分桶策略）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **sandbox**（工具执行环境）：用于工具执行（如 `exec`）的隔离环境（容器/cgroup/none），可持久化工作目录与缓存。
  - Why：影响资源开销、隔离边界、以及多 agent/bot 协作时的“共享工作台”能力。
  - Not：不是 LLM 的“对话上下文”；也不是 Telegram 平台的消息转发机制。
  - Source/Method：configured（由配置决定）、effective（由派生规则决定）。

- **sandbox 分桶**：把一次工具执行映射到某个 sandbox 的规则（决定“复用范围”）。
  - Not：不是 session 的分桶规则。

- **sandbox_scope（对用户口径）**：用户希望可配置的 4 个选项：
  - `session`：每个 session 独立 sandbox（隔离最强）。
  - `chat`：同一 channel 的同一 chat 共享 sandbox（群共享工作台）。
  - `bot`：同一 channel 的同一 bot/account 共享 sandbox（bot 共享工作台）。
  - `global`：本实例共享 1 个 sandbox（最省资源，隔离最弱）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 支持 `sandbox_scope=session|chat|bot|global`，并能稳定派生出 sandbox id key，使容器复用范围与该选项一致。
- [x] 对 Telegram 群聊场景给出明确且可操作的推荐值（默认仍安全）。
- [x] 对非 Telegram（Web UI / CLI / 其他 channel）保持行为兼容：未能派生 chat/bot 时应自动回退为 `session`（避免误共享）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：默认行为不变（仍是 `session` 分桶）。
  - 必须：任何无法可靠解析/派生分桶 key 的场景必须回退到 `session`。
  - 不得：在未显式配置为 `chat/bot/global` 时引入跨 session 的工具状态串线。
  - 不得：把敏感信息（token、私聊内容）写入日志或用于可猜测的 container name。
- 兼容性：只支持 `sandbox.scope=session|chat|bot|global`；非法值应当在启动/校验期直接报错并要求用户修正（避免隐藏行为变化）。
- 可观测性：日志应能明确打印 effective 的 sandbox id（scope + key），便于排障与成本评估。
- 安全与隐私：`chat/bot/global` 属于“主动降低隔离”的行为，必须在文档/配置说明中显式提示风险。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一个 Telegram 群里启用多个 bot，每个 bot 各自触发工具时，会出现多个 sandbox（容器/cgroup scope）被创建/维持。
2) 用户希望“群协作”时工具环境可以共享（减少容器数 + 共享工具产物），但当前做不到。

### 影响（Impact）
- 用户体验：首次工具调用冷启动更慢；并发时更容易卡顿。
- 可靠性：容器数量上升导致资源争用（CPU/内存/磁盘），影响整体稳定性。
- 排障成本：同一群的上下文跨 bot 共享靠“转述/镜像”解决了文本，但工具产物（文件/缓存）仍割裂，用户直觉不一致。

### 复现步骤（Reproduction）
1. 在同一个 Telegram 群里启动 bot1/bot2/bot3。
2. 分别点名 bot1/bot2 执行 `exec`（例如 `ls`, `findmnt`）。
3. 观察日志：每个 session 都会触发 `sandbox ensure_ready`，且 sandbox id key 与 session_key 强相关。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/tools/src/sandbox.rs:2187`：`SandboxRouter::sandbox_id_for(session_key)` 直接使用 `session_key` 派生 sandbox id key（sanitize 后作为容器名后缀）。
  - `crates/tools/src/sandbox.rs:347`：（历史/修复前）`SandboxScope` 曾仅有 `Session|Agent|Shared`；现已收敛为 `Session|Chat|Bot|Global`。
  - `crates/tools/src/sandbox.rs:517`：`SandboxId` 由 `scope + key` 组成，当前 key 的派生策略单一（session_key）。
  - `crates/config/src/template.rs:249`：配置模板已收敛为 `session|chat|bot|global`（并新增 `idle_ttl_secs`），与 tools 层派生规则一致。
  - `crates/gateway/src/session.rs:463` + `crates/tools/src/sandbox.rs:2205`：删除 session 会调用 `cleanup_session(session_key)`；若未来引入 `chat/bot/global` 共享 sandbox，需要明确“删除单个 session 是否应当清理共享 sandbox”（否则可能误删其他会话正在使用的环境）。
- 日志证据（示例关键字）：
  - `sandbox ensure_ready session="telegram:<bot>:<chat_id>" sandbox_id=Session/telegram-...`
- 当前测试覆盖：
  - 已有：sandbox scope 字符串序列化（`crates/tools/src/sandbox.rs` 内部测试）。
  - 缺口：缺少“按 chat/bot/global 派生 key”的单测与回退逻辑测试。

## 根因分析（Root Cause）
- A. 触发：群聊多 bot 场景天然会产生多个 session_key（`telegram:<account_id>:<chat_id>`）。
- B. 逻辑：sandbox id key 目前固定从 `session_key` 派生，因此复用边界被锁死为“每 session 一套”。
- C. 下游：即便存在 `SandboxScope` 枚举，当前也没有不同 scope 对应的 key 派生规则；此外 UI/接口仍以“session”为中心（例如 session 删除触发 cleanup、session 级 image override），因此引入共享 scope 前必须补齐相应的一致性规则。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `sandbox_scope=session` 时：行为与现状一致（每个 session 独立 sandbox）。
  - `sandbox_scope=chat` 时：同一 channel 的同一 chat 共享 sandbox（例如同一 Telegram 群内所有 bot 共享 1 套工具环境）。
  - `sandbox_scope=bot` 时：同一 channel 的同一 bot/account 共享 sandbox（例如 `telegram:lovely:*` 共享）。
  - `sandbox_scope=global` 时：本实例共享 1 个 sandbox（所有 session 都复用）。
  - 对无法解析的 session_key，或无法确定 chat/bot 归属的场景：必须回退到 `session`（避免误共享）。
- 不得：
  - 不得在默认配置下引入跨 session 共享。
  - 不得把群/用户/私聊等敏感原文直接拼进容器名（应 sanitize + 可选 hash）。
- 应当：
  - 应当在日志中输出 effective 的 sandbox id（包含 scope 与 key），并可追踪其来源（configured/effective）。
  - 应当明确共享 scope 下“image override 的归属”：
    - 当前 UI 是按 session 设置 `sandbox_image`，但共享 scope 下实际运行的 sandbox 不是 session 唯一的。
    - V1 建议：当 scope 为 `chat/bot/global` 时，image override 应当绑定到“effective sandbox key”（共享对象）而不是某一个 session（避免同一共享 sandbox 被不同 session 配成不同镜像导致行为不确定）。
  - 应当明确共享 scope 下的“容器生命周期管理”口径（尽量收敛，避免引入额外 UI 概念）：
    - **唯一标识**：以“effective sandbox key”（由 `sandbox_scope` + `session_key` 派生）作为容器复用/回收的唯一标识。
    - **lazy start**：当某次工具执行需要 sandbox 时（enabled 且 mode 允许），对该 effective key `ensure_ready`；不存在则创建，存在则复用。
    - **idle_ttl 自动回收（推荐）**：若 `idle_ttl_secs > 0`，当某 effective key 空闲超过 TTL 后自动 cleanup（回收容器与资源）。
    - **idle_ttl=0 特例（按引用归零回收）**：若 `idle_ttl_secs = 0`，禁用按时间回收；仅当“引用该 effective key 的 sessions 数量归零”时回收容器：
      - 引用口径（V1）：仅统计“会使用 sandbox”的 sessions（即该 session 的 `sandbox_enabled` effective 为 true）。
      - 删除单个 session 不得直接 cleanup 共享容器；
      - 仅当删除后 remaining 相关 sessions 为 0 时才 cleanup（避免误伤其他 sessions）。

## 推荐配置（Recommended Defaults）
> 只给出最常用且最可控的组合，避免配置膨胀。

- 默认（安全、隔离强）：`scope="session"`，`idle_ttl_secs=0`
- Telegram 群聊多 bot 协作（共享工具产物、减少容器数）：`scope="chat"`
  - 建议 `idle_ttl_secs=0`（不按时间清理；当最后一个相关 session 被删除时才回收共享容器）
  - 若你非常在意资源回收：再把 `idle_ttl_secs` 设为 `1800`（30min）或 `3600`（60min）
- 单 bot 跨多个群复用工具环境（谨慎，易串线）：`scope="bot"`
- 全实例共享（最省资源，最易串线）：`scope="global"`

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：新增“key 派生策略”，并提供用户友好枚举（session/chat/bot/global）
- 核心思路：
  - 在 tools/sandbox 层把“scope 枚举”与“key 派生”一起设计：不同 scope 对应不同的 key 规则。
  - 从 `session_key` 中按已知协议解析 `(channel_type, account_id, chat_id)`（例如 `telegram:<account_id>:<chat_id>`），从而派生 chat/bot key。
  - 解析失败或不适用时自动回退 `session`。
- 优点：口径与用户直觉一致；可复用到后续多 agent/会议机制；默认安全。
- 风险/缺点：需要保证 session_key 格式的解析稳定，并对未来新增 channel 类型有明确扩展点。

#### 方案 2（废弃/历史记录）：沿用旧枚举 `session/agent/shared`
- 现状：旧枚举名/别名已移除（仅支持 `session|chat|bot|global`），以避免概念漂移与配置歧义。

### 最终方案（Chosen Approach）
采用 **方案 1**。

#### 行为规范（Normative Rules）
- 规则 0（名词收敛）：所有行为/日志围绕 “effective sandbox key” 收敛；不引入额外 UI 概念。
- 规则 1（默认不变）：未配置时 effective 为 `session`。
- 规则 2（chat 仅对可解析的 channel session 生效）：仅当 session_key 能解析到 `(channel_type, chat_id)` 时才用 chat key，否则回退 session。
- 规则 3（bot 仅对可解析的 channel session 生效）：仅当 session_key 能解析到 `(channel_type, account_id)` 时才用 bot key，否则回退 session。
- 规则 4（global 无条件共享）：key 固定为常量（例如 `"global"`），且需显式配置启用。
- 规则 5（命名安全）：容器名使用 sanitize 后的 key；对长 key 可采用 hash 缩短并保留前缀（便于人肉识别）。
 - 规则 6（生命周期，idle_ttl_secs > 0）：对每个 effective key 维护 `last_used_at`；空闲超过 TTL 自动 cleanup。
- 规则 7（生命周期，idle_ttl_secs = 0）：禁用 TTL；只在 session 删除路径触发 “引用归零回收”（remaining=0 才 cleanup）。
 - 规则 8（删除 session 的 cleanup 语义）：scope≠`session` 时，删除 session 不得直接 cleanup 共享容器（仅按规则 7 “引用归零”触发）。
 - 规则 9（image override 归属）：scope=`chat|bot|global` 时，image override 必须绑定 effective key（共享对象），不得由不同 session 以不同值竞争同一容器镜像。

#### 接口与数据结构（Contracts）
- 配置：
  - 新增 `sandbox.scope` 扩展为：`session|chat|bot|global`（或新增并行字段 `sandbox.scope_mode`，保留旧 `scope` 为兼容）。
  - 新增 `idle_ttl_secs`（或等价字段）：
    - `>0`：启用空闲回收（TTL）
    - `=0`：禁用 TTL，启用“引用归零回收”（仅在 session 删除路径触发）
  - V1 收敛规则（共享 scope 下 image override）：
    - 当 scope=`chat|bot|global` 时：**禁止** session 级 `sandbox_image` override（`sessions.patch.sandbox_image`），统一使用全局默认 image（`tools.exec.sandbox.image` / 预构建 image）。
    - 理由：避免同一共享容器被多个 session 以不同值竞争配置，导致行为不确定；并避免引入新的“按 effective key 持久化 image override”存储/UI。
  - 兼容性：仅支持 `session|chat|bot|global`；非法值应当报错并要求修正（避免隐藏行为变化）。
- 派生规则（effective sandbox id）：
  - `session`：`key = session_key`
  - `chat`：`key = "<channel_type>:chat:<chat_id>"`
  - `bot`：`key = "<channel_type>:bot:<account_id>"`
  - `global`：`key = "global"`
- UI（可选，后续）：提供单选项解释风险，并默认 `session`。

#### 失败模式与降级（Failure modes & Degrade）
- 无法解析 session_key：回退 `session`，并 debug 日志注明原因（不打用户原文）。
- 共享 scope 下并发冲突（同目录写同名文件）：不强行兜底；通过文档建议“使用唯一文件名/子目录”，并可后续引入“每次工具调用临时工作目录”作为增强。
- 共享 scope 下清理误伤：
  - V1：`cleanup_session(session_key)` 在 scope≠`session` 时只清理该 session 的 override/状态；
  - cleanup 共享容器仅来自：
    - `idle_ttl_secs > 0` 的 TTL 回收，或
    - `idle_ttl_secs = 0` 的 “引用归零回收”（删除最后一个相关 session 时触发）

#### 安全与隐私（Security/Privacy）
- `chat/bot/global` 会降低隔离：必须在配置说明中写清楚风险（跨 bot/跨 chat 的文件可见、缓存可见）。
- 容器名/日志不得包含 token/私聊原文；chat_id/account_id 属于 platform 标识，可允许，但建议 sanitize + 可选 hash。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 默认配置不变：同一 session 的工具行为与现状一致（`sandbox_id` key 仍与 session_key 对应）。
- [ ] 配置为 `chat` 后：同一 Telegram 群内 bot1/bot2 执行工具应复用同一 sandbox id（日志可见一致的 sandbox id），且 bot2 能读取 bot1 在工具中写入的文件（证明共享 sandbox 环境）。
- [ ] 配置为 `bot` 后：同一 bot 在不同群执行工具复用同一 sandbox id（并明确记录风险）。
- [ ] 配置为 `global` 后：所有 session 复用同一 sandbox id（并明确记录风险）。
- [x] 解析失败场景会回退到 `session`，不会“误共享”。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] key 派生：`session/chat/bot/global` + Telegram DM 回退：`crates/tools/src/sandbox.rs:2694`
- [x] TTL prune respects leases：`crates/tools/src/sandbox.rs:2935`

### Integration
- [ ] 手工：两个 Telegram bot + 同群，分别执行 `exec "echo hi > /tmp/x"` 与 `exec "cat /tmp/x"`，验证 chat scope 下可互读。

### 人工验收步骤（Manual E2E）
**前置条件**
- 已部署并可重启 Moltis（此项是全局 config 级别变更）。
- 至少 2 个 Telegram bot accounts 在同一个群里（用于 chat scope 验收）。

**配置步骤（必须）**
1) 打开配置文件（你的实际路径按部署方式为准），找到：
   - `[tools.exec.sandbox]`
2) 设置（示例）：
   - `scope = "chat"`
   - `idle_ttl_secs = 0`
3) 重启 Moltis

**UI 校验（不修改 UI，只读确认）**
- Web UI 任意打开一个 chat session，Context/Sandbox 里应能看到：
  - `Scope = chat`（由 gon 提供的 sandbox runtime snapshot）
- Sandbox Image selector 在 scope≠session 时应显示为“managed by config”，且按钮不可点。

**交互验收（Telegram 群内）**
1) 只点名 bot1 执行工具并写文件：
   - `@bot1 执行：echo hi > /tmp/moltis-chat-scope.txt`
2) 只点名 bot2 在同群读取：
   - `@bot2 执行：cat /tmp/moltis-chat-scope.txt`
3) 期望：bot2 能读到 `hi`（证明同群共享 sandbox 环境）

**删除 session 回收验收（idle_ttl_secs=0）**
1) Web UI 中删除群内相关的最后一个 session（你期望“最后一个引用该 chat key 的 session”）
2) 期望：不会误删仍在引用的共享容器；当最后一个相关 session 删除后，后续再次工具调用会重新创建容器（可通过日志 `sandbox ensure_ready` 观察）

### 自动化缺口（如有，必须写手工验收）
- Telegram e2e 环境不可用时：用手工步骤 + 日志关键词验收。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认 `session`；`chat/bot/global` 需要显式配置启用。
- 回滚策略：改回 `session` 并清理旧 sandbox（可选 cleanup），不影响 session 历史。
- 上线观测：
  - 日志：`sandbox ensure_ready` 中打印 `effective_scope/effective_key`。
  - 指标（可选）：活跃容器数量、ensure_ready 耗时分位。

## 实施拆分（Implementation Outline）
- Step 1: 定义新枚举与兼容映射（配置反序列化 + 日志提示）。
- Step 2: 改造 `SandboxRouter::sandbox_id_for(...)`：按 effective scope 派生 key，并对无法解析的 session_key 回退 session。
- Step 3: 补齐生命周期策略：
  - `idle_ttl_secs > 0`：记录 last_used 并支持空闲回收（TTL）
  - `idle_ttl_secs = 0`：在 session 删除路径实现“引用归零回收”（避免共享误伤）
- Step 4: 增补单测覆盖 key 派生、回退、以及引用归零回收边界。
- Step 5（可选）: UI 增加配置入口与风险说明（scope 单选 + 风险提示）；共享 scope 下将 image override 改为绑定 effective key。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/config/src/loader.rs`（若涉及配置结构变更，视实现）
  - `crates/gateway/src/assets/js/...`（可选 UI）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-telegram-bot-to-bot-outbound-mirror-into-sessions.md`（群内多 bot “知情”需求；本单解决工具环境共享/成本问题）

## 未决问题（Open Questions）
- Q1: `chat` scope 的 key 应该使用 `chat_id` 还是 `(chat_id, topic/thread_id)`？（当前建议先不纳入 topic/thread）
- Q2: 容器名过长时是否需要统一的 hash/shorten 规则（避免 docker 名长度限制）。
- Q3: `bot` scope 是否必须限定“同一 channel_type”内共享（建议必须限定，避免跨 channel 污染）。
- Q4: 共享 scope 下 session 级 `sandbox_image`/`sandbox_enabled` 的语义应如何落地？
  - 建议：`sandbox_enabled` 仍可保留 per-session（是否使用 sandbox 执行该 session 的命令）。
  - 建议：`sandbox_image` 改为绑定“effective sandbox key”（共享对象），以避免同一共享 sandbox 被多处配置冲突。
- Q5: `idle_ttl_secs` 字段名与落点：放在 `tools.exec.sandbox` 还是单独一层？（本单建议放 `tools.exec.sandbox.idle_ttl_secs`）
- Q6（已冻结/本单不做）：共享 scope 下 image override 需要持久化与 UI 配套（按 effective key 存储）。V1 已选择“禁止 session 级 override”，因此本单不引入新的持久化结构。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

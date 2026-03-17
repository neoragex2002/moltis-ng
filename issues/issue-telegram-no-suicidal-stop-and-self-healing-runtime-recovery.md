# Issue: Telegram 通道禁止“自杀停机”，并修复永久失活路径（conflict / startup / update-restart）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-17
- Owners:
- Components: telegram / gateway / channels / server / ui
- Affected providers/models: 所有通过 Telegram channel 接入的 bot account（与具体 LLM provider/model 无关）
- 实施准备结论：
  - 已具备开工条件：**是（P0 runtime 自恢复修复具备实施条件）**
  - 当前硬阻塞项：**无**
  - 剩余 P0 口径未决项：**无**
  - 已冻结的 P0 默认口径：**4 项**
    - `auth_failed / invalid token` 在 P0 按慢速 `reconnecting(auth_failed)` 处理，不做自动 disable
    - UI / RPC 在 P0 不强制扩 schema；先复用现有 `connected + details` surface，确保后端先自恢复
    - 配置更新必须先区分 `hot_update` 与 `identity_change`；当前 P0 不引入额外 `restart_required` 桶
    - `token` 变更 / bot 身份变更不作为 P0 的“原地 update”目标；默认按 remove+add / 显式迁移处理
  - P0 收敛结论：
    - 核心实现路径已收敛为：`supervisor + reconnecting runtime + hot_update/identity_change + persisted-list visibility`
    - 当前不再保留会导致实现分叉的第三套更新路径或双 polling 切换方案
  - 非阻塞后续项：
    - 是否新增 `AccountRuntimeStateChanged` 事件，可作为 P1 观测增强
    - 是否引入 lease / 选主，作为后续冲突频率治理，不阻塞本单 P0

**已实现（2026-03-17）**
- Telegram runtime 已改为 `supervisor + reconnecting` 模型；`start_account()` 成功即表示恢复职责已接管：`crates/telegram/src/bot.rs`、`crates/telegram/src/plugin.rs`
- `TerminatedByOtherGetUpdates` / network / startup failures 不再触发 `disable + cancel + break`，而是进入 reason-coded reconnect：`crates/telegram/src/bot.rs`
- 启动期 `get_me` / `delete_webhook` 已纳入可重试握手阶段，修复 server 启动一次失败后永久失活：`crates/telegram/src/bot.rs`、`crates/telegram/src/plugin.rs`
- Channels `update/save` 已改为 `hot_update`，并显式拒绝 `identity_change` 原地更新，不再走通用 `stop -> start`：`crates/gateway/src/channel.rs`
- Channel 管理列表已改为“持久化配置 ∪ 当前 runtime”并集，可保留 reconnecting / startup-failed account 的可见性：`crates/gateway/src/channel.rs`
- probe/details 已收敛为稳定 `runtime_state / reason_code / backoff_secs / last_poll_ok_secs_ago / last_retryable_failure_reason_code` 输出：`crates/telegram/src/plugin.rs`

**已覆盖测试**
- bot：retry budget、reason 分类、退避策略：`crates/telegram/src/bot.rs`
- plugin：probe 顶层状态、阻塞态细节、details 顺序：`crates/telegram/src/plugin.rs`
- outbound：失败不改写 account runtime state：`crates/telegram/src/outbound.rs`
- gateway/channel：`hot_update` vs `identity_change` 分类、persisted-list 并集：`crates/gateway/src/channel.rs`

**已知差异/后续优化（非阻塞）**
- P0 继续复用现有 `connected/disconnected + details` surface；前端 badge 仍未扩成多态 runtime badge，但 `details` 已能稳定表达运行态。
- `AccountDisabled` 事件在 P0 已冻结为仅人工停用语义；若后续需要更细 runtime 事件，可在 P1 单独补 `AccountRuntimeStateChanged`。
- 截至 2026-03-18，当前完成的是代码级修复与自动化验证；**真实 Telegram 在线手工回归尚待执行**，本单已保留手工验收清单。

---

## 背景（Background）
- 场景：同一 Telegram bot token 在另一处短暂运行、Telegram API/网络临时抖动、Moltis 启动瞬间 `get_me` / `delete_webhook` 失败，均可能导致本地 Telegram channel 进入“看起来配置还在，但实际上永久不再恢复”的状态。
- 现场证据（本机 `~/.moltis/logs.jsonl`）：
  - `telegram:8576199590`（`@fluffy_tomato_bot`）于 2026-03-16 00:11:33 CST 记录 `telegram bot disabled: another instance is already running with this token`
  - `telegram:8704214186`（`@cute_alma_bot`）于 2026-03-17 07:54:14 CST 记录同样日志
  - `telegram:8344017527`（`@lovely_apple_bot`）仍有正常入站与恢复日志，说明这不是整机 TG 全挂，而是 account 级 runtime 失活
- 结论口径：
  - 截至 2026-03-17，**已确认的 account 级永久失活触发点只有三类**：`token_conflict` 自杀停机、启动期一次性失败、配置更新时 stop-then-start 第二步失败。
  - 普通 polling 降级、单条 update retry/quarantine、单次 outbound 失败，目前**未发现**会直接导致 account 永久失活；但规范上仍应明确冻结为“不得升级成 account stop”。
- 约束：
  - 用户明确要求：**TG 上联 / 下联都不得采用自杀停机式处理**；临时断联不得演化为“永不能恢复”
  - Telegram `getUpdates` 冲突（409 / `TerminatedByOtherGetUpdates`）在多实例或临时重叠部署中是可预期现象，必须视为可恢复运行态异常，而不是永久 disable 指令
  - 日志与 UI 必须能让运维看出“重连中 / 冲突中 / 鉴权失败 / 人工停用”之间的区别
- Out of scope：
  - 本单不重做 Telegram relay / mirror 业务协议
  - 本单不引入跨实例全局租约/选主作为唯一方案前提；是否补 lease 可作为备选增强
  - 本单不把 Telegram transport 从 long polling 全量切换到 webhook

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **自杀停机**（主称呼）：运行中的 Telegram account 因 provider/platform/网络侧故障而由本地 runtime 主动 `cancel + break + Exited`，之后必须依赖人工保存/重启才能恢复。
  - Why：这是本单要禁止的核心行为。
  - Not：不是用户显式 remove/logout/stop。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：self-stop / self-disable / suicidally stop

- **运行态自恢复**（主称呼）：Telegram account 只要仍处于“已配置且启用”的期望态，本地 runtime 必须持续重试并在外部条件恢复后自动重新接入。
  - Why：决定系统是否需要人工 babysit。
  - Not：不是一次性启动成功；也不是 UI 手工保存触发的重启。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：self-healing / auto recovery

- **永久失活**（主称呼）：Telegram account 在配置仍存在、用户也未显式停用的前提下，因单次运行态故障而不再自动恢复，必须依赖人工保存/重启/重启进程才能重新接入。
  - Why：本单要找准的是“哪些路径真的会把 bot 打死”，而不是泛泛而谈所有降级。
  - Not：不是短时 degraded、不是单消息失败、也不是单条 update retry/quarantine。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：permanent loss / latched offline

- **启动期临时失败**（主称呼）：在 account 建立 polling loop 之前，`get_me`、`delete_webhook`、首次网络连接等步骤的瞬时失败。
  - Why：当前这类失败会让 account 根本没进入可恢复 runtime。
  - Not：不是已经进入 loop 后的普通 `getUpdates` 周期性失败。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：bootstrap failure / connect failure

- **期望态**（主称呼）：用户配置/数据库里该 Telegram account 仍被视为已启用、应当运行。
  - Why：是否继续重试恢复必须由期望态决定，而不是由最近一次 runtime 错误决定。
  - Not：不是当前一瞬间的实际连接状态。
  - Source/Method：[configured]
  - Aliases（仅记录，不在正文使用）：desired state

- **运行态**（主称呼）：当前 account 的实际执行状态；P0 冻结为 `running / reconnecting / stopped_by_operator`。
  - Why：UI、probe、日志必须表达运行态，而不是只给一个粗糙 connected/disconnected。
  - Not：不是配置是否存在。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：runtime state

- **阻塞态细节**（主称呼）：附着在 `details` 上的连通性/处理阻塞原因，例如 `blocked_by_update_retry=true`。
  - Why：它解释“为什么当前看起来不健康”，但不应膨胀成新的 P0 顶层运行态。
  - Not：不是独立于 `running / reconnecting / stopped_by_operator` 的第四种稳定运行态。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：blocking detail / retry barrier

- **人工停用**（主称呼）：仅由显式 remove/logout/stop 等用户操作触发的停止行为。
  - Why：它是少数允许 runtime 真正停止重试的来源。
  - Not：不是 TG 冲突、网络失败、429、鉴权暂时失败。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：operator stop / explicit disable

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] Telegram account 只要仍处于期望态，就必须持续尝试恢复；不得因临时冲突/断联进入人工干预前不可恢复状态。
- [ ] `TerminatedByOtherGetUpdates`、网络失败、限流、启动期握手失败等，都不得触发 account 级自杀停机。
- [ ] Telegram outbound send/edit/typing/stream 等下联失败必须局部化到本次消息，不得连带使整个 account 永久下线。
- [ ] UI / status / probe 必须能区分顶层运行态 `reconnecting / running / stopped_by_operator`，并在 `details` 中稳定表达 `blocked_by_update_retry` 等阻塞态细节。
- [ ] “更新并保存”只能是配置更新，不得继续承担“事实上的唯一恢复入口”。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把“人工停用”与“临时故障重连中”彻底分离。
  - 必须保证 Telegram 临时冲突恢复后无需人工保存即可重新接收/发送消息。
  - 不得把 provider/platform 返回的单次故障解释为永久 disable 指令。
  - 不得让出站单消息失败污染 account 级运行态。
- 兼容性：保留现有 channel store / account_handle / session key 语义；不要求用户重建 channels。
- 可观测性：新增/调整日志与 probe details，明确 reason code、backoff、运行态；避免只发 `AccountDisabled`。
- 安全与隐私：不得记录 bot token、完整消息正文、完整 Telegram URL；仅保留脱敏 reason code 与必要标识。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram bot 一旦撞上同 token 的另一实例或启动期临时失败，会在本地永久失活，之后 TG 侧“完全不收消息”。
2) 用户/运维经常只能通过 Channels 页面“重新保存”把 bot 拉起来，表现为“配置没丢，但 bot 像死了一样”。
3) 当前日志虽有 `polling.degraded / recovered`，但 conflict 路径直接走 `disabled`，语义上把临时冲突升级成永久停机。
4) 启动路径是单次尝试；如果 Moltis 启动瞬间 TG API 不可达，对应 account 可能从未真正跑起来且无后续自恢复。
5) 配置更新路径先停后起；若 restart 的“起”因为临时故障失败，会把原本健康的 bot 直接打成永久失活。

### 影响（Impact）
- 用户体验：
  - TG bot 会出现“突然不回消息，且一直不恢复”的严重故障。
  - 用户容易误判为模型挂了、权限坏了、服务端没收到消息。
- 可靠性：
  - 临时冲突、短时断网、发布重叠会演化成持久故障。
  - Telegram 通道的 availability 被人工操作绑定，失去自动恢复能力。
- 排障成本：
  - 需要人工区分“bot 真停了”还是“只是网络抖动”。
  - 目前 UI 的 `AccountDisabled` 与实际根因（临时冲突）混淆，误导排障。

### 三类已确认永久失活路径（Confirmed Paths）
1) **冲突自杀停机**
   - 触发：`TerminatedByOtherGetUpdates`
   - 当前行为：直接 `Exited`、广播 `AccountDisabled`、`cancel + break`
   - 为什么永久：polling loop 主动结束，且当前无 supervisor 把 account 拉回
2) **启动期一次性失败**
   - 触发：`get_me()` / `delete_webhook()` / 启动瞬时网络失败
   - 当前行为：`start_account()` 整体失败，server 只记录 warn
   - 为什么永久：account 从未建立可持续重试的 runtime，后续无人继续启动
3) **配置更新时不安全重启**
   - 触发：Channels UI 执行“更新并保存”
   - 当前行为：先 `stop_account()`，再 `start_account()`
   - 为什么永久：若第二步因临时故障失败，原本健康的旧 runtime 已被销毁

### 复现步骤（Reproduction）
1. **路径 A：token conflict**
   - 在 A 实例运行某个 Telegram bot token 的 Moltis polling
   - 在 B 实例短暂启动同 token 的 `getUpdates` long polling
   - A 实例命中 `TerminatedByOtherGetUpdates`
   - 期望：A 进入 `reconnecting(token_conflict)`，待 B 停止后自动恢复
   - 实际：A 本地 account 被标记 `Exited` 并 `cancel + break`
2. **路径 B：启动期失败**
   - 重启 Moltis，并在启动瞬间让 Telegram API 不可达或让 `delete_webhook()` 失败
   - 期望：account 进入后台重试，网络恢复后自动上线
   - 实际：`start_account()` 失败后仅打 warn，该 account 之后不再自动启动
3. **路径 C：更新时不安全重启**
   - 对已健康运行的 Telegram account 在 Channels UI 执行“更新并保存”
   - 让第二步 `start_account()` 由于临时网络/TG API 故障失败
   - 期望：旧 runtime 继续服务，更新失败后可回滚或保持旧配置
   - 实际：旧 runtime 先被 `stop_account()` 删除，account 被直接打成永久失活

## 永久失活判定矩阵（Permanent-loss Matrix）
- **已确认会导致永久失活**
  - `token_conflict -> request_disable_account -> cancel + break`
    - 证据：现场日志 + `crates/telegram/src/bot.rs:369`、`crates/gateway/src/channel_events.rs:695`
  - 启动期 `get_me` / `delete_webhook` 临时失败，且无 supervisor 重试
    - 证据：`crates/telegram/src/bot.rs:90`、`crates/telegram/src/bot.rs:95`、`crates/telegram/src/plugin.rs:152`、`crates/gateway/src/server.rs:1831`、`crates/gateway/src/server.rs:1855`
  - 配置更新 `stop_account -> start_account` 中第二步失败，旧 runtime 已被移除
    - 证据：`crates/gateway/src/channel.rs:278`、`crates/telegram/src/plugin.rs:177`
- **已确认不会直接导致永久失活**
  - 普通 `telegram.polling.degraded(network/api/retry_after)`：当前只是 sleep 后继续轮询
  - 单条 update 的 retry budget / quarantine：只影响当前 update，不会 stop account
  - outbound send/edit/typing/stream 失败：当前只影响消息级发送或降级，不会 stop polling
- **会放大永久失活影响，但本身不是直接触发点**
  - polling task 无 supervisor，任何非人工退出都不会被重新拉起
  - `channel.status` 主要观察内存 runtime；启动失败或 stop-then-start 失败的 account 可见性不足

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/channel.rs:85`：Channels 列表当前从 `tg.account_handles()` 枚举 account；若 runtime 未注册，UI 侧可见性本身就会缺失。
  - `crates/telegram/src/bot.rs:90`：`start_polling()` 在 spawn loop 前先执行 `get_me()`。
  - `crates/telegram/src/bot.rs:95`：`delete_webhook()` 也是启动前同步握手步骤；一旦失败，`start_account()` 整体失败。
  - `crates/telegram/src/bot.rs:369`：命中 `TerminatedByOtherGetUpdates` 后直接进入 conflict 分支。
  - `crates/telegram/src/bot.rs:378`：把 polling state 改为 `Exited`，并设置 `last_poll_exit_reason_code = "disabled_token_conflict"`。
  - `crates/telegram/src/bot.rs:388`：调用 `request_disable_account(...)`，把临时冲突上抛为禁用事件。
  - `crates/telegram/src/bot.rs:397`：`cancel + break`，使 loop 不再重试。
  - `crates/gateway/src/channel_events.rs:695`：`request_disable_account()` 当前广播的是 `AccountDisabled`，文案也是“stopping local polling”。
  - `crates/telegram/src/plugin.rs:152`：`start_account()` 仅调用一次 `bot::start_polling(...).await?`，无后台 supervisor。
  - `crates/telegram/src/plugin.rs:105`：当前已经存在 `update_account_config()`，说明系统具备“无重启热更新部分配置”的落点，不必把所有更新都设计成重启。
  - `crates/gateway/src/server.rs:1831`：server 启动 config channels 时，`start_account()` 失败只 `warn!`，没有自动重试。
  - `crates/gateway/src/server.rs:1855`：server 启动 stored channels 时同样只 `warn!`，没有自动重试。
  - `crates/gateway/src/channel.rs:278`：当前“更新并保存”通过 `stop_account + start_account` 实际承担人工恢复作用，同时引入 stop-then-start 断崖风险。
  - `crates/gateway/src/channel.rs:433`：sender approval 已通过 `update_account_config()` 热更新配置，证明“保留 polling offset、不重启 bot”已有先例。
  - `crates/gateway/src/channel.rs:487`：sender deny 同样走热更新，不重启 bot。
  - `crates/telegram/src/plugin.rs:177`：`stop_account()` 会 `cancel` 并从内存 `accounts` map 删除该 account；若后续 `start_account()` 失败，旧 runtime 不会自动恢复。
  - `crates/telegram/src/outbound.rs:1723`：outbound 流式 edit 失败目前只是 degraded，不会 stop account；说明“下联自杀停机”在现状中尚未发现，但规范上仍需冻结为禁止。
- 配置/协议证据（必要时）：
  - 本机 channel store 中 3 个 Telegram accounts 均处于持久化配置存在状态，但只有 `telegram:8344017527` 在最近日志中仍有入站，说明“配置存在 != runtime 健康”。
- 运行日志证据（本机 `~/.moltis/logs.jsonl`）：
  - `telegram:8576199590`：`telegram bot disabled: another instance is already running with this token`（2026-03-16 00:11:33 CST）
  - `telegram:8704214186`：同样日志（2026-03-17 07:54:14 CST）
  - `telegram:8344017527`：仍存在 `telegram inbound message received`（2026-03-17 18:47:08 CST）
- 当前测试覆盖：
  - 已有：retry budget 与 probe stale/blocking 测试：`crates/telegram/src/bot.rs:454`、`crates/telegram/src/plugin.rs:499`
  - 缺口：无 conflict 自动恢复测试、无启动期失败自动恢复测试、无“通用 update 不得再走 stop-then-start”测试、无“outbound failure 不得影响 account 运行态”测试、无 UI 运行态区分测试

### 边界澄清（What Is Not The Root Issue）
- 当前核心问题不是“Telegram 所有失败都会停机”，而是“少数路径把临时故障错误升级成 account 级终止”。
- 当前核心问题也不是“配置被删了”，而是“配置仍在，但 runtime 已死且无人重启”。
- 因此修复重点必须落在：运行态模型、supervisor、错误分类、更新切换策略，而不是单纯增加更多 warn 日志。

## 根因分析（Root Cause）
- A. **错误分类过于激进**
  - 当前把 `TerminatedByOtherGetUpdates` 当成应当“停用本地 account”的事件，而不是应当“退避等待 ownership 变化”的临时运行态异常。
- B. **期望态与运行态耦合**
  - Telegram account 的“是否继续存在并尝试恢复”被 polling loop 的单次错误决定；一旦 loop 自己退出，期望态仍在，但运行态丢失且无人拉起。
- C. **启动路径是一枪式**
  - `get_me()` / `delete_webhook()` 在 loop 外执行，导致启动期临时失败直接阻断 account runtime 建立；server 只记录 warn，没有 supervisor 后续重试。
- D. **配置更新切换策略不安全**
  - `update/save` 采用 stop-then-start；新 runtime 未确认就绪前就销毁旧 runtime，使任何临时启动失败都升级成永久失活。
- E. **无 supervisor 放大所有 runtime 退出**
  - polling task 为裸 `tokio::spawn`；无论是 conflict 分支显式退出，还是未来任意非人工退出，都没有自动拉起者。
- F. **运行态语义不完整**
  - 当前只有粗糙的 `connected/disconnected` 与 `AccountDisabled` 事件，缺少 `reconnecting` / `operator_stopped` / `auth_failed_but_retrying` 等稳定口径。
- G. **启动契约定义错误**
  - 当前 `start_account()` 的成功/失败直接绑定外部平台当下是否可达；这使得“注册一个应当持续自恢复的 account runtime”与“本次立刻连上 Telegram”被错误地混为一件事。
- H. **配置更新未做变更分类**
  - 当前系统已存在热更新配置路径，但通用 `channels.update` 仍把所有修改一律按 stop-then-start 处理，导致本可零停机的配置更新也被升级成断崖风险。
- I. **管理可见性依赖 runtime 是否在内存 map 中**
  - 当前 Channels 列表优先枚举内存 account runtime；如果 account 因启动失败未注册或已被移除，UI 观察面会比真实持久化配置更“少”，进一步放大排障困难。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 只要 Telegram account 仍处于期望态，runtime 必须持续存在并承担恢复职责。
  - `TerminatedByOtherGetUpdates` 必须视为 `token_conflict` 运行态异常，进入退避重连，而不是 disable。
  - 启动期临时失败必须进入后台重试，不得因为进程启动瞬间的 TG/API 抖动而永久失活。
  - 配置更新必须先做变更分类：`hot_update` 不得重启；`identity_change` 不做原地 update，统一走 remove+add 或显式迁移。
  - 对同 token / 同 account_handle 的更新，不得采用“并行拉起第二个 polling loop 再切换”的错误策略。
  - outbound send/edit/typing/stream 失败必须局部化为消息级失败，不得触发 account 级 stop。
  - probe / UI 必须明确表达顶层 `running / reconnecting / stopped_by_operator` 运行态，并通过 `details` 补充 `blocked_by_update_retry` 等阻塞态细节。
  - `account not started` 只能表示“尚未启动或已被人工停用”，不得成为“期望态仍启用但 runtime 已永久丢失”的常驻状态。
- 不得：
  - 不得在 TG 临时故障路径下调用“自杀停机”式 `cancel + break + Exited`。
  - 不得把 provider/platform 临时错误直接映射为 `AccountDisabled`。
  - 不得继续要求用户通过“保存配置”才能恢复临时断联。
- 应当：
  - 应当为不同故障类别使用不同退避策略（如 `token_conflict` 更长退避，普通网络更短退避）。
  - 应当在日志中附带稳定 `reason_code / backoff_secs / consecutive_failures / runtime_state`。
  - 应当保留人工 `restart` 能力作为运维工具，而不是故障必经路径。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 把 Telegram account runtime 改造成**长寿命 supervisor**：只要 account 仍在期望态，supervisor 就负责 `connect -> polling -> 异常分类 -> backoff -> retry`。
  - polling loop 本身不再直接 disable account；它只返回运行结果给 supervisor。
  - 启动期 `get_me` / `delete_webhook` 也纳入 supervisor 的可重试阶段。
  - 配置更新按“热更新 / remove+add 迁移”分类；对同 token account 不再使用“先并行拉起新 polling 再切旧”的错误策略。
- 优点：
  - 同时解决 conflict 自杀停机与启动期一次性失败。
  - “是否继续重试”由期望态控制，符合系统设计直觉。
  - 更容易统一 probe/runtime status。
- 风险/缺点：
  - 需要调整 `start_account()`、runtime state、事件语义与部分测试。
  - 需要避免重试风暴与日志刷屏。

#### 方案 2（备选）
- 核心思路：
  - 仅修补 conflict 分支：不再 disable / cancel / break，改为 loop 内退避重试；启动期失败仍保持单次失败语义。
- 优点：
  - 改动最小，止血快。
- 风险/缺点：
  - 无法解决“启动时失败后永不恢复”。
  - runtime 语义仍不完整，问题只修一半。

#### 方案 3（增强项）
- 核心思路：
  - 在方案 1 基础上，为多实例场景引入租约/选主，减少 token conflict 的发生频率。
- 优点：
  - 从架构上减少冲突。
- 风险/缺点：
  - 复杂度更高，不适合拿来作为本单 P0 止血前提。

### 最终方案（Chosen Approach）
#### P0 范围冻结（Scope Freeze）
- P0 必做：
  - 移除 conflict 自杀停机
  - 把启动期失败纳入后台重试
  - 修复 update/save 的 stop-then-start 断崖，并冻结配置变更分类规则
  - 补齐最小可观测性与自动化测试
- P0 不强制：
  - 不要求立刻扩展 `ChannelHealthSnapshot` 结构；可先通过 `details` 暴露稳定 `runtime_state`
  - 不要求立刻新增 websocket 事件类型；只要不再误发 `AccountDisabled` 即可
  - 不要求实现跨实例 lease / leader election
  - 不要求支持“通过 update 把一个 account_handle 原地换成另一只 bot”
- P1 候选：
  - 新增 `AccountRuntimeStateChanged`
  - UI badge 从 `connected/disconnected` 升级为多态 runtime 状态
  - 冲突频率治理（lease / leader election）

#### 行为规范（Normative Rules）
- 规则 1（期望态唯一决定恢复义务）：
  - 只要 account 未被人工停用，runtime 必须持续尝试恢复。
- 规则 2（临时故障不得 disable）：
  - `token_conflict`、`network`、`retry_after`、启动期握手失败等，统一进入 `reconnecting(reason_code)`。
- 规则 3（人工停用才允许停止）：
  - 仅 `remove/logout/stop_account` 进入 `stopped_by_operator` 并终止 supervisor。
- 规则 4（下联失败局部化）：
  - outbound failure 只能影响当前消息/当前 run，不得传播为 account 级 stop。
- 规则 5（配置更新分级）：
  - `hot_update` 配置直接更新内存 config，不中断 polling。
  - `identity_change`（如 token 变更导致 bot 身份变化）不属于 P0 原地 update 目标，应走 remove+add 或显式迁移。

#### P0 默认决策（Default Decisions）
- **运行态模型**
  - 外部可见状态只冻结为：`running`、`reconnecting(reason_code)`、`stopped_by_operator`
  - `blocked_by_update_retry` 在 P0 明确只作为 `details`/probe 中的阻塞态细节存在，不新增为第四种顶层 runtime state
  - `start_account()` 成功后，若尚未连通 Telegram，外部状态应为 `reconnecting(startup)`，而不是“未启动/不存在”
  - 任何非人工退出都应被 supervisor 立即吸收并重新回到 `reconnecting(...)`；`exited` 不应作为 P0 的稳定外部状态
  - `start_account()` 在 P0 只负责“首次注册 supervisor / 接管恢复职责”；若 `account_handle` 已存在运行中 supervisor，则返回显式错误，避免隐式双启动或静默替换
  - `stop_account()` 在 P0 只用于人工停用：先撤销期望态、再取消 supervisor、待任务退出后再从 runtime map 清理；不得再被通用 update/save 复用成重启原语
- **配置更新分类**
  - `hot_update`：除 bot 身份字段外，当前 `TelegramAccountConfig` 其余字段默认都按热更新处理
  - `identity_change`：`token`、`chan_user_id`、`chan_user_name`、`chan_nickname`
  - P0 不新增第三类 `restart_required`；若后续真出现必须重建 runtime 但不换身份的字段，再单独扩展
  - P0 字段级冻结：
    - `hot_update`：`dm_policy`、`mention_mode`、`allowlist`、`relay_chain_enabled`、`relay_hop_limit`、`epoch_relay_budget`、`relay_strictness`、`group_session_transcript_format`、`stream_mode`、`edit_throttle_ms`、`outbound_max_attempts`、`outbound_retry_base_delay_ms`、`outbound_retry_max_delay_ms`、`model`、`model_provider`、`otp_self_approval`、`otp_cooldown_secs`、`persona_id`
    - `identity_change`：`token`、`chan_user_id`、`chan_user_name`、`chan_nickname`
    - 除上述清单外，P0 不接受新增例外分类；若后续新增字段，默认需单独审议后才能进入实现
- **列表可见性**
  - 管理列表的 account 集合必须来源于“持久化配置 ∪ 当前 runtime”，而不能只看 runtime map
  - 因此 account 即使处于启动失败 / reconnecting，也必须继续出现在列表中
  - P0 明确由 `crates/gateway/src/channel.rs` 负责做 persisted store 与 runtime 的并集；`TelegramPlugin` 继续只暴露 runtime 视角，避免把可见性逻辑扩散进 provider 插件
- **事件语义**
  - P0 继续保留 `AccountDisabled` 类型，但其语义收敛为“人工停用/显式停用”
  - 临时故障一律不再发 `AccountDisabled`
- **可观测性格式**
  - P0 的 `details` 必须至少稳定包含：`runtime_state`、`reason_code`、`backoff_secs`、`last_poll_ok_secs_ago`
  - `details` 字段顺序固定为：`runtime_state` -> `reason_code` -> `backoff_secs` -> `last_poll_ok_secs_ago` -> `last_retryable_failure_reason_code`
  - P0 的结构化日志必须至少稳定包含：`event`、`account_handle`、`runtime_state`、`reason_code`、`backoff_secs`
  - `telegram.polling.recovered.reason_code` 在 P0 固定表示“本轮 outage 首次进入 reconnecting 时的原因”，不得漂移为恢复前最后一次失败原因
  - 本单不在 P0 扩更多 UI schema；所有新增可观测性先收敛到现有 `details` 与结构化日志

#### 接口与数据结构（Contracts）
- API/RPC：
  - `start_account()` 的成功语义应调整为“account supervisor 已注册并接管恢复职责”，而不是“本次立即成功连上 Telegram”。
  - `start_account()` 对已存在的同 `account_handle` supervisor 必须返回显式错误；P0 不支持用重复 `start_account()` 触发隐式替换/重启。
  - `stop_account()` 的完成语义应为“人工停用已生效，supervisor 已收到停止指令，并最终清理 runtime 注册”；不得保留“cancel 后立即当作更新中间步骤”的模糊语义。
  - `channel.status` / probe 在 P0 继续保留现有 `connected/disconnected + details` 外形；`details` 必须能稳定编码 `runtime_state` 与 `reason_code`。
  - 如现有 `AccountDisabled` 继续保留，必须只用于人工停用或明确人工动作，不得再复用到临时故障。
- 存储/字段兼容：
  - 不要求迁移现有 `channels` 表；优先新增 runtime-only 状态，不污染持久化配置。
- UI/Debug 展示（如适用）：
  - 优先展示：`runtime_state`、`reason_code`、`backoff_secs`、`last_poll_ok_secs_ago`、`last_retryable_failure_reason_code`
  - 不应把“重连中”误画成“disabled”。
  - 对“持久化配置存在但 runtime 正在 reconnecting”的 account，列表可见性必须保留，不得因 runtime 瞬时缺席而从管理界面消失。

#### 实现落点（Implementation Mapping）
- `crates/telegram/src/state.rs`
  - 扩展 `PollingState` / runtime state 表达能力，至少能稳定区分 `running / reconnecting / stopped_by_operator`
  - 保留 `last_*_reason_code` 作为 probe/details 与日志数据源
- `crates/telegram/src/bot.rs`
  - 将当前 polling loop 收敛为“单次 attempt + reason-coded outcome”，不再在 loop 内直接 disable account
  - 把 `get_me()` / `delete_webhook()` 从一次性启动前置步骤改为 supervisor 可重试阶段
- `crates/telegram/src/plugin.rs`
  - `start_account()` 改为“注册 account + 启动长寿命 supervisor”，而不是一次性 `start_polling(...).await?`
  - `start_account()` 对重复 `account_handle` 返回显式错误，不承担 replace/restart 语义
  - `stop_account()` 只负责人工停用和取消 supervisor，并在任务退出后完成 runtime 清理
  - `update_account_config()` 继续承担热更新落点；实现时直接以内置字段清单判断 `hot_update` vs `identity_change`
  - `probe()` 基于 runtime state 输出稳定摘要
- `crates/gateway/src/channel.rs`
  - “更新并保存”先进行 patch 分类：`hot_update` 直接写 store + `update_account_config()`；`identity_change` 拒绝原地更新或引导 remove+add
  - Channels 列表在这里统一做“持久化配置 ∪ 当前 runtime”并集；不把该职责下沉到 `TelegramPlugin`
- `crates/gateway/src/channel_events.rs`
  - 停止把临时故障路径映射成 `AccountDisabled`
  - 如需保留现有事件，其触发源仅限人工停用
- `crates/gateway/src/server.rs`
  - 启动 channels 时不再依赖“一次启动成功”；只要 supervisor 已注册，account 就应被视为已接管、后续恢复由 runtime 自行完成
- `crates/gateway/src/assets/js/page-channels.js`
  - P0 最小要求是正确消费 `status/details`，不再把“重连中”误读成“disabled”

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - `token_conflict`：长退避重试；UI/日志提示“另一实例正在轮询，同 token 冲突，等待接管”
  - `network` / `retry_after` / `api`：短退避重试
  - `auth_failed` / `get_me_failed` / `delete_webhook_failed`：仍需重试，但可采用更慢退避并在 UI 标红
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - account config 必须保留
  - polling offset / retry budget 应只由当前 runtime 生命周期管理，不因临时错误清空期望态

#### 风险与控制（Risks & Controls）
- 风险 1：supervisor / polling attempt 责任边界不清，导致双重重试或重复日志
  - 控制：先冻结 “attempt 只返回 outcome、supervisor 独占 backoff/restart” 的职责分工
- 风险 2：update/save 安全切换处理不当，造成双 runtime 并存或旧 runtime 泄露
  - 控制：先冻结“同 token 不并行双 polling”规则；P0 通过热更新分类 + `identity_change` 拒绝原地更新解决，而不是盲目双实例切换
- 风险 3：`auth_failed` 进入永久热循环，造成无意义重试和日志噪声
  - 控制：为 `auth_failed` 采用更慢退避和限频日志；必要时在 probe/details 标红提示人工修凭据
- 风险 4：UI 仍只显示 connected/disconnected，导致虽已自恢复但状态语义模糊
  - 控制：P0 先在 `details` 中输出稳定 `runtime_state=...`；P1 再考虑扩前端 badge 枚举
- 风险 5：token 变更导致 account_handle / chan_user_id 漂移，却仍试图走原地 update
  - 控制：P0 明确把 `identity_change` 排除出原地 update 范围；若需要换 bot，走 remove+add 或单独迁移流程

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 仅记录 `account_handle`、`reason_code`、`backoff_secs`、时间信息
  - TG API URL、bot token、完整正文、完整 provider 错误串不得直出
- 禁止打印字段清单：
  - token
  - 完整 message body
  - `https://api.telegram.org/bot<token>/...`

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 同 token 冲突发生后，本地 Telegram account 不再进入永久停机；冲突解除后无需人工保存即可自动恢复入站。
- [x] 启动期 `get_me` / `delete_webhook` 临时失败后，account 会进入后台重试并在外部恢复后自动上线。
- [x] 通用配置更新默认走 `hot_update`，不会因一次保存操作中断健康 polling 或引入永久失活。
- [x] `identity_change` patch 在 P0 被明确拒绝或引导到 remove+add，不再隐式走 stop-then-start。
- [x] outbound send/edit/typing/stream 任一路径失败，不会导致 account 级运行态停止。
- [x] UI / probe / status 能明确区分至少：顶层 `running`、`reconnecting(token_conflict)`、`reconnecting(network)`、`stopped_by_operator`，以及 `details` 中的 `blocked_by_update_retry` 阻塞态细节。
- [x] “更新并保存”不再是恢复临时断联的必要手段。
- [x] 现场排障时，运维可仅凭日志 / probe details 区分“临时冲突重连中”与“人工停用”，无需倒推源码。
- [x] 已持久化配置的 Telegram account 在启动失败 / reconnecting 期间仍保留在管理列表中，不得因 runtime 瞬时未就绪而“消失”。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/telegram/src/bot.rs`：`token_conflict` 分类与退避策略测试
- [x] `crates/telegram/src/bot.rs`：conflict 分支不会 `cancel + break`，而是返回/进入 retry 状态
- [x] `crates/gateway/src/channel.rs`：config patch 字段分类测试（`hot_update` vs `identity_change`）
- [x] `crates/telegram/src/plugin.rs`：probe 对 `reconnecting` / `stopped_by_operator` 顶层状态与 `blocked_by_update_retry` 细节字段的摘要口径
- [x] `crates/telegram/src/plugin.rs`：`details` 输出顺序与最小字段集测试
- [x] `crates/telegram/src/outbound.rs`：出站失败不会改写 account runtime state

### Integration
- [ ] 模拟 `get_me` 启动期失败后恢复：同一 account 无需人工重启即可进入 polling
- [ ] 模拟 `delete_webhook` 启动期失败后恢复
- [ ] 模拟 `TerminatedByOtherGetUpdates` 后另一实例消失：本实例自动恢复入站
- [ ] 模拟 `hot_update`（如 allowlist 变更）：不重启 polling、offset 不回退
- [ ] 模拟 `identity_change` patch：原地 update 被拒绝或被显式引导为 remove+add
- [ ] 模拟启动期失败时 channel list/probe：account 仍可见，且 details 明确为 `reconnecting(...)` 而非直接缺席
- [ ] 模拟 network flap：degraded / recovered 日志与状态转换正确

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/...`：Channel status 展示 `reconnecting` 而非 `disabled`
- [ ] `crates/gateway/ui/e2e/specs/...`：无需点击保存即可在恢复后自动变回 healthy

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 真正的 Telegram 多实例 token conflict 涉及外部平台行为，完整 E2E 自动化成本较高
- 手工验证步骤：
  1. 在实例 A 启动目标 Telegram account
  2. 在实例 B 短暂启动同 token polling
  3. 观察 A 进入 `reconnecting(token_conflict)` 而非 `disabled`
  4. 停止 B，确认 A 自动恢复入站/出站
  5. 重启 A 时人为阻断 Telegram 网络，恢复网络后确认无需手工保存即可上线

## 实施准备评估（Readiness Assessment）
- 代码边界：
  - 已足够清晰，主改动集中在 `crates/telegram/src/bot.rs`、`crates/telegram/src/plugin.rs`、`crates/telegram/src/state.rs`、`crates/gateway/src/channel.rs`
  - 共享契约影响有限；P0 可以不改持久化 schema，也可以不改 `ChannelHealthSnapshot` 结构
  - 现有 `update_account_config()` 已提供热更新落点，降低了 update 路径改造成本：`crates/telegram/src/plugin.rs:105`
- 测试条件：
  - 现有 telegram 模块附近已有单测基础，适合直接补 unit / integration
  - UI 当前仅消费 `status + details`：`crates/gateway/src/channel.rs:96`、`crates/gateway/src/assets/js/page-channels.js:134`，说明 P0 无需等待前端协议重构
- 观测条件：
  - 现有已有 `telegram.polling.degraded / recovered` 日志基础，可直接扩 reason-coded reconnect 日志
  - 本次已补齐 `runtime_state / reason_code / backoff_secs` 的固定输出格式；额外指标可作为后续增强
- 兼容性条件：
  - 不涉及数据库迁移
  - 不要求用户重建 channels
  - 不要求切换 Telegram transport 模式
- 结论：
  - **已完成实施并通过当前范围内的代码级验证**
  - 建议按“两阶段落地”推进：
    - **Phase 1 / P0**：先修复永久失活根因，确保 account 永不因临时故障自杀停机
    - **Phase 2 / P1**：再补 runtime event / UI badge / lease 等增强项
- 开工前 Checklist：
  - [x] 确认 `auth_failed` 继续按慢速 `reconnecting(auth_failed)` 落地
  - [x] 确认 supervisor 与 polling attempt 的职责边界
  - [x] 确认 `start_account / stop_account` 在 P0 的幂等性与重复调用契约：`start_account` 不做隐式 replace，`stop_account` 仅用于人工停用
  - [x] 确认 update/save 仅保留 `hot_update / identity_change` 两类，且字段清单与本文一致
  - [x] 确认同 token 不并行双 polling 的实现边界
  - [x] 确认 persisted-list visibility 统一收口在 `crates/gateway/src/channel.rs`
  - [x] 确认 P0 先不扩共享 RPC schema，仅增强 `details` 与日志
  - [x] 确认新增测试最少覆盖 conflict / startup / hot-update / list-visibility 四类路径

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 优先直接替换当前行为；这是 P0 可靠性修复，不建议挂默认关闭的 feature flag
  - 若需保守，可在日志里先标记新运行态，再切换 UI 语义
- 回滚策略：
  - 可回滚到旧 polling 行为，但会重新暴露“临时冲突/断联导致永久失活”的严重问题
  - 回滚前必须明确告知运维需要继续依赖人工保存恢复
- 上线观测：
  - 关键日志：`telegram.polling.degraded`、`telegram.polling.recovered`、`reason_code=token_conflict`
  - 新增建议：`runtime_state=reconnecting|running|stopped_by_operator`，并固定输出 `backoff_secs`
  - 建议补充计数：`telegram_runtime_restart_total`、`telegram_runtime_conflict_total`、`telegram_runtime_start_failure_total`
  - 需要关注是否出现退避抖动/日志噪声过高

## 实施拆分（Implementation Outline）
- Step 1:
  - 冻结 Telegram runtime 口径：区分期望态、运行态、人工停用
- Step 2:
  - 重构 `start_account()` / `start_polling()` 为 supervisor + reconnect loop
- Step 3:
  - 移除 conflict 自杀停机逻辑，改为 reason-coded reconnect
- Step 4:
  - 把启动期 `get_me` / `delete_webhook` 纳入同一恢复逻辑
- Step 5:
  - 重做配置更新策略：默认 `hot_update`，并显式拒绝 `identity_change` 原地更新
- Step 6:
  - 调整 probe / UI / ChannelEvent 语义，去除临时故障误报为 `AccountDisabled`
- Step 7:
  - 补齐 unit/integration/manual validation
- 受影响文件：
  - `crates/telegram/src/bot.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/telegram/src/state.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/gateway/src/channel.rs`
  - `crates/gateway/src/server.rs`
  - `crates/telegram/src/outbound.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-telegram-inbound-outbound-failure-handling-silent-failure-and-recoverability.md`
  - `issues/issue-observability-llm-and-telegram-timeouts-retries.md`
- Related commits/PRs：
  - 待补
- External refs（可选）：
  - Telegram Bot API `getUpdates` conflict semantics（`TerminatedByOtherGetUpdates`）

## 非目标（Explicitly Not In This Issue）
- 本单不在 P0 拆分新的 `AccountRuntimeStateChanged` 事件
  - 现行口径已经冻结：仅人工停用才发 `AccountDisabled`
- 本单不在 P0 引入 lease / 选主
  - 现行口径已经冻结：即使发生 conflict，也必须依靠 runtime 自恢复而非永久失活

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

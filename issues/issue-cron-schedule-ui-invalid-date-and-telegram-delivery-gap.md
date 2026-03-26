# SUPERSEDED BY `issues/issue-cron-system-governance-one-cut.md`
#
# 2026-03-27 决策更新：
# - cron/heartbeat 的系统级治理已收敛为 one-cut 主单：`issues/issue-cron-system-governance-one-cut.md`。
# - 本单只保留旧问题现象与旧路径证据，不再作为实施依据。

# Issue: cron 定时提醒的 UI 展示异常与 Telegram 投递缺口（cron / telegram）

## 实施现状（Status）【增量更新主入口】
- Status: SUPERSEDED（不再推进；以新主单为唯一实施准绳）
- Priority: P1
- Updated: 2026-03-27
- Owners: TBD
- Components: cron / gateway / ui / telegram
- Affected providers/models: openai-responses::gpt-5.2

**已实现（如有，写日期）**
- 无

**已覆盖测试（如有）**
- 无

**已知差异/后续优化（非阻塞）**
- N/A：本单已 superseded。若需实施与验证，以 `issues/issue-cron-system-governance-one-cut.md` 为准。

---

## 背景（Background）
- 场景：用户通过自然语言让 agent 创建一个“北京时间 12:00 吃饭提醒”，创建时已经明确强调时区为东八区。
- 现象：Web UI 的 `Schedule` 列显示 `At Invalid Date`；到点后 cron 确实执行了，但用户没有收到 Telegram 定时提醒。
- 约束：
  - 当前问题以“代码现状 review + 问题收敛”为主，先不改重试/轮询机制。
  - 需要区分两类问题：UI 展示错误，和提醒触发后投递目标走错。
- Out of scope：
  - Telegram polling warning 的网络稳定性治理
  - LLM / Telegram 重试策略重构
  - 其它无关 cron 功能优化

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **cron 触发**（主称呼）：调度器判定任务到期并开始执行 job。
  - Why：这是“有没有到点”的判断口径。
  - Not：不等于“已经发出 Telegram 消息”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：到点执行 / scheduler fire

- **主会话注入**（主称呼）：把 cron 文本当作一条普通消息送进 `main` 会话，让 LLM 在主会话里继续推理。
  - Why：这是当前 `SystemEvent` 的实际执行路径。
  - Not：不等于“发 Telegram 提醒给用户”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：main session inject

- **Telegram 投递**（主称呼）：把 cron 结果按指定频道/账号/聊天目标真正发到 Telegram。
  - Why：这是“用户在 Telegram 收到提醒”的最终口径。
  - Not：不等于“cron 已运行”或“LLM 已生成文本”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：channel delivery / outbound send

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] Web UI 必须正确显示 `At` 类型 cron 的时间，不得出现无意义的 `At Invalid Date`。
- [ ] cron 到点后，若该提醒意图是“发给 Telegram 用户/聊天”，则必须真正投递到对应 Telegram 目标，而不是只注入 `main` 会话。
- [ ] 调度、推理、投递这三个阶段的行为边界必须清楚，避免“已经执行了但用户没收到”时只能靠猜。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须能明确区分“cron 已触发”和“Telegram 已投递”。
  - 不得把“主会话注入”误当成“已发送 Telegram 提醒”。
  - 不得因为前后端字段名不一致导致 UI 时间展示失真。
- 兼容性：如历史上已经存在 camelCase / snake_case 混用数据，修复时应兼容读取，避免旧 job 在 UI 中继续异常。
- 可观测性：命中 cron 执行、主会话注入、Telegram 实际投递/未投递时，必须能从日志或 UI 判断停在哪一层。
- 安全与隐私：日志避免打印敏感 token、长正文；正文如需排障，仅做短预览。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. Web UI 的 `Schedule` 列显示 `At Invalid Date`，与用户设定的“北京时间 12:00”不符。
2. cron 到点后，日志显示任务确实执行了，但 Telegram 侧没有收到提醒。
3. 到点后系统反而把提醒文本送进了主会话，继续跑 LLM，并产生了与“吃饭提醒”无关的后续行为。

### 影响（Impact）
- 用户体验：
  - 用户在 UI 上看不到正确的提醒时间。
  - 用户以为自己创建了 Telegram 定时提醒，但到点没有收到消息。
- 可靠性：
  - 调度层“已触发”与投递层“未送达”被混在一起，功能表面上像是“偶发失灵”，实际是路径不对。
- 排障成本：
  - 没有把“执行”和“投递”明确拆开时，用户会误以为是时区、轮询或网络问题。

### 复现步骤（Reproduction）
1. 让 agent 创建一个“北京时间 12:00 吃饭提醒”的 cron 任务。
2. 打开 Web UI 的 Cron Jobs 列表，观察 `Schedule` 列。
3. 到 2026-03-10 12:00（Asia/Shanghai）后，观察：
   - cron service 日志
   - gateway chat 日志
   - Telegram 实际收件情况
4. 期望 vs 实际：
   - 期望：UI 显示正确时间，且到点后用户在 Telegram 收到提醒。
   - 实际：UI 显示 `At Invalid Date`；cron 已触发，但提醒进入 `main` 会话，没有实际投递到 Telegram。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/assets/js/page-crons.js:100`：`formatSchedule()` 读取 `sched.atMs` / `sched.everyMs`，而后端实际字段为 `at_ms` / `every_ms`，会导致 `new Date(undefined)`，最终出现 `At Invalid Date`。
  - `crates/gateway/src/assets/js/page-crons.js:567`：`parseScheduleFromForm()` 提交时也写入 `atMs` / `everyMs`，前后端字段名口径不一致。
  - `crates/cron/src/types.rs:8`：`CronSchedule::At { at_ms }`、`Every { every_ms, anchor_ms }` 明确使用 snake_case 字段。
  - `crates/cron/src/service.rs:494`：`AgentTurn` 执行时确实把 `deliver/channel/to` 带入了 `AgentTurnRequest`。
  - `crates/gateway/src/server.rs:1371`：`on_agent_turn` 实际只做 `chat.send_sync(...)` 到 `cron:*` session，没有消费 `req.deliver`、`req.channel`、`req.to`。
  - `crates/gateway/src/server.rs:1356`：`SystemEvent` 固定走 `chat.send({ text })` 注入主会话，不带 Telegram 目标。
- 日志证据：
  - `2026-03-10T04:16:24`：`moltis_cron::service: executing cron job ...`，说明 cron 确实到点执行。
  - `2026-03-10T04:16:29`：`moltis_gateway::chat: chat.send ... session=main user_message=提醒：到 12:00 了，去吃饭～`，说明当前执行路径是主会话注入，不是 Telegram 投递。
  - `2026-03-10T04:16:53`：agent 后续回了“已处理重复提醒”，进一步证明提醒文本被送进了 LLM 主会话工作流。
- 当前测试覆盖：
  - 已有：`crates/cron/src/types.rs` 对 `CronSchedule` roundtrip 有基础序列化测试。
  - 缺口：
    - UI 没有覆盖 `At` 类型 schedule 展示。
    - cron 到 Telegram 的投递路径没有自动化测试证明。
    - `SystemEvent` 与 `AgentTurn(deliver=true)` 的行为边界缺少回归测试。

## 根因分析（Root Cause）
- A. UI 字段名口径不一致
  - 后端 `CronSchedule` 使用 snake_case 字段；
  - 前端 `page-crons.js` 却按 camelCase 读取和提交 `atMs/everyMs`；
  - 结果是 UI 渲染时读不到值，显示 `Invalid Date`。
- B. cron 执行与 Telegram 投递之间没有打通
  - 调度层支持 `AgentTurnRequest { deliver, channel, to }`；
  - 但网关执行层没有真正使用这些字段；
  - 所以“需要发 Telegram”的 job，到了执行阶段仍只是在内部 session 里跑 LLM。
- C. `SystemEvent` 当前语义就是“主会话注入”，并不适合拿来承载 Telegram 定时提醒
  - 一旦把提醒走成 `SystemEvent`，就只会进入 `main`；
  - 用户想要的是“到点收到 Telegram 消息”，不是“让 main 会话继续思考一轮”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - `At` / `Every` 类型 schedule 在 UI 中必须按后端真实字段正确显示。
  - 若 cron job 的目标是 Telegram 提醒，则执行成功后必须真正走 Telegram 投递链路。
  - 日志必须能区分：`cron triggered`、`llm turn started/completed`、`telegram outbound sent/failed/skipped`。
- 不得：
  - 不得把“主会话注入成功”当成“Telegram 已提醒用户”。
  - 不得让 UI 因字段名不一致显示 `Invalid Date` 之类无效信息。
- 应当：
  - 应当兼容识别旧 UI 可能写出的 camelCase 字段，避免历史 job 在修复后仍不可读。
  - 应当在 cron 列表或调试信息里明确展示该 job 的执行目标类型。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 先修复前端 schedule 字段名读取/提交问题；
  - 再把“cron 到 Telegram”的投递链路明确打通，而不是继续借道 `main` 会话。
- 优点：
  - 问题边界清晰，能同时解决“看错”和“没发到”两件事。
  - 调度、推理、投递职责分离，后续排障清楚。
- 风险/缺点：
  - 需要明确 `SystemEvent` 和 `AgentTurn(deliver=true)` 的最终职责边界。

#### 方案 2（备选）
- 核心思路：继续保留 `SystemEvent -> main session` 的方式，只在 prompt 或后续逻辑里想办法让 agent 再发 Telegram。
- 缺点：
  - 路径绕，语义混乱；
  - 到点提醒会受主会话上下文干扰；
  - 不能保证一定送达 Telegram；
  - 排障仍然很差。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：UI 以“后端真实字段”为准，`At/Every` 显示和编辑必须使用与 `CronSchedule` 一致的字段口径。
- 规则 2：面向 Telegram 的 cron 提醒，必须有显式的 Telegram 目标和投递动作，不能只注入 `main` 会话。
- 规则 3：`SystemEvent` 默认语义冻结为“主会话注入”；它不是 Telegram reminder transport。
- 规则 4：`AgentTurn(deliver=true, channel/to=...)` 若保留在协议中，执行层必须真正消费并生效；否则应删除或改名，避免假能力。

#### 接口与数据结构（Contracts）
- API/RPC：
  - `cron.list` / `cron.add` / `cron.update` 中的 `schedule` 字段口径必须统一。
- 存储/字段兼容：
  - 读路径应兼容旧 camelCase；
  - 写路径统一为一套口径，避免继续扩散混用。
- UI/Debug 展示（如适用）：
  - `Schedule` 列正确展示时间；
  - 可补充 `Target` / `Delivery` 信息，明确是 `main` 还是 `telegram`。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - UI 解析失败时，不应显示 `Invalid Date` 这种裸错误；至少应显示可理解的 fallback。
  - Telegram 目标缺失或投递未实现时，应显式记录 reason code，而不是“静默只跑 main”。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - cron 执行完成后，无论成功或失败，都应准确更新 run record 和 last status。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 日志中避免打印完整提醒正文和敏感目标标识，可做短预览。
- 禁止打印字段清单：
  - Telegram token
  - 长消息全文

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 创建“北京时间 12:00”提醒后，Web UI `Schedule` 正确显示时间，不再出现 `At Invalid Date`。
- [ ] 同一提醒到点后，用户能在预期的 Telegram 目标中实际收到提醒。
- [ ] 日志能明确看出 cron 是“已触发但未投递”，还是“已投递成功”。
- [ ] `SystemEvent` 与 Telegram reminder 的行为边界在代码和文档中保持一致。
- [ ] 不影响现有非 Telegram 的普通 cron / heartbeat 路径。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/gateway/src/assets/js/page-crons.js`：覆盖 `At` / `Every` schedule 的字段解析与展示。
- [ ] `crates/gateway/src/server.rs`：覆盖 `AgentTurn(deliver/channel/to)` 被真正消费的路径。
- [ ] `crates/cron/src/types.rs` / `crates/cron/src/service.rs`：覆盖新旧字段兼容与执行语义边界。

### Integration
- [ ] 新增 cron reminder 集成测试：创建 job -> 到点执行 -> 断言进入正确投递路径。

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/cron-reminder-display-and-delivery.spec.js`：覆盖 schedule 展示与提醒创建后的基本链路。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - Telegram 实际外发可能仍需手工联调环境验证。
- 手工验证步骤：
  1. 在 Telegram 会话中创建一个 1 分钟后的提醒；
  2. 在 Web UI 确认 `Schedule` 显示正确；
  3. 等待触发，观察 cron/gateway/telegram 三段日志；
  4. 确认 Telegram 实际收到消息；
  5. 若失败，确认是停在调度、LLM、还是 outbound。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先修 UI 与投递链路，默认开启，无需额外 feature flag。
- 回滚策略：
  - UI 部分可独立回滚；
  - 投递链路若有风险，可临时退回到“仅主会话注入”，但必须保留显式日志，避免再次误判为“已提醒”。
- 上线观测：
  - `moltis_cron::service` 的 job execute/finish 日志
  - `moltis_gateway::chat` 的 session send / channel delivery 日志
  - `moltis_telegram::outbound` 的 sent/failed 日志

## 实施拆分（Implementation Outline）
- Step 1:
  - 修复 `page-crons.js` 的 schedule 字段读取/提交口径，补前端测试。
- Step 2:
  - 明确 `SystemEvent` 与 Telegram reminder 的职责边界，收敛协议。
- Step 3:
  - 打通 `AgentTurn(deliver/channel/to)` 的真实投递实现，补日志与测试。
- 受影响文件：
  - `crates/gateway/src/assets/js/page-crons.js`
  - `crates/cron/src/types.rs`
  - `crates/cron/src/service.rs`
  - `crates/gateway/src/server.rs`
  - `crates/telegram/src/outbound.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - 无
- Related commits/PRs：
  - 待补
- External refs（可选）：
  - 无

## 未决问题（Open Questions）
- Q1: 面向 Telegram 的自然语言“提醒我……”最终应统一落到哪一种 payload 语义上：扩展 `SystemEvent`，还是规范化为 `AgentTurn + deliver`？
- Q2: Telegram reminder 的目标口径是否需要冻结到“创建该提醒的原始 channel/session”，还是允许显式选择其它 Telegram 目标？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

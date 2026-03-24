# Issue: Telegram 配置事实源分裂导致 owner 不清（single_source_of_truth / one_cut）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Updated: 2026-03-24
- Owners: Codex
- Components: gateway / telegram / config / ui / people
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 无；本单当前目标是先把问题、边界、取舍与验收口径冻结，供审阅后再实施

**已覆盖测试（如有）**
- 无；本单尚未进入实现阶段

**已知差异/后续优化（非阻塞）**
- 本单不讨论 `dm_scope` / `group_scope` 的具体语义设计；它们的取值集合已在 `crates/config/src/telegram.rs:8` 冻结
- 本单不顺手处理 Telegram 群聊转写、record/dispatch 或 speaker 识别逻辑；只聚焦“配置 owner 收口”

---

## 背景（Background）
- 场景：当前 Telegram 相关配置事实分散在 `PEOPLE.md`、`moltis.toml`、SQLite `channels` 表与 UI 表单之间，导致“谁是 owner、谁只是镜像/入口、谁优先生效”不清晰。
- 约束：
  - 必须遵循第一性原则：同一类事实只能有一个 owner。
  - 必须遵循不后向兼容原则：一旦确定 owner，旧路径不得继续作为并行生效来源。
  - 必须遵循唯一事实来源原则：不能再保留“config 先启动，DB 补剩余”的混源启动模型。
  - 必须遵循核心测试覆盖原则：无论最终选 DB 还是配置文件，都要补齐启动、更新、拒绝与 UI/接口覆盖。
- Out of scope：
  - 不重新设计 `dm_scope` / `group_scope` 的语义。
  - 不重做 Telegram adapter 的会话/派发机制。
  - 不扩展到 Telegram 以外的渠道。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **配置 owner**（主称呼）：某一类配置事实的唯一写入口与唯一生效来源。
  - Why：没有 owner，系统就无法回答“改哪里才算真改”。
  - Not：不是“若干入口里优先级更高的那个”，也不是“先读这个、再补另一个”的 precedence 规则。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：single source / authoritative owner

- **Telegram bot 账号配置**（主称呼）：某个具体 Telegram bot 实例的运行配置，例如 `token`、`agent_id`、`dm_policy`、`allowlist`、`dm_scope`、`group_scope`、`model` 等。
  - Why：这是当前分裂最严重的一类事实。
  - Not：不是 Telegram 渠道级共享策略，也不是 people roster / identity link。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：bot runtime config / account config

- **Telegram 渠道级共享策略**（主称呼）：对整个 Telegram 渠道共享生效的配置，例如 `bot_dispatch_cycle_budget`。
  - Why：它天然不是 per-bot 事实，应与 bot 账号配置分开治理。
  - Not：不是某一个 bot 的私有配置。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：channel-wide policy

- **identity link**（主称呼）：`agent_id` 与 Telegram 用户身份之间的映射关系。
  - Why：它回答“这个 Telegram speaker 对应哪个 agent/person”。
  - Not：不是 bot token、allowlist 或 `dm_scope/group_scope` 之类的 bot runtime 配置。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：people link / speaker link

- **混源启动**（主称呼）：系统在启动时同时从多条 owner 候选路径读取同类事实，并用 precedence 或补集规则拼出最终运行态。
  - Why：这正是当前 Telegram bot 账号配置的核心问题。
  - Not：不是只读缓存，也不是导出/展示镜像。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：dual-source startup / merged owner

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 为 Telegram 配置事实建立清晰 owner 边界
- [ ] 删除 Telegram bot 账号配置的混源启动模型
- [ ] 让 UI、文件、DB 与 `PEOPLE.md` 的职责边界一致
- [ ] 让用户能明确知道“改哪里才会真正生效”

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须让每一类 Telegram 相关事实只有一个 owner
  - 必须把“identity link”“渠道级共享策略”“bot 账号配置”三类事实分开判断 owner
  - 必须删除当前“配置文件先启动 + 数据库补剩余”的同类事实拼接模型
  - 必须让 `dm_scope` / `group_scope` 这类真实生效字段在 owner 路径中可见、可改、可验证
  - 不得继续保留文件与 DB 同时都能作为 Telegram bot 账号配置生效来源
  - 不得用 fallback、alias、自动双写、静默同步来掩盖 owner 分裂
  - 不得让 UI 继续编辑一部分 bot 字段，而把另一部分同层级字段隐藏在 owner 之外
- 兼容性：
  - 本单按 hard-cut/one-cut 收口设计，不以保留旧混源行为为目标
  - 一旦最终 owner 确定，非 owner 路径上的 Telegram bot 账号事实必须报错或强告警，不做 silent precedence
- 可观测性：
  - 启动时若检测到非 owner 路径仍携带 Telegram bot 账号事实，必须有结构化日志
  - owner 拒绝/冲突必须有固定 `reason_code`
- 安全与隐私：
  - 文档与日志讨论 token 时只能说字段，不打印真实 token
  - 真实 bot 身份探测字段（如 `chan_user_id`）可记录，但不得暴露敏感凭据

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. Telegram bot 账号配置目前既可以来自 `moltis.toml`，也可以来自 SQLite `channels` 表，启动时还会把两边拼起来。
2. `identity link` 又来自 `PEOPLE.md`，与 bot 账号配置不在同一 owner 面上。
3. UI 的 Telegram 账号编辑入口只暴露了 `dm_policy`、`allowlist`、group dispatch 开关等少数字段，`dm_scope` / `group_scope` 虽然真实生效，但 UI 完全看不到、也改不了。
4. 因为 owner 不清，用户很难判断“该改配置文件、改 UI，还是改 DB”；系统也难以给出明确拒绝口径。

### 影响（Impact）
- 用户体验：
  - 用户无法一眼看清 Telegram bot 的完整配置真相
  - UI 可编辑项与真实生效项不对称，容易形成“改了但不是我以为的那层”的错觉
- 可靠性：
  - 同类事实跨文件与 DB 分裂时，运行态依赖 precedence 与补集规则，容易出现隐藏状态
  - 未来继续扩字段时，会进一步扩大 UI、DB、文件三者漂移
- 排障成本：
  - 很难快速回答某个 bot 的 `dm_scope/group_scope/agent_id` 到底从哪里来
  - 很难判断一个字段改完为何没生效，是 owner 错了、字段没暴露，还是被另一来源覆盖了

### 复现步骤（Reproduction）
1. 阅读启动路径，观察 Telegram plugin 在启动时先读配置文件账号，再读数据库 stored channels
2. 阅读 `PEOPLE.md` 提取路径，观察 identity link 由另一条独立来源注入 Telegram plugin
3. 阅读 UI Telegram 账号表单，观察其未暴露 `dm_scope` / `group_scope`
4. 期望 vs 实际：
   - 期望：每类事实只有一个 owner，且所有真实生效字段都在 owner 路径中可见可改
   - 实际：当前 Telegram bot 账号配置存在文件/DB 混源，identity link 独立于 bot config，UI 只暴露部分字段

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/server.rs:1854`：启动时先从配置文件启动 Telegram 账号
  - `crates/gateway/src/server.rs:1867`：随后再加载数据库 stored channels，并启动那些“不在配置文件里”的账号
  - `crates/gateway/src/people.rs:139`：Telegram `identity link` 从 `PEOPLE.md` 解析，不属于 bot 账号配置入口
  - `crates/gateway/src/server.rs:1840`：启动时把 `PEOPLE.md` 解析出的 identity links 注入 Telegram plugin
  - `crates/gateway/src/server.rs:1852`：Telegram 渠道级共享策略 `bot_dispatch_cycle_budget` 从配置文件注入
  - `crates/gateway/src/channel_store.rs:79`：stored channel 把整段 `config` JSON 持久化到 SQLite
  - `crates/telegram/src/plugin.rs:175`：Telegram plugin 启动账号时，直接把 JSON 反序列化为完整 `TelegramAccountConfig`
  - `crates/gateway/src/channel.rs:261`：`channels.add` 入口按完整 `TelegramAccountConfig` 校验并持久化
  - `crates/gateway/src/channel.rs:367`：`channels.update` 同样按完整 `TelegramAccountConfig` 校验并持久化
  - `crates/config/src/telegram.rs:11`：`dm_scope` 是 TelegramAccountConfig 的正式字段
  - `crates/config/src/telegram.rs:26`：`group_scope` 是 TelegramAccountConfig 的正式字段
  - `crates/gateway/src/assets/js/page-channels.js:321`：新增 Telegram bot UI 草稿字段不包含 `dm_scope/group_scope`
  - `crates/gateway/src/assets/js/page-channels.js:401`：`channels.add` 的 UI payload 未发送 `dm_scope/group_scope`
  - `crates/gateway/src/assets/js/page-channels.js:333`：编辑 Telegram bot UI 草稿字段不包含 `dm_scope/group_scope`
  - `crates/gateway/src/assets/js/page-channels.js:559`：`channels.update` 的 UI payload 未发送 `dm_scope/group_scope`
  - `crates/gateway/src/assets/js/onboarding-view.js:2023`：onboarding 连接 Telegram bot 时同样未发送 `dm_scope/group_scope`
- 配置/协议证据（必要时）：
  - `crates/config/src/telegram.rs:53`：`TelegramChannelsConfig` 只表达 Telegram 渠道级共享配置与账号映射
- 当前测试覆盖：
  - 已有：`channels.add/update` 与 Telegram 启动路径具备基础测试
  - 缺口：没有测试冻结“Telegram bot 账号配置 owner 只能有一个”“UI 暴露字段必须与 owner 生效字段一致”“非 owner 路径应如何拒绝”

## 根因分析（Root Cause）
- A. Telegram 相关事实没有先做 owner 切分，直接沿实现便利分散到 `PEOPLE.md`、配置文件、DB 与 UI 多条路径
- B. Telegram bot 账号配置路径历史上同时支持“文件启动”和“UI/DB 持久化启动”，但没有硬切换掉其中一条
- C. UI 只覆盖了早期少数字段，没有随着 `TelegramAccountConfig` 扩展同步补齐同层级入口
- D. 当前系统靠 precedence（配置先、DB 后）而不是 owner 来维持运行，导致混源状态长期存在

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 必须先冻结 Telegram 相关事实的分类边界：
    - `identity link`
    - 渠道级共享策略
    - bot 账号配置
  - 必须为每一类事实指定唯一 owner
  - 必须让 Telegram bot 账号配置只剩一条生效路径：**要么 DB，要么配置文件**
  - 必须让 `dm_scope` / `group_scope` 这类真实生效字段在最终 owner 路径中可见、可改、可验证
  - 必须补齐 owner 冲突或 legacy 路径继续使用时的结构化日志
- 不得：
  - 不得继续保留“配置先、DB 后”的 Telegram bot 账号混源启动
  - 不得保留“UI 可编辑部分 bot 字段、另一些同层级字段只能藏在 DB/文件里”的不对称状态
  - 不得通过双写、自动同步、fallback 或 precedence 继续维持双 owner
  - 不得为了兼容旧路径而保留长期并行生效模型
- 应当：
  - 应当把最终 rejected 的非 owner 路径变成显式失败或强告警，而不是悄悄忽略
  - 应当让文档、模板、UI 与运行时 owner 边界同步

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1：Telegram bot 账号配置全部以 DB 为 owner
- 核心思路：
  - Telegram 渠道级共享策略继续留在 `moltis.toml`
  - identity link 继续留在 `PEOPLE.md`
  - 所有 Telegram bot 账号配置统一只存在于 stored channel DB
  - UI 成为 DB owner 的主编辑入口，必须补齐完整字段
- 优点：
  - 更像产品化控制面
  - 运行态、热更新与持久化路径天然一致
  - 不需要用户同时维护多个 bot 账号块
- 风险/缺点：
  - UI 需要系统性补齐，不是只加两个下拉框就结束
  - 当前 UI 结构与 onboarding 都偏轻量，需要重构字段分组与说明
  - 若 UI 未补齐前硬切到 DB owner，会产生“owner 在 DB，但用户无完整入口”的断层

#### 方案 2：Telegram bot 账号配置全部以配置文件为 owner
- 核心思路：
  - Telegram 渠道级共享策略与 Telegram bot 账号配置统一收口到 `moltis.toml`
  - identity link 继续留在 `PEOPLE.md`
  - stored channel DB 不再作为 Telegram bot 账号配置 owner
  - UI 对 Telegram bot 改为只读展示或显式提示“请去配置文件修改”
- 优点：
  - 最快实现唯一事实源
  - 所有 bot 字段都可以立刻完整摊平，不受当前 UI 字段缺失限制
  - 更适合当前“先收口真相，再重做 UI”的阶段
- 风险/缺点：
  - 用户编辑体验较差
  - 与目前 channel add/update 的 UI/DB 产品流不一致
  - 后续若还想做完整 UI 管理，需要再从文件 owner 迁移到 DB owner

#### 方案 3：继续保留文件 + DB 混源，但补说明/补 UI
- 核心思路：维持现状，只把规则说明得更清楚，或给 UI 补一部分隐藏字段
- 风险/缺点：
  - 本质上仍然没有唯一 owner
  - precedence 仍在，事实分裂仍在
  - 与第一性原则、one-cut、不后向兼容、唯一事实来源原则直接冲突

### 最终方案（Chosen Approach）
- 当前阶段**暂不冻结最终 owner 介质**（DB 或配置文件二选一），先冻结决策边界：
  - 方案 3 直接淘汰，不进入实施候选
  - 只允许在方案 1 与方案 2 中二选一
  - 审阅重点是：哪个方案更符合当前阶段的第一性原则与实际落地成本

#### 行为规范（Normative Rules）
- 规则 1：同一类 Telegram 事实只能有一个 owner；owner 之外的路径不得再生效
- 规则 2：Telegram bot 账号配置必须整体收口；不能把 `token/agent_id` 放一处、`dm_scope/group_scope` 放另一处、UI 再只暴露一半
- 规则 3：无论最终选 DB owner 还是配置文件 owner，`identity link` 与渠道级共享策略都继续单独评估，不自动与 bot 账号配置绑成一处
- 规则 4：一旦 owner 确定，非 owner 路径命中 Telegram bot 账号配置时必须直接拒绝或强告警，不做 silent precedence

#### 接口与数据结构（Contracts）
- API/RPC：
  - 若选 DB owner：`channels.add/update/remove` 继续作为 Telegram bot 账号配置主入口，但必须补齐完整字段
  - 若选配置文件 owner：`channels.add/update/remove` 对 Telegram 应改为拒绝或只读，不得继续写入 Telegram bot 配置事实
- 存储/字段兼容：
  - 若选 DB owner：`StoredChannel.config` 必须覆盖完整 `TelegramAccountConfig`
  - 若选配置文件 owner：`moltis.toml` 中的 `[channels.telegram.<bot>]` 必须覆盖完整 `TelegramAccountConfig`
- UI/Debug 展示（如适用）：
  - 若选 DB owner：UI 表单必须暴露包括 `dm_scope/group_scope` 在内的完整关键字段
  - 若选配置文件 owner：UI 必须明确展示这些字段来自配置文件，而非可编辑本地状态

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 选 DB owner 后，若配置文件仍携带 Telegram bot 账号块：启动拒绝或强告警
  - 选配置文件 owner 后，若 DB 仍携带 Telegram stored channel：启动拒绝或强告警
  - 若 UI 尝试修改一个不再归 UI/DB 管的 Telegram 字段：接口直接拒绝
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 旧 owner 数据不得继续参与 Telegram bot 账号启动
  - 是否保留旧数据副本用于人工迁移，可作为实施细节，但不得继续生效

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - token 不得在日志/UI 中明文显示
  - owner 冲突日志只记录 account handle、字段名、路径与 remediation
- 禁止打印字段清单：
  - token
  - 其他凭据型 secret

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] Telegram 相关三类事实（identity link / 渠道级共享策略 / bot 账号配置）的 owner 已冻结
- [ ] Telegram bot 账号配置不再存在文件与 DB 双生效路径
- [ ] `dm_scope` / `group_scope` 在最终 owner 路径中可见、可改、可验证
- [ ] UI、文档、模板与运行时行为对 owner 的描述一致
- [ ] 非 owner 路径命中 Telegram bot 账号配置时，不会 silent precedence，而是直接拒绝或强告警

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/gateway/src/server.rs`：覆盖 Telegram 启动时不会再混合读取两个 Telegram bot 账号 owner
- [ ] `crates/gateway/src/channel.rs`：覆盖非 owner 路径对 Telegram bot 配置的拒绝/强告警
- [ ] `crates/config/src/telegram.rs` / 相邻测试：覆盖最终 owner 形状包含完整 `TelegramAccountConfig` 关键字段

### Integration
- [ ] 若选 DB owner：覆盖 `channels.add/update` 后，`dm_scope/group_scope` 可持久化、重启后保持、生效路径唯一
- [ ] 若选配置文件 owner：覆盖 `moltis.toml` 中 Telegram bot 账号配置可启动、生效路径唯一、DB 中旧 Telegram channel 不再参与启动

### UI E2E（Playwright，如适用）
- [ ] 若选 DB owner：`crates/gateway/ui/e2e/specs/channels.spec.js` 增补 `dm_scope/group_scope` 的新增与编辑覆盖
- [ ] 若选配置文件 owner：覆盖 UI 对 Telegram bot 配置的只读/拒绝提示，不再伪装成完整可编辑入口

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 最终 owner 路径尚未冻结，故测试需待方案确定后落地
- 手工验证步骤：
  - 分别准备“仅文件有 Telegram bot 配置”“仅 DB 有 Telegram bot 配置”“两边同时有 Telegram bot 配置”三组场景
  - 验证最终实现只允许一种 owner 生效，其他路径直接拒绝或强告警
  - 验证 `dm_scope/group_scope` 能从最终 owner 路径完整读取并实际影响运行时

## 发布与回滚（Rollout & Rollback）
- 发布策略：先冻结 owner，再一次性 hard-cut，不做长期灰度双写
- 回滚策略：仅在审阅确认最终 owner 路径不可落地时，整体回滚本单；不得在实现中保留长期双 owner 作为“回滚保险”
- 上线观测：
  - 关注 Telegram 启动日志里 owner 冲突/拒绝相关 `reason_code`
  - 关注 UI/接口对非 owner 路径更新的拒绝率

## 实施拆分（Implementation Outline）
- Step 1: 冻结 Telegram 三类事实的 owner 边界，明确方案 1 或方案 2
- Step 2: 删除 Telegram bot 账号混源启动路径
- Step 3: 补齐最终 owner 路径的 UI/接口/模板/文档与测试
- 受影响文件：
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/channel.rs`
  - `crates/gateway/src/channel_store.rs`
  - `crates/gateway/src/people.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/config/src/telegram.rs`
  - `crates/gateway/src/assets/js/page-channels.js`
  - `crates/gateway/src/assets/js/onboarding-view.js`
  - `crates/config/src/template.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-v3-telegram-adapter-and-session-semantics.md`
  - `issues/issue-v3-telegram-speaker-link-identity-and-tg-gst-one-cut.md`
  - `docs/src/refactor/dm-scope.md`
  - `docs/src/refactor/group-scope.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- Q1: Telegram bot 账号配置的唯一 owner 最终选 DB 还是配置文件？
- Q2: 若选配置文件 owner，UI 对 Telegram channel 的增删改能力要保留到什么程度？
- Q3: 若选 DB owner，UI 是否需要顺手补齐完整字段，还是先提供结构化 advanced/raw editor 再逐步表单化？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

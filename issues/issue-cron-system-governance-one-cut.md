# Issue: cron / heartbeat 系统级治理 one-cut（owner / contract / lifecycle / ui / observability / tests）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P0
- Updated: 2026-03-27
- Checklist discipline: 每次增量更新除补“已实现 / 已覆盖测试”外，必须同步勾选正文里对应的 checklist；禁止出现文首已完成、正文 TODO 未更新的漂移
- Owners: TBD
- Components: cron / gateway / ui / agents / config / telegram
- Affected providers/models: openai-responses::gpt-5.2

**已实现（如有，必须逐条写日期）**
- 无

**已覆盖测试（如有）**
- 无

**已知差异/后续优化（非阻塞）**
- 本单已完成对 `docs/plans/2026-03-26-cron-heartbeat-model-design.md` 的实施回写；后续代码实施以本单为唯一实施准绳，设计稿保留为设计依据与追溯依据。
- `issues/issue-session-page-cron-session-delete-entry-missing.md` 已过时，仅保留旧模型问题证据；不再作为实施依据。

---

## 背景（Background）
- 场景：当前仓库里的 `cron`、`heartbeat`、会话、投递、持久化、UI 表面仍然混有多套历史语义；继续在旧单上局部修补，只会让实现和评审继续漂移。
- 约束：
  - 本单是后续实施主单，目标模型的设计依据来自 `docs/plans/2026-03-26-cron-heartbeat-model-design.md`，但后续代码实施与 review 只以本单为唯一实施准绳。
  - 本单按 strict one-cut 执行，不保留 backward compatibility，不保留 fallback、alias、shim、双写、双读、silent degrade。
  - 外部 JSON / RPC / UI 合同统一使用 `camelCase`；内部代码标识符统一使用 `snake_case`。
- Out of scope：
  - 不在本单重新讨论产品需求方向；目标模型已由本单冻结。
  - 不在本单顺手做“未来可扩展”的抽象层、总线、通用 channel 框架。
  - 不在本单保留旧 `session cron`、根级 `HEARTBEAT.md`、legacy store 的兼容尾巴。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **设计依据**（主称呼）：`docs/plans/2026-03-26-cron-heartbeat-model-design.md`
  - Why：该文负责保留目标模型的设计依据与推导背景。
  - Not：它不再是后续代码实施时的主执行清单。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：设计稿 / 历史冻结合同

- **实施主单**（主称呼）：本文件
  - Why：本单已承接并冻结后续实施所需的 owner、边界、失败语义、测试面与验收面；后续代码与 review 只以本单为唯一实施准绳。
  - Not：它不是新的需求讨论稿；也不是与设计稿并列的第二准绳。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：治理主单 / implementation issue

- **cron**（主称呼）：精确定时、一次性执行承载、无会话上下文、面向单一明确任务的定时任务。
  - Why：这是后续系统保留的第一类任务系统。
  - Not：不等于会话型继续对话；不等于“把消息注入已有会话再跑”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：定时任务

- **heartbeat**（主称呼）：周期性 agent 唤醒、依赖明确会话上下文、面向轻量持续关注的任务系统。
  - Why：这是后续系统保留的第二类任务系统。
  - Not：不等于后台重型任务；不等于系统全局单例；不等于无上下文巡检。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：心跳任务

- **main 会话**（主称呼）：某个 `agent` 逻辑上固定拥有且只拥有一个 `main` 会话。
  - Why：`heartbeat` 与 `cron.session` 都允许显式绑定 `main`。
  - Not：不等于系统全局唯一 main；不等于“用户先手工创建才允许存在”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：主会话

- **显式会话**（主称呼）：除 `main` 以外，已经存在且可稳定引用的具体会话。
  - Why：`heartbeat` 与 `cron.session` 只允许绑定 `main` 或正式具体会话。
  - Not：不包括临时分支会话、内部 lane 会话、不可稳定引用的对象。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：正式会话

- **投递目标**（主称呼）：某次任务运行完成后，结果被发送到哪里。
  - Why：投递目标与运行上下文必须分离，不能混为一个概念。
  - Not：不等于执行上下文；不等于 channel 泛词。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：delivery target

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件或 DB 原始值
  - effective：合并/默认/校验后的生效值
  - as-sent：最终写入运行时请求体、实际发送给下游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 把 `cron` 与 `heartbeat` 收敛成两套且仅两套任务系统，不再保留 `session cron`、`systemEvent + main 注入` 等旧执行模型。
- [ ] 冻结并落实唯一事实来源：结构化任务配置 / 状态归 DB，`heartbeat` 长文本 prompt 归 `agents/<agent_id>/HEARTBEAT.md`，`persona` 归 agent 身份文档体系。
- [ ] 冻结并落实唯一外部合同：外部 JSON / RPC / UI 统一 `camelCase`，并与本单“最终字段冻结（Final External Shapes）”完全一致。
- [ ] 冻结并落实唯一运行语义：`cron` 执行时无会话上下文；`heartbeat` 执行时必须绑定明确会话上下文。
- [ ] 冻结并落实 UI owner：`cron` 与 `heartbeat` 使用两套明确表面，不再让 generic session UI、旧隐式 prompt 来源、旧字段表面继续指导实现。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须只有一个语义准绳、一个实施主单、一个字段合同。
  - 必须把运行上下文、投递目标、persona、model、prompt、持久化 owner 分开建模，不得再混源。
  - 不得保留 backward compatibility、fallback、alias、shim、silent degrade。
  - 不得为 legacy 专门再加一层“兼容识别分支”；命中 legacy 直接失败并给 remediation。
- 兼容性：strict one-cut。旧字段、旧文件、旧路径、旧持久化形状命中时直接 reject；不自动迁移。
- 可观测性：所有拒绝、跳过、投递、自动创建 `main`、legacy 命中、DB 不可用都必须有结构化日志，至少包含 `event`、`reason_code`、`decision`、`policy`。
- 安全与隐私：日志不得打印敏感 token、完整正文、完整 prompt；如需排障，仅允许 `preview` / `len` / `hash` 等有限诊断字段。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 当前代码仍同时存在 `cron`、`heartbeat`、`sessionTarget`、`payloadKind`、`systemEvent`、`agentTurn`、`deliver/channel/to`、根级 `HEARTBEAT.md`、`heartbeat.prompt`、多 store 并存等多套旧语义。
2. 当前 Web UI 仍把 `heartbeat` 当成“根级 `HEARTBEAT.md` + config prompt 覆盖 + ackMax”模式来配置，不符合已冻结模型。
3. 当前 `cron` 仍以内嵌会话目标、`systemEvent/main` 注入、旧毫秒字段、旧 store 形状为中心，无法作为后续 one-cut 治理的唯一实施基础。
4. 当前仍有旧子 issue 围绕 `cron execution session`、generic session UI 暴露、删除入口错位等旧模型做局部修补，这些不应再继续充当准绳。

### 影响（Impact）
- 用户体验：
  - UI 暴露的配置项、术语和最终系统目标不一致，后续实现容易再次返工。
  - 用户会看到“能配、能保存、但运行口径不是这一套”的假闭环。
- 可靠性：
  - 同一事实存在多 owner：prompt、store、运行上下文、投递目标都仍有分叉。
  - legacy 语义若继续残留，会直接破坏 one-cut 收敛。
- 排障成本：
  - 文档、代码、UI、测试没有单点准绳，review 会持续在旧口径与新口径之间来回猜。

### 复现步骤（Reproduction）
1. 打开 `docs/plans/2026-03-26-cron-heartbeat-model-design.md`，再对照本单与前端 / 后端代码。
2. 检查 heartbeat UI 是否仍显示根级 `HEARTBEAT.md`、`heartbeat.prompt`、`ackMax` 等旧概念。
3. 检查 cron 类型、payload、store、sessionTarget、投递字段是否仍围绕旧模型实现。
4. 期望 vs 实际：
   - 期望：只有一套冻结模型指导后续实现。
   - 实际：代码现状仍残留旧模型，尚未按本单收敛。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 文档证据：
  - `docs/plans/2026-03-26-cron-heartbeat-model-design.md:4`：设计稿状态已经更新为“语义已定稿，已回写实施主单”。
  - `docs/plans/2026-03-26-cron-heartbeat-model-design.md:11`：明确写死“后续代码实施与 review 以 `issues/issue-cron-system-governance-one-cut.md` 为唯一实施准绳”。
  - `docs/plans/2026-03-26-cron-heartbeat-model-design.md:24`：系统只保留两类定时任务语义：`cron` 与 `heartbeat`。
  - `docs/plans/2026-03-26-cron-heartbeat-model-design.md:497`：one-cut 删除项已明确包含 `payloadKind = systemEvent | agentTurn`（以及同段落里的 `deliver/channel/to`、`anchor_ms`、`tz`、根级 `HEARTBEAT.md`、`heartbeat.prompt`、`heartbeat.ack_max_chars` 等）。

- 代码证据：`cron` 仍停留在旧模型
  - `crates/cron/src/types.rs:9`：`CronSchedule` 仍使用 `at_ms`、`every_ms`、`anchor_ms`、`tz`。
  - `crates/cron/src/types.rs:28`：`CronPayload` 仍保留 `SystemEvent` / `AgentTurn` 两类旧 payload 语义。
  - `crates/cron/src/types.rs:39`：`AgentTurn` 仍保留 `deliver`、`channel`、`to` 这组 legacy 投递字段。
  - `crates/cron/src/types.rs:100`：`CronJob` 仍把 `session_target` 作为执行期合同的一部分。
  - `crates/cron/src/service.rs:641`：运行时仍按 `session_target + payload` 组合做旧合法性校验。
  - `crates/cron/src/service.rs:506`：运行时仍把 `deliver/channel/to/session_target` 一起塞进 `AgentTurnRequest`。
  - `crates/cron/src/store_file.rs:17`、`crates/cron/src/store_memory.rs:16`、`crates/cron/src/store_sqlite.rs:15`：三套 store 仍并存于产品路径。

- 代码证据：`heartbeat` 仍停留在旧模型
  - `crates/cron/src/heartbeat.rs:164`：当前 heartbeat prompt 解析仍把 `HEARTBEAT.md` 作为输入源之一。
  - `crates/gateway/src/methods.rs:1930`：`heartbeat.status` 仍从根级 `HEARTBEAT.md` 取 prompt。
  - `crates/gateway/src/methods.rs:2016`：`heartbeat.update` 仍记录“loaded heartbeat prompt from HEARTBEAT.md”。
  - `crates/gateway/src/server.rs:4766`：gateway 启动仍会在工作区根目录 seed `HEARTBEAT.md`。
  - `crates/gateway/src/server.rs:5020`：默认文案仍把根级 `HEARTBEAT.md` 写成 heartbeat prompt 来源。
  - `crates/gateway/src/assets/js/page-crons.js:341`：heartbeat UI 仍告诉用户“留空则使用 workspace root 的 `HEARTBEAT.md`”。
  - `crates/gateway/src/assets/js/page-crons.js:346`：heartbeat UI 仍暴露 `ack_max_chars` 输入项。

- 代码证据：启动 / owner / 可观测性仍停留在旧模型
  - `crates/gateway/src/server.rs:1408`：gateway 启动仍默认选择 `FileStore`。
  - `crates/gateway/src/server.rs:1411`：`FileStore` 不可用时仍 warn 并降级到 `InMemoryStore`。
  - `crates/gateway/src/server.rs:3032`：`cron_service.start()` 失败仍只是 warn，不阻断服务启动。
  - `crates/gateway/ui/e2e/specs/cron.spec.js:13`：现有 E2E 仍围绕旧 heartbeat tab 进行浅层加载验证，未冻结新主路径。
  - `crates/sessions/src/key.rs:33`：`main` 会话 key 已经是按 agent 维度建模（`SessionKey::main(agent_id) -> agent:<agent_id>:main`），可作为本单“main 归属 agent”的实现基础。
  - `crates/gateway/src/personas.rs:15`：agent id 已有合法性校验（拒绝空白、空格、路径穿越等），可作为本单 `agentId` 合同约束的实现基础。

- 当前测试覆盖：
  - 已有：`crates/cron/src/types.rs`、`crates/cron/src/service.rs` 对旧字段、旧 payload、旧 store 行为有基础测试。
  - 缺口：
    - 没有围绕新 `cron` / `heartbeat` 模型的主路径测试。
    - 没有 legacy 命中直接 reject 的关键测试矩阵。
    - 没有 DB-only owner、agent 级 `HEARTBEAT.md`、`main` 自动物化、`cron.delivery` 三分法的自动化证明。

## 根因分析（Root Cause）
- A. 旧系统在一个对象里同时混了“调度”“会话”“投递”“persona”“prompt 来源”“存储 owner”，没有第一性拆分。
- B. 设计稿已经冻结，但旧主 issue 和代码现状没有同步回写，导致文档准绳与实施准绳分裂。
- C. 系统仍残留大量历史兼容思路：根级 `HEARTBEAT.md`、`heartbeat.prompt` 覆盖、`ack_max_chars`、`payloadKind`、`systemEvent/main` 注入、`deliver/channel/to`、多 store 并存。
- D. 测试口径仍围绕旧行为做基础覆盖，没有先冻结新核心路径，导致任何实现都容易继续在旧路径上打补丁。

## 唯一事实来源（Single Source of Truth）
### 语义 owner
- 设计依据 owner：`docs/plans/2026-03-26-cron-heartbeat-model-design.md`
- 当前唯一实施 owner：本单
- 主从关系冻结：
  - 设计稿负责保留目标模型与推导背景
  - 本单负责承接并冻结实施范围、失败语义、测试面、验收面
  - 后续实施与 review 只以本单为唯一实施准绳
  - 若两者发现冲突，先修正文档，未修正文档前禁止开工

### persisted backing
- `cron` 结构化配置：DB
- `cron` 运行状态：DB
- `cron` run history：DB
- `heartbeat` 结构化配置：DB
- `heartbeat` 运行状态：DB
- `heartbeat` 长文本 prompt：`agents/<agent_id>/HEARTBEAT.md`
- `persona`：agent 身份文档体系
- DB 具体落点冻结为：gateway 的 canonical SQLite（复用 `db_pool`；与 sessions/metrics 等同一 DB 文件），不再允许 cron 独立 file store 或内存 store

### runtime owner
- 调度 runtime 的唯一生效来源：从 DB 装载后的内存任务视图
- `main` 的逻辑存在性 owner：agent 合同
- `main` 的物理会话物化 owner：session service
- 显式具体会话存在性 owner：session service
- 会话归属校验 owner：session service
- Telegram 目标地址 owner：`cron.delivery.telegram.target`

### projection / cache
- Web UI：只允许做 projection，不允许自造字段、不允许自造默认语义
- RPC 响应：只允许做 typed projection，不允许拼接第二事实源
- 内存 cache：只允许缓存 authoritative backing 的投影，不允许成为平级 owner

### 唯一写入口
- `cron`：统一通过一套 typed service / RPC / tool 合同写入 DB
- `heartbeat`：结构化配置统一通过一套 typed service / RPC 合同写入 DB；长文本 prompt 统一写入 `agents/<agent_id>/HEARTBEAT.md`
- 不允许出现“UI 写一套、tool 写一套、后台修补一套”的多入口分叉

## 范围与边界（Scope & Boundaries）
- 本轮允许修改的层：
  - `crates/cron/*`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/assets/js/page-crons.js`
  - `crates/gateway/ui/e2e/specs/cron.spec.js`
  - agent 目录相关文件读写与默认 seed 逻辑
- 本轮闭环必改项：
  - `cron / heartbeat` typed contract
  - DB-only owner
  - agent 级 `HEARTBEAT.md`
  - `main` ensure / create 合同
  - `cron.delivery` 三分法
  - 结构化日志
  - 自动化测试与手工验收说明
- 本轮禁止外溢的层：
  - 不顺手重构 generic session UI
  - 不顺手重做 Telegram 频道框架
  - 不顺手做新的通用调度抽象
  - 不顺手扩展更多 delivery 类型
  - 不顺手处理与本单无关的 session branch / lane 收敛

## One-cut 删除项（Normative Deletions）
### `cron`
- 删除 `payloadKind = systemEvent | agentTurn`
- 删除 `sessionTarget` 作为执行上下文字段
- 删除 `deliver/channel/to`
- 删除 `anchor_ms`
- 删除 `tz`
- 删除 file store / memory store 作为产品持久化合同
- 删除 job 级 `sandbox`
- 删除 `Named(...)` 等内部 lane 暴露

### `heartbeat`
- 删除工作区根级 `HEARTBEAT.md`
- 删除 `heartbeat.prompt`
- 删除 `heartbeat.ack_max_chars`
- 删除 `heartbeat` 独立 persona
- 删除 `heartbeat` 私有 sandbox 配置

### 通用
- 删除 `channel` 这个空泛投递总称
- 删除空值代表 `main`
- 删除自动猜最近活跃会话 / 最后会话 / 任意会话
- 删除自动迁移 legacy 字段 / 文件 / 持久化形状

## 失败语义冻结（Failure Semantics）
### 配置期 reject
- `agentId` 不存在、非法、或无对应 agent 身份目录：reject
- `cron.prompt` 为空：reject
- `cron.schedule.kind="once"` 且 `at` 非法（不满足 RFC3339）或已过期：reject
- `cron.schedule.kind="every"` 且 `every` 非法（不满足约定的 interval 语法）或小于等于零：reject
- `cron.schedule.kind="cron"` 且 `expr` 非法：reject
- `cron.schedule.kind="cron"` 且 `timezone` 非法：reject
- `cron.deleteAfterRun=true` 但任务不是 `once`：reject
- `cron.delivery.kind="session"` 但 target 非法：reject
- `cron.delivery.kind="session"` 但目标会话不属于该 `agentId`：reject
- `cron.delivery.kind="telegram"` 但缺少 `accountKey` 或 `chatId`：reject
- `heartbeat.enabled=true` 但 `agents/<agent_id>/HEARTBEAT.md` 缺失：reject
- `heartbeat.enabled=true` 但 `agents/<agent_id>/HEARTBEAT.md` 有效内容为空（见“heartbeat prompt 有效空内容定义”）：reject
- `heartbeat.sessionTarget.kind="session"` 但目标会话不存在：reject
- `heartbeat.sessionTarget.kind="session"` 但目标会话不属于该 `agentId`：reject
- `heartbeat.activeHours` 若提供，则必须合法（start/end/timezone 形状非法或不可解析）：reject
- `modelSelector.kind="explicit"` 但模型不存在：reject

### 启动期 / bootstrap reject
- DB 不可用：gateway 启动失败（不允许启动后 silent degrade）
- 迁移失败：gateway 启动失败
- 发现 legacy cron file store 持久化形状：gateway 启动失败，并给 remediation（不允许 silent 丢任务）
  - legacy 形状定义：存在 `~/.clawdbot/cron/jobs.json` 或 `~/.clawdbot/cron/runs/`
  - remediation：移除 `~/.clawdbot/cron/` 后按新合同在 UI/Tool 中重建任务

### 运行期 reject / fail
- `cron.delivery.kind="session"` 且目标会话在投递前已被删除：fail
- `cron.delivery.kind="session"` 且目标为 `main`，`main` 物化失败：fail
- `cron.delivery.kind="telegram"` 且 `accountKey` 不存在：fail
- `cron.delivery.kind="telegram"` 且 `chatId / threadId` 不被下游接受：fail
- `heartbeat` 绑定的具体会话在运行前已被删除：fail
- `heartbeat` 绑定 `main` 但自动创建失败：fail
- `cron.timeoutSecs` 到期：fail，并记录 `cron_run_timeout`
- `heartbeat` 命中 `activeHours` 之外：skip，不补跑、不排队累积

### stale / race / contract violation
- 同一 agent 已存在 heartbeat，再创建第二个 heartbeat：reject
- 运行期间任务被删除：
  - 若调度尚未真正开始：drop 并记录日志
  - 若运行已开始：允许本轮 run 结束，但不得继续 reschedule 已删除对象
- 更新与运行并发：
  - 以已持久化成功的最新 DB 配置作为下一轮 authoritative 配置
  - 不允许 UI 或内存态偷偷覆盖 DB authoritative 状态
- 命中 legacy 字段、legacy 文件、legacy store：reject + remediation
- `enabled=false` 期间的错过触发：直接丢弃，不做 catch-up
- `enabled=false -> true`：
  - `cron.once`：若原定时间已过去，直接 reject，要求用户重建
  - `cron.every / cron.cron`：从重新启用时刻开始重新计算下一次触发
  - `heartbeat`：从重新启用时刻开始恢复节奏，不补跑历史节拍

## 可观测性冻结（Observability Contract）
- 所有以下路径都必须记结构化日志：
  - legacy 命中 reject
  - DB 不可用 reject
  - `main` ensure / create success / fail
  - `heartbeat` target reject
  - `cron` 选择 `silent`
  - `cron` 投递到 `session` success / fail
  - `cron` 投递到 `telegram` success / fail
  - 运行前对象已删除导致 drop
- 日志最小字段固定为：
  - `event`
  - `reason_code`
  - `decision`
  - `policy`
- 允许按上下文补充：
  - `agent_id`
  - `job_id`
  - `session_key`
  - `delivery_kind`
  - `account_key`
  - `chat_id`
  - `run_id`
  - `remediation`
- 本轮 `policy` 固定为：
  - `cron_heartbeat_governance_v1`
- 本轮 `decision` 允许值冻结为：
  - `allow`
  - `reject`
  - `skip`
  - `drop`
  - `fail`
  - `ok`
- 日志不得打印：
  - token
  - 完整 prompt
  - 完整消息正文
  - 完整 transcript
- 本轮最小 `reason_code` 集合冻结为：
  - `legacy_contract_rejected`
  - `cron_schedule_invalid`
  - `cron_schedule_past`
  - `cron_prompt_missing`
  - `cron_delivery_target_invalid`
  - `cron_delivery_account_missing`
  - `cron_delivery_session_missing`
  - `cron_object_deleted_before_run`
  - `cron_run_timeout`
  - `heartbeat_prompt_missing`
  - `heartbeat_prompt_empty`
  - `heartbeat_target_missing`
  - `session_agent_mismatch`
  - `agent_missing`
  - `active_hours_invalid`
  - `main_materialize_failed`
  - `cron_store_unavailable`
  - `db_migration_failed`

## 边缘条件（Edge Conditions）
- 边缘条件优先级低于第一性原则、唯一真源原则、不后向兼容原则；前三者未定稿前，不允许为了边缘覆盖继续扩概念。
- `main` 逻辑存在但物理记录缺失：允许按合同自动创建
- 普通具体会话缺失：不自动补建，直接失败
- 临时分支会话 / 内部 lane 会话：不允许绑定
- 具体会话必须归属于任务所属 `agentId`；不允许跨 agent 绑定
- `heartbeat` 一次只服务一个上下文，不允许一轮同时绑定多个用户会话
- `cron` 即使结果投到会话，也不改变其“无会话上下文执行”的本质
- `telegram.threadId` 仅在 topic / 子线程场景出现；缺失时按普通 chat 投递理解，不自动猜测 topic
- 无输出的 `heartbeat`：安静结束，但保留运行结果语义；不得为了“看起来执行了”强塞一条消息
- `cron.schedule.kind="cron"` 的 DST 语义冻结为：
  - 本地时间不存在的触发点：跳过，不补跑
  - 本地时间重复出现的触发点：按两个实际时刻各触发一次
  - 不再额外引入自定义 DST 修正层
- `activeHours` 语义冻结为：
  - 只对 `heartbeat` 生效
  - 命中窗口外则直接 skip
  - 不做 catch-up
- `heartbeat` prompt 有效空内容定义冻结为：
  - 若 `agents/<agent_id>/HEARTBEAT.md` 每一行都满足以下任一条件，则视为“有效内容为空”：
    - 空行
    - 仅包含 Markdown 标题（行首 `#`）
    - 空的列表项（`-`、`*`、`- `、`* `）
- `deleteAfterRun` 语义冻结为：
  - 只允许 `cron.once`
  - 一次终态 run 结束后删除，不区分 success / fail
  - 删除前必须先记录 run history
- `timeoutSecs` 语义冻结为：
  - 超时视为本次 run fail
  - 先记录失败 run，再按 `deleteAfterRun` 与任务类型决定是否删除 / reschedule

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 必须只保留两类任务系统：`cron` 与 `heartbeat`。
  - 必须把结构化配置 / 运行状态收敛到 DB，把 `heartbeat` 长文本 prompt 收敛到 `agents/<agent_id>/HEARTBEAT.md`。
  - 必须把 `persona` 收敛为 agent 自身身份文档的唯一事实来源。
  - 必须保证 `heartbeat` 与 `cron.session` 只能绑定同 agent 的会话。
  - 必须把 `cron` 定义为“无会话上下文执行，执行后再按 `silent / session / telegram` 投递”。
  - 必须把 `heartbeat` 定义为“绑定 `main` 或显式会话的一轮周期性关注”，且每个 agent 最多一个 heartbeat。
  - 必须支持 `main` 显式绑定，且在首次引用时按合同自动物化。
  - 必须在主路径和拒绝路径上补齐结构化日志。
- 不得：
  - 不得继续保留 `session cron`。
  - 不得继续保留 `payloadKind = systemEvent | agentTurn`。
  - 不得继续保留 `deliver/channel/to`、`anchor_ms`、`tz`、根级 `HEARTBEAT.md`、`heartbeat.prompt`、`heartbeat.ack_max_chars`、job 级 `sandbox`。
  - 不得继续允许 file store / memory store 作为产品级事实源。
  - 不得自动猜测“最近活跃会话”“最后会话”“任意会话”。
  - 不得 silent degrade 到别的 store、别的 prompt 源、别的会话、别的投递路径。
- 应当：
  - 应当把 UI 明确拆成两套表面：`cron` 面板与 `heartbeat` 面板。
  - 应当把 legacy 命中统一转成 reject + `reason_code` + `remediation`。
  - 应当先补冻结测试面，再进入代码实现。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 以本单为唯一实施准绳，设计稿仅保留为设计依据；
  - 本单原地回写为实施主单；
  - 后续代码按本单收敛，并确保不与设计依据冲突；旧子 issue 统一退场。
- 优点：
  - 唯一真源清楚。
  - 不需要一边实现一边猜口径。
  - 最符合第一性原则与 one-cut。
- 风险/缺点：
  - 需要一次性删掉一批 legacy 入口，短期改动面不小。

#### 方案 2（不推荐）
- 核心思路：
  - 继续在旧主 issue 基础上“哪里有 bug 修哪里”，逐步兼容现状。
- 风险/缺点：
  - 会继续保留多真源、多术语、多表面、多测试口径。
  - 返工成本高，且无法形成唯一准绳。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：本单是后续代码实施与 review 的唯一实施准绳；设计稿只保留为设计依据，不再作为并列执行清单。
- 规则 2：`cron` 与 `heartbeat` 是两套上层任务系统；底层可以共用调度骨架，但产品语义、配置表面、运行合同、测试合同必须分开。
- 规则 3：`cron` 执行阶段无会话上下文；结果阶段只允许 `silent`、`session`、`telegram` 三种投递策略。
- 规则 4：`heartbeat` 必须绑定 `main` 或显式会话；每个 agent 最多一个 heartbeat；结果默认落在绑定会话。
- 规则 5：`main` 是 agent 合同的一部分；首次引用时允许自动物化；普通具体会话不存在时必须直接失败。
- 规则 6：外部 JSON / RPC / UI 字段统一使用本单中的 `camelCase` 最终字段冻结；内部代码实现使用 `snake_case`，通过显式 serde rename 或 typed mapping 做边界转换。
- 规则 7：legacy 输入、legacy 文件、legacy 持久化形状命中时直接 reject；不做自动迁移，不做双读双写。
- 规则 8：DB 是结构化配置 / 状态 / run history 的唯一 owner；agent 目录文件是 `heartbeat` 长文本 prompt 的唯一 owner；`persona` 是 agent 身份文档的唯一 owner。
- 规则 9：可观测性必须覆盖所有 strict reject、skip、drop、delivery、`main` ensure/create、DB 不可用、legacy 命中。

#### 接口与数据结构（Contracts）
- API / RPC：
  - `cron` 配置字段必须收敛为：`jobId`、`agentId`、`name`、`enabled`、`schedule`、`prompt`、`modelSelector`、`timeoutSecs?`、`delivery`、`deleteAfterRun`。
  - `heartbeat` 配置字段必须收敛为：`agentId`、`enabled`、`every`、`sessionTarget`、`modelSelector`、`activeHours?`。
  - `heartbeat` prompt 文件唯一来源：`agents/<agent_id>/HEARTBEAT.md`。
- 存储 / 字段兼容：
  - 结构化配置与状态：DB only。
  - 不再接受：`payloadKind`、`deliver/channel/to`、`anchor_ms`、`tz`、`heartbeat.prompt`、`heartbeat.ack_max_chars`、job 级 `sandbox`、file store、memory store。
- UI 展示（如适用）：
  - `cron` 面板必须显式展示 `prompt`、`schedule`、`delivery`、`modelSelector`。
  - `heartbeat` 面板必须显式展示 `sessionTarget`、`modelSelector`、`activeHours`，并把 prompt 来源指向 agent 目录文件，而不是根级文件或内嵌 textarea。

#### 最终字段冻结（Final External Shapes）
- `heartbeat`
  - `agentId`
  - `enabled`
  - `every`
  - `sessionTarget = { kind: "main" } | { kind: "session", sessionKey: "..." }`
  - `modelSelector = { kind: "inherit" } | { kind: "explicit", modelId: "..." }`
  - `activeHours? = { start, end, timezone }`
- `heartbeat` prompt 文件
  - `agents/<agent_id>/HEARTBEAT.md`
- `cron`
  - `jobId`
  - `agentId`
  - `name`
  - `enabled`
  - `schedule = { kind: "once", at } | { kind: "every", every } | { kind: "cron", expr, timezone }`
  - `prompt`
  - `modelSelector = { kind: "inherit" } | { kind: "explicit", modelId: "..." }`
  - `timeoutSecs?`
  - `delivery = { kind: "silent" } | { kind: "session", target } | { kind: "telegram", target }`
  - `deleteAfterRun`
- `cron.session.target`
  - `{ kind: "main" }`
  - `{ kind: "session", sessionKey: "..." }`
- `cron.telegram.target`
  - `{ accountKey: "...", chatId: "..." }`
  - `{ accountKey: "...", chatId: "...", threadId: "..." }`
- 运行时状态字段
  - `nextRunAt`
  - `runningAt`
  - `lastRunAt`
  - `lastStatus`
  - `lastError`
  - `lastDurationMs`
- run history 字段
  - `runId`
  - `jobId`
  - `startedAt`
  - `finishedAt`
  - `status`
  - `error`
  - `outputPreview`
  - `inputTokens`
  - `outputTokens`
- 明确禁止出现在外部合同中的字段
  - `payloadKind`
  - `sessionTarget` 作为 `cron` 执行上下文字段
  - `deliver`
  - `channel`
  - `to`
  - `at_ms`
  - `every_ms`
  - `anchor_ms`
  - `tz`
  - `heartbeat.prompt`
  - `heartbeat.ack_max_chars`
  - `sandbox`

#### 字段类型与格式冻结（Field Formats）
> 这些是外部合同的“可解析性 + 不歧义”硬约束；命中非法值必须 reject（不做猜测与隐式 fallback）。

- `agentId`
  - 必须通过 `is_valid_agent_id` 校验（禁止空串、空格、路径穿越等）
  - 必须存在对应目录：`agents/<agent_id>/`
- `jobId`
  - 必须是 UUID 字符串（canonical hyphenated），建议统一输出小写
- `sessionKey`
  - 必须是 `agent:<agent_id>:<bucket_key>` 形状（禁止 `system:*`）
  - 必须满足：`<agent_id> == agentId`（否则 `session_agent_mismatch`）
- `schedule.kind="once".at`
  - 必须是 RFC3339 字符串（必须携带时区偏移或 `Z`）
- `schedule.kind="every".every` / `heartbeat.every`
  - 必须是 interval 字符串：`<positive-int><unit>`
  - 仅允许单位：`s` / `m` / `h` / `d`
  - 不允许无单位裸数字（禁止隐式毫秒）
- `schedule.kind="cron".expr`
  - 只接受 5-field 标准或 6-field（带秒）表达式
- `schedule.kind="cron".timezone` / `activeHours.timezone`
  - 必须是 IANA timezone（例如 `Asia/Shanghai`），不得为空
- `activeHours.start` / `activeHours.end`
  - 必须为 `HH:MM`（24 小时制），`end` 允许 `24:00`
  - 允许跨午夜窗口；语义与 `is_within_active_hours` 一致
- `cron.telegram.target.chatId` / `threadId`
  - 必须是十进制字符串（可表示 i64，禁止 JS number 精度丢失）

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - `enabled=true` 但 `agents/<agent_id>/HEARTBEAT.md` 缺失或为空：直接 reject。
  - `heartbeat.sessionTarget.kind="session"` 但目标会话不存在：直接 reject。
  - `cron.schedule` 非法、`prompt` 为空、`delivery.target` 非法：直接 reject。
  - DB 不可用：相关能力直接失败，不降级到 file store / memory store。
  - legacy 字段或 legacy 文件命中：直接 reject，并返回 remediation。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - `deleteAfterRun` 只允许一次性 `cron`。
  - `heartbeat` 没有输出时安静结束，但必须保留运行记录语义。
  - 拒绝路径不允许偷偷切到别的 prompt 源、别的 store、别的会话、别的投递路径。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 日志只记录必要的结构化字段与有限预览。
- 禁止打印字段清单：
  - token、完整 prompt、完整消息正文、完整会话 transcript。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] `cron` 与 `heartbeat` 的最终语义、字段、owner、失败语义与本单冻结一致，且不与设计依据冲突。
- [ ] 外部 JSON / RPC / UI 合同全部收敛为本单“最终字段冻结（Final External Shapes）”中的 `camelCase` 形状。
- [ ] `cron` 不再依赖会话上下文执行；`heartbeat` 不再作为无上下文重型任务执行。
- [ ] 根级 `HEARTBEAT.md`、`heartbeat.prompt`、`heartbeat.ack_max_chars`、`payloadKind`、`deliver/channel/to`、`anchor_ms`、`tz`、job 级 `sandbox`、file store、memory store 已从产品合同删除。
- [ ] `main` 显式绑定、自动物化、显式会话绑定拒绝语义全部落地，且有结构化日志。
- [ ] 跨 agent 会话绑定会被直接拒绝，且有稳定 `reason_code`。
- [ ] `cron.delivery = silent | session | telegram` 三分法落地，且投递语义与运行上下文严格分离。
- [ ] 所有 strict reject / skip / legacy 命中 / DB 不可用 / delivery 成败都具备结构化日志。
- [ ] 关键主路径、关键边界、关键失败面、关键 legacy reject 路径具备自动化测试或明示的手工验收缺口说明。

## 测试计划（Test Plan）【不可省略】
> 已完成且有证据的测试项必须同步勾选；未勾选项表示当前仍未补到自动化证据或手工验收说明。
### Unit
- [ ] `crates/cron/src/types.rs`：冻结 `cron.schedule`、`cron.delivery`、`heartbeat` 最终外部字段形状，并补 legacy 反向用例。
- [ ] `crates/cron/src/heartbeat.rs`：覆盖 `HEARTBEAT.md` 有效空内容判定与 `activeHours` 解析失败直接 reject（禁止“无效配置 -> 永远 active”）。
- [ ] `crates/cron/src/service.rs`：覆盖 `cron` 无会话执行语义、`heartbeat` 会话绑定语义、`main` 自动物化语义。
- [ ] `crates/cron/src/service.rs`：覆盖 legacy 命中 reject、`deleteAfterRun` 限定、DB 不可用失败语义、`reason_code` 可观测性。
- [ ] `crates/cron/src/service.rs`：覆盖跨 agent 会话绑定直接 reject。
- [ ] `crates/gateway/src/methods.rs` / `crates/gateway/src/server.rs`：覆盖 agent 级 `HEARTBEAT.md`、根级 `HEARTBEAT.md` 退场、UI / RPC 合同变更。
- [ ] `crates/cron/src/types.rs`：覆盖 `cron.telegram.target` 只接受 `accountKey + chatId + threadId?`，拒绝 `username / peer_id / message_id / bucket_key`。
- [ ] `crates/cron/src/types.rs`：覆盖 `modelSelector = { kind: \"inherit\" } | { kind: \"explicit\", modelId }`，不接受空值 / 隐式 fallback。

### Integration
- [ ] `cron`：`once / every / cron` 三种 schedule 的保存、调度、运行、投递闭环。
- [ ] `heartbeat`：绑定 `main`、绑定具体会话、目标缺失失败、无输出安静结束。
- [ ] DB-only owner：DB 不可用时相关能力直接失败，不降级到 file / memory store。
- [ ] 并发边界：删除中的对象不再进入下一轮调度；运行中的删除不会悄悄复活对象。
- [ ] 唯一性边界：同一 agent 创建第二个 heartbeat 直接 reject。
- [ ] 生命周期边界：`enabled=false` 不补跑、重新启用后按新语义重新计算。
- [ ] 时间边界：`cron.timeoutSecs`、`deleteAfterRun`、DST 语义按冻结合同执行。

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/cron.spec.js`：覆盖 `cron` 与 `heartbeat` 两套 UI 表面、关键字段展示、保存与拒绝语义。
- [ ] `crates/gateway/ui/e2e/specs/cron.spec.js`：覆盖 `heartbeat` 不再使用根级 `HEARTBEAT.md` / textarea prompt / `ackMax`。
- [ ] `crates/gateway/ui/e2e/specs/cron.spec.js`：覆盖 `main` 可被显式选择，即使尚未物化。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - `telegram` 真正外发需要真实账号与目标 chat，CI 不能直接持有生产凭据。
- 手工验证步骤：
  1. 配置一个有 agent 目录与 DB 的最小环境。
  2. 为指定 agent 写入 `agents/<agent_id>/HEARTBEAT.md`。
  3. 创建一个 `heartbeat` 绑定 `main`，验证首次运行时会自动物化 `main`。
  4. 创建一个 `cron`，分别验证 `silent`、`session(main)`、`telegram` 三种投递。
  5. 用 legacy 字段 / legacy 文件 / DB 不可用场景验证 strict reject 与结构化日志。

## 发布与回滚（Rollout & Rollback）
- 发布策略：按 one-cut 直接切换，不保留旧合同并行期。
- 回滚策略：仅允许通过回滚代码版本回滚；不允许在运行时重新加回 legacy 兼容分支。
- 上线观测：
  - `heartbeat`：`main` ensure / create、target reject、run result
  - `cron`：schedule validate、run start / finish、delivery success / fail / silent
  - `store`：DB init success / fail、legacy hit reject

## 开工条件（Implementation Gate）
- [x] 本单已完成对设计稿的实施回写，后续实施以本单为唯一实施准绳
- [x] 唯一事实来源、范围边界、one-cut 删除项、失败语义、可观测性、测试面都已冻结
- [x] `cron` 与 `heartbeat` 最终字段总表已冻结，且无第二套旧字段仍在正文里被当成目标合同
- [x] 过时子 issue 已退场，不再作为实现依据
- [x] 没有阻塞实施的未决问题
- [x] 若后续 review 再发现系统性缺口，必须先回写本单，再继续代码实现

## 实施拆分（Implementation Outline）
- Step 1:
  - 先把 `cron` / `heartbeat` 的最终字段与 owner 收口到 typed contract，删除旧字段、旧 enum、旧输入路径。
- Step 2:
  - 删掉根级 `HEARTBEAT.md`、`heartbeat.prompt`、`heartbeat.ack_max_chars`，接入 agent 级 `HEARTBEAT.md`。
- Step 3:
  - 重做 `cron` 运行模型：执行无会话上下文，结果只走 `silent / session / telegram`。
- Step 4:
  - 重做 `heartbeat` 运行模型：每 agent 最多一个、显式绑定 `main` 或具体会话、支持 `main` 自动物化。
- Step 5:
  - 收口 DB-only store，删除 file / memory 产品路径与 silent degrade。
- Step 6:
  - 回写 UI、结构化日志、测试矩阵与运维文档。
- 受影响文件：
  - `crates/cron/src/types.rs`
  - `crates/cron/src/service.rs`
  - `crates/cron/src/heartbeat.rs`
  - `crates/cron/src/store_sqlite.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/methods.rs`
  - `crates/gateway/src/assets/js/page-crons.js`
  - `crates/gateway/ui/e2e/specs/cron.spec.js`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/plans/2026-03-26-cron-heartbeat-model-design.md`
  - `issues/issue-session-page-cron-session-delete-entry-missing.md`（已过时，仅保留旧模型证据）
  - `issues/discussions/cron-trigger-execution-delivery-model.md`（历史讨论稿，仅供追溯）
- Related commits/PRs：
  - 待补
- External refs（可选）：
  - 无

## 未决问题（Open Questions）
- 本单当前无阻塞语义未决；若后续实施发现新分歧，必须先回写本单，再继续写代码。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（strict one-cut；legacy 直接 reject）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

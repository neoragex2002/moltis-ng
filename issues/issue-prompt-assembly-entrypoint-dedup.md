# Issue: Prompt assembly 入口去重（gateway chat / send_sync / debug / spawn_agent）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-05
- Owners: <TBD>
- Components: gateway / tools / agents
- Affected providers/models: all（openai-responses 受影响最大）

**已实现（如有，写日期）**
- gateway 侧预加载 persona merge：`crates/gateway/src/chat.rs:811`
- (2026-03-04) gateway 所有入口统一使用 canonical v1 builder（不再分散拼装旧 builder）：`crates/gateway/src/chat.rs:2401`
- (2026-03-04) spawn_agent 统一使用 canonical v1 builder（不再 Responses/non-Responses 分支）：`crates/tools/src/spawn_agent.rs:235`
- (2026-03-04) openai-responses 在 gateway debug 中的 asSentPreamble 折叠为单条 developer item：`crates/gateway/src/chat.rs:3651`

**已覆盖测试（如有）**
- gateway debug endpoints：openai-responses asSentPreamble 长度=1：`crates/gateway/src/chat.rs:8298`
- spawn_agent（openai-responses provider 下单条 system message）：`crates/gateway/tests/spawn_agent_openai_responses.rs:43`

**已知差异/后续优化（非阻塞）**
- 后续新增/调整入口点时，需继续复用 canonical v1 builder（避免引入新的 prompt drift 分支）。

---

## 背景（Background）
- 场景：同一 session 的 prompt 在不同阶段/入口会被构造多次：token estimate/auto-compact preflight、run_with_tools、run_streaming、send_sync、debug endpoints、tools.spawn_agent。
- 约束：
  - openai-responses 的 prompt product 是 developer role 语义（input[] items），而非 “messages[]/role=system”。
  - stream_only / supports_tools / native_tools / mcp_disabled 会影响 prompt 形态与工具注入。
  - 历史上 voice mode 指引（`VOICE_REPLY_SUFFIX`）在不同 provider/product 下追加位置不同（Responses runtime_snapshot vs system_prompt tail），属于典型 drift 来源；现已通过 canonical v1 的模板变量 `voice_reply_suffix_md` 收敛为“同一入口同一产物”。
- Out of scope：不在本 issue 内重写 PromptParts/renderer（由 ID=1 承担），但需要为后续 PromptParts 做入口收敛准备。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **entry point**（主称呼）：会触发 prompt 构造的调用路径（preflight/run/send_sync/debug/spawn_agent）。
  - Why：入口分散会导致 drift，进而影响 compaction、debug 证据链与行为一致性。
  - Source/Method：configured/effective/as-sent

- **prompt products**（主称呼）：一次运行中 prompt 的“产物集合”（token estimate 文本、typed messages、as-sent debug 形态）。
  - Why：同一个输入应在所有入口得到同一个 prompt products（除非明确标注 method 差异）。
  - Source/Method：effective/as-sent/estimate

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 将 gateway chat 的 preflight/run/send_sync/debug 统一调用同一套 canonical prompt builder（避免 provider-specific 分支）。
- [x] 将 tools.spawn_agent 的 prompt 构造收敛到同一入口（复用同一 canonical builder 与同一 requiredness matrix）。
- [x] 对 openai-responses 与 non-responses：跨 provider 层只产出 1 条 canonical `ChatMessage::System`，provider adapter 自行映射。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：同一输入（persona/runtime/tools/skills/project_context）在不同入口输出一致（除非入口明确要求 minimal/estimate）。
  - 不得：在 provider adapter 层“重新拼 prompt 内容结构/章节布局”（仅协议映射）。
- 可观测性：debug endpoints 必须能展示 prompt products 的关键字段（含 method 标注）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) gateway chat 的 preflight 与 run phase 各自构造 prompt，且分支复杂；spawn_agent 另有一套，易漂移。
2) 同一 provider 在不同入口的 tools/skills/runtime 注入可能不一致，导致 estimate 与 as-sent 不等价（触发 compaction 早/晚、debug 难复盘）。

### 影响（Impact）
- 可靠性：修复一处容易漏另一处；行为漂移难以避免。
- 排障成本：用户看到的 debug/as-sent 与实际 run 路径可能不一致，证据链断裂。

### 复现步骤（Reproduction）
1. 同一 session 在 openai-responses 下分别走 preflight（token estimate）与 run_with_tools。
2. 比较 debug/raw_prompt/full_context 的 preamble 与实际 as-sent 的一致性。
3. 期望 vs 实际：期望所有入口产物一致；历史上 drift 主要来自 tools/skills 过滤与 voice mode 指引注入方式不一致（已在 v1 收敛）。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - gateway preflight 多处 prompt 构造分支：`crates/gateway/src/chat.rs:2405`
  - spawn_agent 重复构造：`crates/tools/src/spawn_agent.rs:236`
- 当前测试覆盖：
  - 已有：openai-responses asSentPreamble 基础断言：`crates/gateway/src/chat.rs:8711`
  - 缺口：缺少“同输入跨入口一致性”的测试（尤其 spawn_agent）。

## 根因分析（Root Cause）
- A. prompt builder 逻辑分散在 gateway/tools 多处，且按运行模式分支重复拷贝。
- B. 缺少 canonical 的 prompt products 中间表示与单一入口函数。
- C. 结果：不同入口因参数传递/过滤细节差异产生 drift。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - gateway 的所有入口都从“同一个 prompt products 构造入口”获取产物（或明确标注 minimal/estimate 分支）。
  - spawn_agent 必须复用同一套 prompt products 构造（至少在 openai-responses 与 non-responses 的分支结构上对齐）。
- 不得：
  - 不得在 provider adapter 中拼装/重排 prompt 内容结构（只做协议映射）。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（effective）：在 gateway/tools 层构造 canonical prompt products（含 token estimate 文本、typed messages 前缀、debug 展示字段），并在所有入口复用。
- 规则 2（as-sent）：provider adapter 只负责“typed messages → as-sent 请求体”的���议映射，不负责 prompt 内容拼装。

#### 接口与数据结构（Contracts）
- 新增（或在 ID=1 PromptParts 后引入）结构：`PromptProducts`（命名以代码实现为准），至少包含：
  - `estimate_joined_text`（用于 token estimate；method=estimate）
  - `prefix_messages`（typed messages，用于 run；method=effective）
  - `debug_as_sent_summary`（用于 debug；method=as-sent）

#### 失败模式与降级（Failure modes & Degrade）
- 若 runtime_context 缺失：仍必须输出稳定的占位（例如 “未知/无”），避免不同入口出现空洞差异。

#### 安全与隐私（Security/Privacy）
- debug/hook 输出必须继续遵循脱敏策略（例如 remote_ip、location 等字段策略由对应 issue/模块控制）。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] gateway 的 preflight/run/send_sync/debug 统一使用 canonical v1 builder（不再走旧三段/多分支拼装）。
- [x] spawn_agent 的 prompt products 与 gateway 路径对齐（同样走 canonical v1 builder）。
- [x] 增加自动化测试覆盖 spawn_agent 的 system preamble 形态（openai-responses）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 断言测试：canonical v1 builder 覆盖 native/non-native/no-tools 三种模式：`crates/agents/src/prompt.rs:1718`

### Integration
- [x] 覆盖 spawn_agent：openai-responses provider 下只注入 1 条 system message：`crates/gateway/tests/spawn_agent_openai_responses.rs:43`

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：部分入口依赖真实 provider/runner，难以在 unit 完整覆盖。
- 手工验证步骤：
  1. 启动 gateway，分别触发 chat.run 与 tools.spawn_agent。
  2. 对比 debug endpoints 的 asSentPreamble / system_prompt（按 provider）是否一致。

## 发布与回滚（Rollout & Rollback）
- 发布策略：先只做重构去重（不改 prompt 内容），通过 golden/快照确保无行为变化；再逐步引入 PromptParts/renderer。
- 回滚策略：保留旧入口实现一段时间（feature flag 或分支开关），出现不一致可快速切回。

## 实施拆分（Implementation Outline）
- Step 1: 抽取 prompt products 构造入口（gateway 内部函数/模块）。
- Step 2: gateway 各入口改为调用统一入口。
- Step 3: spawn_agent 改为调用统一入口（或复用同一模块）。
- Step 4: 补齐一致性测试。
- 受影响文件：
  - `crates/gateway/src/chat.rs`
  - `crates/tools/src/spawn_agent.rs`
  - `crates/agents/src/prompt.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/overall_type4_system_prompt_assembly_v1.md`
  - `issues/issue-persona-prompt-configurable-assembly-and-builtin-separation.md`

## 未决问题（Open Questions）
- Q1: 统一入口应放在 gateway 还是 agents（以避免 tools crate 反向依赖）？
- Q2: 对 minimal prompt（stream_only）是否也必须走同一 products 入口（建议是，差异仅由参数驱动）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

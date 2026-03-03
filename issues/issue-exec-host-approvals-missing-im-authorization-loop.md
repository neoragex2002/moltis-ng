# Issue: exec 宿主机审批仅 Web UI，缺少 IM（Telegram）授权闭环（exec / approvals）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Owners: <TBD>
- Components: gateway / tools / ui / telegram / channels
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- Web UI 审批卡片（Allow/Deny + 倒计时）与 RPC resolve：`crates/gateway/src/assets/js/chat-ui.js:183`
- WebSocket 事件 `exec.approval.requested` → 渲染审批卡片：`crates/gateway/src/assets/js/websocket.js:591`
- 宿主机（unsandboxed）exec 命令审批闸门：`crates/tools/src/exec.rs:324`
- RPC：`exec.approval.resolve`（把决策写回 `ApprovalManager`）：`crates/gateway/src/approval.rs:56`

**已覆盖测试（如有）**
- `exec.approval.resolve` service 单测：`crates/gateway/src/approval.rs:115`
- `exec` tool 审批等待/超时等路径单测（部分）：`crates/tools/src/exec.rs:830`

**已知差异/后续优化（非阻塞）**
- 当前审批只解决“是否允许执行命令”，不解决“需要交互式 sudo 密码输入”的宿主机系统变更（`exec` 非交互 stdin）：`crates/tools/src/exec.rs:96`

---

## 背景（Background）
- 场景：当 sandbox 不可用/关闭/降级（backend=none）导致 `exec` 在宿主机执行时，系统会触发审批闸门；Web UI 可点 Allow/Deny，但 Telegram 等 IM 场景无法完成授权，导致命令等待超时或被拒绝。
- 约束：
  - `exec` 是非交互执行（stdin=null），因此宿主机需要输入 sudo 密码的路径天然无法自动化完成。
  - 审批是“操作员确认/授权”的产品能力，必须跨入口一致（Web UI / Telegram / 未来其它 channels）。
- Out of scope（本 Issue 不做）：
  - 在 IM 内输入/转发 sudo 密码等交互式提权流程。
  - 复杂 RBAC/多租户审批体系（先做最小可用闭环）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **exec 宿主机审批**（主称呼）：当 `exec` 将在宿主机执行且命中审批策略时，需要操作员显式 Allow/Deny 才能继续执行。
  - Why：避免 LLM 在宿主机随意执行命令；在安全/可靠性上是关键闸门。
  - Not：不等价于 sudo 提权；不保证命令在宿主机一定可成功执行。
  - Source/Method：authoritative（由 `ApprovalManager` 判定与等待决策）。
  - Aliases（仅记录，不在正文使用）：host approval / exec approvals

- **审批请求（Approval Request）**（主称呼）：包含 `request_id` + `command` 的一次待决授权，最终被 resolve 为 approved/denied/timeout。
  - Source/Method：authoritative（`ApprovalManager.create_request()` 生成，`resolve()` 结案）。

- **授权入口（Approval Surface）**（主称呼）：用户/操作员可以完成 Allow/Deny 的交互入口。
  - Source/Method：configured（入口是否可用取决于部署形态与 channel 能力）。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] Telegram（以及未来其它 channel）在触发宿主机审批时，必须存在可用的授权闭环（Allow/Deny），不依赖 Web UI。
- [ ] 审批请求必须能明确“发到哪里/由谁处理”（至少能回到触发该请求的同一 chat，或回到一个配置的 operator chat）。
- [ ] 授权决策必须回写到同一个 `ApprovalManager` 请求上（与 Web UI 同源，不出现两套口径）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：同一条审批请求只允许 resolve 一次（幂等/重复点击不应破坏状态）。
  - 必须：请求超时应当明确回执到触发方（并可重试发起新请求）。
  - 不得：把审批命令/请求 ID 泄露到不相关 chat（尤其是群聊场景）。
- 兼容性：Web UI 现有审批流程不回归；RPC `exec.approval.resolve` 保持兼容。
- 可观测性：在 `chat.context` / `channel` 日志中能定位“请求产生/被谁批准/超时”。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Web UI 模式下：会看到审批卡片，可点 Allow/Deny，命令继续执行。
2) Telegram 模式下：触发审批后没有任何可操作授权入口，最终命令等待超时（或用户不知道发生了什么）。

### 影响（Impact）
- 用户体验：Telegram 场景下几乎无法使用“宿主机执行 + 审批”能力。
- 可靠性：审批请求大量超时，导致任务失败或卡住。
- 排障成本：用户需要额外开 Web UI 才能批准命令，心智负担高且不符合 IM 使用方式。

### 复现步骤（Reproduction）
1. 在 Telegram chat 中触发一次需要 `exec` 的任务。
2. 让该次 `exec` 落到宿主机路径（例如 sandbox 关闭，或 backend=none 降级）。
3. 期望 vs 实际：
   - 期望：Telegram 内可 Allow/Deny 并继续执行，或至少有明确指引如何完成授权。
   - 实际：仅 Web UI 会收到 `exec.approval.requested` 事件并可操作；Telegram 无法 resolve。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/tools/src/exec.rs:324`：宿主机 unsandboxed 时触发审批、创建 request 并等待 decision（approved/denied/timeout）。
  - `crates/gateway/src/approval.rs:93`：`GatewayApprovalBroadcaster` 仅广播 WebSocket 事件 `exec.approval.requested`，无 channel/IM 路由。
  - `crates/gateway/src/assets/js/chat-ui.js:183`：Web UI 审批卡片通过 `exec.approval.resolve` 完成决策写回。
  - `crates/telegram/src/handlers.rs:674`：Telegram `/help` 命令列表不包含任何与 exec 审批相关的命令（例如 `/approve`）。
  - `crates/gateway/src/channel_events.rs:1076`：channel slash commands 列表中无 exec 审批相关分支（仅 clear/compact/context/model/sandbox 等）。
- 配置/协议证据（必要时）：
  - RPC：`exec.approval.resolve` 存在且可被任意客户端调用（需 scope）：`crates/gateway/src/approval.rs:56`
  - WebSocket feature events 含 `exec.approval.requested`：`crates/gateway/src/ws.rs:264`
- 当前测试覆盖：
  - 已有：Web UI/RPC resolve 单测（service 层）：`crates/gateway/src/approval.rs:115`
  - 缺口：Telegram 侧“审批请求通知 + allow/deny 回传 resolve”的闭环测试完全缺失。

## 根因分析（Root Cause）
- A. 审批请求的广播只面向 WebSocket/Web UI（`ApprovalBroadcaster` 只发 `request_id + command`），缺少“发往哪个 channel/chat”的信息与实现。
- B. Telegram 侧没有任何“审批”命令/回调处理，也没有 UI（inline keyboard）来回传 `exec.approval.resolve`。
- C. `exec` 非交互，且宿主机 sudo 常需要密码：即便实现了授权闭环，仍需把“sudo 密码不可用”的失败模式明确化，避免误解。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当 `exec` 在宿主机路径触发审批时，若请求来自 Telegram，会在 Telegram 内提供可用授权入口（Allow/Deny）并能完成 resolve。
  - 审批入口必须清晰展示：请求 ID（或短码）+ 命令摘要 + 超时信息。
  - 审批决策必须与 Web UI 同源（同一个 `ApprovalManager` request）。
- 不得：
  - 不得要求用户必须打开 Web UI 才能完成 Telegram 触发的审批。
  - 不得把审批请求广播到无关 chat（尤其群聊）。
- 应当：
  - 对超时/已结案的请求，应当有明确回执与重试指引。
  - 应当支持最小可用：文本命令授权（例如 `/approve <id> allow|deny`），再迭代到 inline keyboard。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐，最小闭环：命令式授权）
- 核心思路：
  - 当审批请求产生时，把请求以文本消息发送到 Telegram（同 chat 或 operator chat）。
  - 新增 Telegram 命令：`/approve <request_id> allow|deny`（或 `/approve_<short_code>`），由 gateway 调用 `ApprovalManager.resolve()`。
- 优点：实现简单、无需 Telegram callback/keyboard，容易测试与落地。
- 风险/缺点：交互不如按钮直观；request_id 可能较长（需短码映射）。

#### 方案 2（体验更好：inline keyboard 按钮）
- 核心思路：审批请求消息附带 inline keyboard（Allow/Deny），点击按钮触发 callback 直接 resolve。
- 优点：体验好；减少手输 ID。
- 风险/缺点：需要 Telegram callback handler、状态管理与去重；实现面更大。

### 最终方案（Chosen Approach）
> 本 Issue 先备档，不做实现；推荐后续以“方案 1”为 Phase 1，方案 2 为增强。

#### 行为规范（Normative Rules）
- 规则 1（溯源）：审批请求必须绑定一个“审批回执目标”（trigger chat 或 operator chat）。
- 规则 2（最小权限）：只有被允许的主体（至少：owner/operator/allowlist）可以 resolve；默认不在群聊开放任意人批准。
- 规则 3（可观测）：每条审批请求必须能在日志中定位到：来源（chan_type/account/chat）+ request_id + 决策者（如可得）+ 决策结果。

#### 接口与数据结构（Contracts）
- RPC：
  - 复用 `exec.approval.resolve`（外部 camelCase：`requestId`/`decision`/`command`）。
  - 需要新增或复用一个“审批请求状态查询”入口（用于 IM 侧显示 pending/expired；当前仅有 `pending_ids()` 列表能力）：`crates/gateway/src/approval.rs:51`
- UI/Channel：
  - Telegram：新增 `/approve` 命令与回执文本格式（或按钮交互）。

#### 失败模式与降级（Failure modes & Degrade）
- 如果无法把审批请求送达 Telegram（无 outbound/无权限/发送失败）：必须 fail-fast，并给出明确错误回执（提示“需要 Web UI 或配置 operator chat”）。
- 如果请求超时：必须回执“已过期”，提示重新发起命令。
- 如果宿主机 sudo 需要密码：即使允许执行，命令仍可能失败；必须明确错误分类（“已批准但宿主机需要交互式 sudo”）。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] Telegram 触发的宿主机审批，在 Telegram 内可完成 Allow/Deny，并能驱动命令继续/中止。
- [ ] Web UI 审批不回归（仍可正常显示并 resolve）。
- [ ] 审批请求不会泄露到错误的 chat（群聊/其它会话）。
- [ ] 超时/已结案请求有清晰回执与重试指引。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] 新增：审批请求“绑定目标 + resolve 权限校验 + 幂等”单测
- [ ] 新增：Telegram `/approve` 解析与调用 resolve 单测

### Integration
- [ ] 端到端：Telegram 触发需要审批的宿主机 `exec`，在 Telegram 内批准后继续执行

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：Telegram inline keyboard / callback 难以在纯单测覆盖（若采用方案 2）。
- 手工验证步骤：
  1) 在 Telegram DM 触发一个会走宿主机且需审批的命令
  2) 在 Telegram 内执行 allow/deny
  3) 验证命令继续/终止与回执一致

## 发布与回滚（Rollout & Rollback）
- 发布策略：建议默认关闭/按 channel 配置开启（避免群聊误开放）。
- 回滚策略：可通过配置关闭 Telegram 审批入口，回退到仅 Web UI 审批。

## 实施拆分（Implementation Outline）
- Step 1: 定义“审批回执目标”与 request_id ↔ target 的短期存储（内存/TTL）。
- Step 2: 扩展审批广播接口，使其携带来源上下文（至少 `_chanChatKey` 或 `ChannelReplyTarget`）。
- Step 3: Telegram：新增 `/approve` 命令（方案 1），并把审批请求通知发送到正确 chat。
- Step 4: 观测与回执：超时/拒绝/已批准但 sudo 密码失败等分类。
- 受影响文件（预估）：
  - `crates/tools/src/exec.rs`
  - `crates/gateway/src/approval.rs`
  - `crates/gateway/src/channel_events.rs`
  - `crates/telegram/src/handlers.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-persona-prompt-configurable-assembly-and-builtin-separation.md`（其中提到“宿主机变更需先确认”的口径）

## 未决问题（Open Questions）
- Q1: 审批请求默认发到“触发的 chat”还是“配置的 operator DM”？（群聊安全性 vs 可用性）
- Q2: request_id 是否需要短码（避免手输长 UUID）？短码冲突与 TTL 策略如何定义？
- Q3: 是否需要在 channel 层引入 `operator.approvals` 类似 scope 概念（与 Web UI scopes 对齐）？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

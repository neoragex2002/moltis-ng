# Issue: onboarding 首屏 WebSocket 就绪竞态（onboarding / websocket）

## 实施现状（Status）【增量更新主入口】
- Status: IN-PROGRESS
- Priority: P1
- Updated: 2026-03-21
- Owners: Codex
- Components: gateway/ui/onboarding
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-21：在 onboarding 页内收敛出带重试的 RPC 包装，避免 WebSocket 尚未 ready 时直接把首个 RPC 打成用户错误：`crates/gateway/src/assets/js/onboarding-view.js:26`
- 2026-03-21：auth step 的 `Skip for now` 现在会先启动 WebSocket，再进入下一步，避免进入 Agent 步后连接根本未开始：`crates/gateway/src/assets/js/onboarding-view.js:145`
- 2026-03-21：自动重试仅覆盖 `WebSocket not connected`；对 `WebSocket disconnected` 不做隐式重放，避免非幂等 RPC 被重复执行：`crates/gateway/src/assets/js/onboarding-view.js:41`

**已覆盖测试（如有）**
- 新增 E2E 回归用例，模拟延迟打开 WebSocket 后立即点击 Agent 步 Continue：`crates/gateway/ui/e2e/specs/onboarding.spec.js:237`

**已知差异/后续优化（非阻塞）**
- 当前机器缺 Playwright Chromium，新增 E2E 未在本地实际跑通，需要补环境后复验。
- `onboarding.spec.js` 中仍有若干旧文案断言使用 “Set up your identity”，后续可统一为 Agent/Identity 兼容口径。

---

## 背景（Background）
- 场景：用户重置到 V3 后首次进入 `/onboarding`，在 “Set up your agent” 步骤点击 Continue。
- 约束：不改协议、不加后向兼容、不引入新的持久化迁移。
- Out of scope：Telegram 渠道配置恢复、TOML/DB 迁移、auth 配置策略调整。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **WebSocket 就绪**（主称呼）：onboarding 页的共享 WebSocket 已完成浏览器连接并通过 `connect` 握手，前端可安全发送 RPC。
  - Why：首屏步骤依赖 RPC 保存 identity、拉取 provider、写入 channel。
  - Not：不是“页面已加载完 HTML”，也不是“只创建了 WebSocket 对象”。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：ws ready / connected

- **竞态**（主称呼）：页面已允许用户点击 Continue，但 WebSocket 尚未 ready，导致首个 RPC 直接失败。
  - Why：问题根因是前端初始化时序，而不是业务逻辑本身。
  - Not：不是 provider 校验失败，也不是后端方法返回错误。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：race / init race

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] onboarding 的首个 RPC 不得因为 WebSocket 尚未 ready 而直接向用户抛出 `WebSocket not connected`
- [x] auth step 被跳过时，必须启动 onboarding 所需的 WebSocket 连接

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把修复收敛在 onboarding 前端初始化与交互层
  - 不得改动后端协议、配置格式或持久化结构
- 兼容性：无持久化字段变更，无迁移要求
- 可观测性：保留现有错误面板语义；此问题不是静默分支，不额外加日志
- 安全与隐私：不记录 token、用户正文等敏感信息

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 用户在 onboarding 的 Agent 步点击 Continue，页面提示 `Error: WebSocket not connected`
2. 在 auth step 可跳过的路径下，进入下一步后 WebSocket 可能尚未 ready，甚至尚未开始连接

### 影响（Impact）
- 用户体验：首次配置流程被卡住，看起来像“保存 identity 失败”
- 可靠性：onboarding 首个 RPC 对时序敏感，存在非确定性失败
- 排障成本：表象像业务错误，实际是前端初始化竞态

### 复现步骤（Reproduction）
1. 打开 `/onboarding`
2. 若出现 auth step，点击 `Skip for now`
3. 在 “Set up your agent” 中填写名字并立即点击 Continue
4. 期望 vs 实际：期望进入 LLM 步；实际可能直接出现 `WebSocket not connected`

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/assets/js/helpers.js:21`：基础 `sendRpc()` 在 `S.ws` 未打开时直接返回 `WebSocket not connected`
  - `crates/gateway/src/assets/js/onboarding-view.js:469`：Agent 步保存 identity 依赖 RPC
  - `crates/gateway/src/assets/js/onboarding-view.js:145`：auth step skip 之前缺少统一的连接启动兜底
- 当前测试覆盖：
  - 已有：onboarding 基础渲染与跳转测试
  - 缺口：延迟 WebSocket 打开时，Agent 步 Continue 的时序回归

## 根因分析（Root Cause）
- A. onboarding 页面允许用户在 WebSocket 握手完成前进入可发送 RPC 的步骤
- B. Agent 步的 Continue 直接调用基础 `sendRpc()`，缺少“等待连接 ready”的薄封装
- C. auth step 的部分 skip 路径只切步骤，不保证启动 WebSocket，因此会把问题从“竞态”放大成“未连接”

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - onboarding 内部 RPC 必须等待共享 WebSocket ready 后再发送，或在短时间内进行有限重试
  - auth step 的 skip 路径必须显式启动 WebSocket
- 不得：
  - 不得把短暂初始化竞态直接暴露成用户错误
  - 不得把修复扩散成协议改造或配置迁移
- 应当：
  - 应当用单一、收敛的前端包装解决 onboarding 页内同类 RPC

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（onboarding 专用 RPC）：`onboarding-view.js` 内统一经由本地包装 `sendRpc()`，先 `ensureWsConnected()`，再等待 `S.connected && S.ws.readyState === WebSocket.OPEN`
- 规则 2（有限重试）：仅对 `WebSocket not connected` / `WebSocket disconnected` 做短暂重试，避免把真正业务错误吞掉
- 规则 2（有限重试）：仅对发送前的 `WebSocket not connected` 做短暂重试；对发送后的 `WebSocket disconnected` 不自动重放，避免副作用重复
- 规则 3（skip 一致性）：所有 auth skip 路径统一先启动连接，再进入下一步

#### 接口与数据结构（Contracts）
- API/RPC：无变更
- 存储/字段兼容：无变更
- UI/Debug 展示（如适用）：仍沿用现有错误面板；目标是避免误报

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：连接在有限时间内仍未 ready 时，仍返回 `WebSocket not connected`
- 队列/状态清理（必须 drain/必须删除/必须保留）：不新增本地队列，不改现有 pending 语义

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：无新增日志
- 禁止打印字段清单：token、正文、凭证

## 验收标准（Acceptance Criteria）【不可省略】
- [x] onboarding 的 Agent 步 Continue 不再因为短暂连接竞态直接失败
- [x] auth step 的 skip 路径会启动 WebSocket
- [ ] 延迟 WebSocket 打开时的 E2E 回归测试可在具备浏览器环境的机器上通过

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] N/A：本次为前端页面时序修复，未单独拆出 JS unit harness

### Integration
- [ ] N/A

### UI E2E（Playwright，如适用）
- [x] `crates/gateway/ui/e2e/specs/onboarding.spec.js`：新增延迟 WebSocket 打开后的 Continue 回归用例

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：当前机器未安装 Playwright Chromium，`npx playwright test ...` 无法真正启动浏览器
- 手工验证步骤：
  1. 打开 `/onboarding`
  2. 若出现 `Secure your instance`，点击 `Skip for now`
  3. 在 `Set up your agent` 中填写 `Your name` 与 `Agent name`
  4. 立即点击 `Continue`
  5. 验收口径：应进入 `Add LLMs`，且不出现 `Error: WebSocket not connected`

## 发布与回滚（Rollout & Rollback）
- 发布策略：前端默认生效，无开关
- 回滚策略：回退 `crates/gateway/src/assets/js/onboarding-view.js` 中 onboarding 专用 RPC 包装与 auth skip 调整
- 上线观测：观察 onboarding 首屏是否仍出现 `WebSocket not connected` 用户报错

## 实施拆分（Implementation Outline）
- Step 1: 在 onboarding 页建立 WebSocket ready 判定与有限重试包装
- Step 2: 修正 auth skip 路径，保证切步前启动连接
- Step 3: 增加延迟 WebSocket 打开的 Playwright 回归用例
- 受影响文件：
  - `crates/gateway/src/assets/js/onboarding-view.js`
  - `crates/gateway/ui/e2e/specs/onboarding.spec.js`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-v3-one-cut-readiness-gaps.md`
  - `docs/src/config-reset-and-recovery.md`
- Related commits/PRs：
- External refs（可选）：

## 未决问题（Open Questions）
- Q1: 是否要顺手统一 `onboarding.spec.js` 中旧的 “identity” 文案断言，避免 UI 改名导致测试误报？
- Q2: 是否需要把 onboarding 的 WebSocket ready 状态做成显式 UI 文案（例如 Connecting…）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

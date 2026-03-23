# Issue: 后台编辑表单草稿态回滚与 Telegram Bot 编辑契约错位（channels / crons）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（实现与仓库内自动化验证已闭环）
- Priority: P1
- Updated: 2026-03-24
- Owners: <TBD>
- Components: gateway / ui / telegram / cron
- Affected providers/models: N/A

**已实现（如有，写日期）**
- Telegram channel update 后端已经区分 `HotUpdate` 与 `IdentityChange`：`crates/gateway/src/channel.rs:57`
- Telegram channel add 路径已经通过 `getMe` 探测 bot 身份并派生 `chanAccountKey`：`crates/gateway/src/channel.rs:255`
- Cron 页面已有基础 Playwright 冒烟覆盖：`crates/gateway/ui/e2e/specs/cron.spec.js:1`
- 2026-03-24：`Edit Telegram Bot` 改为单一草稿态驱动渲染与提交，`channels.update` 不再发送 `token`；`model` 未变化时保留既有 `model_provider`，避免普通编辑误改展示/排障字段：`crates/gateway/src/assets/js/page-channels.js:351`
- 2026-03-24：`Add Telegram Bot` 改为完整草稿态，关闭/成功后统一重置默认值与瞬时状态：`crates/gateway/src/assets/js/page-channels.js:368`
- 2026-03-24：`Channels` Add/Edit 与 `CronModal` 都加入本地请求代次失效保护；弹窗关闭后，陈旧保存回包不再污染下一次打开：`crates/gateway/src/assets/js/page-channels.js:372`、`crates/gateway/src/assets/js/page-crons.js:637`
- 2026-03-24：`CronModal` 改为稳定组件级草稿态，保存失败时保留输入并在弹窗内显示错误：`crates/gateway/src/assets/js/page-crons.js:630`

**已覆盖测试（如有）**
- Telegram patch 分类已有单元测试：`crates/gateway/src/channel.rs:712`
- 2026-03-24：补充 Telegram 热更新字段集合与 `model/model_provider` 清空 merge 语义测试：`crates/gateway/src/channel.rs:730`
- 2026-03-24：补充 `Cron` 编辑态回滚回归用例：`crates/gateway/ui/e2e/specs/cron.spec.js:38`
- 2026-03-24：`cargo test -p moltis-gateway channel -- --nocapture` 实跑通过（`48 passed`）
- 2026-03-24：`LD_LIBRARY_PATH=/tmp/pw-libs/root/usr/lib/x86_64-linux-gnu npx playwright test e2e/specs/cron.spec.js` 实跑通过（`6 passed`）
- 2026-03-24：`page-channels.js` / `page-crons.js` 经过 esbuild 解析检查通过
- 2026-03-24：新增 `Channels` 页面 E2E，通过浏览器内 WS mock 覆盖 edit/add 失败保草稿、`token` 不出现在 edit payload、`agent_id` 清空发 `null`、关闭后陈旧回包不污染重开弹窗：`crates/gateway/ui/e2e/specs/channels.spec.js:162`

**已知差异/后续优化（非阻塞）**
- 本单不设计新的 Telegram `Reconnect / Replace Bot` 产品流程
- 本单不引入跨页面通用表单框架，只修当前已确认问题页面
- 为跑通本机 Playwright，验证阶段使用了用户态注入的浏览器动态库路径（`LD_LIBRARY_PATH=/tmp/pw-libs/root/usr/lib/x86_64-linux-gnu`）；仓库实现与运行契约未因此扩大

---

## 背景（Background）
- 场景：用户在 Web UI 的 `Edit Telegram Bot` 中修改 agent / DM policy / group dispatch 等可编辑项，或在 `Cron` 弹窗中修改任务配置后点击保存。
- 约束：Telegram bot 身份字段（`token` / `chan_user_id` / `chan_user_name` / `chan_nickname`）当前由后端视为账号身份，不允许通过普通 `channels.update` 原地替换。
- Out of scope：
  - 不修改 Telegram bot 身份字段的产品口径
  - 不补新的 RPC（例如 `channels.replace`）
  - 不顺手改造 MCP / Hooks / Settings 等已采用独立草稿态的页面

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **表单草稿态**（主称呼）：弹窗打开后，页面用于承载当前用户输入的本地可变状态。
  - Why：rerender 后必须继续显示用户刚刚编辑过的值，不能回退到旧配置或默认值
  - Not：不是 DOM 上当前值的临时查询结果；不是后端返回的只读基线配置
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：draft state / local form state

- **Telegram 身份字段**（主称呼）：`token`、`chan_user_id`、`chan_user_name`、`chan_nickname`
  - Why：这些字段决定 Telegram bot 的真实账号身份与 `chanAccountKey`
  - Not：不是普通热更新配置；不是 `Edit Telegram Bot` 这次要支持的字段
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：identity fields

- **Telegram 可热更新字段**（主称呼）：`channels.update` 允许原地修改且不改变 bot 身份的配置字段
  - Why：普通编辑弹窗只能提交这组字段
  - Not：不包含任何 Telegram 身份字段
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：hot-update fields

- **表单初始化时机**（主称呼）：弹窗从“关闭”进入“打开”，或编辑目标从一个对象切换为另一个对象时，才允许用基线配置重建草稿态。
  - Why：避免保存中、报错中、普通 rerender 时把用户输入覆盖回旧值
  - Not：不是每次组件 rerender 都重新从 `cfg` / `job` 回灌
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：draft bootstrap timing

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] `Edit Telegram Bot` 只提交 Telegram 可热更新字段，不再误带 `token`
- [x] `Edit Telegram Bot` 在保存中、校验失败或其他 rerender 时，表单仍保持当前草稿值
- [x] `Add Telegram Bot` 在弹窗 rerender 时，已输入值不被默认值或空值刷回
- [x] `Cron` 的 Add/Edit 弹窗在保存与本地交互时，表单仍保持当前草稿值
- [x] 弹窗关闭并重新打开后，错误提示与临时 saving 状态正确重置，不残留上次失败痕迹

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把“普通编辑”与“更换 bot 身份”严格分开
  - 必须避免 `saving=true`、错误状态、局部 rerender 导致表单闪回旧值
  - 必须把修复收敛在当前问题页面与现有 Telegram update 边界内闭环
  - 不得为了修这个问题引入新的跨页面通用状态框架
  - 不得扩展 Telegram 身份字段的原地热更新能力
- 收敛性：
  - 只允许修改当前直接命中的 `Channels` / `Crons` 编辑路径
  - 不抽象新的公共 form store / form helper / form framework
  - 不新增跨模块共享概念，避免把页面局部问题扩散到 gateway 其他区域
- 兼容性：保持现有 Telegram 后端“身份字段不能原地更新”的硬约束不变；仅修 UI 提交口径与表单状态管理
- 可观测性：至少保留现有后端错误信息；前端保存失败时应继续在弹窗内显示错误，不得静默关闭
- 安全与隐私：Telegram token 仍不得在 UI 明文展示、日志打印或错误提示中泄露

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 在 `Edit Telegram Bot` 中仅修改普通字段并点击 `Save Changes`，会收到报错：`telegram identity fields cannot be updated in place; remove and re-add the bot`
2) 在 `Edit Telegram Bot` 中点击保存后，弹窗关闭前一瞬间，表单控件会闪回保存前的旧值/默认值
3) `Cron` Add/Edit 弹窗采用相同的“旧 props 回灌 + DOM 取值”模式，存在同类输入回退风险

### 影响（Impact）
- 用户体验：编辑弹窗行为明显反直觉，用户会误以为保存未生效或界面状态失真
- 可靠性：前端提交契约与后端热更新契约错位，导致普通编辑稳定失败
- 排障成本：问题表面看像“后端不支持 edit”，实际是 UI 提交与表单状态双重缺陷，容易误判

### 复现步骤（Reproduction）
1. 打开 `Channels`，对某个已接入的 Telegram bot 点击 `Edit`
2. 只修改 `Agent`、`DM Policy` 或 `Group Dispatch` 中任一普通字段
3. 点击 `Save Changes`
4. 期望 vs 实际：
   - 期望：普通字段保存成功，弹窗稳定关闭
   - 实际：后端报“身份字段不能原地更新”；且弹窗关闭前表单会瞬时回退旧值

补充复现：
1. 打开 `Crons` → `Edit Job`
2. 修改名称 / schedule / payload 等任一字段
3. 在触发保存或本地 rerender 的过程中，表单可能回灌旧值或默认值

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/channel.rs:63`：`classify_telegram_config_patch` 将 `token`、`chan_user_id`、`chan_user_name`、`chan_nickname` 归类为 `IdentityChange`
  - `crates/gateway/src/channel.rs:383`：Telegram update 命中 `IdentityChange` 时直接报错，要求 remove and re-add
  - `crates/gateway/src/assets/js/page-channels.js:351`：`buildModelUpdateFields` 只在 `model` 真变化时重算 `model_provider`；未改模型时保留存量 provider，避免普通编辑误改无关字段
  - `crates/gateway/src/assets/js/page-channels.js:368`：`AddChannelModal` 改为完整草稿态驱动提交，不再依赖 DOM 临时取值
  - `crates/gateway/src/assets/js/page-channels.js:522`：`EditChannelModal` 改为完整草稿态驱动提交，`channels.update` payload 不再包含 `token`
  - `crates/gateway/src/assets/js/page-channels.js:372`：Channels Add/Edit 通过 `requestVersion` 失效陈旧异步回包，关闭后旧请求不再污染下一次打开
  - `crates/gateway/src/assets/js/page-crons.js:630`：`CronModal` 改为稳定组件级草稿态（`useState`），不再在 rerender 路径里重建局部 signal
  - `crates/gateway/src/assets/js/page-crons.js:637`：`CronModal` 通过 `requestVersionRef` 失效陈旧异步回包，关闭后旧请求不再污染下一次打开
- 配置/协议证据（必要时）：
  - `crates/gateway/src/channel.rs:255`：Telegram add 路径通过 `getMe` 探测真实 bot 身份，并派生 `chanAccountKey = telegram:<chan_user_id>`
- 当前测试覆盖：
  - `crates/gateway/src/channel.rs:730`：热更新字段集合正向测试
  - `crates/gateway/src/channel.rs:748`：`model/model_provider` 清空 merge 语义测试
  - `crates/gateway/ui/e2e/specs/cron.spec.js:38`：`Cron` 编辑态在校验失败 rerender 时保持当前草稿
  - `crates/gateway/ui/e2e/specs/channels.spec.js:162`：Channels 关键编辑/新增回归通过浏览器内 WS mock 自动化覆盖

## 根因分析（Root Cause）
- A. Telegram 后端把 bot 身份字段与普通热更新字段明确分开，这是合理的账号边界约束
- B. `Edit Telegram Bot` 前端在普通编辑请求里误带 `token`，把“普通编辑”错误升级成“身份变更”
- C. `Channels` 与 `Cron` 两处弹窗都采用“部分 signal + 部分旧 props 回填 + 保存时直接 query DOM”的混合模式；一旦 `saving`、错误态或局部状态变化触发 rerender，就会把表单刷新回旧配置或默认值
- D. `CronModal` 还在组件函数体内直接创建 `signal()`，这会让局部状态生命周期跟随 rerender 重建，进一步放大输入回退问题

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - `Edit Telegram Bot` 只提交 Telegram 可热更新字段
  - `Edit Telegram Bot` / `Add Telegram Bot` / `Cron` 弹窗都维护完整表单草稿态
  - 保存请求发出后，表单继续显示当前草稿值，直到成功关闭或用户手动取消
  - 保存失败时，错误提示留在当前弹窗中，草稿值原样保留
  - `Edit Telegram Bot` 的 model 清空操作必须真正清空 `model` 与 `model_provider`
  - 弹窗每次重新打开时，必须清空上次遗留的 error / saving / errorField 等瞬时状态
- 不得：
  - 不得在 `Edit Telegram Bot` 的普通保存请求中携带 `token`
  - 不得在 rerender 时重新从旧 `cfg` / 旧 `job` 回灌可编辑控件
  - 不得以本单为由放开 Telegram 身份字段原地更新
  - 不得为解决单页问题引入新的通用表单基础设施
  - 不得把这次修复扩展成“统一后台所有弹窗状态管理”的顺手重构
- 应当：
  - 应当把弹窗打开时的基线值一次性复制到本地草稿对象
  - 应当让渲染、校验、提交三者都只依赖同一份草稿态

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：保留 Telegram 后端身份字段硬约束不变；把 `Channels` 和 `Cron` 相关弹窗改为显式本地草稿态，并修正 `Edit Telegram Bot` 提交口径
- 优点：
  - 范围小，直接命中根因
  - 高内聚，状态逻辑留在原页面内部
  - 不改变现有后端身份边界
  - 不把复杂度扩到其他已正常页面
- 风险/缺点：
  - 需要在 `page-channels.js` 和 `page-crons.js` 分别整理表单状态
  - Channels UI 自动化覆盖仍可能受 Telegram 外部依赖限制

#### 方案 2（备选）
- 核心思路：抽一套通用 modal/form 框架，统一所有后台页面的草稿态与提交逻辑
- 优点：
  - 看起来“统一”
- 风险/缺点：
  - 明显超出本单范围
  - 会把简单修复扩成全局重构
  - 不符合当前 one-cut 收敛要求

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：`Edit Telegram Bot` 生成的 `channels.update` payload 只能包含当前 UI 暴露且后端允许热更新的字段：`agent_id`、`dm_policy`、`group_line_start_mention_dispatch`、`group_reply_to_dispatch`、`allowlist`、`model`、`model_provider`
- 规则 2：Telegram 身份字段仍只允许在 add / remove-and-readd 口径下变化
- 规则 3：`Channels` Add/Edit 与 `Cron` Add/Edit 的每个可编辑控件都必须绑定到同一份本地草稿态
- 规则 4：`saving`、错误态、局部切换引起的 rerender 不得改变当前草稿值
- 规则 5：实现必须优先在原页面局部收口，不新增通用抽象层
- 规则 6：弹窗草稿态只能在“打开弹窗”或“切换编辑目标”时从基线值初始化；普通 rerender 不得重置
- 规则 7：Add 弹窗关闭后必须重置回冻结默认值，避免上次未提交输入泄漏到下次打开
- 规则 8：`Cron` 仅要求当前可见字段在无关 rerender 中保持稳定；不要求为“已切换隐藏的 schedule kind”额外保留独立历史草稿
- 规则 9：组件局部状态必须使用稳定的组件级 state 容器（如 `useSignal` / `useState`），不得在函数体 rerender 路径中直接新建 `signal(...)`
- 规则 10：`Edit Telegram Bot` 当前的 “Missing: <agent_id>” 回显能力必须保留，重构后不能把未知 agent 强行吞掉或改写

#### 接口与数据结构（Contracts）
- API/RPC：
  - `channels.update`：本单后 UI 仅发送热更新字段，不再发送 `token`
  - `channels.add`：继续允许发送 `token`，但 Add 弹窗需要本地草稿态承载所有输入
  - `cron.add` / `cron.update`：提交值统一来自本地草稿态，不再运行时拼凑旧 `job` + DOM 查询
- 字段清空语义：
  - `agent_id`：用户选回 `(default)` 时，必须发送 `null`
  - `model`：用户清空默认模型时，必须发送 `null`
  - `model_provider`：当 `model` 被清空时，必须同时发送 `null`，防止旧 provider 残留
  - `model_provider`：当用户选中了新 `model` 但当前前端列表无法解析 provider 时，也必须发送 `null`，不得沿用旧 provider
  - `model_provider`：当用户未改动 `model` 时，必须保留存量 `model_provider`，不得因当前前端模型列表缺失而把普通编辑误变成 provider 清空
  - `allowlist`：以当前草稿数组整体发送
- 存储/字段兼容：
  - 不新增持久化字段
  - 不修改 Telegram 现有存储 schema
  - 继续依赖 `channels.update` 当前的 merge-patch 语义：未出现在 patch 中的字段保持存量值不变
- UI/Debug 展示（如适用）：
  - 保存失败时继续在当前弹窗显示 error message
  - 不新增额外 debug UI
  - 已绑定但本地 agent 列表不存在的 `agent_id`，继续以 `Missing: <agent_id>` 形式展示

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - Channels 普通编辑失败时，继续显示后端错误，但不得丢草稿态
  - Cron 保存失败时，继续停留在当前弹窗，保留已编辑内容
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 成功关闭或用户取消后，必须清理草稿态与瞬时错误状态
  - 失败时必须保留草稿态

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - Telegram token 继续只存在于 Add 提交流程，不在 Edit 弹窗中展示
- 禁止打印字段清单：
  - `token`

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `Edit Telegram Bot` 修改普通字段后可成功保存，不再触发“remove and re-add the bot”
- [x] `Edit Telegram Bot` 点击保存后，弹窗关闭前不再闪回旧值
- [x] `Edit Telegram Bot` 将默认模型清空后，`model` 与 `model_provider` 均被真正清空，不残留旧 provider
- [x] `Edit Telegram Bot` 在已绑定 agent 缺失于当前列表时，仍保留 `Missing: <agent_id>` 回显与可清空能力
- [x] `Add Telegram Bot` 在发生 rerender 时，不会把已输入草稿刷回默认值或空值
- [x] `Add Telegram Bot` 关闭并重新打开后，表单恢复冻结默认值，不保留上次未提交输入
- [x] `Cron` Add/Edit 弹窗在保存与本地交互期间保持草稿态稳定
- [x] `Channels` / `Cron` 弹窗在关闭重开后，不残留上一次 error / saving 状态
- [x] Telegram 身份字段仍保持“不可原地更新”的后端约束

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 更新 `crates/gateway/src/channel.rs` 现有测试：继续证明 `token` 属于 `IdentityChange`，同时补当前 UI 使用的热更新字段集合正向覆盖
- [x] 补一条 merge-patch 语义测试：`model: null` / `model_provider: null` 可正确清空，且不影响未出现在 patch 中的 `token`

### UI E2E（Playwright，如适用）
- [x] 更新 `crates/gateway/ui/e2e/specs/cron.spec.js`：覆盖 `Cron` 弹窗编辑后在 rerender/save 期间不闪回旧值
- [x] 新增 `crates/gateway/ui/e2e/specs/channels.spec.js`：覆盖 `Channels` 编辑/新增失败时保草稿、edit payload 不带 `token`、关闭后陈旧回包不污染重开弹窗

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 仓库内仍没有可直接复用的真实 Telegram provider 端到端 fixture；本单继续避免把外部 Telegram 依赖引入 CI
  - `LiveChannelService` 当前没有低成本 mock seam；本单采用浏览器内 WS mock 做页面契约回归，而不是新增更重的 service integration harness
- 手工验证步骤：
  1. 如需真实 provider 验收，再接入一个测试 Telegram bot
  2. 在 `Edit Telegram Bot` 中分别修改 `Agent`、`DM Policy`、group dispatch checkbox
  3. 点击 `Save Changes`，确认无“remove and re-add”报错，且关闭前不闪回
  4. 在 `Edit Telegram Bot` 中清空默认模型，确认保存后 `model` 与 `model_provider` 同步清空
  5. 对一个 agent 已丢失的 Telegram bot 打开编辑弹窗，确认仍显示 `Missing: <agent_id>`，且用户可以清空为 `(default)`
  6. 打开 `Add Telegram Bot`，确认失败保草稿；关闭并重开后恢复默认值且无旧 error
  7. 打开 `Cron` 的 Add/Edit 弹窗，确认保存/失败期间不闪回；关闭并重开后无旧 error / saving 状态

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随 gateway/ui 常规发布；无 feature flag
- 回滚策略：回滚本单涉及的 `page-channels.js` / `page-crons.js` / `channel.rs` 修改
- 上线观测：重点观察 Channels/Cron 页面前端报错、`channels.update` 失败率、用户是否再报告保存瞬间闪回

## 实施拆分（Implementation Outline）
- Step 1: 锁定 Telegram update 契约与测试口径
- Step 2: 重构 `Add Telegram Bot` 与 `Edit Telegram Bot` 为统一草稿态
- Step 3: 修正 `Edit Telegram Bot` 的 update payload，不再发送 `token`
- Step 4: 冻结并实现 `agent_id / model / model_provider` 清空语义
- Step 5: 重构 `CronModal` 为统一草稿态
- Step 6: 补齐自动化测试与手工验收记录
- 实施约束：
  - 优先在 `page-channels.js` 与 `page-crons.js` 内局部闭环
  - 仅在确有必要时，才对 `channel.rs` 做最小契约侧补充
  - 不新增公共 form 基础设施，不做 unrelated cleanup
- 受影响文件：
  - `crates/gateway/src/channel.rs`
  - `crates/gateway/src/assets/js/page-channels.js`
  - `crates/gateway/src/assets/js/page-crons.js`
  - `crates/gateway/ui/e2e/specs/cron.spec.js`

## 详细实施计划（Implementation Plan）

### Task 1：锁定 Telegram update 契约与清空语义

**Files**
- Modify: `crates/gateway/src/channel.rs`
- Test: `crates/gateway/src/channel.rs`

**Step 1：补契约测试覆盖**
- 在 `classify_telegram_config_patch_detects_identity_changes` 附近补两类断言：
  - 仅包含当前 UI 使用的热更新字段的 patch 仍判定为 `HotUpdate`
  - `token` / `chan_user_*` 仍判定为 `IdentityChange`
- 补一条 merge-patch 语义测试：
  - `model: null` + `model_provider: null` 会真正清空
  - 未出现在 patch 中的 `token` 保持存量值

**Step 2：运行测试确认当前口径**
- Run: `cargo test -p moltis-gateway channel -- --nocapture`
- Expected:
  - patch 分类与 merge 相关测试通过

**Step 3：实现最小后端修正**
- 保持 Telegram 生产契约不变
- 仅在测试暴露真实契约缺口时，才对 `channel.rs` 做最小修正
- 不扩展新 RPC，不引入新的 service test seam

**Step 4：重新运行测试**
- Run: `cargo test -p moltis-gateway channel -- --nocapture`
- Expected: PASS

### Task 2：重构 `Edit Telegram Bot` 草稿态并修正 payload

**Files**
- Modify: `crates/gateway/src/assets/js/page-channels.js`

**Step 1：建立单一草稿对象**
- 在 `EditChannelModal` 中引入单一草稿对象，至少覆盖：
  - `agent_id`
  - `dm_policy`
  - `group_line_start_mention_dispatch`
  - `group_reply_to_dispatch`
  - `model`
  - `allowlist`
- 弹窗打开时一次性从 `ch.config` 初始化
- 仅在“切换编辑目标 / 重新打开弹窗”时重建草稿；保存中与报错时不得重建

**Step 2：让渲染完全依赖草稿态**
- 把 `select` / `checkbox` / `ModelSelect` / `AllowlistInput` 全部改为读写草稿态
- 删除保存时对这些字段的 `querySelector(...)` 依赖
- 保留当前 `Missing: <agent_id>` 的回显与清空行为

**Step 3：修正提交契约**
- `channels.update` payload 中删除 `token`
- 仅发送当前 UI 暴露的热更新字段
- `agent_id` 选回默认时发送 `null`
- 默认模型清空时同时发送 `model: null` 与 `model_provider: null`
- 保存失败时保留弹窗与草稿态
- 重新打开弹窗时清空旧 error / saving 状态

**Step 4：手工核对关键路径**
- 点击 `Save Changes` 前后，确认不会因 `saving=true` 而闪回旧值
- 保存失败时，错误消息显示在弹窗内且输入不丢

### Task 3：重构 `Add Telegram Bot` 草稿态

**Files**
- Modify: `crates/gateway/src/assets/js/page-channels.js`

**Step 1：建立完整草稿态**
- 覆盖：
  - `token`
  - `agent_id`
  - `dm_policy`
  - `group_line_start_mention_dispatch`
  - `group_reply_to_dispatch`
  - `model`
  - `allowlist`
- 关闭弹窗后重置为冻结默认值，不保留上次未提交输入

**Step 2：让 Add 弹窗渲染与提交都只读草稿态**
- 删除对 DOM 的临时取值依赖，`onSubmit` 直接读取草稿对象
- 关闭成功与手动关闭都显式重置 Add 草稿态
- 同时重置旧 error / saving 状态

**Step 3：手工核对 rerender 保持**
- 在 agent 列表加载、错误提示出现、`saving=true` 等情况下，已输入内容不应丢失

### Task 4：重构 `CronModal` 草稿态

**Files**
- Modify: `crates/gateway/src/assets/js/page-crons.js`
- Test: `crates/gateway/ui/e2e/specs/cron.spec.js`

**Step 1：扩展 E2E**
- 扩展 `cron.spec.js`，走完整 UI 流程：
  - 创建一个 job
  - 打开该 job 的 `Edit`
  - 修改字段后触发保存
  - 断言保存期间与保存后不存在输入闪回或页面报错

**Step 2：引入统一草稿态**
- 覆盖：
  - `name`
  - `schedule.kind`
  - `schedule.at/every/cron/tz`
  - `payloadKind`
  - `message`
  - `sessionTarget`
  - `deleteAfterRun`
  - `enabled`
- 改用稳定的组件级 state 容器，不再在组件函数体中直接 `signal(...)`

**Step 3：移除混合模式**
- 渲染只绑定草稿态
- 保存时从草稿态构造 `cron.add` / `cron.update` payload

**Step 4：运行 E2E**
- Run: `just ui-e2e`
- Expected:
  - `cron.spec.js` 新增用例通过
  - 无新增页面级 JS error

### Task 5：全量验证与交付

**Files**
- Modify: `issues/issue-gateway-ui-edit-draft-state-and-telegram-channel-update-contract.md`

**Step 1：运行针对性验证**
- Run:
  - `cargo test -p moltis-gateway channel -- --nocapture`
  - `just ui-e2e`
- Expected:
  - 后端契约测试通过
  - UI E2E 不回归

**Step 2：记录自动化缺口与手工验收**
- 在 issue 中明确记录：
  - Channels 深度 UI 以手工验收为准
  - 本单不为 `LiveChannelService` 额外引入新的 integration test seam

**Step 3：更新 issue 状态**
- 在本 issue 中回填：
  - 实现点
  - 测试覆盖
  - 已知差异
  - `Status / Updated`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-telegram-group-bot-to-bot-mentions-relay-via-moltis.md`
  - `issues/done/issue-telegram-group-ingest-reply-decoupling.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

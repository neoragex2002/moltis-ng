# Issue: <一句话标题>（<关键词1> / <关键词2>）

## 实施现状（Status）【增量更新主入口】
- Status: [TODO|IN-PROGRESS|DONE|SURVEY]
- Priority: [P0|P1|P2|P3]
- Updated: <YYYY-MM-DD>
- Owners: <人/组，可选>
- Components: <gateway/agents/sessions/ui/telegram/...>
- Affected providers/models: <如 openai-responses::gpt-5.2，可选>

**已实现（如有，写日期）**
- <实现点一句话>：`path/to/file:line`
- <实现点一句话>：`path/to/file:line`

**已覆盖测试（如有）**
- <test 名称/覆盖点>：`path/to/file:line`

**已知差异/后续优化（非阻塞）**
- <非阻塞点1>
- <非阻塞点2>

---

## 背景（Background）
- 场景：<谁在什么路径触发>
- 约束：<Responses API、自定义 base_url、token 口径、隐私等>
- Out of scope：<明确不做>

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **<主概念A>**（主称呼）：<一句话定义（What）>
  - Why：<为什么重要>
  - Not：<它不是什么/不包含什么>
  - Source/Method：[authoritative|estimate|configured|effective|as-sent]
  - Aliases（仅记录，不在正文使用）：<别名1/别名2>

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] <目标1>
- [ ] <目标2>

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - <必须…>
  - <不得…>
- 兼容性：<老配置/老数据/行为兼容策略>
- 可观测性：<日志、debug 面板字段、指标>
- 安全与隐私：<脱敏/不打印敏感字段>

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) <用户看到的现象>
2) <系统层现象>

### 影响（Impact）
- 用户体验：
- 可靠性：
- 排障成本：

### 复现步骤（Reproduction）
1. <步骤1>
2. <步骤2>
3. 期望 vs 实际：<对比>

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `path/to/file:line`：<说明>
- 配置/协议证据（必要时）：
  - <字段名/枚举值/失败模式>
- 当前测试覆盖：
  - 已有：<test refs>
  - 缺口：<未覆盖路径>

## 根因分析（Root Cause）
- A. <上游/触发>
- B. <中间逻辑缺陷>
- C. <下游表现/为什么没兜住>

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - <必须…>
- 不得：
  - <不得…>
- 应当：
  - <应当…>

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
- 优点：
- 风险/缺点：

#### 方案 2（备选）
- …

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（明确 source/method）：<…>
- 规则 2：

#### 接口与数据结构（Contracts）
- API/RPC：
- 存储/字段兼容：
- UI/Debug 展示（如适用）：<字段顺序：常量在前、变量在后；重要在前>

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
- 队列/状态清理（必须 drain/必须删除/必须保留）：

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
- 禁止打印字段清单：

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] <验收点1>
- [ ] <验收点2>
- [ ] <回归点>

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] <test 名>：`path/to/file:line`

### Integration
- [ ] <test/说明>

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/<name>.spec.js`：<覆盖点>

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
- 手工验证步骤：

## 发布与回滚（Rollout & Rollback）
- 发布策略：<feature flag/默认关闭/灰度>
- 回滚策略：<如何回滚，回滚风险>
- 上线观测：<关键日志/指标/报警>

## 实施拆分（Implementation Outline）
- Step 1:
- Step 2:
- Step 3:
- 受影响文件：
  - `path/to/file`

## 交叉引用（Cross References）
- Related issues/docs：
- Related commits/PRs：
- External refs（可选）：

## 未决问题（Open Questions）
- Q1:
- Q2:

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

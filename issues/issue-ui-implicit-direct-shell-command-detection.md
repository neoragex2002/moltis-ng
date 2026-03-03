# Issue: WebUI 隐式“命令输入”识别规则不显性导致误触发 exec（ui / agents）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Owners: <TBD>
- Components: agents/runner, gateway/ui
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- direct-shell-command 启发式：把用户最新一条纯文本输入在满足若干条件时识别为“直接 shell 命令”：`crates/agents/src/runner.rs:153`
- 当模型未返回结构化 tool_calls 且识别到 direct shell command 时，runner 会强制注入一次 `exec` tool call（UI 侧表现为“输入即执行”）：`crates/agents/src/runner.rs:880`

**已覆盖测试（如有）**
- <N/A>（当前未发现针对 “hello 被误判为命令” 的测试用例）

**已知差异/后续优化（非阻塞）**
- 该误判也可能间接放大 sandbox 相关时序问题（例如启动后更容易触发首次 exec）：`issues/issue-sandbox-prebuild-race-home-sandbox-workdir.md:64`

---

## 背景（Background）
- 场景：用户在 WebUI 的 chat 输入框里，清空会话后输入 `hello` 这类“自然语言问候/短词”，期望进入普通对话。
- 现状：系统将其识别为“直接 shell 命令”，并在模型返回纯文本时强制触发一次 `exec`，导致 `sh: 1: hello: not found`（exit 127）。
- 约束：
  - UI 需要支持“直接命令输入”的快捷体验（例如输入 `pwd` / `ls` 立即执行）。
  - 但规则必须可理解、可预测、可关闭/可配置，避免 silent magic。
- Out of scope：
  - 本 issue 不重做整体工具调用/权限系统；聚焦在“命令识别规则显性化 + 降低误判”。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **direct shell command**（主称呼）：runner 对用户最新输入做启发式判断，认为其“形似 shell 命令”的输入，并在特定情况下走 exec 强制调用路径。  
  - Why：让 UI 里的“命令输入” deterministic（无需依赖 LLM 决策）。
  - Not：它不是用户显式点击“运行命令”按钮或显式选择 tool 的操作（当前没有明确 UI 交互信号）。
  - Source/Method：effective（运行态启发式）
  - Aliases（仅记录，不在正文使用）：direct command input / command turn

- **误判（false positive）**（主称呼）：像 `hello` 这种并非命令的输入被当作命令执行。  
  - Why：会引入困惑、破坏对话体验、造成不必要的沙盒执行与噪声日志。
  - Not：不是“命令执行失败”（例如真的运行了 `git` 但 exit 非 0）；而是入口分类错误。
  - Source/Method：as-observed（日志/复现）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 用户能明确知道何时输入会被当作命令执行（规则显性化）。
- [ ] 降低误判：`hello`、`thanks`、`ok` 等常见短词不应触发 exec。
- [ ] 提供显式/可控入口：例如 UI toggle、“以 `>` 开头才当命令”、或专用命令模式输入框。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：自然语言输入默认走对话，不应 silent 触发 exec。
  - 不得：在没有 UI 明确信号的情况下做高风险/不可预测的工具执行（尤其当 sandbox 关闭时还会触发 approval 流程）。
- 可观测性：
  - 需要日志/事件明确标注“为什么被判定为 direct shell command”（至少给出命中的规则/原因码）。
- 安全与隐私：
  - 避免把用户自然语言误当命令执行（减少意外副作用）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) WebUI 输入 `hello` 后，runner 日志出现：
   - `forcing exec tool call from direct command input command=hello`
2) exec 工具执行 `sh -c hello`，并返回：
   - `sh: 1: hello: not found`（exit 127）

### 影响（Impact）
- 用户体验：用户不知道系统把输入当命令执行，导致“我只是打招呼怎么跑去执行了？”的强困惑。
- 可靠性：会产生不必要的 sandbox 容器 ensure_ready/exec 开销；在某些环境下可能触发额外错误（例如容器 workdir 问题）。
- 排障成本：需要用户阅读日志/理解隐式规则才能解释行为。

### 复现步骤（Reproduction）
1. WebUI 进入 main 会话，点击 clear。
2. 输入：`hello`（无标点、无换行）。
3. 期望 vs 实际：
   - 期望：进入对话，assistant 回复问候或解释。
   - 实际：触发 exec，报 `hello: not found`。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/agents/src/runner.rs:153`：`direct_shell_command_from_user_content()` 的启发式规则会对 `hello` 返回 Some（满足 “短、单行、ASCII、无标点结尾、首 token 合法” 等条件）。
  - `crates/agents/src/runner.rs:880`：当模型无 tool_calls 且识别到 direct shell command 时强制注入 exec tool call。
- 日志证据（关键词）：
  - `forcing exec tool call from direct command input`
  - `exec tool invoked command=hello`

## 根因分析（Root Cause）
- A. 入口信号缺失：UI 没有提供“命令模式”的显式开关/前缀/按钮。
- B. 启发式过宽：`direct_shell_command_from_user_content()` 仅做“形状”判断，无法区分问候/短词与真实命令。
- C. 强制执行策略：当 LLM 返回纯文本时，runner 会强制注入 exec，放大误判后果（从“分类错误”变成“真实执行”）。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 只有在用户显式进入“命令模式”时才执行 exec（或至少需要高置信度命令识别）。
  - 自然语言短词（`hello`/`thanks`/`ok`）不得触发 exec。
- 应当：
  - 在 UI 中展示清晰提示（例如“以 `>` 开头运行命令”或一个显式 toggle）。
  - 日志应记录命中规则（reason code），便于排障。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：显式命令前缀 / 模式开关
- 核心思路：只有当输入以 `>`（或 `/exec `）开头时才视为 direct shell command；否则一律当普通对话。
- 优点：规则清晰、可文档化、可教学。
- 风险/缺点：破坏现有“裸输入 pwd 即执行”的习惯（需要迁移提示）。

#### 方案 2（备选）：收紧启发式 + allowlist 常见命令
- 核心思路：对 first token 做 allowlist（`pwd/ls/cd/echo/git/...`），或 denylist 常见口语词（`hello/hi/ok/thanks/...`）。
- 优点：保持现有快捷体验。
- 风险/缺点：规则仍不直观，边界难维护；对多语言输入更复杂。

#### 方案 3（备选）：仅在模型明确请求 exec 时才执行（移除强制注入）
- 核心思路：取消“最终兜底强制 exec”，让 LLM 自己决定是否调用工具。
- 优点：减少 silent magic，避免误判变成真实执行。
- 风险/缺点：命令执行不再 deterministic；可能增加 LLM 往返。

### 最终方案（Chosen Approach）
- <TBD>

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] WebUI 输入 `hello` 不会触发 exec。
- [ ] 有明确且可发现的方式触发“命令输入”（例如前缀或 toggle）。
- [ ] 日志能定位“为何识别为命令”的原因码（或禁用状态）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/agents/src/runner.rs`：新增测试覆盖 `hello` 不应被识别为 direct shell command（以及 `pwd` 应该被识别）。

### UI E2E（Playwright，如适用）
- [ ] `crates/gateway/ui/e2e/specs/<name>.spec.js`：覆盖 “普通输入不触发 exec、命令模式触发 exec”。

### 手工验证步骤
- 在 WebUI 中分别输入 `hello`、`pwd`、以及“命令模式”输入，确认行为符合预期。

## 发布与回滚（Rollout & Rollback）
- 发布策略：若引入前缀/模式，需在 UI 显示迁移提示；可短期支持旧行为的兼容开关（若需要）。
- 回滚策略：恢复旧启发式与强制注入逻辑（会重新引入误判风险）。

## 实施拆分（Implementation Outline）
- Step 1: 定义显式命令入口（前缀或 UI toggle）并在 runner 侧读取该信号。
- Step 2: 收紧/替换现有启发式；增加 reason-code 日志。
- Step 3: 补齐 unit/UI e2e 测试；更新 docs/提示文案。
- 受影响文件：
  - `crates/agents/src/runner.rs`
  - `crates/gateway/src/assets/js/*`（若需要 UI 开关/提示）
  - `docs/src/*`（若需要文档化）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-sandbox-prebuild-race-home-sandbox-workdir.md`

## 未决问题（Open Questions）
- Q1: 命令模式前缀选择：`>` vs `/exec` vs 单独按钮，哪个最符合现有交互？
- Q2: 是否需要保留 legacy 行为的 config 开关（默认关闭）？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

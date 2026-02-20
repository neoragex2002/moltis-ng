# Issue: Telegram `/context` Debug 信息补齐与口径收敛（Token Debug / LLM overrides / mounts / compact）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-19）
- Priority: P1（影响排障与 token/compact 预期）
- Components: telegram / gateway(channel_events, chat.context) / debug contracts
- Affected providers/models: all（展示层；数据源来自 `chat.context`）

**已实现**
- gateway `dispatch_command("context")` 对 Telegram 返回版本化 JSON contract：`crates/gateway/src/channel_events.rs`（`context.v1`，`payload` 等价于 `chat.context` 原始 JSON）。
- Telegram `/context` 优先解析 `context.v1` 并渲染结构化 HTML 卡片（含 tokenDebug/LLM overrides/sandbox mounts/compaction/skills 摘要）；失败时回退旧 markdown 解析：`crates/telegram/src/handlers.rs`。
- Telegram 侧补齐 token 字段兼容：当旧 markdown 没有 `Tokens:` 时，自动使用 `Last:`/`Next (est):` 组合显示（避免 token 空白回归）：`crates/telegram/src/handlers.rs`。
- 裁剪/折叠策略：列表字段（mounts/skills）限制行数并提示 “(+N more)”；整体超长时降级为最小摘要（避免截断破坏 HTML 导致发送失败）：`crates/telegram/src/handlers.rs`。

**已覆盖测试**
- Gateway：`context.v1` contract 包装单测：`crates/gateway/src/channel_events.rs`。
- Telegram：`context.v1` payload 解析单测：`crates/telegram/src/handlers.rs`。
- Telegram：HTML 渲染包含关键字段 + 列表裁剪/降级路径单测：`crates/telegram/src/handlers.rs`。

**已知差异/后续优化（非阻塞）**
- Telegram 侧不存在“未发送输入框草稿（draftText）”，因此 `Next request` 的 `pending_user_toks_est` 默认只能是 `0`（除非额外设计 `/context <draft>` 之类的扩展）。
- Telegram 单条消息存在长度与格式约束；需要明确裁剪/折叠策略（见 Spec）。

---

## 背景（Background）
- 场景：用户在 Telegram 会话里使用命令 `/context`，希望看到与 Web Chat Debug/Context 面板一致的关键运行态信息（尤其是 token/compact 风险评估与模型参数 overrides）。
- 约束：
  - Web UI 的 `/context` 是结构化 JSON 渲染（`chat.context`）。
  - Telegram 的 `/context` 目前走 “gateway 生成 markdown 文本 → telegram 解析 markdown” 的链路，天然易漂移。
- Out of scope：
  - 不在本单要求 Telegram 提供 Web UI 同级别的完整 Debug 面板交互（只要求 `/context` 输出信息口径收敛且关键字段补齐）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **Telegram `/context`**：Telegram 侧 slash command 的返回卡片（信息密度高、用于排障）。
- **Web `/context`**：Web Chat 中的 context/debug 卡片（RPC `chat.context` 的结构化渲染）。
- **Last request (authoritative)**：来自 provider 返回的权威 usage（`usage.input_tokens/output_tokens/cached_tokens`）。
- **Next request (estimate, heuristic)**：对“若立刻发送下一条用户消息”的输入 tokens 风险评估（必须标注 `method=heuristic`；用于预判 auto-compact，不可当真值）。
- **口径收敛（parity）**：同一 session 上，Telegram `/context` 与 Web `/context` 对同名字段表达含义一致（允许展示形式不同，但不得出现“字段名相同却口径不同”）。
- **contract（版本化契约）**：gateway 返回给 Telegram 的 `/context` 数据传输格式（建议 `context.v1`），用于替代脆弱的 markdown label 协议。
- **裁剪/折叠（truncate/fold）**：为适配 Telegram 的消息长度上限与 HTML 渲染限制，对超长字段（mounts/tools/mcpServers 等）做摘要展示，并提供“更多信息去 Web `/context` 查看”的提示。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] Telegram `/context` 输出必须包含（至少）：
  - session key / provider / model
  - LLM overrides（尤其：`prompt_cache_key`、`generation.max_output_tokens`、`reasoning_effort`、`text_verbosity`、`temperature` 等 as-sent/effective 信息）
  - sandbox mounts 关键摘要（enabled/backend/image + external mounts 状态 + mounts 明细/allowlist 摘要）
  - compaction 状态（是否已 compact、summary 元信息、keep window）
  - tokenDebug（Last request authoritative + Next request estimate，字段含义与 Web 一致）
- [x] Telegram `/context` 的 token/compact 口径必须与既有 Web token debug 口径一致（参考既有规范单）。
- [x] 输出必须能稳定演进（避免“markdown 字段名变了 Telegram 就丢字段”）。
- [x] 必须明确并实现 Telegram 输出的裁剪/折叠策略（避免超长导致发送失败或用户无法阅读）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：明确区分 authoritative vs estimate（并标注 method/source）。
  - 不得：把 estimate 冒充 authoritative。
  - 不得：Telegram `/context` 丢失关键字段（至少 token/compact/overrides）。
- 兼容性：升级过程中允许“旧格式 fallback”，避免 Telegram `/context` 直接报错或空白。
- 可观测性：当解析失败时（格式不符/字段缺失），应输出可定位的简短错误（不泄露敏感内容）。
- 安全与隐私：`prompt_cache_key` 是否脱敏需明确策略（默认可先保持与 Web 一致；如需脱敏另开优化单）。
- Telegram 兼容性：
  - Telegram 消息 parse_mode=HTML 有标签/嵌套限制；必须保证输出为有效 HTML。
  - Telegram 单条消息存在长度上限；必须保证超长时仍可发送（可通过裁剪/分段/降级纯文本实现）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) Telegram `/context` 没有展示完整 token 相关信息，甚至可能完全空缺。
2) Telegram `/context` 的信息口径/字段与 Web `/context` 不一致，导致用户无法用同一套规则理解 token/compact 风险。

### 影响（Impact）
- 用户体验：Telegram 用户侧无法判断“是否接近 auto-compact/是否会触发 compact”，也难以自证模型参数是否生效。
- 可靠性：在 token/compact 边界附近，缺少可观测信息会导致误判与重复操作。
- 排障成本：需要回看服务器日志或切到 Web UI 才能确认关键字段。

### 复现步骤（Reproduction）
1. 在 Telegram 任意会话发送 `/context`。
2. 观察返回卡片中的 token/compact/model overrides 信息。
3. 对比 Web Chat 的 `/context`（Debug/Context 卡片）字段。
4. 期望 vs 实际：
   - 期望：两端口径一致且 Telegram 不缺关键字段。
   - 实际：Telegram token 字段缺失/口径漂移。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据（链路与 bug）：
  - `crates/telegram/src/handlers.rs:920`：`send_context_card()` 当前是“解析 markdown context response”并取 `Tokens:` 字段（`let tokens = get("Tokens:");`）。
  - `crates/gateway/src/channel_events.rs:857`：gateway 的 `dispatch_command("context")` 组装文本使用的是 `**Last:** ...` 与 `**Next (est):** ...`，并未输出 `**Tokens:** ...`，导致 Telegram 侧解析不到 token 字段。
- 结构化数据源已经具备：
  - `crates/gateway/src/chat.rs:3199`：`chat.context` 已返回 `tokenDebug`、`llm.overrides`、`sandbox.mounts`、`compaction` 等结构化信息。
  - `crates/gateway/src/chat.rs:4688`：`build_token_debug_info()` 已按“Last authoritative + Next estimate(heuristic)”产出字段（并包含 `details.method=heuristic`）。
- 当前测试覆盖：
  - 已有：Web token debug 相关单测（见 `issues/issue-chat-debug-panel-llm-session-sandbox-compaction.md` 的“已覆盖测试”清单）。
  - 缺口：无 Telegram `/context` 输出的自动化测试；且 Telegram `/context` 仍依赖 markdown 字段名契约，易回归。

## 根因分析（Root Cause）
- A. 契约不一致：Telegram 解析器期待 `Tokens:`，但 gateway 文本输出改为 `Last/Next` 后未同步 Telegram。
- B. 双重渲染/重复逻辑：同一份 `chat.context` JSON 被 Web 直接渲染，但 Telegram 先 stringify 成 markdown 再 parse，导致字段漂移与信息丢失。
- C. 演进缺少“版本化 contract”：无法在不破坏 Telegram 的情况下渐进扩展字段。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - Telegram `/context` 能展示与 Web `/context` 同一套关键 debug 字段（至少 tokenDebug/overrides/mounts/compaction/session/model）。
  - `Last request` 必须是 authoritative usage；`Next request` 必须标注 estimate + `method=heuristic`。
  - Telegram `/context` 不再依赖脆弱的 markdown label 字段名契约来“解析” token/关键字段。
- 不得：
  - 不得出现 Telegram `/context` 显示为空/缺关键字段的情况（除非明确提示“数据不可用”）。
  - 不得把敏感信息意外扩大暴露面（例如将完整 headers/token 打印进卡片）。
- 应当：
  - 使用结构化 contract（版本化）承载字段，避免演进导致解析失败。
  - 解析失败时 fallback 到可读的纯文本，并提示“context format mismatch”以便定位。
  - Telegram 输出必须“可读且可发送”：对列表类字段提供摘要，超长时裁剪并提示去 Web 查看完整详情。
  - 对潜在敏感字段（尤其 host 路径与 `prompt_cache_key`）明确展示策略（见 Security/Privacy）。

## 方案（Proposed Solution）
### 方案对比（Options）
#### 方案 1（补丁式：继续 markdown contract）
- 核心思路：gateway `dispatch_command("context")` 输出固定 label（补回 `**Tokens:**` 或改 Telegram 解析为 `Last/Next`）。
- 优点：改动小。
- 风险/缺点：仍然是“字符串协议”，每次字段调整都要双端同步；很难做到与 Web 永久一致。

#### 方案 2（推荐：结构化 contract，Telegram 直接渲染 JSON）
- 核心思路：gateway `dispatch_command("context")` 返回一个版本化 JSON（字符串形式），Telegram `/context` 检测并 `serde_json` 解析，然后直接渲染字段（不再做 markdown parse）。
- 优点：单一数据源（`chat.context` JSON）；可版本化、可演进；与 Web 口径天然一致。
- 风险/缺点：需要一次性调整 Telegram `/context` 渲染实现；要做 fallback 以兼容旧格式。

### 最终方案（Chosen Approach）
选择 **方案 2**。

#### 行为规范（Normative Rules）
- 规则 1（source/method 明确）：
  - Telegram `/context` 的 `Last request` 字段必须来自 `tokenDebug.lastRequest`（authoritative）。
  - Telegram `/context` 的 `Next request` 字段必须来自 `tokenDebug.nextRequest`（estimate），并展示 `method=heuristic`。
- 规则 2（contract 版本化）：
  - `dispatch_command("context")` 返回值以 `{"format":"context.v1", "payload": <chat.context json>}` 形式返回（字符串）。
  - Telegram 侧只在识别到 `format=context.v1` 时走 JSON 渲染；否则 fallback 到旧 markdown 解析（避免回滚/混版本导致不可用）。
- 规则 3（payload 等价性，避免二次加工漂移）：
  - `payload` 必须等价于 `chat.context` 的原始 JSON 返回（不在 `dispatch_command("context")` 二次裁剪/重命名字段）。
  - Telegram 侧负责“展示裁剪/折叠”，从而让 contract 稳定、可演进。
- 规则 4（draftText 口径）：
  - Telegram `/context` 的 `Next request` 估算里 `pendingUserToksEst` 默认展示为 `0`（并注明 “Telegram has no draftText”），避免与 Web 的输入框草稿估算混淆。

#### 接口与数据结构（Contracts）
- Gateway（channel command）：
  - `dispatch_command("context") -> String`：返回 JSON 字符串（见上）。
- Telegram 渲染：
  - 以结构化字段渲染卡片，字段顺序遵循：“常量/不变在前，变量/风险在后；重要在前”。
  - Token 部分对齐 Web 的收敛展示：`Last request (authoritative)` + `Next request (compact risk)`。

#### 失败模式与降级（Failure modes & Degrade）
- JSON 解析失败 / format 不匹配：
  - 记录 warn 日志（包含 `format`/长度/会话 key 等非敏信息）。
  - Telegram 回退到旧的 markdown parse 或直接发送原始文本（避免用户完全无回执）。

#### 安全与隐私（Security/Privacy）
- 禁止在 Telegram `/context` 输出中展示：
  - Authorization / API key / raw headers
  - sandbox 可能包含的敏感宿主路径（如需要可用“只显示 basename/短路径”策略，另开优化单）
- `prompt_cache_key` 脱敏策略：先与 Web 保持一致；若用户希望 Telegram 更严格脱敏，则新增可配置项（另单）。
- **字段展示策略（必须明确）**
  - `prompt_cache_key`：建议默认与 Web 一致（明文），但可在 Telegram 侧提供“默认截断 + 可复制”的实现（或直接截断显示前缀），避免在聊天窗口刷出长 key。
  - `sandbox.mounts[].hostDir`：建议默认只显示短路径（例如保留末尾 1–2 段）或对 home 目录做脱敏（如 `~`），避免在群聊中泄露本机目录结构。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] Telegram `/context` 至少展示：session/provider/model、LLM overrides、sandbox mounts 摘要、compaction 状态、tokenDebug（Last/Next）。
- [x] Telegram `/context` 的 token 口径与 Web `/context` 一致（authoritative vs estimate，且标注 method）。
- [x] gateway 与 Telegram 的字段演进不会因 label 变化导致“token 空白”（版本化 contract + fallback 生效）。
- [x] 解析失败可降级，不会让 Telegram 用户“无任何信息返回”。
- [x] 超长字段（mounts/tools/mcpServers）不会导致 Telegram 发送失败；输出包含摘要与“去 Web 查看完整信息”的提示。
- [x] Telegram 输出为有效 HTML（无破坏性标签/未转义字符导致的发送失败）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] Telegram：新增“context JSON 渲染”单测（建议抽出纯函数：`render_context_card_v1(payload) -> String` 便于测试）。
- [x] Telegram：新增“旧 markdown fallback 仍可用”的回归测试（至少覆盖 token 字段不会空）。
- [x] Telegram：新增“裁剪/折叠策略”单测（超长 mounts/tools 时仍能生成可发送长度的 HTML，且包含提示文案）。

### Integration
- [x] Gateway：`dispatch_command("context")` 返回 `format=context.v1` 的 contract 测试（可在 `crates/gateway` 的测试中覆盖）。

### UI E2E（Playwright，如适用）
- N/A（本单为 Telegram 命令；Web E2E 不覆盖 Telegram）。

### 自动化缺口（如有，必须写手工验收）
- 若 Telegram 环境难以在 CI 跑：补充手工验收步骤：
  1) Telegram 发送 `/context`，确认 Token/Compact/Overrides/Mounts 都出现。
  2) 切到 Web `/context` 对照同 session 的字段含义与关键数值（Last usage/Next threshold/progress）一致。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用 JSON 渲染；保留 fallback（混版本或回滚期间仍可工作）。
- 回滚策略：回滚 Telegram 侧 JSON 解析时，gateway 可暂时继续输出旧格式（或同时兼容输出）。
- 上线观测：增加日志关键词（例如 `telegram_context_format=context.v1` / `context_json_parse_failed`）。

## 实施拆分（Implementation Outline）
- Step 1: gateway `dispatch_command("context")` 改为返回版本化 JSON（封装 `chat.context` 原始 payload）。
- Step 2: Telegram `/context` 优先解析 JSON 并渲染；失败则 fallback 旧 markdown parse。
- Step 3: 补齐 Telegram `/context` 卡片字段（overrides/mounts/compaction/tokenDebug）。
- Step 4: 增补单测（Telegram 渲染、contract）。
- Step 5: 增补裁剪/折叠策略与脱敏策略（确保群聊可用且不泄露敏感路径）。
- 受影响文件：
  - `crates/gateway/src/channel_events.rs`
  - `crates/telegram/src/handlers.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-chat-debug-panel-llm-session-sandbox-compaction.md`（Web `/context` 的口径与 token debug 收敛规范）
  - `issues/done/issue-telegram-channel-no-error-reply-on-llm-failure.md`（Telegram 渠道体验/一致性相关）

## 未决问题（Open Questions）
- Q1: Telegram `/context` 是否需要展示 `prompt_cache_key` 的明文？是否默认脱敏？
- Q2: 是否支持 `/context <draft>` 以评估“下一条要发的文本”对 `Next request` 的影响（相当于 `draftText`）？
- Q3: Telegram 输出对 mounts/tools/mcpServers 的默认展示上限是多少（条数/字符数）？（建议先给 conservative 默认值并可配置）

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

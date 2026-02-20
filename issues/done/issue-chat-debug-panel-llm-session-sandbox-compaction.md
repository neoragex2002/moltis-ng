# Issue: 在 Web Chat Debug/Context 中打印关键运行态信息（模型参数 / session key / mounts / compact）

## 实施现状（Status）
- Status: DONE（2026-02-18）
- Priority: P1
- Components: gateway / agents / web-ui
- Affected providers/models: all（展示层 + request overrides）

**已实现（2026-02-18）**：Web Chat 的 `/context` 卡片与 Debug panel 现在会展示 LLM overrides、compaction 状态、以及 sandbox 外部 mounts 明细。

- Provider debug hook：`crates/agents/src/model.rs`（新增 `LlmProvider::debug_request_overrides()`）
- OpenAI Responses overrides：`crates/agents/src/providers/openai_responses.rs`
  - `prompt_cache_key`（最终发送值）
  - `prompt_cache`（`enabled/source/hashed`）
  - `generation`（`max_output_tokens` 的 configured/effective/limit/clamped + `reasoning_effort/text_verbosity/temperature`）
- `chat.context` payload 扩展：`crates/gateway/src/chat.rs`
  - `llm`：`{ provider, model, overrides }`
  - `compaction`：`{ isCompacted, summaryCreatedAt, summaryLen, keptMessageCount, keepLastUserRounds }`
  - `sandbox.mountAllowlist / sandbox.mounts / sandbox.externalMountsStatus`
- Web UI 展示：`crates/gateway/src/assets/js/page-chat.js`
  - 新增 `LLM` section（含 prompt cache + generation）
  - 新增 `Compaction` section
  - `Sandbox` section 扩展 mounts 明细

**已实现（2026-02-18）**：Token Debug 呈现已收敛为 `Last request (authoritative)` + `Next request (compact risk)`，用于直观评估 auto-compact 风险（不再展示 cumulative 的 `input/output/total`）。

- Gateway `chat.context`：`crates/gateway/src/chat.rs`
  - 新增 `tokenDebug.lastRequest`（`inputTokens/outputTokens/cachedTokens`）
  - 新增 `tokenDebug.nextRequest`（`contextWindow/plannedMaxOutputToks/maxInputToks/autoCompactToksThred/promptInputToksEst/compactProgress`）
  - `promptInputToksEst` 为启发式估算（`method=heuristic`），用于回答：**“如果现在立刻发送下一条用户消息，这次请求预计的输入 tokens 是多少”**
    - 必须包含：system prompt + history + 重建后的工具链（`assistant.tool_calls` + `role=tool` output）+ 当前 UI 输入框尚未发送的 `draftText`
    - 计算口径：`promptInputToksEst = historyInputToksEst + pendingUserToksEst + reserveSafetyToks`
    - 其中：`historyInputToksEst` / `pendingUserToksEst` / `reserveSafetyToks` 会在 `tokenDebug.nextRequest.details` 中拆分展示（便于核对“是否包含 draftText / tool 链”）
- Web UI：`crates/gateway/src/assets/js/page-chat.js`
  - Context/Debug panel 的 token 区域改为两块（Last/Next）
  - `chat.context` 请求附带 `draftText`（来自输入框，尚未发送）
- Token bar：`crates/gateway/src/assets/js/sessions.js` + `crates/gateway/src/assets/js/chat-ui.js`
  - 从 `tokenDebug.nextRequest` 读取并展示 `Compact progress`

**已覆盖测试**
- OpenAI Responses overrides 单测：`crates/agents/src/providers/openai_responses.rs`
- Gateway compaction/mount status 单测：`crates/gateway/src/chat.rs`
- Gateway tokenDebug 单测：`crates/gateway/src/chat.rs`
- sessions `cachedTokens` 序列化/反序列化单测：`crates/sessions/src/message.rs`
- agents runner usage 累计 cached_tokens 单测：`crates/agents/src/runner.rs`

**已知差异/后续优化（非阻塞）**
- `externalMountsStatus` 目前表达为 `none/router_unavailable/unsupported_backend/deny_by_default/configured`；尚未做“对实际可用性（canonicalize/allowlist 校验）”的精确判定。
- `prompt_cache_key` 目前在 UI 里完整明文展示；如需更强隐私保护，可改为默认脱敏（前缀 + copy 按钮）。
- `promptInputToksEst` 目前按启发式估算（bytes/3 + safety reserve），未引入 tokenizer，因此数值为保守近似值而非 provider 侧精确 token 计数。

## 背景
目前 Web Chat 的 `/context` 卡片与右上角 Debug panel（RPC：`chat.context`）能展示部分会话上下文（Session Key、模型、Provider、工具、Sandbox、Token Usage 等），但对排障最关键的一些“运行态/请求态”信息仍不可见，导致：

- 很难确认某些 **模型参数**（例如 OpenAI Responses 的 `reasoning.effort`、`text.verbosity`、`temperature`、`max_output_tokens`）是否在实际请求中生效。
- 很难在 UI 侧核对 **prompt cache 分桶**是否真的按不同 `session_key` 写入 `prompt_cache_key`（只能看服务器日志或抓包）。
- Sandbox 的 **mount** 只展示 `workspace_mount`，但不展示外部 mounts（`mounts[]` / `mount_allowlist[]`）及其是否生效（例如 backend 不支持导致失效）。
- **compact** 相关只展示 token/budget 水位线，不展示“当前会话是否已 compact / summary 长度 / keep window 实际保留量”等状态；只有执行 `/compact` 后会出现一次性的 compact 卡片，缺少常驻 debug 信息。

## 目标（Desired Behavior）
在 Web Chat 的 Debug/Context 中补齐并稳定展示以下四类信息（以当前会话为维度）：

1) **模型参数（LLM request overrides）**
   - 以“最终会写入 LLM API 请求”的字段为准（而不是仅展示配置文件原始值）。
   - 至少覆盖：`max_output_tokens`、`reasoning_effort`、`text_verbosity`、`temperature`、`prompt_cache_key`。

2) **Session Key**
   - UI 里已展示，继续保留，并与 `prompt_cache_key` 的来源关系清晰可见（例如 `prompt_cache_key = hash(session_key)` 或 `raw(session_key)`，以及 fallback 的说明）。

3) **Mount 情况（Sandbox）**
   - 展示 `workspace_mount`（已有）+ `mount_allowlist[]` + `mounts[]`（每条 host->guest + ro/rw）。
   - 展示外部 mounts 是否“已生效/被拒绝/不支持”（例如 apple-container 后端不支持外部 mounts）。

4) **Compact 情况**
   - 展示当前会话是否处于 compacted 状态（是否存在 summary 头消息）。
   - 展示 summary 元信息：summary 长度、summary created_at（若有）、keep window 实际消息条数、`KEEP_LAST_USER_ROUNDS` 常量值。

5) **Token Debug（重点：auto-compact 风险直观可见）**
   - UI 只展示两块：`Last request`（权威 usage）与 `Next request`（下一轮 compact 风险）。
   - 不再展示任何“会话累计 input/output/total”。
   - `Next request` 的核心目标是直观看到 `prompt_input_toks_est` 相对 `auto_compact_toks_thred` 的进度（compact progress）。

## 现状核查（As-is）
### UI 已打印
- Session：`Key / Messages / Model / Provider / Label / Tool Support`
- LLM：`prompt_cache_key` + prompt cache（enabled/source/hashed）+ generation（max_output_tokens clamped 信息、reasoning_effort、text_verbosity、temperature）
- Compaction：`isCompacted / summaryLen / keptMessageCount / keepLastUserRounds / summaryCreatedAt`
- Sandbox：`Enabled / Backend / Mode / Scope / Workspace Mount / Image / Container` + `mountAllowlist / mounts / externalMountsStatus`
- Token Debug：已收敛为 `Last request (authoritative)` + `Next request (compact risk)`（不再展示 cumulative 的 `Input/Output/Total`）。

### UI 未打印（缺口）
- 可选增强：`prompt_cache_key` 默认脱敏（只显示前缀 + copy 按钮）
- 可选增强：external mounts 的“实际可用性”校验（当前仅显示配置/后端约束层面的 status）
  - 可选增强：Next request 的 `promptInputToksEst` 引入 tokenizer 做更精细计数（当前明确按启发式估算）。
    5. `prompt_input_toks_est`（**启发式估算，暂不使用 tokenizer**；但必须包含 tool_calls + tool 输出重建 + 当前待发送 user 输入）
    6. `compact_progress`（= `prompt_input_toks_est / auto_compact_toks_thred`，建议以百分比展示）
  - 明确“估算 vs 权威 usage”的边界：`Last request` 精确；`Next request` 为估算（用于提前判断 auto-compact 风险）。
  - `prompt_input_toks_est` 的口径必须明确且可解释（避免歧义/误导）：
    - **含义**：如果你现在立刻发送“下一条用户消息”，这次请求预计的 input tokens。
    - **必须包含**：`system prompt + history + 重建后的工具链（assistant.tool_calls + tool/function_call_output）+ 当前要发送的 user 输入`。
    - **估算方法**：先使用启发式方法，并在 UI 标注 `method=heuristic`（暂不引入 tokenizer）。
    - **拆分口径（建议作为折叠详情展示，主视图仍只保留 6 个字段）**：
      - `history_input_toks_est`：`system prompt + history (+ 重建 tool)` 的估算 input tokens
      - `pending_user_toks_est`：当前输入框/本次待发送 user 内容的估算 input tokens
      - `reserve_safety_toks`：保守安全预留（当前实现常量为 `SAFETY_MARGIN_TOKENS=1024`）
      - 公式：`prompt_input_toks_est = history_input_toks_est + pending_user_toks_est + reserve_safety_toks`

## Token Debug 呈现收敛（Spec，待实现）
本节将 UI 的 token 相关展示收敛为两块：`Last request (authoritative)` 与 `Next request (compact risk)`，用于排障与 auto-compact 风险评估。

### A. 移除累计展示（明确用户需求）
- 删除/隐藏任何“会话累计（cumulative）”的 `input/output/total` 展示（不再计算/不再渲染）。
- UI 仅保留以下两块；所有字段命名必须显式表明“权威值 vs 估算值”，避免含混。

### B. Last request (authoritative)
**目标**：只显示“最近一次 agent run 完成后”的权威 token 使用情况（来自 provider 返回的 `usage` 统计），不做启发式估算。

字段（精确）：
- `input_tokens`：来自 provider `usage.input_tokens`
- `output_tokens`：来自 provider `usage.output_tokens`
- `cached_tokens`：来自 provider `usage.cache_read_tokens`（OpenAI Responses: `usage.input_tokens_details.cached_tokens`）

实现注意：
- 当前 runner 会在多次迭代（tool loop）中累计 `input/output`，但 **未累计 cached_tokens**（`Usage.cache_read_tokens` 在返回处仍为默认值）。需补齐“跨迭代累计 cached_tokens”以保证 Last request 的 cached_tokens 正确。
- 当前 gateway 持久化 assistant 消息只包含 `inputTokens/outputTokens`；需考虑把 `cached_tokens` 也持久化/回传给 UI（建议字段名 `cachedTokens`，可选字段兼容旧记录），或在不落库的情况下至少在 `chat.context` 返回“最近一次 run 的 usage”。

### C. Next request (compact risk)
**目标**：用一组收敛字段直观表达“下一条消息如果立刻发送，离 auto-compact 阈值还有多远”。

主视图仅展示 6 个字段（常量/上限在前，变量/估算在后）：
1) `context_window`
2) `planned_max_output_toks`
3) `max_input_toks`
4) `auto_compact_toks_thred`
5) `prompt_input_toks_est`
6) `compact_progress`

字段口径与计算：
- `context_window`：模型上下文总窗口（input+output 上限），来自 `provider.context_window()`
- `planned_max_output_toks`：下一次请求预留的输出上限（clamp 后实际值）
  - 优先从 provider 的 request overrides/debug 中读取“最终会发出去的值”（OpenAI Responses：`generation.max_output_tokens.effective`）
  - 无 overrides 时 fallback：`provider.output_limit()` 或 `min(16384, context_window/5)`（与现有预算逻辑一致）
- `max_input_toks`：输入硬上限（用于 auto-compact 预算），来自 `provider.input_limit()`；若为 `None` 则使用派生值 `floor(context_window * 0.8)`，并在 UI 标注 `derived=true`
- `auto_compact_toks_thred`：`floor(max_input_toks * 0.85)`
- `prompt_input_toks_est`：**启发式估算**（必须标注 `method=heuristic`；暂不引入 tokenizer）
  - `prompt_input_toks_est = history_input_toks_est + pending_user_toks_est + reserve_safety_toks`
  - `reserve_safety_toks`：保守安全预留（当前常量 `SAFETY_MARGIN_TOKENS=1024`）
  - 必须包含：`system prompt + history + 重建后的工具链（assistant.tool_calls + tool/function_call_output）+ 当前 draftText`
- `compact_progress`：`prompt_input_toks_est / auto_compact_toks_thred`（建议以百分比展示，范围可 clamp 到 `[0, 1+]`）

折叠详情（非主视图）建议至少包含：
- `history_input_toks_est`、`pending_user_toks_est`、`reserve_safety_toks`、`method`

实现注意（关键前置条件）：
- `pending_user_toks_est` 必须来自 UI 的“未发送草稿文本”：
  - `chat.context` RPC 需要支持可选参数 `draftText`（来自 web 页面输入框、尚未发送）
  - 不提供 draftText 时，`pending_user_toks_est=0` 且 UI 明确标注“未提供 draftText”
- “工具链重建规则”必须明确并一致（否则估算不稳定/不可复现）：
  - 输入：session history 中的 `role:"tool_result"` 条目（含 `tool_call_id / tool_name / arguments / result / error`）
  - 输出：用于估算的“LLM 视角消息序列”必须包含：
    - `assistant.tool_calls`（由 `tool_name + arguments + tool_call_id` 重建）
    - `tool/function_call_output`（由 `error` 或 `result` 序列化为字符串 output 重建）
  - 顺序：按历史出现顺序重建；建议每条 `tool_result` 生成一对 `assistant(tool_calls)` + `tool(output)`，以保证 call_id 配对完整
  - 大小控制：沿用现有对 stdout/stderr 的截断与截图脱敏规则；如 `result` 可能过大，需追加上限并在 output 中标注 `[truncated]`，避免估算与真实请求失控

## 方案（Proposed Solution）
核心原则：**Debug UI 展示的字段应尽可能接近“真实会发出去的请求参数”。**

### A. 在 `chat.context` payload 增加 `llm`（或 `llmRequest`）字段
扩展 `chat.context` 的返回 JSON，新增一块 `llm`，用于展示 LLM 请求参数/分桶信息。

建议结构（示例）：

```jsonc
{
  "session": { "key": "telegram:bot:123", "model": "openai-responses::gpt-5.2", "provider": "openai-responses", ... },
  "llm": {
    "promptCache": {
      "enabled": true,
      "bucketKeyMode": "prompt_cache_key",
      "promptCacheKey": "b3... (final value sent to /v1/responses)",
      "source": "session_key",          // session_key | fallback | unknown
      "hashed": true,                   // whether it was hashed
      "note": "prompt_cache enabled but session_key missing -> fallback"
    },
    "generation": {
      "maxOutputTokens": { "configured": 2048, "effective": 1024, "clamped": true, "limit": 1024 },
      "reasoningEffort": "medium",
      "textVerbosity": "high",
      "temperature": 0.2
    }
  }
}
```

#### A1. 不让 gateway “猜” provider 行为：给 `LlmProvider` 增加 debug hook（推荐）
在 `moltis_agents::model::LlmProvider` trait 增加一个默认实现方法（不破坏现有 provider）：

- `fn debug_request_overrides(&self, ctx: Option<&LlmRequestContext>) -> serde_json::Value { json!({}) }`

然后在 `OpenAiResponsesProvider` 中实现该方法，返回其“构造请求 body”时会写入的关键字段：

- `prompt_cache_key`（最终值）
- `max_output_tokens`（clamp 后）
- `reasoning.effort`
- `text.verbosity`
- `temperature`

这样 gateway 只需要在 `chat.context` 里调用 `provider.debug_request_overrides(Some(&llm_context))` 并合并进 payload，避免在 gateway 侧复制 `prompt_cache_key` 算法 / clamp 算法导致与实际请求不一致。

> 备选（不推荐）：gateway 直接读取配置并“自己算”这些字段。缺点是容易与 provider 演进脱节，且多 provider 时难统一。

### B. `chat.context` 增加 `compaction` 字段（常驻 compact 状态）
从 session history 推断：

- `isCompacted`：若历史第一条为 assistant，且 `content` 以 `"[Conversation Summary]"` 开头（与当前 compaction 实现保持一致）。
- `summaryCreatedAt`：取第一条消息的 `created_at`（若有）
- `summaryLen`：summary 内容长度（建议去掉固定前缀后再统计）
- `keptMessageCount`：`history.len() - 1`
- `keepLastUserRounds`：`KEEP_LAST_USER_ROUNDS`

示例：

```jsonc
{
  "compaction": {
    "isCompacted": true,
    "summaryCreatedAt": 1739880000000,
    "summaryLen": 1820,
    "keptMessageCount": 17,
    "keepLastUserRounds": 4
  }
}
```

### C. `chat.context` sandbox 字段补齐外部 mounts 明细 + 生效状态
在现有 `sandbox` 结构上新增：

- `mountAllowlist`: `string[]`
- `mounts`: `{ hostDir, guestDir, mode }[]`
- `externalMountsStatus`: `"ok" | "unsupported_backend" | "deny_by_default" | "error:<msg>"`

说明：
- 外部 mounts 在不同 backend 可能不支持（例如 apple-container），且存在 deny-by-default 行为（`mounts[]` 配了但 `mount_allowlist[]` 为空会拒绝）。
- Debug UI 需要告诉用户“配了但没生效”的原因。

### D. 前端 Debug/Context UI 的展示建议
在 `page-chat.js` 的 context card / debug panel 中新增 section（保持现有风格）：

1. `LLM`：
   - Provider / Model（已有，可重复或引用 session 信息）
   - `prompt_cache_key`（建议加“复制按钮”或默认折叠，避免误贴）
   - Generation overrides 列表（`max_output_tokens` 显示 `configured → effective` + clamped/limit）

2. `Compaction`：
   - `isCompacted`
   - `summaryLen`
   - `keptMessageCount`
   - `keepLastUserRounds`
   - `summaryCreatedAt`（有则展示）

3. `Sandbox` 扩展：
   - `mountAllowlist`（列表）
   - `mounts`（列表：`hostDir -> guestDir (ro/rw)`）
   - `externalMountsStatus`（红/黄提示）

## 安全与隐私（Security/Privacy）
- `prompt_cache_key` 可能是 session_key 的 hash 或 raw 值（取决于配置）。即使 hash，也可能被用户当作“可识别标识”传播。
  - 建议：默认折叠 + Copy 按钮；或者展示前 8 位 + 完整值需点击展开。
- mounts 可能暴露本机目录结构（host path）。Debug 面板通常面向管理员/自用，但仍建议只在 debug/context 中展示（不进入普通聊天消息流）。

## 验收标准（Acceptance Criteria）
- `/context` 卡片与 Debug panel 能看到：
  - 模型参数 overrides（含最终 `prompt_cache_key`）
  - session key（现有）
  - mounts 明细（allowlist + mounts + 状态）
  - compact 状态（是否 compacted + summary 元信息）
- 不同会话（例如两个 Telegram 会话）切换时，`prompt_cache_key` 与 session key 的关联清晰可验证。
- 不同 sandbox backend/配置下，mount status 能反映真实生效状态。

## 测试计划（Test Plan）
单元测试/服务测试建议：
- `chat.context` 返回 JSON：
  - 包含 `llm` 字段，且来自 provider debug hook（对 OpenAI Responses provider 的 test double/fixture）。
  - compaction 推断：构造 history（含 summary 头）断言 `isCompacted=true`、`keptMessageCount` 正确。
  - mounts 展示：构造 sandbox config（含 mounts + allowlist），断言 payload 正确；对不支持 backend 分支断言 `externalMountsStatus`。
- 前端（轻量）：
  - 组件渲染 smoke test（如有现成前端测试框架则补；否则在手工验证清单中描述）。

## 估算与拆分（Implementation Outline）
1. agents：`LlmProvider` 增加 `debug_request_overrides()`（默认空实现）；OpenAI Responses provider 实现。
2. gateway：`chat.context` 聚合 `llm` + `compaction` + sandbox mounts 明细与状态。
3. web UI：`page-chat.js` 新增/扩展 sections 渲染。
4. tests：覆盖 gateway payload + provider debug hook + compaction/mounts 推断逻辑。

## 未决问题（Open Questions）
1. `prompt_cache_key` 的 UI 展示策略：默认隐藏？部分脱敏？是否需要“一键复制”？
2. 多 provider 场景：哪些 provider 需要实现 `debug_request_overrides()`？（至少 OpenAI Responses；其他 provider 可渐进式补齐）
3. `max_output_tokens` 的“effective”来源：是否需要明确展示“clamp 的模型上限来自哪里”（静态 limits vs models.dev cache）？

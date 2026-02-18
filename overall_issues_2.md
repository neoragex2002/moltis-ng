# Overall Issues (v2) — Follow-up Audit + Proposed Roadmap

本文件记录当前代码库（Moltis）在实际使用过程中暴露出的第二批关键遗留问题，并给出讨论后的修复思路、方案与落地计划。

本轮讨论参与：
- Oracle（架构/正确性/安全审阅）
- Momus（产品风险/UX/优先级审阅）
- Librarian（外部资料与成熟实现 survey：OpenAI prompt caching / compaction patterns）

> 约束与范围（按你的要求）
> - 以 OpenAI Responses API + 自定义 `base_url` + API key 的场景为主
> - `/chat/completions` 不是重点
> - “多 bot 群聊/多 bot 响应”仅做 issue 记录与初步 survey（不承诺本轮实现）

状态标记：

- **[DONE]**：已在当前 working tree 落地（含测试/证据）
- **[TODO]**：已收敛方案，但尚未实现
- **[SURVEY]**：仅记录与调研，不承诺实现

---

## Executive Summary（结论与优先级）

### 已完成（本轮落地）

0) **[DONE] OpenAI Responses 内置 Web Search（Web live）provider 级开关 + 单元测试**
   - 目标：不依赖外部付费 `web_search` KEY，使用 Responses API 内置 `tools:[{"type":"web_search"}]` 实现联网检索。
   - 配置：`[providers.openai-responses.builtin_web_search]`（默认关闭，需显式 `enabled=true`）。
   - 关键行为：启用 built-in 时，自动从 function tools 里过滤掉本地同名 `web_search`（避免模型在“内置 web_search”和“本地 web_search function”之间摇摆；也避免误用外部 KEY）。
   - 证据（实现/配置/测试）：
     - schema：`crates/config/src/schema.rs:1594`
     - validation：`crates/config/src/validate.rs:902`
     - request body 注入：`crates/agents/src/providers/openai_responses.rs:623`
     - 单测：`crates/agents/src/providers/openai_responses.rs:930`
   - 设计与字段解释详见：`openai-responses-builtin-web-search.md`
    - 官方字段名/取值证据（避免拼错，便于后续 UI/日志可解释）：见 Appendix C

1) **[DONE] OpenAI Responses generation options（max_output_tokens / reasoning_effort / text_verbosity / temperature）**
   - 目标：为 `openai-responses` provider 提供最小集的生成参数控制，并保持默认行为可预测。
   - 配置：`[providers.openai-responses.generation]`（仅当该块存在时才会写入对应字段）。
   - 关键行为：当 generation 块存在时，即使未显式配置 `max_output_tokens`，也会按 models.dev（或 fallback）解析出的 `limit.output` 写入请求体，保证预算明确。
   - 证据：
     - schema：`crates/config/src/schema.rs`（`OpenAiResponsesGenerationConfig`）
     - validation：`crates/config/src/validate.rs`（temperature 约束与范围校验）
     - request body 注入：`crates/agents/src/providers/openai_responses.rs`（`apply_generation_options`）
     - 单测：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_applies_generation_options_when_configured`）

2) **[DONE] OpenAI Responses Prompt Cache 显式分桶（prompt_cache_key）**
   - 目标：按 session_key 做 prompt caching 分桶（可选 hash），并把 bucket id 写入官方字段 `prompt_cache_key`。
   - 配置：`[providers.openai-responses.prompt_cache]`（该块存在且 enabled=true 时才会发送 `prompt_cache_key`）。
   - 关键行为：`bucket_hash = "auto"` 按 session_key 的 UTF-8 字节长度阈值（>64）决定是否 hash；短 key 不 hash。
   - 补强（可靠性）：当启用 prompt_cache 但本次请求缺失 session_key 时，仍会发送一个确定性的 fallback `prompt_cache_key`（`moltis:<provider>:<model>:no-session`，同样受 bucket_hash 影响），避免“必须提供 bucket key”的 Responses 网关直接拒绝请求。
   - 证据：
     - schema：`crates/config/src/schema.rs`（`OpenAiResponsesPromptCacheConfig` / `PromptCacheBucketHashConfig`）
     - validation：`crates/config/src/validate.rs`（prompt_cache keys 纳入 unknown-field 检测）
     - request body 注入：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_with_context`）
     - ctx 透传修复：`crates/agents/src/providers/mod.rs`（`RegistryModelProvider::{complete_with_context,stream_with_tools_with_context}`）
     - 单测：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_includes_prompt_cache_key_when_enabled_and_session_key_provided` / `build_responses_body_hashes_prompt_cache_key_when_auto_and_long` / `build_responses_body_uses_fallback_prompt_cache_key_when_context_missing`）
     - 回归单测：`crates/agents/src/providers/mod.rs`（`registry_model_provider_forwards_complete_with_context` / `registry_model_provider_forwards_stream_with_tools_with_context`）

### MUST-FIX（阻断可靠性/安全性）

1) **[DONE] 上下文压缩（compaction）触发口径修正 + summary + keep window（last 4 user rounds）+ 恢复模式**
   - 关键行为：
     - Proactive：改为估算“下一次请求的 prompt input tokens”（system + history + 当前 user + safety margin），用 `HIGH_WATERMARK = floor(input_hard_cap*0.85)` 触发（不再累加 persisted `inputTokens`）。
     - Action：compaction 后 history 固定为 `1 条 summary assistant + 最近 4 轮 user rounds 原文`（keep window byte-for-byte），并保留 keep window 内的 `tool_result` 元数据。
     - Recovery：如果 “keep window + 当前 user” 自身就超过 `input_hard_cap`，进入 `keep_window_overflow`（本轮不调用模型，但继续持久化消息并给出可执行建议）。
   - 证据（实现/事件/返回值）：
     - proactive 估算与触发：`crates/gateway/src/chat.rs:2252` / `crates/gateway/src/chat.rs:2319`
     - keep window overflow（结构化错误）：`crates/gateway/src/chat.rs:2284`
     - compaction helper（summary + keep window）：`crates/gateway/src/chat.rs:4604` / `crates/gateway/src/chat.rs:4563`
     - manual / reactive 统一走 shared helper：`crates/gateway/src/chat.rs:3084` / `crates/gateway/src/chat.rs:4265`
   - 单测：
     - keep window 边界与 byte-for-byte 保留：`crates/gateway/src/chat.rs:7089`
     - 预算/水位线数学与估算启发式：`crates/gateway/src/chat.rs:7118`

2) **[DONE] 沙箱容器外部目录挂载（RO/RW）+ allowlist + canonicalize 防逃逸**
   - 配置：新增 `tools.exec.sandbox.mounts = [...]` + `tools.exec.sandbox.mount_allowlist = [...]`（deny-by-default）。
   - 关键行为：Docker backend 生成 `-v host:guest:ro|rw`，对 host_dir 做 canonicalize 并必须落在 allowlist roots 内；guest_dir 强约束在 `/mnt/host/...`，拒绝危险路径。
   - 证据：
     - schema：`crates/config/src/schema.rs:1279` / `crates/config/src/schema.rs:1296`
     - validation：`crates/config/src/validate.rs:911`
     - Docker args：`crates/tools/src/sandbox.rs:824` / `crates/tools/src/sandbox.rs:976`
   - 单测：
     - 配置校验：`crates/config/src/validate.rs:1689`
     - Docker args / symlink 逃逸拒绝：`crates/tools/src/sandbox.rs:2459`

### SHOULD-FIX（提升可控性/成本/体验）

3) **[DONE] Compaction 预算与可解释性（models.dev limit.input/limit.output → budget → UI/事件）**
   - `chat.context` 返回 `budget` 字段（estimated prompt input / keep window prompt input / watermarks / caps）：`crates/gateway/src/chat.rs:3401`
   - Web UI 显示预算与 token bar：`crates/gateway/src/assets/js/page-chat.js:331` / `crates/gateway/src/assets/js/chat-ui.js:293`

### Survey-only（仅记录/调研）

5) **[SURVEY] 1 群 N bot 响应 / 群内多 bot 自主互聊**：系统层可启动多 Telegram bot account，但“bot-bot 互聊”通常受 Telegram 平台 update 分发限制，需要网关层编排/转发才能可靠实现。
   - 多 account 启动：`crates/gateway/src/server.rs:1728`
   - session key 含 account_id：`crates/telegram/src/handlers.rs:1471`
   - 群聊默认 mention 模式：`crates/channels/src/gating.rs:57`

---

## 建议实施顺序（收敛版）

为了降低“行为变更惊吓”、减少线上事故半径，并让每一步都可验证，建议按以下顺序推进：

1) **Issue 2（compaction trigger 口径修正 + 可解释性）**
   - 已完成（见 Executive Summary + Issue 2 当前实现/单测证据）。
2) **Issue 5（sandbox 外部 mounts）**
   - 已完成（见 Executive Summary + Issue 5 当前实现/单测证据）。
3) **Issue 3（prompt cache 显式分桶）**
   - 已完成（见 Executive Summary）。如需开启：新增 `[providers.openai-responses.prompt_cache]`；分桶按 session_key；`bucket_hash=auto`（按 UTF-8 字节长度阈值 64 决定是否 hash）。
4) **Issue 1（generation options 最小集）**
   - 已完成（见 Executive Summary）。温度采样严格按 GPT‑5.2 约束 gating。

## 最小验证矩阵（每个 P0/P1 至少 1 条自动化）

| Item | 验证点 | 最小自动化 |
|---|---|---|
| [DONE] builtin web_search | request body 注入/过滤/stream 兼容 | 已有单测（见 Executive Summary 证据） |
| [DONE] compaction 触发口径 | proactive + reactive + manual 三路径正确，且可解释日志字段齐全 | 已有单测（预算数学/keep window 保留/启发式估算）：`crates/gateway/src/chat.rs:7089` |
| [DONE] sandbox mounts | allowlist/canonicalize/危险 guest_dir 拒绝；Docker 生效 | 已有单测（config 校验 + docker args 断言）：`crates/config/src/validate.rs:1689` / `crates/tools/src/sandbox.rs:2459` |
| [DONE] prompt cache | session bucket id 写入 `prompt_cache_key`；可选 hash；stream/json 均能解析 cached_tokens | 已有单测（request body 断言 + SSE 解析） |
| [DONE] generation options | 字段名正确、按 model 约束 gating（GPT‑5.2: temperature 仅允许 reasoning.effort="none"） | 已有单测（request body 断言） |

## 1) [DONE] 模型细节参数配置（OpenAI Responses / gpt-5.2）

### 现状与证据

- `openai-responses` provider 已支持 generation options：`max_output_tokens` / `reasoning.effort` / `text.verbosity` / `temperature`（temperature 由 config 校验 gating）。
  - schema：`crates/config/src/schema.rs`（`OpenAiResponsesGenerationConfig`）
  - validation：`crates/config/src/validate.rs`（temperature requires reasoning_effort="none"）
  - request body：`crates/agents/src/providers/openai_responses.rs`（`apply_generation_options`）
  - tests：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_applies_generation_options_when_configured`）

### 仓库内已有“先例”（说明这些字段不是新概念，只是 openai-responses 缺失）

- `openai-codex` provider 构造的 Responses body 已经使用了 `text.verbosity` 与 `include`（说明仓库对 Responses 侧字段的使用是允许且有先例的）。
  - 证据：`crates/agents/src/providers/openai_codex.rs:553`
- `anthropic` provider 虽未做配置化，但已在请求体固定写入 `max_tokens`（说明“输出预算字段”在其它 provider 中已是必要参数）。
  - 证据：`crates/agents/src/providers/anthropic.rs:200`

### 你提出的补充要求

- 在“模型参数”里补充：
  - **最大上下文长度**（max context length）：用于预算与 compaction 的 context window 依据
  - **最大返回长度**（max return length）：对应 Responses API 的 `max_output_tokens`

### 讨论后的方案（收敛配置面 + 默认值清晰）

我们建议新增一个“最小可用”的 generation 配置块，并按你的要求进一步收敛：

- **真正可配置的 generation 参数只保留 4 个**（且都可选、互不重复）。
- “最大上下文/最大输出”属于 **能力上限**：按你要求统一从 models.dev 拉取，不做一堆可配字段。

关键收敛原则（避免配置面发散、避免 silent ignore）：

- **只给 `openai-responses` provider 使用**（不污染所有 provider）。
- 字段总体原则：**配置了才发送**（避免把一堆默认值强塞给后端）。
- 例外（按你已确认的规则）：`max_output_tokens` 未配置时也会按 models.dev 上限自动确定，并写入请求体（保证输出上限明确）。
- 对明显错误/危险配置 fail-fast（`moltis config check` 直接报出原因 + 路径）。
- 把“用于 compaction 的预算字段”与“纯生成采样字段”分组，避免误用。

#### 配置结构（建议，收敛版）

`[providers.openai-responses.generation]`（provider 级默认值）

字段建议（按优先级，先少后多；已去掉重复无用项）：

1) `max_output_tokens`（可选，但建议提供默认值）
- **用途**：限制返回长度，并作为 compaction/裁剪的“输出预留空间”。
- 收敛规则（按你的最新要求）：**不引入任何“全局默认”**，未配置时直接使用 models.dev 上限。
  - 若配置了 `generation.max_output_tokens`：使用该值，但必须 `<= limit.output`（models.dev）。
  - 若未配置：默认 `max_output_tokens = limit.output`（models.dev），并把该值写入请求体。
  - 说明：这不会引入“仓库默认值”，只是在“未配置”时按模型能力上限自动确定。

2) `reasoning_effort`（可选）
- **用途**：对应 GPT‑5 系列的 reasoning effort（更像“think budget”）；建议用枚举（none/minimal/low/medium/high/xhigh）。
- 默认：不设置（跟随模型默认）。

3) `text_verbosity`（可选）
- **用途**：对应 Responses 的 `text.verbosity`（low/medium/high）。
- 默认：不设置。

4) `temperature`（高级可选）
- **用途**：采样控制。
- 收敛点：只暴露一个采样参数（`temperature`），不同时提供 `top_p`，避免重复/误用。

#### 字段名与约束（外部证据，避免拼错 + 避免后端报错）

以下字段名/取值来自 OpenAI 官方文档与官方 SDK 类型（Appendix A 之外的补充证据）：

- `max_output_tokens`
  - 含义：限制输出 token 上限（包含可见输出与 reasoning tokens）。
  - 证据：
    - openai-node：`max_output_tokens` 定义：
      - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L6455-L6461
    - openai-python：`max_output_tokens` 定义：
      - https://github.com/openai/openai-python/blob/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_create_params.py#L97-L102

- `reasoning.effort`
  - SDK 取值（枚举）：`none|minimal|low|medium|high|xhigh`。
  - 证据：
    - openai-node：
      - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/shared.ts#L245-L305
    - openai-python：
      - https://github.com/openai/openai-python/blob/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/shared_params/reasoning.py#L13-L35

- `text.verbosity`
  - SDK 取值（枚举）：`low|medium|high`。
  - 证据：
    - openai-node：
      - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L5454-L5483
    - openai-python：
      - https://github.com/openai/openai-python/blob/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_text_config_param.py#L13-L44

- `temperature`（范围：0–2）
  - 证据：openai-node `temperature` 定义：
    - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L6571-L6578
  - GPT‑5.2 约束（重要）：`temperature` 仅在 `reasoning.effort = "none"` 时支持，否则请求会报错。
    - 证据：OpenAI 最新模型指南：
      - https://developers.openai.com/api/docs/guides/latest-model/

#### 方案补强：把“预算字段”作为 compaction 的前置依赖

为了让 Issue 2（compaction 预算）能落地且可解释，建议把以下字段作为“预算输入源”，并明确优先级：

- `effective_context_window`
  - 优先级：使用 models.dev 的 `limit.context`，缺失时 fallback 到 `context_window_for_model(model)`。
- `reserved_output_tokens`
  - 优先级：使用“最终 resolved 的 max_output_tokens”（= provider 配置值 或 models.dev 的 `limit.output`）。

#### 实施要点（避免“配置了但不生效”）

- Provider 只对 `openai-responses` 生效，字段不应污染所有 provider 的 UI。
- 在 `validate.rs` 做 range 校验（例如 temperature、max_output_tokens > 0）。
- 在请求体中按需写入（Option 不写则不传）。
- 例外：`max_output_tokens` 即使未配置，也会写入 resolved 值（= `limit.output`）。

#### 新要求（你已确认）：OpenAI 相关 provider 的“最大限制”统一从 models.dev 拉取

你要求：**OpenAI 的所有 provider 的 max 等相关信息，从 `https://models.dev/api.json` 拉取**，因此建议引入一个明确的“模型能力来源”层（仅用于 max/capabilities，不等同于默认生成参数）：

- 数据源：`https://models.dev/api.json`
- 数据结构（实际抓取结果）：顶层是 provider map（例如 `openai`/`anthropic`/`openrouter`），每个 provider 下有 `models` map。
- OpenAI 模型条目里包含：`limit.context` / `limit.output`，并且 `limit.input` **可能存在**（可选字段；缺失时需要 deterministic fallback）。

来自 `models.dev/api.json` 的具体例子（供落地时对齐字段名；注意这里对应你说的 `openai/gpt-5.2`）：

- `openai.models["gpt-5.2"].limit.context = 400000`
- `openai.models["gpt-5.2"].limit.input = 272000`
- `openai.models["gpt-5.2"].limit.output = 128000`
- `openai.models["o3"].limit.context = 200000`

字段名（从 models.dev 数据本身可见）：

- provider 级：`<provider>.models` 是一个 map（key=模型 id，value=模型能力对象）。
- model 级：`limit.context` / `limit.output`，以及可选的 `limit.input`。

#### 上限字段到底几个？各自啥意思？默认为何？怎么“配置”？（一页讲清）

按 models.dev 的 OpenAI 数据结构，**上限字段只有 3 个**，全部位于 `limit`：

1) `limit.context`
- **含义**：最大上下文窗口（input + output 的总 budget 上限）。
- **默认为何**：不是“默认值”，是模型能力上限；默认就取 models.dev 给的数字。
- **怎么配置**：不提供配置字段；唯一配置方式是“选择 model id”。换模型就换上限。

2) `limit.input`（可选字段）
- **含义**：最大输入 token 上限（prompt 上限）。
- **默认为何**：若 models.dev 提供则取其值；若缺失则按固定规则从 `limit.context` 推导（见 Issue 2）。
- **怎么配置**：不提供配置字段；随 model id 变化。

3) `limit.output`
- **含义**：最大输出 token 上限（completion 上限；在 Responses 里对应 `max_output_tokens` 的硬上限）。
- **默认为何**：同上，取 models.dev。
- **怎么配置**：同上，不配置；随 model id 变化。

这 3 个字段是“能力上限”，不是“生成参数”。它们不参与任何“默认策略/用户调参”，只用于：

- 校验：`generation.max_output_tokens` 不能超过 `limit.output`
- 预算：compaction 触发/裁剪使用 `limit.input/limit.context` 作为预算上限
- 展示：UI 的 context 上限展示

模型示例（你关心的 `openai/gpt-5.2`）：

- `openai.models["gpt-5.2"].limit.context = 400000`
- `openai.models["gpt-5.2"].limit.input = 272000`
- `openai.models["gpt-5.2"].limit.output = 128000`

#### “OpenAI 相关 provider”到底怎么映射到 models.dev 的 `openai`？

仓库里存在多个 OpenAI 相关 provider（`openai` / `openai-responses` / `openai-codex`）。按你的要求，**它们的上限能力统一从 models.dev 的 `openai` provider 取**：

- models.dev provider 固定使用：`openai`
- models.dev model id 使用：去掉前缀后的模型名（例如 `gpt-5.2`）

也就是说：

- `openai::gpt-5.2` → lookup `models.dev.openai.models["gpt-5.2"]`
- `openai-responses::gpt-5.2` → lookup `models.dev.openai.models["gpt-5.2"]`
- `openai-codex::gpt-5.2` → lookup `models.dev.openai.models["gpt-5.2"]`

如果 models.dev 缺失某个 model 条目，则 fallback 到仓库已有的 `context_window_for_model(model)`（离线可用兜底）。

（以上例子来自本地抓取解析；落地时必须做缓存与更新策略，避免每次启动都硬依赖外网。）

models.dev 不可用时的行为（必须明确，避免实现分歧）：

- 若本地已有缓存（例如 `data_dir` 下的 models.dev 快照）：使用缓存。
- 缓存写入：使用临时文件 + 原子替换；Windows 下若目标文件已存在会先 remove 再 rename（避免 refresh 一次后无法更新）。
- 解析缓存的 memoization：按 `cache_path + (mtime, len)` 做 key，避免不同 `data_dir` 的缓存互相污染。
- 若无缓存：fallback 到仓库内置的 `context_window_for_model(model)`，并对缺失字段使用确定的保守推导值（避免实现分歧）：
  - `limit.context = context_window_for_model(model)`
  - `limit.input = floor(limit.context * 0.8)`
  - `limit.output = min(16384, floor(limit.context * 0.2))`
- 同时输出 warning 日志：说明 models.dev 不可用、使用了哪个 fallback 数据源。

建议用法（收敛、不引入额外复杂度）：

1) 用 `limit.context` 作为 context window 的真实上限（用于 UI 展示与总预算上限）。
2) 用 `limit.input` 作为 input budget 的真实上限（用于 compaction 的 `effective_input_budget`）。
3) 用 `limit.output` 作为 `max_output_tokens` 的硬上限校验。
4) 若 models.dev 中缺失某 model，则 fallback 到现有 `context_window_for_model()` 映射（保证离线可用）。

#### 默认法（默认值）与确定法（每次请求如何确定）

本方案的“模型细节配置参数”最终收敛为 **4 个**（只影响 `openai-responses` 请求 body；其它 OpenAI provider 可复用同一套逻辑）：

1) `max_output_tokens`
- 默认法：**不提供仓库全局默认**。
- 确定法：
  - 若 provider 配置了 `generation.max_output_tokens`：使用配置值，并校验 `<= limit.output`。
  - 若未配置：使用 `limit.output`。

2) `reasoning_effort`
- 默认法：不配置则不发送（跟随模型默认）。
- 确定法：若配置则写入 `reasoning.effort`；并作为 `temperature` 的 gating 条件。

3) `text_verbosity`
- 默认法：不配置则不发送（跟随模型默认）。
- 确定法：若配置则写入 `text.verbosity`。

4) `temperature`
- 默认法：不配置则不发送。
- 确定法：只有当用户**显式**设置 `reasoning_effort = "none"` 时才允许发送（否则 config check 直接报错）。

约束范围（v1 收敛口径）：

- 为避免模型默认行为不透明导致后端报错，v1 对 **openai-responses provider 的所有模型** 都采用该保守 gating。

#### 验收标准

- 配置写入后，能够在 `openai-responses` 请求 body 中看到对应字段。
- 未配置时：`max_output_tokens` 会显式写入 models.dev 的上限（这是有意的行为变更，用于“输出上限明确可控”）。
- 对不支持字段的 model/provider：要么不发送字段，要么明确 fail-fast + 清晰错误（不要 silent ignore）。

建议增加一条可测试的“可观测性”验收：

- 日志/事件输出能展示：`max_output_tokens/reasoning_effort/text_verbosity` 是否生效（至少 debug 级别，避免在生产 info 级刷屏）。

---

## 2) [DONE] Issue: 上下文压缩/裁剪机制（compaction + trimming）存在过早触发风险

### 当前实现（已落地）

#### 2.1 Auto-compact 触发逻辑（proactive / reactive / manual）

Auto-compact 仍然有三条触发路径（proactive / reactive / manual），但 Decision 口径已统一为：

- **只看“下一次请求的 prompt input 预算是否接近耗尽”**（启发式估算，保守高估，含 safety margin），不再累加 persisted `inputTokens`（避免 O(turn²) 的系统性过早触发）。
- Action 固定为：`summary + keep window(last 4 user rounds)`，并提供 `keep_window_overflow` 可恢复降级路径。

1) **Proactive（阈值触发，最常走）**

- 触发条件：`estimated_next_input_tokens >= HIGH_WATERMARK`，其中：
  - `estimated_next_input_tokens = estimate(system + history + current user) + SAFETY_MARGIN`
  - `HIGH_WATERMARK = floor(input_hard_cap * 0.85)`
- 同时进行 preflight：若 `keep window(last 4 user rounds) + current user` 自身已超过 `input_hard_cap`，进入恢复模式并返回结构化错误 `keep_window_overflow`（本轮不调用模型）。
- 证据：
  - 估算与触发：`crates/gateway/src/chat.rs:2252` / `crates/gateway/src/chat.rs:2319`
  - 恢复模式：`crates/gateway/src/chat.rs:2284`

2) **Reactive（溢出重试触发，兜底）**

- 当 provider 抛 `ContextWindowExceeded` 时，会 inline compact（summary + keep window）+ reload history + retry 一次。
  - 证据：`crates/gateway/src/chat.rs:4229` / `crates/gateway/src/chat.rs:4265`

3) **Manual（手动触发）**

- `chat.compact` RPC：无阈值判断，用户/渠道调用即触发（同样产出 summary + keep window）。
  - 证据：
    - RPC 注册：`crates/gateway/src/methods.rs:2023`
    - 实现入口：`crates/gateway/src/chat.rs:3084`
    - Web UI `/compact`：`crates/gateway/src/assets/js/page-chat.js:651`
    - Channel 命令 `compact`：`crates/gateway/src/channel_events.rs:850`

#### 2.2 `inputTokens` 的真实语义（仍保留，但不再用于 compaction Decision）

- `inputTokens` 存在于 assistant 消息中（不是 user 消息的字段）。
  - 证据：`crates/sessions/src/message.rs:36`

- 该字段是在每次 agent run 完成后，把本次调用的 usage 写入 assistant message。
  - 证据：`crates/gateway/src/chat.rs:2392`

这意味着：如果你每轮都把“包含全部历史”的 prompt 发出去，那么每一轮的 `inputTokens` 都包含前文，累加会增长得很快（大约 O(turn²)）。因此 v2 的 compaction Decision **不再**基于 persisted `inputTokens` 的 plain sum。

当前 UI/可解释性口径收敛为两套信息（避免混淆）：

- `tokenUsage.*`：展示“历史调用的真实 usage 统计”（仍然是 plain sum）。
- `budget.*`：展示“下一次 prompt input 的保守估算 + watermarks/caps”，用于 auto-compact 与 UI 的 Context 进度条。
  - 证据：`crates/gateway/src/chat.rs:3401` / `crates/gateway/src/assets/js/page-chat.js:331`

#### 2.3 Compaction 实施策略（summary + keep window）

- compaction 会 summarizer 旧内容，然后把 history 替换为：`[Conversation Summary]` + keep window（最近 4 轮 user rounds 原文）。
  - 证据：`crates/gateway/src/chat.rs:4604` / `crates/gateway/src/chat.rs:4563`

- keep window 保留 `tool_result` 元数据（UI-only）；summarizer 输入会跳过 `tool_result`（避免把 UI 证据当成 LLM 上下文）。
  - 证据：`crates/agents/src/model.rs:263`

补充：目前还存在一个“看起来像 compaction 入口但实际上无效”的路径，容易造成实现/文档混淆：

- `sessions.compact` RPC 存在，但 session service 实现是 no-op；真实 compaction 逻辑在 `chat.compact`。
  - 证据：
    - RPC 注册：`crates/gateway/src/methods.rs:1383`
    - no-op：`crates/gateway/src/session.rs:490`

### 讨论后的方案（参考成熟实现，修正确性 + 增加可解释性）

我们建议把 compaction 方案收敛为一个“**两段式**”规则：

1) **Decision（何时压缩）**：只看“下一次请求的 input 预算是否快用完”。
2) **Action（怎么压缩）**：把旧内容浓缩成 1 条 summary，并保留最近 4 轮原文。

这套结构主要参考并对齐以下成熟实现：

- **LangChain `ConversationSummaryBufferMemory`**：摘要 + 最近窗口原文。
- **opencode compaction.ts**：基于硬上限做 reserved/usable 预算 + watermarks。
- **LlamaIndex `ChatMemoryBuffer`**：按 token/预算做缓冲区裁剪/保留最近（概念对齐；v1 不引 tokenizer）。
- **Microsoft Semantic Kernel**：用“trigger vs target”的双阈值思想避免 oscillation，并强调边界不切断 tool/function 对。

（外部链接见 Appendix B。）

---

### 针对你的 4 个担忧的“结论收敛”（一句话版）

1) **Summary 模板成熟度**：成熟做法不是“写得漂亮”，而是“结构固定 + 只记事实 + 可回溯证据”。我们对齐 LangChain/LlamaIndex 的“summary + recent buffer”结构，并补充结构化/确定性约束，避免漂移。
2) **工具调用结果正确性**：需要区分 `tool`（会进 LLM 上下文）与 `tool_result`（UI 元数据）；compaction 必须确保 keep_window 内的 `tool` 不被打断，并在 summary 中提供可追溯的 tool digest。
3) **4 轮 overflow 不应导致会话停机**：v1 不应该“崩掉”，而应该进入可恢复的降级路径（预警 → 暂停模型调用但继续保存/整理 → 用户选择拆分/新会话/紧急放宽）。
4) **token 启发式必须保守**：OpenAI 的“1 token≈4 chars/bytes”是英语经验值，不适用于所有语言；v1 采用更保守的 UTF‑8 bytes 估算（宁可高估），并在临界点提前降级，避免“发送后才失败”。

#### 2.4 V1 约束（已确认，写死不谈判）

- `keep_last_user_rounds = 4`
- v1 仅 heuristic 估算（不引 tokenizer）
- 上限来源：`models.dev/api.json`

#### 2.5 Decision：什么时候触发 auto-compact（简洁且可解释）

输入硬上限（优先用 `limit.input`，最不歧义）：

- `input_hard_cap = limit.input`（来自 models.dev；例如 `openai/gpt-5.2` 是 272000）

若 `limit.input` 缺失（models.dev 未提供），v1 统一采用保守推导值（避免实现分歧）：

- `input_hard_cap = floor(limit.context * 0.8)`
- 同时输出 warning 日志：说明 `limit.input` 缺失、使用了推导值。

水位线（你已确认可用我建议值）：

- `HIGH_WATERMARK = floor(input_hard_cap * 0.85)`
- `LOW_WATERMARK  = floor(input_hard_cap * 0.60)`

状态机（hysteresis，避免抖动）：

- `normal -> compacting`：当 `estimated_next_input_tokens >= HIGH_WATERMARK`
- `compacting -> normal`：当压缩后 `estimated_next_input_tokens <= LOW_WATERMARK`

`estimated_next_input_tokens` 的口径（v1 明确写死，便于实现与测试）：

- 取“下一次请求实际会发送给模型的 messages”的序列化文本总量做估算。
- 包含：system/instructions + summary（如存在）+ keep_window（最后 4 轮原文）+ 本轮 user content。
- 不包含：`tool_result`（UI-only 元数据不会进入 `ChatMessage`，见 `crates/agents/src/model.rs:263`）。

计算方法（v1 写死，避免实现/测试漂移）：

- 估算对象：上述“会发送给模型的 messages”中所有 **文本 content**（按消息顺序拼接，但不依赖 JSON 序列化格式）。
- 对每段文本执行：`tokens_est = ceil(utf8_bytes / 3)`。
- 总估算：`estimated_next_input_tokens = sum(tokens_est(text_chunks)) + SAFETY_MARGIN_TOKENS`。
- `SAFETY_MARGIN_TOKENS`（v1 固定常量）：`1024`。

#### 2.6 heuristic token 估算（v1 只保留 1 个规则 + 1 个加严分支）

估算目标：不是“算准”，是“不要低估”。

v1 收敛到 **一个规则**（更简单、更保守）：

- `tokens_est = ceil(utf8_bytes / 3)`

理由（讲人话）：

- 英文经验值约 `bytes/4`，但 CJK/代码/混合文本很容易低估；`bytes/3` 更保守，能显著降低超限失败。

证据：仓库内已有同款 fallback（local backend 缺失 token 计数时估算）：

- `crates/agents/src/providers/local_llm/backend.rs:846`

外部依据（“讲人话”的估算口径来源）：OpenAI 的 token 经验法则（1 token ≈ 4 characters）：

- https://help.openai.com/en/articles/4936856-what-are-tokens-and-how-to-count-them

补充外部依据（同一口径的官方页面）：

- https://platform.openai.com/tokenizer

#### 2.7 Action：怎么压缩（讲人话 + 可验收）

一句话：**把“旧聊天记录”变成 1 条摘要，把“最近 4 轮”原封不动留着。**

步骤：

1) 按 user round 切分 history。
2) 找到最近 4 个 `User` round 的起点 `keep_start_idx`。
3) `keep_window = history[keep_start_idx..]`：原样保留（包括其中的 `Tool/ToolResult`）。
4) `old_segment = history[..keep_start_idx]`：送 summarizer 输出 summary。
5) 替换 history 为：`[Conversation Summary]\n\n<summary>` + `keep_window`。

关键点：

- 最近 4 轮 **必须 byte-for-byte 不变**。
- keep_window 内出现的 `ToolResult` 必须保留，否则 UI/工具证据会断裂。

补充约束（参考 LlamaIndex/Semantic Kernel 的成熟做法）：

- keep_window 不能从 `Assistant/Tool/ToolResult` 开头（必须从 `User` round 边界开始），避免出现“悬空的 tool result/assistant continuation”。

---

### 2.x 你担心的 4 个点：收敛后的明确答复（讲人话 + 带证据）

#### (1) Summary 模板是否成熟？如何避免“越总结越漂移”？

成熟的点不在“句子好不好看”，而在“**固定结构 + 只记事实 + 可回溯证据**”。

收敛策略（v1）：

- summary 仍用固定标题（Context/Decisions/Plan/Open Questions/Artifacts），并强制“事实优先、未知标注”。
- 任何不确定信息必须写成 `未知/待确认`，禁止编造。
- 摘要更新方式建议走“增量更新”（不要每次整段重写），减少漂移。

参考来源（我们借鉴的成熟结构）：

- LangChain `ConversationSummaryBufferMemory`（moving summary + recent buffer）
- LlamaIndex `ChatSummaryMemoryBuffer`（summary + recent raw，并对边界做修正）

#### (2) 工具调用结果在 summary + 4 轮模式下是否正确？

这里必须把“对 LLM 有效的上下文”与“UI 证据”分开：

- `tool` role message 是会进入 LLM 对话上下文的（代表工具输出给模型看的文本）。
- `tool_result` role message 在当前仓库里是 UI-only 元数据，转换成 `ChatMessage` 时会被跳过。
  - 证据：`crates/agents/src/model.rs:263`

因此，compaction 的正确性要求（v1）收敛为两条：

1) **keep_window 必须保留 `tool` 消息**（它影响模型下一轮推理）。
2) **keep_window 也要保留 `tool_result` 元数据**（用于 UI 回放/审计），但它不需要进入 summarizer 的 LLM 上下文。

另外一个关键事实：工具输出在进入 LLM 前已经有 sanitize/truncate：

- `sanitize_tool_result()` 会剔除 base64/hex blob，并按 `max_bytes` 截断（保证 UTF‑8 边界）：`crates/agents/src/runner.rs:472`
- exec 工具也会按 `max_output_bytes` 截断 stdout/stderr：`crates/tools/src/exec.rs:112`

这意味着：多数情况下“工具输出爆炸导致 4 轮 overflow”的风险主要来自 **用户粘贴超长文本**，而不是工具结果无限增长。

#### (3) 如果最后 4 轮原文本身就 overflow，会不会直接停机？怎么处理才合理？

你担心是对的：直接 error 会让用户觉得“会话崩了”。

收敛的 v1 行为（简洁、有效、可恢复）：

1) **预检查**：发送前估算 `keep_window + system` 是否接近/超过 `limit.input`。
2) **不崩溃**：如果超限，进入“恢复模式”——继续保存消息/证据，但暂停本轮模型调用。
3) **给用户 2 个明确选项**：
   - A. 拆分/缩短最近一轮内容（推荐）
   - B. 新开 session（推荐）

仅在用户显式同意的情况下，才提供 break-glass：允许临时放宽“保留 4 轮原文”的约束（例如改保留 3/2/1 轮）。默认不启用。

恢复模式的对外表现（v1 写死，便于实现与测试）：

- 本轮**不调用模型**（避免重复失败/浪费成本）。
- 会话仍可用：消息继续保存，UI 能看到“超限原因”。
- 返回结构化错误：`keep_window_overflow`（或等价错误码），并附带两条可执行建议：
  - “缩短/拆分最近输入（推荐）”
  - “新开 session（推荐）”

错误码命名（v1 写死，避免实现分歧）：

- 统一使用：`keep_window_overflow`

（Momus 的产品结论：超限时要“可恢复的降级”，不要“神秘失败/硬崩”。）

#### (4) OpenAI token 启发式是否理解正确？我们是否在乱用“4 chars/token”？

我们对“4 chars/token”的理解是：**英语/常见文本的经验法则，不是普适公式**。

官方依据：

- OpenAI Help Center：1 token ≈ 4 characters（并提示不同语言 token 比例不同）：
  - https://help.openai.com/en/articles/4936856-what-are-tokens-and-how-to-count-them
- OpenAI Tokenizer 页面也给出同样的经验值，并建议用 `tiktoken` 做程序化计数：
  - https://platform.openai.com/tokenizer

因此 v1 才会采用更保守的 `utf8_bytes/3`（宁可早触发预警，也不让你“发出去才 ContextWindowExceeded”）。

实现落点（对应当前代码结构，说明差异）：

- 现在的实现是“全量 summary → replace_history(summary-only)”。
  - 证据：`crates/gateway/src/chat.rs:4265`
- v1 要改成 “summary + keep_window”。

#### 2.8 人话示例（用 `openai/gpt-5.2` 真数据）

假设当前模型：`openai/gpt-5.2`，models.dev 给：

- `limit.input = 272000`
- `HIGH_WATERMARK = 231200`（85%）
- `LOW_WATERMARK = 163200`（60%）

场景：会话很长，估算下一次请求 input 将达到 `240000`。

- 因为 `240000 >= 231200`，进入 compacting。
- 压缩后：旧内容变为 1 条 summary，保留最近 4 轮原文。
- 若压缩后估算 input 降到 `150000`，则 `150000 <= 163200`，退出 compacting（回到 normal）。

#### 2.9 硬边界（必须提前说清楚，不装能自动修）

- **keep_window overflow**：如果“最近 4 轮原文”自己就超过 `limit.input`，那自动压缩无法满足约束。
  - v1 行为：进入“恢复模式”（本轮不调用模型，但会话继续可用），并返回可执行指引：缩短/拆分最近输入或新开 session（不做隐式截断）。

#### 2.7 裁剪（trimming）相关补充

目前仓库已有部分“安全裁剪”工具（char boundary + 附注），可以复用其模式：
- tool result 截断：`crates/agents/src/runner.rs:476`
- silent_turn 截断已修复为 char-safe（避免 panic）：`crates/agents/src/silent_turn.rs:217`

但 compaction 仍需解决“何时触发 + 触发后保留什么”的正确性与 UX 可解释性。

### 验收标准

- 长会话中 compaction 触发次数显著下降（不再随着轮数二次增长）。
- 触发 compaction 时的日志/事件可解释：输出 `effective_context_window/max_output_tokens/reserve/estimated_next_input_tokens`。
- compaction 后 retry 不应频繁失败（`ContextWindowExceeded` 的重试命中率提高）。

补充与你的需求直接相关的验收：

- compaction 后，history 必须包含：1 条 summary + 最近 4 轮原文（轮次按 user message 计）。
- compaction 不得丢失最近窗口内的 `ToolResult`（否则 UI/工具链断裂）。

建议增加两条可操作验收：

- 触发 auto-compact 的日志（或广播事件）必须包含：`effective_context_window / reserved_output_tokens / reserve_safety_tokens / effective_input_budget / estimated_next_input_tokens / HIGH_WATERMARK`。
- 当 provider 抛 `ContextWindowExceeded` 触发 inline compact + retry 时：必须把“estimated_next_input_tokens 与实际失败”的偏差记录下来（用于校准 reserve/估算策略）。

---

## 3) [DONE] OpenAI Responses Prompt Cache 显式分桶（prompt_cache_key）

### 现状与证据

- `openai-responses` provider 在启用 prompt_cache 时，会把分桶写入官方字段 `prompt_cache_key`：
  - 优先使用 session_key（来自 gateway 显式传入的 request context）。
  - 若本次请求缺失 session_key，则使用确定性的 fallback：`moltis:<provider>:<model>:no-session`（同样受 bucket_hash 影响）。
  - 注意：模型注册表的 provider wrapper 必须转发 `*_with_context`（否则 ctx 会被默认实现丢弃，进而导致所有请求都落到 fallback `no-session` 分桶）。
  - request body：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_with_context`）
  - tests：`crates/agents/src/providers/openai_responses.rs`（`build_responses_body_includes_prompt_cache_key_when_enabled_and_session_key_provided` / `build_responses_body_uses_fallback_prompt_cache_key_when_context_missing`）

- `cached_tokens` 的解析已覆盖 JSON 与 SSE 两条路径，统一写入 `Usage.cache_read_tokens`。
  - JSON：`crates/agents/src/providers/openai_responses.rs`（`parse_responses_output`）
  - SSE：`crates/agents/src/providers/openai_responses.rs`（`ResponsesSseParser` 解析 `input_tokens_details.cached_tokens`）
  - tests：`crates/agents/src/providers/openai_responses.rs`（SSE payload 带 `cached_tokens` 的断言）

### 外部证据：OpenAI Responses 支持 request-side 参数

Librarian 提供的证据（SDK 类型明确存在）：

- `prompt_cache_key`：用于提高 cache hit rate（替代 `user` 字段）

（按你的要求：本方案只保留 `prompt_cache_key` 作为实施依据。）

详见 Appendix A（含链接）。

### 讨论后的方案（收敛：仅做 session 分桶）

#### 配置面（建议）

新增：`[providers.openai-responses.prompt_cache]`

字段最小集（按你的最新要求进一步收敛）：

1) `enabled`（当 `prompt_cache` block 存在时默认 true）
- 默认行为：未配置 `[providers.openai-responses.prompt_cache]` 时不发送 `prompt_cache_key`；配置了该 block 且未显式设置 enabled 时，enabled 按默认值为 true。

2) `bucket_hash`（可选，默认 auto）
- **用途**：当 session_key 可能过长时，自动把它转换成固定长度的 bucket id（64 hex）。
- 默认规则（你已确认）：
  - 长度判定口径：按 **UTF-8 字节长度**（`session_key.as_bytes().len()`）。
  - 若 `session_key` UTF-8 字节长度 `<= 64`：默认不 hash（bucket_id=session_key）
  - 若 `session_key` UTF-8 字节长度 `> 64`：默认 hash（bucket_id=hash64hex(session_key)）
- 覆盖规则（仍保持参数收敛）：
  - 显式 `bucket_hash=true`：强制 hash（无论长度）
  - 显式 `bucket_hash=false`：强制不 hash（无论长度）

#### 具体落地点（OpenAI Responses 请求体）

- 当 enabled=true 时，只写入：
  - `prompt_cache_key: <bucket_id>`

按你的要求：只写 `prompt_cache_key`，不引入其它 caching 相关参数。

> 备注：你提到“把 session id 放到头部字段”
> - OpenAI SDK/文档证据指向 request body 字段 `prompt_cache_key`，而不是 header。
> - 因此本方案使用 `prompt_cache_key` 作为南向分桶依据。

#### 风险与安全约束（Momus 强调）

- 绝不允许 cache key 派生自用户敏感内容（避免把隐私纳入 cache key）。
- UI/日志需要可解释：至少输出“cache enabled/bucket_hash/cached_tokens 命中情况”。
-- 提供“禁用 cache”的管理入口（初期至少在文档 + config 层提供）。

隐私/合规说明（必须提前写清楚，避免误解）：

- 当 `bucket_hash=auto` 且 session_key UTF-8 字节长度 `<=64` 时，`prompt_cache_key` 会直接等于 session_key。
- 这可能包含渠道标识（例如 Telegram 的 account_id/group_id）。如果你希望永不外发这些标识，应强制 `bucket_hash=true`（始终用 64 hex）。

补充强约束（在 enabled 默认 true 的前提下，尽量不让行为“黑箱”）：

- UI/日志必须可解释：至少能看到“本次请求是否发送 prompt_cache_key / bucket_id 是否 hash / cached_tokens 命中”。

#### 推荐默认值与“最小惊讶”UX（Momus 收敛建议）

v1 的目标不是“让缓存无处不在”，而是“默认不惊吓 + 显式可控 + 可审计”。建议：

v1 建议（更偏向“最小惊讶/显式可控”）：

- 默认：不配置 prompt_cache block（不发送 `prompt_cache_key`）。
- 若要启用：显式增加 `[providers.openai-responses.prompt_cache]`（enabled 默认 true，或显式写 enabled=true）。
- 默认：`bucket_hash=auto`（按 session_key 长度决定）。
- 可观测性：每次请求至少能看到 `prompt_cache_enabled/bucket_hash/cached_tokens`（日志或 debug 面板）。

#### bucket_id 生成规则（你要求给出具体示例供确认）

本 issue 里讨论的“session id / session key”，指的是 **Gateway 层 session_key 字符串**（也就是 `chat.rs` 在处理消息、落 session store、广播 UI 时用的 key）。它在代码库里不是唯一一种 SessionKey 形态，但这是我们用于 prompt cache 分桶的那一种。

Gateway 层 session_key 的真实格式在仓库内已有证据（明确示例）：

1) Web UI 默认会话：`main`
- 证据：未命中 connection active session 时回落到 `"main"`：`crates/gateway/src/chat.rs:1703`

2) Web/Channel 新建会话：`session:<uuid>`
- fork 新 key：`format!("session:{}", uuid::Uuid::new_v4())`
  - 证据：`crates/gateway/src/session.rs:523`
- Channel `/new` 新 key：`format!("session:{}", uuid::Uuid::new_v4())`
  - 证据：`crates/gateway/src/channel_events.rs:717`

- Telegram DM：`telegram:{account_id}:dm:{peer_id}`
  - 证据：`crates/telegram/src/handlers.rs:1478`
- Telegram Group/Channel：`telegram:{account_id}:group:{group_id}`
  - 证据：`crates/telegram/src/handlers.rs:1481`

另外还有一种 channel 默认 session key 格式（不含 dm/group 细分）：

- `telegram:{account_id}:{chat_id}`
  - 证据：`crates/gateway/src/channel_events.rs:1331`

我们建议 `bucket_id` 生成规则如下（默认 session，不加 hash）：

- `bucket_id = session_key`

示例（会直接写入 `prompt_cache_key`）：

- `prompt_cache_key = "main"`
- `prompt_cache_key = "session:eaf73632-f040-45af-ae03-94c831df5242"`
- `prompt_cache_key = "telegram:bot1:dm:user123"`
- `prompt_cache_key = "telegram:bot1:group:-100999"`
- `prompt_cache_key = "telegram:bot1:-100999"`

如果你确认要在 session 基础上加 hash（`bucket_hash=true`），则：

- `bucket_id = hash64hex(session_key)`
  - 输出形式：固定 64 个小写 hex（长度可控）
  - 算法（已确认）：`hash64hex = blake3(session_key)` 的 hex 编码（32 bytes → 64 hex），对同一个 `session_key` 必须稳定、跨进程一致。

默认策略（你已确认）：`bucket_hash=auto`（len<=64 不 hash；len>64 hash）。

#### 实施 seam（已确认）：session_key 必须显式传递

为避免从 messages 反推导致不可靠，本方案要求：gateway 在调用 `openai-responses` provider 构造请求体时，**显式传入 session_key**（作为 `prompt_cache_key` 的输入源）。

### 验收标准

- enabled=true 时，请求体包含 `prompt_cache_key`。
- usage 里的 `cached_tokens` 在 metrics/UI 里可见（已有基础）。
- 不同 session 不应出现跨 session 的缓存命中（bucket_id=session_key 或 hash(session_key) 时）。

---

## 4) Issue: 1 群 N bot 响应 / 群内多 bot 自主互聊（Survey-only）

### 已核实事实（系统层）

- Moltis 支持从配置启动多个 Telegram bot account：`crates/gateway/src/server.rs:1728`
- session key 包含 account_id：`crates/telegram/src/handlers.rs:1471`
- 群聊默认 mention 模式（不 @bot 不响应）：`crates/channels/src/gating.rs:57`

### 平台侧限制（需要在 issue 记录中写清楚）

- Telegram Bot API 通常不会把“另一个 bot 发的消息”分发为 update 给你的 bot，因此“bot-bot 自主互聊”在平台层面就不可靠。
- “1 群 N bot 响应用户指令”更可行：每个 bot 只响应用户（可能要求 @mention 或自定义 mention_mode）。

### Survey 结论（只记录，不做实现承诺）

建议在 issue 中明确区分两个目标：

1) **N bot 都能响应用户**（同一群里多个 bot 各自工作）
- 需要：配置层多 bot account + mention_mode 策略 + 避免刷屏的速率/礼貌策略。

2) **多 bot 自主互聊**（bot 之间触发彼此）
- 平台限制强，需要网关层“中介编排”（例如把 A 的输出作为 B 的输入并通过系统内部事件分发）。
- 风险：会放大 compaction/caching/tool 权限的问题；应在 compaction/prompt cache/sandbox 更成熟后再做。

---

## 5) [DONE] Issue: 沙箱容器挂载（外部目录 RO/RW 映射）配置方案

### 当前实现（已落地）

- 配置：新增 `tools.exec.sandbox.mount_allowlist` + `tools.exec.sandbox.mounts`（deny-by-default）。
  - schema：`crates/config/src/schema.rs:1279` / `crates/config/src/schema.rs:1296`
  - template：`crates/config/src/template.rs:267`
- 校验：host/guest 必须绝对路径；guest_dir 必须位于 `/mnt/host/`；`mode` 仅允许 `ro|rw`；配置了 mounts 但 allowlist 为空会直接报错。
  - validation：`crates/config/src/validate.rs:911`
- Docker backend：将 external mounts 转为 `-v host:guest:ro|rw` 并追加到 docker run args；对 host_dir 做 canonicalize 且必须落在 allowlist roots 内（阻断 symlink 逃逸）。
  - 实现：`crates/tools/src/sandbox.rs:824` / `crates/tools/src/sandbox.rs:976`
- 单测：
  - 配置校验：`crates/config/src/validate.rs:1689`
  - Docker args / symlink 逃逸拒绝：`crates/tools/src/sandbox.rs:2459`

### 备注：`workspace_mount` 的语义（未改变）

- Docker backend 的 `workspace_mount` 仍然挂载的是 `moltis_config::data_dir()`（不是进程 cwd/repo）。
  - 证据：`crates/tools/src/sandbox.rs:761`

### 设计与实现细节（v1 落地版）

新增：`tools.exec.sandbox.mounts = [...]`

收敛后的 mount entry 字段（尽量少、且行为明确；命名更直观）：

- `host_dir`（宿主机路径，必须绝对路径；且必须存在）
- `guest_dir`（容器内路径，必须绝对路径；且必须在受控前缀下，例如 `/mnt/host/...`）
- `mode`（`ro|rw`，默认 ro）

（删除 `required` 字段：为了收敛配置面，v1 统一 fail-fast —— 配了 mount 但路径不存在就直接报错；不需要“可选 mount”。）

同时增加 allowlist（已收敛默认与示例，避免 `...` 带来实现/理解分歧）：

- 默认：`tools.exec.sandbox.mount_allowlist = []`（deny-by-default，不允许任何外部挂载）
- 示例：`tools.exec.sandbox.mount_allowlist = ["/mnt/c/dev"]`

并在运行前做严格校验：

- canonicalize host_dir（解析 `..` 与 symlink），并确认在 allowlist roots 内
- 拒绝危险 guest_dir（如 `/`, `/proc`, `/sys`, `/dev` 等）
- 默认拒绝不在 allowlist 的 mount（deny-by-default）
- 对 `rw` mount 追加更强警告/确认（若未来接 UI）

额外收敛约束（Linux/Docker v1）：

- **只允许挂载目录**（不允许单文件 mount），避免 file-level 意外覆盖与可执行注入面扩大。
- `guest_dir` 只允许位于 `/mnt/host/` 之下（固定前缀；减少攻击面与误配）。

#### 方案补强：把 mount 设计成“强约束 + 易审计”

为了降低误配与越权风险，建议在文档与实现层都明确：

- `guest_dir` 必须在固定前缀下（例如强制 `/mnt/host/...`），不允许挂载到任意路径。
- 默认所有 mounts 都是 `ro`（除非显式 `rw`）。
- 将最终生效的 mounts（host_dir/guest_dir/mode）打印在 debug 日志（或 `moltis sandbox ...` 诊断输出）中，便于审计。

#### 后端差异与收敛策略（避免“配了但不生效”）

本轮只需要收敛到 Linux/Docker：

- v1：仅保证 Docker backend 支持外部 mounts（最小可控闭环）。

### 实施落点建议

- 在拼 Docker/Podman args 的地方统一处理 mounts（DockerSandbox 已集中生成 args）：
  - 证据：`crates/tools/src/sandbox.rs:784`（`ensure_ready` 构造 docker run args）

### 验收标准

- 配置能表达：外部目录 ro/rw mount
- 默认安全：没有配置时行为与当前一致（仅 workspace ro/rw/none）
- 有校验：越权 mount（非 allowlist、symlink 逃逸、危险 guest_dir）被拒绝并给出清晰错误

建议补充两条可测试验收：

- 当 `host_dir` 不存在：启动/执行应 fail-fast，错误消息包含 mount index 与路径。
- 当 `host_dir` 通过 symlink 逃逸 allowlist 时：必须被拒绝（以 `canonicalize` 后路径判定）。

### 已确认决策（实现按此执行）

1) `guest_dir`：必填。
2) `mode`：默认 `ro`；仅在显式 `mode="rw"` 时允许写入。
3) mounts：只允许目录挂载；`guest_dir` 必须位于 `/mnt/host/` 之下；`host_dir` 必须 canonicalize 后仍位于 allowlist roots 内。

---

## Appendix A: OpenAI Responses prompt caching 字段证据（外部）

- Prompt caching guide：
  - https://developers.openai.com/api/docs/guides/prompt-caching/

- `prompt_cache_key`（openai-node SDK types）：
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L783-L789



---

## Appendix B: Compaction 成熟实现 survey 参考（外部）

以下链接用于对齐成熟模式与术语（不要求照抄，只用于校准方案的合理性）：

- LangChain token buffer memory（sliding window）：
  - https://github.com/langchain-ai/langchain/blob/c997955bf3b76e5fe5c5e05648c8978c2320d9c4/libs/langchain/langchain_classic/memory/token_buffer.py#L61-L71

- LangChain summary buffer memory（moving summary）：
  - https://github.com/langchain-ai/langchain/blob/c997955bf3b76e5fe5c5e05648c8978c2320d9c4/libs/langchain/langchain_classic/memory/summary_buffer.py#L112-L124

- opencode compaction reserved/usable budget 思路：
  - https://github.com/anomalyco/opencode/blob/d338bd528c010bdab481e0e9ecc637674a2d5246/packages/opencode/src/session/compaction.ts#L30-L48

- LlamaIndex ChatSummaryMemoryBuffer（summary + recent buffer + 边界处理）：
  - summary + recent 组合：
    - https://github.com/run-llama/llama_index/blob/c49d0344d76ae09f6c189a1635b4996054e59b32/llama-index-core/llama_index/core/memory/chat_summary_memory_buffer.py#L179-L197
  - recent selection（按 token limit 反向保留最新）：
    - https://github.com/run-llama/llama_index/blob/c49d0344d76ae09f6c189a1635b4996054e59b32/llama-index-core/llama_index/core/memory/chat_summary_memory_buffer.py#L229-L273
  - 边界修正（避免以 ASSISTANT/TOOL 开头）：
    - https://github.com/run-llama/llama_index/blob/c49d0344d76ae09f6c189a1635b4996054e59b32/llama-index-core/llama_index/core/memory/chat_summary_memory_buffer.py#L318-L338

- Microsoft Semantic Kernel（双阈值 + 保留 function/tool 成对边界）：
  - hysteresis knobs：
    - https://github.com/microsoft/semantic-kernel/blob/8513c2ac00fbbd61af714e15a3ca4803db570d5b/python/semantic_kernel/contents/history_reducer/chat_history_reducer.py#L20-L48
  - trigger condition：
    - https://github.com/microsoft/semantic-kernel/blob/8513c2ac00fbbd61af714e15a3ca4803db570d5b/python/semantic_kernel/contents/history_reducer/chat_history_summarization_reducer.py#L82-L90
  - boundary-aware extraction（preserve_pairs）：
    - https://github.com/microsoft/semantic-kernel/blob/8513c2ac00fbbd61af714e15a3ca4803db570d5b/python/semantic_kernel/contents/history_reducer/chat_history_summarization_reducer.py#L108-L121

---

## Appendix C: OpenAI Responses built-in web_search 字段证据（外部）

> 目的：把「字段名 / 枚举值 / includables」写死到可引用证据上，避免后续改 UI/日志/配置时拼写漂移。

### C.1 tool 参数名（`tools: [{"type":"web_search", ...}]`）

- `filters.allowed_domains` / `search_context_size` / `user_location`：
  - openai-node `WebSearchTool`：
    - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L6316-L6383

### C.2 include strings（includables）

- `web_search_call.action.sources`
- `web_search_call.results`
  - openai-node `ResponseIncludable`：
    - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L2895-L2926

### C.3 失败模式与流式事件（可观测性依据）

- web search call `status` 包含 `failed`：
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L2657-L2680
- 流式事件类型：`response.web_search_call.in_progress/searching/completed`
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L5696-L5769

### C.4 自定义 `base_url` / 代理兼容性（SDK 证据）

- Node SDK 支持覆写 `baseURL`（含 `OPENAI_BASE_URL`），因此“自定义 `base_url`”是一个官方支持的客户端形态。
  - 证据：
    - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/client.ts#L265-L270

# OpenAI Responses 内置 Web Search（Web live）接入方案（Provider 级开关）

本文档面向 **Moltis 代码库**，目标是在不依赖任何外部付费搜索 KEY 的前提下，使用 **OpenAI Responses API 的内置 Web Search（`tools: [{type: "web_search"}]`）** 来实现联网实时检索，并提供与之配套的 **provider 级配置/管理开关**。

> 适用场景（你强调的重点）：
> - 使用 `openai-responses` provider
> - 使用自定义 `base_url`（只要它实现/转发 OpenAI Responses 能力）
> - 使用 API Key
> - `/chat/completions` 不是重点，Azure 暂不考虑

---

## 1. 目标与非目标

### 目标

1) **Provider 级开关**控制“是否启用 OpenAI 内置 Web Search”。

2) 配置面 **收敛（字段最小集）**，并且 **默认值行为清晰、可预测、不会误触发额外成本**。

3) 解释清楚：
- 每个字段到底控制什么；
- 不同字段不填/填了会发生什么；
- 这个内置 web search 与自定义 tool calling 是否互斥；
- 启用后有哪些限制与影响（成本、延迟、输出形态、UI 支持）。

### 非目标（本阶段不做/不保证）

- 不以 `/chat/completions` 的 web search（`web_search_options` + search-preview models）为核心路径。
- 不要求 UI 在第一版就结构化展示 citations/sources（可以先让文本输出可用）。
- 不接入/依赖任何第三方搜索服务（Brave/Perplexity 等）或其 KEY。

---

## 2. “内置 Web Search”是什么？（一句话）

OpenAI Responses API 内置 Web Search 是 **OpenAI 托管的内置工具**：你在请求中声明 `tools: [{"type":"web_search"}]`，模型会在生成前自行执行联网检索，并在输出中带引用（annotations/citations）。你无需在本地实现搜索后端，也不需要配置 Brave/Perplexity 之类的付费 KEY。

官方示例（工具开关最简形态）：

```json
{
  "model": "gpt-5",
  "tools": [{ "type": "web_search" }],
  "input": "What was a positive news story from today?"
}
```

---

## 3. Provider 级最小字段集（每个字段详细解释）

建议新增配置块：

```toml
[providers.openai-responses.builtin_web_search]
...
```

字段最小集如下（**只做这几个，避免配置面发散**）：

### 3.1 `enabled`（必选开关，默认关闭）

- **含义**：是否在 `openai-responses` 的 `/responses` 请求中注入内置 web search 工具。
- **为什么必须显式**：开启 web search 通常会带来额外成本和延迟，必须避免“用户没意识到就开了”。
- **默认行为（最重要）**：
  - 如果 `[providers.openai-responses.builtin_web_search]` 整块不存在 → 视为关闭。
  - 如果存在但 `enabled = false` → 视为关闭。
  - **只有 `enabled = true` 才会真正生效**。

> 推荐策略：如果用户写了其他字段（如 `allowed_domains`）但忘了写 `enabled = true`，配置校验应报错或强警告，避免“以为开了但其实没开”。

当前实现（与上面推荐略有不同，避免对“空 block”过度报错）：

- 如果 `[providers.openai-responses.builtin_web_search]` 存在，但 `enabled = false`（或未写，按默认值为 false），且同时配置了其它字段（`allowed_domains/search_context_size/user_location/include_sources`）→ **配置校验报错**。
- 如果只写了一个空 block（没有其它字段）→ **不报错**，但功能不会生效。

### 3.2 `allowed_domains`（可选：限定搜索域名范围）

对应 OpenAI tool 参数：`filters.allowed_domains`。

- **含义（通俗解释）**：把“能搜到的网页范围”限制在某些域名里（白名单），减少跑偏与降低提示注入风险。
- **典型用途**：只允许搜你信任的资料源，例如 `openai.com`、`docs.rs`、`kubernetes.io`。
- **怎么写**：通常写域名，不要带 `https://` 前缀；可包含子域名。
- **默认行为**：不配置时 → 不限制域名（全网范围）。

示例：

```toml
allowed_domains = ["openai.com", "docs.rs", "kubernetes.io"]
```

### 3.3 `search_context_size`（可选：控制“检索上下文量”）

对应 OpenAI tool 参数：`search_context_size`（允许值：`"low" | "medium" | "high"`）。

- **含义（通俗解释）**：模型从网页检索结果中“带回多少信息”来用于回答。
- **影响**：
  - `low`：更快/更省，但可能信息不足、引用少。
  - `medium`：平衡（OpenAI 文档侧通常默认相当于 medium）。
  - `high`：信息更充分，但通常更慢/成本更高。
- **默认行为**：不配置时 → 让 OpenAI 端使用默认值（等价于 `medium` 的行为预期）。

示例：

```toml
search_context_size = "medium"
```

### 3.4 `user_location`（可选：让搜索结果更贴近地理位置）

对应 OpenAI tool 参数：`user_location`。

- **含义（通俗解释）**：告诉模型“用户大概在哪”，让它在搜索时更偏向该地区的结果（例如本地新闻、当地政策、附近服务）。
- **你需要知道的点**：
  - 这是“近似位置”偏好，不是精确 GPS。
  - 不填写不会影响功能，只是搜索结果可能不够本地化。
- **建议最小写法**：
  - `type = "approximate"`
  - 再选填 `country/region/city/timezone`
- **默认行为**：不配置时 → 不提供位置信息。

示例：

```toml
[providers.openai-responses.builtin_web_search.user_location]
type = "approximate"
country = "US"
region = "California"
city = "San Francisco"
timezone = "America/Los_Angeles"
```

### 3.5 `include_sources`（可选：是否请求“完整 sources 列表”）

对应 OpenAI Responses 参数：`include: ["web_search_call.action.sources"]`。

- **含义（通俗解释）**：
  - 默认情况下，回答文本可能会带“少量关键引用/内联 citations”。
  - `include_sources=true` 会请求模型返回“它检索过程中用到的 sources 列表（可能比 citations 多）”。
- **影响**：返回体更大、潜在成本更高、UI 若不支持展示则用户看不到这些信息。
- **默认行为（推荐）**：`false`。

补充说明：
- 官方还提供另一个相关 includable 值：`web_search_call.results`（偏“结果列表/条目”，而不是最终用到的 sources）。
- 这两个字段名在官方 SDK 的类型定义里是硬编码枚举值，不要拼错。

> 推荐：第一版先保持 `include_sources=false`。等 UI 能展示 sources/citations 再开启。

---

## 4. 内置 Web Search 与自定义 Tool Calling 是否互斥？

**不互斥。**

在 Responses API 中，`tools` 数组允许同时包含：

- **内置工具**：例如 `{ "type": "web_search" }`
- **函数工具**：例如 `{ "type": "function", "name": "exec", ... }`

因此：

1) 你可以同时开启内置 web search + 继续使用 Moltis 的其它工具（例如 `exec`、`browser`、skills 等）。
2) 唯一需要注意的是“工具歧义”：当前 Moltis 里本地也存在一个 function tool 叫 `web_search`（`crates/tools/src/web_search.rs`），它是 **外部搜索服务**（Brave/Perplexity）+ 无 KEY 时 fallback 到 DuckDuckGo HTML。

### 推荐的“无歧义策略”（不增加额外配置字段）

当 `providers.openai-responses.builtin_web_search.enabled=true` 时：

- **仍允许**：其它 function tools（exec/web_fetch/browser/skills/...）
- **自动排除**：function tool 名称为 `web_search` 的本地工具（避免模型在“内置 web search”和“本地 web_search function”之间摇摆）

这样能保证你想要的“完全不使用外部 web_search KEY”目标成立，并且不会破坏其它工具能力。

> 进一步的“强保证”（可选）：
> - 你也可以在全局工具配置中直接关闭本地外部搜索工具：`tools.web.search.enabled = false`。
> - 这不是必须条件（因为我们建议 provider 在 built-in 开启时自动过滤本地 `web_search` function tool），但对“运维层面明确禁用外部搜索”更直观。

---

## 5. 重要限制条件与影响（必须看）

### 5.1 限制条件（满足不了会失败/不生效）

1) **必须走 Responses API**
- 该方案只针对 `openai-responses` provider（`/responses`）。

2) **`base_url` 必须指向真正支持 built-in web search 的服务端**
- 你强调的“自定义 URL + API key”是允许的，但前提是你的 `base_url` 对应的服务端必须实现/转发 OpenAI 的内置 web search。
- 很多“只做 OpenAI 兼容转发”的第三方网关/代理，未必支持 `web_search` 这个 built-in tool；如果不支持，会返回 4xx 错误（例如 unknown tool type）。

3) **模型/账号能力限制**
- 并非所有模型都支持内置 web search；也可能受账号权限/区域影响。
- 失败表现：Responses 返回 `error` 或 `response.failed`（你们当前 `openai_responses` collector 会 fail-fast）。

### 5.2 影响（开启后你会看到什么变化）

1) **成本**：可能产生工具调用费用 + 更多 token（开启前要认可这一点）。

2) **延迟**：平均响应时间更长（因为需要联网检索）。

2.1) **是否一定会去搜？**：不一定。
- 你通常会把 `tool_choice` 设为 `"auto"`（你们当前实现就是），这意味着“模型自己决定是否调用 web search”。
- 如果你希望它更倾向于搜索，可以通过 system prompt / task prompt 明确要求“回答前先搜索并引用来源”。

3) **输出形态**：
- 流式文本仍然通过 `response.output_text.delta` 输出。
- 流中会出现 web search 的事件：
  - `response.web_search_call.in_progress`
  - `response.web_search_call.searching`
  - `response.web_search_call.completed`
  这些事件可以先忽略，不影响最终文本输出（官方 SDK 也不依赖它们来拼出 `output_text`）。

补充说明（事件里通常会带哪些字段）：
- `item_id`：该 web search tool call 对应的 item id
- `output_index`：对应输出数组索引
- `sequence_number`：事件序号（便于排序/去重）

4) **引用展示（citations/sources）**：
- OpenAI 会在输出中提供 citations/annotations；
- 但 Moltis UI 目前未必会把它们结构化渲染成“可点击引用列表”。
- 这不影响你“能联网搜并回答”，但会影响“引用体验”。

---

## 6. 修改后的实施方案（OpenAI Responses + 自定义 URL + API key）

### 6.1 配置形态（最终对用户暴露）

最小可用配置：

```toml
[providers.openai-responses]
enabled = true
api_key = "sk-..."                      # 或环境变量 OPENAI_RESPONSES_API_KEY
base_url = "https://api.openai.com/v1" # 你也可以换成自定义 URL，但必须支持 Responses + built-in web search

[providers.openai-responses.builtin_web_search]
enabled = true
# allowed_domains = ["openai.com", "docs.rs"]
# search_context_size = "medium"
# include_sources = false
```

### 6.2 请求体拼装规则（明确、可测试）

当 `enabled=true` 时，`openai-responses` provider 的 `/responses` body 应满足：

- `tools` 数组包含 built-in web search tool：`{"type":"web_search" ...}`
- 同时仍可包含 function tools：`{"type":"function","name":...,"parameters":...}`
- `tool_choice` 仍为 `"auto"`
- 若 `include_sources=true`，在 body 增加：`include: ["web_search_call.action.sources"]`

并且当 built-in 开启时：自动从 function tools 中过滤掉 name == `"web_search"` 的本地工具（避免歧义）。

关于 `include_sources`：
- 当 `include_sources=true` 时，`include` 中要加入 `"web_search_call.action.sources"`。
- 这是 Responses API 的通用机制（`include` 是一个字符串枚举数组），只对支持该 includable 的字段生效。

### 6.3 配置校验（避免误配）

在 `crates/config/src/validate.rs` 增加语义校验：

- 如果 `[providers.openai-responses.builtin_web_search]` 存在：
  - 若 `enabled=false`（或未显式设置，按默认 false）但还配置了 `allowed_domains/search_context_size/user_location/include_sources` → 报错（避免“以为生效其实没生效”）。

---

## 7. 代码落点清单（实现时会改哪些文件）

1) `crates/config/src/schema.rs`
- 给 `ProviderEntry` 增加 `builtin_web_search: Option<BuiltinWebSearchConfig>`（仅 `openai-responses` 使用，但字段存在于通用结构里）。

2) `crates/config/src/validate.rs`
- 更新 `build_schema_map()` 的 provider entry known keys，把 `builtin_web_search` 及其子字段纳入 unknown-field 检测。
- 增加 semantic 校验规则（见 6.3）。

> 注意：本仓库的 config unknown-field 检测不是靠 serde 的 `deny_unknown_fields`，而是靠 `build_schema_map()` 的已知键树。
> 也就是说：你新增了 schema 字段，必须同步更新 `build_schema_map()`，否则用户写了字段会被报 unknown-field。

3) `crates/config/src/template.rs`
- 在 `[providers.openai-responses]` 注释块下补充 `builtin_web_search` 示例与成本/延迟说明。

4) `crates/agents/src/providers/mod.rs`
- 构造 `OpenAiResponsesProvider` 时，把该 provider 的 `builtin_web_search` 配置传入（provider 实例需要拿到这个开关）。

5) `crates/agents/src/providers/openai_responses.rs`
- 在 `build_responses_body()` 中：
  - 即便 function tools 为空，只要 built-in enabled 也要写入 `tools`。
  - 合并 function tools（转换后）+ built-in web search tool。
  - built-in enabled 时过滤掉 function tool 名称 `web_search`（避免歧义）。
  - `include_sources=true` 时写 `include`。

6) （可选）`crates/agents/src/providers/openai_responses.rs` tests
- 增加一个 unit test：输入包含 `response.web_search_call.*` 事件，确保 collector 不崩并能生成最终 `output_text`。

---

## 8. 你会如何“管理开关”？（不做新 UI 也能用）

第一阶段建议直接通过 Settings → `Configuration` 页面编辑 TOML（你们已有 config get/validate/save 的管理入口）。

后续如果要在 provider 弹窗里加一键开关，也可以做，但属于 UI 增强，不是 MVP 的硬依赖。

---

## 9. 关键结论（给决策用）

- 字段最小集：`enabled / allowed_domains / search_context_size / user_location / include_sources`。
- 默认值：**全部默认关闭**；只有 `enabled=true` 生效；`include_sources` 默认 false。
- 不互斥：内置 web search 与其它 function tools 可以同时存在；但建议在 built-in 开启时自动过滤本地 function `web_search` 以避免歧义。
- 最大风险：自定义 `base_url` 未必支持 built-in web search；需要明确失败提示。

---

## Appendix: 官方字段名证据（便于核对/避免拼写错误）

> 这部分不是你必须理解的内容，只是给“实现时严格对齐字段名”用。

### A.0 web_search tool 参数名（Responses）

- `filters.allowed_domains` / `search_context_size` / `user_location`
  - openai-node `WebSearchTool`：
    - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L6316-L6383

### A.1 `include` 的 web search sources 字段名

- OpenAI Node SDK（`ResponseIncludable`）：包含 `web_search_call.action.sources` 和 `web_search_call.results`
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L2895-L2926

### A.1.1 自定义 base_url / 代理兼容性（SDK 证据）

- Node SDK 支持覆写 `baseURL`（含 `OPENAI_BASE_URL`）。
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/client.ts#L265-L270

### A.2 web search streaming event type 字符串

- OpenAI Node SDK（`ResponseStreamEvent` union）：
  - `response.web_search_call.in_progress`
  - `response.web_search_call.searching`
  - `response.web_search_call.completed`
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L5408-L5451

### A.3 失败模式（tool call status）

- web search tool call `status` 包含 `failed`。
  - https://github.com/openai/openai-node/blob/fe49a7b4826956bf80445f379eee6039a478d410/src/resources/responses/responses.ts#L2657-L2680

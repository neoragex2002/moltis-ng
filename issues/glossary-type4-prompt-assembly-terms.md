# Glossary — Prompt Assembly 术语表（Type4 / Providers）

Updated: 2026-03-04

## 背景（Background）
- 场景：system prompt / developer preamble 的拼接治理与排障中，经常出现“同一概念不同叫法”“同一叫法不同含义”。
- 约束：本文档**只在此处声明别名**；其它文档/issue 正文必须使用这里定义的“主称呼”。
- Out of scope：具体实现重构；本文是概念收敛的基础设施。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

### Source / Method（必须项）
- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包/trace 的权威值。
  - Why：排障与度量必须以权威值为准。
  - Not：不是本地推导/估算的结果。
  - Source/Method：authoritative

- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
  - Why：token 预算、compaction gating 等需要估算，但不能当“上游真实输入/输出”。
  - Not：不是“最终发出去的请求体”。
  - Source/Method：estimate

- **configured**：配置文件/磁盘文件里原始值（尚未合并/默认/过滤）。
  - Why：解释“用户写了什么”。
  - Not：不代表运行时一定生效。
  - Source/Method：configured

- **effective**：合并/默认/clamp/过滤后的生效值（本次运行实际采用的 inputs）。
  - Why：解释“本次运行到底用了什么”。
  - Not：不等于 as-sent（还没经过 provider adapter）。
  - Source/Method：effective

- **as-sent**：最终写入请求体、实际发送给上游 provider 的值/结构。
  - Why：排障的第一证据；必须能复现“到底发了什么”。
  - Not：不等于 UI 里展示的 prompt 文本（UI 可能展示的是 estimate 或内部中间形态）。
  - Source/Method：as-sent
  - Aliases（仅记录，不在正文使用）：wire, payload, request body

### Prompt 相关主概念
- **Prompt Assembly**（主称呼）：把 configured/effective 的 inputs 组合成 prompt 产物（messages 或字符串）的过程。
  - Why：是治理对象；混乱主要来源于“多入口重复拼接 + 多 provider 渲染差异”。
  - Not：不等同 provider adapter（adapter 是 assembly 之后的协议映射）。
  - Source/Method：effective → as-sent
  - Aliases（仅记录，不在正文使用）：prompt build, prompt concat

- **Prompt Product**（主称呼）：一次 assembly 的最终产物形态。
  - Why：不同 provider 产物形态不同（messages 数组、developer layers、模板字符串），必须先把“产物”定义清楚。
  - Not：不是“某一段 prompt 文本”。
  - Source/Method：as-sent

- **System prompt**（主称呼）：传统 chat-completions 风格的单段 system 文本（role=system 的内容）。
  - Why：多数 provider 仍以 system message 作为主要指令注入方式。
  - Not：不包含 user/assistant 的历史消息。
  - Source/Method：as-sent（对于 chat-completions 兼容 provider）

- **Developer message / developer preamble**（主称呼）：Responses API 里 role=developer 的前置指令消息。
  - Why：openai-responses 将“系统约束”放在 developer role；与 system role 的语义不同。
  - Not：不是 chat-completions 的 role=system。
  - Source/Method：as-sent（对于 openai-responses）
  - Aliases（仅记录，不在正文使用）：dev msg, dev prompt

- **Preamble**（主称呼）：对话历史之前注入的指令/上下文块（system/developer 均可）。
  - Why：统一称呼“最前面那些约束/上下文”。
  - Not：不包含历史消息本体。
  - Source/Method：as-sent 或 estimate（必须标注）

- **Layer**（主称呼）：把 preamble 按职责分层（例如 system/persona/runtime_snapshot）。
  - Why：帮助定义“哪段是什么、谁负责、如何 debug”。
  - Not：不是 provider 的 role。
  - Source/Method：effective → as-sent

- **Runtime snapshot**（主称呼）：本次运行的运行环境事实注入（provider/model/session/sandbox/tools/skills 等）。
  - Why：它是“事实信息”，应当优先级高、稳定、可追溯。
  - Not：不是用户可配置的 persona 文案。
  - Source/Method：effective

- **as-sent preamble / asSentPreamble**（主称呼）：用于 debug/trace 的“as-sent 证据链”，展示最终发给 provider 的 preamble 结构与文本。
  - Why：减少“我以为发的是 system，其实 provider 收到的是 developer”等误判。
  - Not：不是 token estimate 拼接字符串。
  - Source/Method：as-sent

### Provider / Adapter 相关
- **Provider**（主称呼）：上游模型服务实现（openai/anthropic/openai-responses/local-llm…）。
  - Why：不同 provider 的协议差异会改变 as-sent 形态。
  - Not：不是 model id。
  - Source/Method：configured / effective

- **Adapter**（主称呼）：把内部 `ChatMessage` 映射为 provider 协议请求体的实现。
  - Why：role mapping、system 抽取、tool schema 格式转换都在这里发生。
  - Not：不负责选择 persona/user/tool 的 inputs。
  - Source/Method：as-sent
  - Aliases（仅记录，不在正文使用）：provider glue, transport layer

- **Role mapping**（主称呼）：内部 role（system/user/assistant/tool）到 provider role/字段的映射。
  - Why：会导致“同一个 system message 在不同 provider 里语义不同”的情况。
  - Not：不等于 prompt layer。
  - Source/Method：as-sent

### Tools 相关
- **Native tools**（主称呼）：provider 原生支持 tool calling（schema 走 API 字段）。
  - Why：决定 prompt 里是否需要重复塞 JSON schema。
  - Not：不代表 gateway 运行时一定能同步执行工具（与 stream_only 无关）。
  - Source/Method：effective

- **Fallback tools / non-native tools**（主称呼）：provider 不支持原生工具，必须在 prompt 内说明 tool_call JSON 格式。
  - Why：是“如何调用工具”的文本规范来源。
  - Not：不代表工具一定可用（仍受 registry filters 影响）。
  - Source/Method：effective

- **Tool schema**（主称呼）：工具参数的 JSON Schema（name/description/parameters）。
  - Why：是 tool calling 的 contract。
  - Not：不包含运行环境（sandbox/no_network 等）。
  - Source/Method：authoritative（代码定义）

- **Tool registry**（主称呼）：本次运行可用的工具集合（可能经过过滤）。
  - Why：决定 runtime_snapshot / tool list。
  - Not：不等于“所有工具”。
  - Source/Method：effective

### 运行模式 / 执行路径
- **Entry point**（主称呼）：触发 prompt assembly 的入口（gateway chat / spawn_agent / debug endpoints）。
  - Why：入口去重是治理成本的关键。
  - Not：不是 provider adapter。
  - Source/Method：configured / effective

- **stream_only**（主称呼）：当前进程无法同步执行工具，prompt 构造会走简化路径。
  - Why：会影响是否注入 tools/skills、以及最终 as-sent preamble 的内容。
  - Not：不是 provider 是否支持流式输出。
  - Source/Method：effective

### 数据治理
- **SOT (Source of Truth)**（主称呼）：某条信息的唯一权威来源。
  - Why：多 SOT 必然导致漂移与排障困难。
  - Not：不是“可以 fallback 的可选来源”。
  - Source/Method：configured / authoritative

- **Derived view**（主称呼）：从其他权威数据源生成的只读视图文件（例如 `PEOPLE.md`）。
  - Why：可以给模型/人看，但不能反向当成配置权威。
  - Not：不是可编辑的 SOT。
  - Source/Method：effective

---

## 使用方式（How to Use）
- 本文档是术语权威表：其它 issue/docs 不要重复定义，直接引用术语并遵循这里的口径。
- 如果需要引入新术语：只在本文档增补，并更新 `Updated: YYYY-MM-DD`。

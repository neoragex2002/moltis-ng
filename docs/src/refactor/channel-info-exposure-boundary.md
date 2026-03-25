# V3 渠道信息暴露边界（Gateway / UI / Hooks / Tools）

本文档定义一件事：

- 在 V3（不改落盘）阶段，**TG 适配层之外**（gateway/core/ui/hooks/tools）允许看到哪些“渠道信息”，以及这些信息如何生成、如何流转、如何被限制。

本文档是实施级方案（可直接转成改造任务清单）。

配套文档：

- `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
- `docs/src/refactor/session-key-bucket-key-one-cut.md`
- `docs/src/refactor/telegram-adapter-boundary.md`
- `docs/src/refactor/channel-adapter-generic-interfaces.md`
- `docs/src/refactor/session-context-layering.md`

## 总原则（默认不暴露）

一句话：

- **渠道细节若不是必须由 core/ui/hooks/tools 知道，就全部收敛到渠道适配层。**

落地为三条硬规则：

1. **core 不识别渠道**：core 不解析 Telegram/Discord 私有字段，不通过前缀/字符串猜测渠道类型。
2. **投递细节不外溢**：`chat_id/thread_id/message_id/account_key/...` 这类“能定位平台对象”的字段，不进入 LLM 上下文、不进入 UI、hooks 默认不提供。
3. **唯一回投入口**：任何“发回渠道”的动作只允许走：
   - `session_id -> session metadata.channel_binding (opaque)`，或 turn 级 `reply_target_ref (opaque)` -> adapter/outbound

## 字段分层（3 层模型）

把所有“看起来像渠道信息”的字段按用途分为三层：

### 1) 跨渠道语义（core 允许知道）

用于会话语义、上下文、策略（跨渠道一致）：

- `session_id`：会话实例唯一 id（prompt cache / worktree / sandbox 默认分桶）
- `session_key`：逻辑桶 key（路由/分桶语义；替代 V2 “确定性渠道对话坐标”旧键）
- `chat_kind`：`direct | group`（跨渠道抽象）
- `addressed`：是否明确点名/触发意图（跨渠道抽象，由适配层计算）
- `mode`：`dispatch | record_only`（是否触发 LLM；可被策略/Hook/限流降级）
- `message_kind`：`text | voice | photo | location | ...`（跨渠道媒体类型）
- `text`：入站最终文本（由适配层提供；core 不再拼装 TG transcript）

> `addressed` 与 `mode` 不重复：
> - `addressed` 是“事实语义”（是否点名）
> - `mode` 是“动作决策”（是否跑 LLM）
> 允许出现 `addressed=true, mode=record_only`（点名但被策略降级）等组合，避免把系统逻辑写死。

### 2) 展示信息（UI/日志可见，但不用于路由/回投）

仅用于 UI footer 与调试展示（不应影响会话/路由/回投）：

- `channel.type`：例如 `"telegram"`
- `channel.senderName?` / `channel.username?`
- `channel.messageKind?`
- `channel.model?`（如 UI 需要）

UI 侧明确 **不需要**：

- `accountKey/chatId/threadId/messageId`
- `senderId/senderIsBot`
- `transcriptFormat`

### 3) 投递目标细节（强渠道内部信息）

用于把回复/工具结果准确发回平台（强渠道私有）：

- `account_key`
- `chat_id`
- `thread_id`（topic/thread）
- `message_id`（reply-to）
- 任何平台内部的 `sender_id`（例如 TG 数字 id）

这些信息 **只能存在于**：

- 渠道适配层内部对象；或
- session metadata 的 `channel_binding`（opaque 字符串，adapter 定义结构，gateway 仅用于回投）

## 关键契约（TG 适配层之外的目标形态）

本节列出 gateway/core/ui/hooks/tools 可见的“最终形态”（收敛版）。

### A) 入站主链（adapter -> gateway/core）

内部（Rust，snake_case）建议最小字段：

```text
inbound_envelope_v3 {
  session_id
  session_key
  chat_kind
  addressed
  mode
  message_kind?
  text
  attachments?        // 多模态载荷（如有）
}
```

约束：

- `text` 必须是“最终可给模型看的文本”：
  - DM：原文（或语音转写后的文本）
  - Group：使用 TG-GST v1（或当前 TG 侧既有群聊拼装格式）
- gateway/core 不再根据 Telegram 私有字段重写/拼装 `text`。

### B) 会话绑定（session_id -> channel_binding）

session metadata 保存（opaque）：

```text
channel_binding: String  // adapter 定义的 JSON 字符串（包含投递细节）
```

约束：

- gateway 可以把它交给 adapter/helper 的本地解析 helper，用于回投（outbound）或生成 `channelTarget`；但不得把它作为“通用渠道信息”向外透传。
- tools/hook/UI 不直接接触 `channel_binding` 原文。

补充：

- per-turn reply / typing / edit / topic/thread 等运行时回投，使用 adapter 私有 `reply_target_ref`。
- `reply_target_ref` 也是 opaque，不应展开成跨层公共字段。

### C) UI 消息展示（gateway -> WS/UI）

外部（JSON，camelCase）建议字段：

```text
channel {
  type
  senderName?
  username?
  messageKind?
  model?
}
```

来源与生成：

- 由 adapter 的入站请求提供展示字段（如 `sender_name/username/message_kind/model`），gateway 组装并做 sanitize 后挂到消息上。
- Web UI 自己发出的消息通常没有 `channel`（不是渠道入站）。

### D) Hooks（gateway -> hooks）

hooks 默认只拿跨渠道语义，避免泄露投递细节：

```text
hook payload {
  sessionId
  sessionKey?
  channel?            // "telegram" 等
  chatKind?
  addressed?
  mode?
  channelTarget?      // 默认 null（见下）
}
```

`channelTarget`（可空）是“可选展开的投递目标细节”，用于需要与外部系统做显式对接的 hook：

```text
channelTarget {
  type
  accountKey
  chatId
  threadId?
}
```

约束：

- `channelTarget` 的数据来源必须是 `session_id -> channel_binding -> adapter/helper 本地解析 helper` 的结果（由 gateway 填充）。
- schema 稳定：所有带 `sessionId` 的事件都应包含 `channelTarget` 字段（可为 `null`），避免事件间字段漂移。
- 默认策略：shell hooks 默认 `channelTarget=null`（最小暴露）；需要显式对接坐标的 hook 可通过 hook env 开启：
  - `MOLTIS_HOOK_INCLUDE_CHANNEL_TARGET=1`

### E) Tools（LLM -> tools -> gateway）

tool context（JSON）只允许：

- `_sessionId`（必需）
- `_sessionKey`（可选，仅用于跨渠道策略/分桶；不得用于回投定位）

任何“渠道交互能力”（示例：位置请求）一律按 `session_id` 路由：

```text
tools.location(_sessionId, ...) -> gateway.request_channel_location(session_id)
  -> gateway: session_id -> channel_binding
  -> adapter/outbound: 发送 TG 位置请求 / 按钮
```

失败与可观测性：

- 缺少 `channel_binding` 或渠道不支持：立即 `NotSupported`，并记录结构化日志 `reason_code`：
  - `missing_channel_binding`
  - `channel_not_supported`
  - `missing_session_id`

## 冗余字段清单（应从跨层契约中移除）

下列字段属于“渠道内部细节”，不应出现在 TG 适配层之外的契约对象中：

- `sender_id`（平台内部 id）
- `sender_is_bot`（平台侧判断结果）
- `transcript_format`（由适配层决定是否以及如何转写）
- `chat_id/thread_id/message_id/account_key`（投递细节）
- 任意 V2 legacy：旧“渠道对话坐标键”、旧工具上下文字段、旧渠道账号键等

## 实施清单（可直接拆任务）

1. TG-GST v1 转写职责回收：
   - TG adapter 产出最终 `text`（群聊使用 TG-GST v1/现有格式）。
   - gateway/core 删除基于 Telegram meta 的二次转写逻辑。
2. UI meta 收敛：
   - UI 仅保留 `channel.type/senderName/username/messageKind/model`。
   - 禁止任何投递细节出现在 WS/UI payload。
3. Hooks 收敛：
   - hooks payload 增补跨渠道字段（`sessionKey/chatKind/addressed/mode/channel`）。
   - `channelTarget` 作为可选展开字段（默认 null），来源仅 `channel_binding`。
4. Tools 收敛：
   - tool context 只用 `_sessionId/_sessionKey`。
   - 任何渠道交互能力仅按 `_sessionId` 路由；unsupported 立刻失败并带 `reason_code`。

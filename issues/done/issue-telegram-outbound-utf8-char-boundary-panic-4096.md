# Issue: Telegram 出站分片/截断在 UTF-8 非字符边界处 panic（4096 限制 / 中文场景）

## 实施现状（Status）【增量更新主入口】
- Status: DONE（2026-02-19）
- Priority: P0
- Components: telegram / gateway / outbound / streaming

**已实现（2026-02-19）**
- `chunk_message()` 按 UTF-8 字符边界切分，避免 `[..4096]` 字节切片 panic：`crates/telegram/src/markdown.rs:262`
- 流式 Edit-in-place 截断改用 UTF-8 安全截断（不再 `&html[..4096]`）：`crates/telegram/src/outbound.rs:608`

**已覆盖测试**
- Unicode 分片不 panic 且可 roundtrip：`crates/telegram/src/markdown.rs:360`
- UTF-8 安全截断行为：`crates/telegram/src/markdown.rs:378`

**已知差异/后续优化（非阻塞）**
- 目前仍按 **bytes** 计 `TELEGRAM_MAX_MESSAGE_LEN=4096`；Telegram 文档口径为“字符数”但实践中按 UTF-16/字符计的差异较小，且我们已保证不 panic（如需严格口径可后续再收敛）。

---

## 背景（Background）
- 场景：Telegram 渠道回包较长、且包含中文/Emoji 等多字节字符时，需要按 Telegram 上限（4096）做截断/分片；否则发送/编辑会失败。
- 约束：Rust `&str` 的切片索引是字节下标，必须落在 UTF-8 字符边界；否则会 panic。
- Out of scope：本单只修复 “panic/崩溃” 与 “UTF-8 边界安全”；不调整 Telegram HTML 渲染/转义策略。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **char boundary**：Rust UTF-8 字符边界（`str::is_char_boundary` 为真）；切片必须在边界上。
- **Telegram 4096 限制**：出站消息/编辑文本最大长度（常用口径 4096）；本仓库实现按 bytes 上限做 conservative 截断/分片，但必须保证不 panic。

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] Telegram 长文本分片/截断不得 panic（包含中文/Emoji）。
- [x] streaming edit-in-place 的中途截断不得 panic。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：任何用户内容都不得触发 `byte index ... is not a char boundary` 之类 panic。
  - 不得：不得用 `&text[..N]` 在未知字符集上直接截断。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) gateway 日志出现 channel reply join 失败，原因是 task panic：
   - `channel reply task join failed error=... panicked with message "byte index 4096 is not a char boundary; it is inside '按' ..."`
2) Telegram 渠道本次回复可能中断/丢失（panic 发生在 reply 发送任务内）。

### 影响（Impact）
- 用户体验：Telegram 端“长回复突然没有/只回一半/卡住”。
- 可靠性：panic 会导致该次 channel reply task 直接失败。
- 排障成本：表面看像“LLM/网络错误”，但根因是本地字符串切片 panic。

### 复现步骤（Reproduction）
1. 让 bot 在 Telegram 里输出一段 >4096 且包含中文的长文本（例如重复“按”）。
2. 触发出站分片（send_text / send_stream 途中 edit 截断）。
3. 实际：panic；期望：自动分片/截断，无崩溃。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 日志证据：
  - 关键词：`byte index 4096 is not a char boundary`、`channel reply task join failed`（join 失败处：`crates/gateway/src/chat.rs:5523` / `crates/gateway/src/chat.rs:5842`）
- 代码证据（修复后）：
  - `crates/telegram/src/markdown.rs:240`：`floor_char_boundary` / `truncate_utf8` 保证 slice 安全
  - `crates/telegram/src/markdown.rs:262`：`chunk_message` 分片逻辑使用 `floor_char_boundary` 保证 slice 安全
  - `crates/telegram/src/outbound.rs:608`：streaming edit 截断使用 `truncate_utf8`
- 当前测试覆盖：
  - 已有：见 “已覆盖测试”
  - 缺口：暂无（panic 路径已被单测覆盖）

## 根因分析（Root Cause）
- A. 触发：Telegram 需要 4096 限制分片/截断。
- B. 缺陷：实现使用字节切片 `&str[..4096]` / `&remaining[..max_len]`，当 4096 落在中文等多字节字符中间时 panic。
- C. 下游：panic 发生在异步发送任务内，gateway 只看到 join failed；Telegram 无法收到完整回复。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 任意 Unicode 文本分片/截断必须在 UTF-8 字符边界进行。
  - streaming edit-in-place 截断同样必须边界安全。
- 不得：
  - 不得对未知字符集直接使用字节下标切片。

## 方案（Proposed Solution）
### 最终方案（Chosen Approach）
- 增加 UTF-8 安全截断工具函数 `truncate_utf8()` / `floor_char_boundary()`，并在所有 4096 相关截断/分片点复用。
- `chunk_message()` 在计算 “hard slice” 与 “split_at” 时都进行字符边界下取整，保证不会 panic 且可 roundtrip。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 长中文文本（>4096）不会触发 panic，能正确分片发送。
- [x] 流式 edit-in-place 中途截断不会触发 panic。
- [x] 单元测试覆盖并通过。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `chunk_unicode_does_not_panic_and_roundtrips`：`crates/telegram/src/markdown.rs:360`
- [x] `truncate_utf8_respects_char_boundaries`：`crates/telegram/src/markdown.rs:378`

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用（属于 bugfix；不改变用户可见配置）。
- 回滚策略：回滚到旧实现会重新引入 panic（不建议）；如需回滚，应保留 UTF-8 安全截断。

## 交叉引用（Cross References）
- Related logs：`channel reply task join failed`（`crates/gateway/src/chat.rs:5523` / `crates/gateway/src/chat.rs:5842`）
- Related code paths：Telegram outbound（`crates/telegram/src/outbound.rs`）、markdown chunking（`crates/telegram/src/markdown.rs`）

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（UTF-8 边界安全）
- [x] 已补齐自动化测试
- [x] 回滚策略明确

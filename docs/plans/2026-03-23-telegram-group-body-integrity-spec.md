# Telegram 群聊正文完整性规范

**状态：** 已冻结，可进入实现评审

**范围：** 仅限 Telegram 适配层（`crates/telegram/*`）中的群聊 planner 正文透传语义

**不在范围内：** 不处理 bot 到 bot 扩散保险丝（该内容见独立规范）；不修改 gateway/core；不改动非 Telegram 渠道

---

## 1. 目标

修复 Telegram 群聊 planner 在 `Dispatch` / `RecordOnly` 时按行首点名片段切正文的问题。

冻结后的要求是：

- 行首点名只用于目标判定
- 正文原则上原文透传
- planner 不再通过切段、重排、压缩正文去改变消息语义

---

## 2. 问题定义

当前 Telegram planner 会提取行首点名片段，并只把这些片段转发给 gateway。这会改变原消息正文的原始语义。

当前风险：

- 某个 bot 收到的可能只是整段规范说明中的“坏例子片段”。
- 一条本来是在解释政策 / 对比反例的消息，可能被错误变形成一条直接指令。
- planner 同时做了“目标识别”和“正文重写”两件事，违反了预期的适配层边界。

---

## 3. 设计原则

1. planner 只负责判定：
   - target eligibility
   - `addressed`
   - `mode`（`Dispatch` 或 `RecordOnly`）
2. planner 不重写正文语义。
3. 行首点名是目标识别信号，不是正文切段边界。
4. 多目标命中的情况下，不允许出现按目标分别裁切的正文变体。
5. 正文语义必须在 Telegram adapter handoff 前就稳定，不能把“切段修补”丢给 gateway。

---

## 4. 核心规则

Telegram 群聊 `Dispatch` / `RecordOnly` 的 body 原则上必须保持原始消息正文。

这里的“原始消息正文”指的是 Telegram adapter 在做目标判定之前拿到的那一份统一源正文，不是按目标派生出来的局部片段。

这里冻结的是“语义正文完整性”，不是在讨论是否保留现有转写链路中的外层封套。

如果当前 Telegram 转写协议在 body 外层已有固定 envelope（例如 `发送者 -> you:` 这类标识），本规范只要求：

- envelope 内承载的正文不能再被按目标切片
- 不得借“正文完整性”之名顺手删改既有 envelope 语义
- envelope 是否存在、如何渲染，继续由既有转写协议单独负责，不在本规范内扩写

行首点名只决定：

- 哪些目标被命中
- 该目标是否 `addressed`
- 该目标进入 `Dispatch` 还是 `RecordOnly`

行首点名不决定：

- body 切段范围
- body 删除哪些段落
- body 如何为不同目标生成不同文本

---

## 5. 允许的归一化

允许的清洗必须严格收窄为：

- 对整条消息 body 在最外层做一次 `trim`
- 保留内部段落
- 保留内部换行
- 保留 mention 顺序
- 保留目标行前后的解释性上下文

这是上限，不允许再扩大。

这里的“对 body 做最外层 `trim`”指的是对语义正文载荷本身做处理，不包括借机重写、移除或重排外层 envelope。

---

## 6. 禁止的重写

planner 不得：

- 按行首 mention 把消息切成多个片段
- 因为后文又出现另一个目标 mention 而丢掉解释性段落
- 为不同目标生成不同的正文切片
- 重排段落
- 把多段内容压成合成摘要
- 只保留“命中的那一段”并丢弃上层语境

---

## 7. 多目标行为

如果同一条原始 bot 消息可以派发到多个目标，则每个目标收到的都应该是同一份原文，而不是针对各目标裁出的不同切片。

planner 的按目标输出允许变化的只有：

- `target`
- `addressed`
- `mode`
- `reason_code`

planner 的按目标输出不允许在正文语义上发生变化，除了最外层 `trim` 之外。

---

## 8. 示例场景

### 示例 A：规范说明 + 坏例子

输入：

```text
@cute_alma_bot ...（前文规范说明）

我会刻意避免的错误写法（示例）
@cute_alma_bot @lovely_apple_bot 我先说下：我做了一半，等会再补。
（问题：...）
```

当前错误行为：

- planner 只截取后面的 mention 片段
- `@lovely_apple_bot` 收到的只有坏例子片段

修复后的要求：

- 目标识别仍然可以判断 `@lovely_apple_bot` 是被命中的目标之一
- 下游 body 必须保持原始消息正文，从而保留“这是一个坏例子”的上层语境

### 示例 B：同一条消息命中多个目标

输入：

```text
@bot_a 你负责日志
@bot_b 你负责配置
下面是统一背景、边界和注意事项...
```

修复后的要求：

- `@bot_a` 与 `@bot_b` 的目标判定可以不同
- 但两者拿到的 body 都必须是同一份完整原文
- 不能出现 `bot_a` 只拿第一段、`bot_b` 只拿第二段的按目标切片

---

## 9. 适配层归属

这个修复属于 Telegram 适配层 / 运行时边界。

预期归属面：

- Telegram 入站 / 出站规划
- Telegram adapter helper
- Telegram 测试

明确不允许：

- 在 gateway/core 中新增 Telegram 群聊正文修补逻辑
- adapter handoff 后再做按目标切正文

---

## 10. 必备测试

实现至少必须覆盖：

1. “规范说明 + 坏例子”这类消息在 dispatch / record 时不会再被切片。
2. 多目标收到的是同一份原文，不会出现按目标裁切的正文变体。
3. `Dispatch` 与 `RecordOnly` 之间的差异不影响 body 是否保持原文。
4. 允许的最外层 `trim` 不会破坏内部段落和换行。

---

## 11. 收敛性评估

这份规范已经足够收敛，可以直接进入实现。

剩余空间只属于低价值实现细节，例如：

- 当前 planner helper 具体如何删掉 segment extraction
- body 原文在 inbound / outbound 路径中由哪个局部 helper 统一保留
- 测试夹具如何最小化复用

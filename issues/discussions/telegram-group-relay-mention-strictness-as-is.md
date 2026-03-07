# Telegram 群聊 bot→bot Relay：Strict/Loose 点名规则（as-is，人话版 + 示例）

> 本文只讲 **Telegram 群聊里 bot 输出文本 → 触发另一个 bot 推理** 的 “relay 点名”机制（bot→bot）。  
> 不讨论 slash 命令；也不等同于“群里人 @ bot 让它回复”的 `mention_mode`（那是人→bot 的唤醒策略，另一个维度）。

## 1. 这套机制到底在解决什么？

Telegram 不支持 bot 直接私聊另一个 bot。为了实现“群里多 bot 协作”，当前实现采用 **文本扫描 + 服务器侧转发**：

- bot A 在群里发消息，里面写了 `@botC ...`（像在点名派活）
- gateway 扫描到这个点名后，把一条“relay 注入消息”写进 bot C 对应的 session，并触发 `chat.send(...)`
- bot C 进行一次 LLM 推理（如果它当时正忙，就进入队列，稍后重放）

## 2. 关键名词（先对齐）

- **relay**：从 bot A 的群消息里识别出“给另一个 bot 的指令”，并触发目标 bot 的推理。
- **Strict / Loose**：`relay_strictness`，决定“哪些 @ 点名算指令、值得触发 relay”。
- **触发推理**：最终会调用目标 bot 的 `chat.send(...)`，因此会真的跑一次 LLM（或入队排队）。
- **入队排队**：同一 session 正在推理时，新触发不会并行跑，会入队，等当前 run 结束后按 Followup/Collect 策略重放。

## 3. 共同前置条件（Strict/Loose 都一样）

无论 Strict 还是 Loose，只有同时满足这些前置条件，才可能触发 relay：

1) **必须是 Telegram 群聊**  
   - 只有 `chat_id` 为负数的群聊才会扫描/触发 relay。私聊 DM / 频道不会走这条 bot→bot relay 分支。

2) **必须能识别出“@某个 bot 的用户名”**（不是随便 @ 谁都算）  
   - 只认 `@` + `[a-zA-Z0-9_]` 的 token（长度 3～32），例如 `@bot_123`。  
   - 只有当这个用户名命中“系统已知的 bot 用户名列表”（bus accounts snapshot）时，才认为它是可 relay 的目标。  
   - `@all/@here/@everyone` 这种广播提及会被忽略。

3) **必须有任务文本（task_text）**  
   - `@c`、`@c ，`、`@c ...`（只有空白/标点）都不算“派活”，不会触发 relay。  
   - 换句话说：必须能抽取出“点名之后，你要它干什么”这段文字。

4) **会跳过“看起来像引用/示例/代码”的区域**  
   - 代码块（``` fenced code ```）里的 @ 会忽略。  
   - 引用行（以 `>` 开头）里的 @ 会忽略。  
   - 行内代码（`` `...` ``）里的 @ 会忽略。  
   目的：避免“教程/引用/复述”触发一堆 bot。

5) **目标 bot 的该群 session 必须已存在**（防幽灵会话）  
   - 如果目标 bot 在该群对应的 session 不存在，会直接跳过（不触发推理）。

## 4. Strict：什么样的点名才会触发 relay？

一句话：**Strict 只认“行首点名”是指令，其它位置的 @ 一律当引用/装饰，不触发 relay。**

### 4.1 “行首点名”是什么意思？

“行首”不是指整条消息的开头，而是指 **某一行** 的开头：  
在这个 `@` 之前，这一行只能有空白字符（允许缩进空格）。

### 4.2 Strict 会触发的例子

1) 单个 bot 派活（最标准）
```
@c 请执行 X
```

2) 允许缩进
```
    @c 请执行 X
```

3) 一行点名多个 bot，给同一个任务（相邻 @ 之间只有空白/标点会被认为是“同一组点名”）
```
@a @b @c 请执行 X
```
含义：会对 a/b/c 各自触发一次 relay（各跑各的）。

4) 多段派活（按“点名组”分段）
```
@a 请做 X；@b 请做 Y
```
含义：会拆成两组：给 a 的 X、给 b 的 Y。

### 4.3 Strict 不会触发的例子（最常见误解）

1) 非行首点名（Strict 下永远不算指令）
```
请 @c 执行 X
```

2) 只有点名，没有任务文本（会被过滤）
```
@c
```

3) 引用/代码里的点名（会被跳过）
```
> @c 请执行 X
```
或
```
这里是示例：`@c 请执行 X`
```

## 5. Loose：什么样的点名才会触发 relay？

一句话：**Loose = 行首点名仍然必 relay；非行首点名则“先问一次 LLM 做分类”，只有被判为 directive 才 relay。**

### 5.1 Loose 对“非行首点名”的额外流程

当点名不在行首时：

- gateway 会把这条点名的上下文（包含：点名所在行、点名 token、抽取到的任务文本）打包
- 调用一次内部 LLM 分类器，让它把每个点名标成：
  - `directive`：作者是在派活（触发 relay）
  - `reference`：作者只是提到/举例/引用（不触发）
- **失败/解析失败时默认当 reference**（宁可少触发，也不乱触发）

### 5.2 Loose 会触发的例子（非行首也可能触发）

1) 句中派活（如果分类器判为 directive）
```
麻烦 @c 帮我把 X 做一下
```

2) 解释里夹带指令（仍可能触发，取决于分类）
```
我们现在需要 @c 负责 X，其他人先等等
```

### 5.3 Loose 不会触发的例子（典型 reference）

1) 教程/说明/引用（通常会被判为 reference；且代码/引用本身也会被 sanitize 跳过）
```
例如你可以这样写：@c 请执行 X
```

## 6. 什么时候会“触发推理排队”（queue）？

只要某次 relay 最终决定要触发 `chat.send(...)`：

- 如果目标 bot 的该 session **当前没有 run 在跑** → 立即开始一次推理
- 如果目标 bot 的该 session **当前有 run 在跑** → 这次触发会 **入队**（queueing），稍后重放

你在日志里通常会看到类似语义：
- `queueing message (run active)`（入队）
- run 结束后：
  - Followup：逐条重放 queued
  - Collect：把 queued 合并后重放一次

## 7. 最简“判断清单”（你看到文本就能预判会不会触发）

对一条 bot A 的群消息，问自己 5 个问题：

1) 这是群聊吗（chat_id 负数）？
2) 消息里有没有 `@xxx`，并且 `xxx` 是已知 bot 用户名？
3) 这个 @ 在引用/代码块/行内代码里吗（如果是，忽略）？
4) 点名后面有没有“要它干什么”的任务文本？
5) 严格度是 Strict 还是 Loose？
   - Strict：必须是行首点名
   - Loose：行首点名必触发；非行首要过一次 LLM 分类器（directive 才触发）

满足后才会：**触发目标 bot 推理；若目标 bot 正忙则入队。**

## 8. 代码定位（方便你对照实现）

- relay 主逻辑（解析/Strict vs Loose/LLM 分类）：`crates/gateway/src/chat.rs:6626`
- 提取点名分组 + 行首判定 + task_text 抽取：`crates/gateway/src/chat.rs:6495`
- 真正触发目标 bot 推理（dispatch relay → `chat.send`）：`crates/gateway/src/chat.rs:6571`
- `chat.send` 入队条件（session run active）：`crates/gateway/src/chat.rs:2350`
- Telegram bot 侧配置项（`relay_strictness`、`mention_mode`）：`crates/telegram/src/config.rs:7`

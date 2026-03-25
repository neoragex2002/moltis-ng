# Issue: Telegram 群共享运行时误把 bot-local `message_id` 当成 shared key（mirror / relay / reply-target / dedupe）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-25
- Checklist discipline: 每次增量更新除补“已实现 / 已覆盖测试”外，必须同步勾选正文里对应的 checklist；禁止出现文首已完成、正文 TODO 未更新的漂移
- Owners: Codex
- Components: telegram / gateway / docs
- Affected providers/models: N/A

**已实现（如有，必须逐条写日期）**
- 2026-03-25：`crates/telegram/src/state.rs` 已 one-cut 收敛为 `participants + author_bindings + namespaced dedupe`；`GroupLocalMessageRef` 成为共享运行时唯一消息身份，旧 root / budget / fuse / lineage 运行时状态与 API 已删除
- 2026-03-25：`crates/telegram/src/adapter.rs` 已删除 `lineage_message_id` 与相关 decode 旁路；`reply_target_ref` 只保留 direct-delivery 坐标，legacy `lineageMessageId` 形状直接拒绝
- 2026-03-25：`crates/telegram/src/handlers.rs` / `crates/telegram/src/outbound.rs` 已统一切到 observer/source scoped dedupe key，并改为按 `local_msg_ref` 做 reply 作者绑定
- 2026-03-25：`crates/config/src/{telegram.rs,template.rs,validate.rs,schema.rs}`、`crates/telegram/src/plugin.rs`、`crates/gateway/src/server.rs` 已删除 `bot_dispatch_cycle_budget` 全链路入口；不保留兼容
- 2026-03-25：`crates/gateway/src/chat.rs` 与 Telegram 入/出站日志已收敛到 `channel_turn_id + channel_type + bucket_key + reply_target_ref_hash + local_msg_ref + text_preview` 合同，不再以完整正文为 INFO 主索引

**已覆盖测试（如有）**
- `cargo test -p moltis-config -q`
- `cargo test -p moltis-telegram -q`
- `cargo test -p moltis-gateway --lib -q`
- 关键新增/更新用例：
  - `crates/telegram/src/handlers.rs:4986` `resolve_reply_to_target_account_uses_observer_local_message_ref`
  - `crates/telegram/src/handlers.rs:5033` `inbound_handoff_does_not_collide_with_existing_outbound_plan_key`
  - `crates/telegram/src/handlers.rs:5114` `inbound_dispatch_log_contains_local_msg_ref_and_preview`
  - `crates/telegram/src/outbound.rs:4160` `group_visible_outbound_log_contains_local_msg_ref_and_preview`
  - `crates/telegram/src/outbound.rs:3884` `send_text_by_reply_target_ref_registers_source_author_binding_for_group_send`
  - `crates/gateway/src/chat.rs:7666` `gateway_log_text_preview_collapses_whitespace_and_truncates`
  - `crates/gateway/src/chat.rs:7672` `first_reply_target_ref_hash_uses_channel_turn_context`
  - `crates/gateway/src/chat.rs:7698` `session_channel_binding_fields_reads_channel_type_and_bucket_key`

**已知差异/后续优化（非阻塞）**
- 本单自动化闭环已完成；真实 Telegram 三 bot 群烟测仍建议按正文手工步骤上线前复核
- 仓库当前仍存在与本单无关的 `cargo test -p moltis-gateway` integration 基线失败（`spawn_agent_openai_responses`），本单未扩 scope 处理该问题

---

## 背景（Background）
- 场景：Telegram 群里 3 个受管 bot 同时参与 mirror / relay / group-visible fanout；现场出现“某些消息某些 bot 能收到，另一些 bot 收不到”的不稳定现象。
- 约束：
  - 必须遵循 strict one-cut：不保留 fallback、alias、silent degrade。
  - 修复必须收敛在 Telegram 适配层共享运行时 owner 内闭环，不把 Telegram 专属复杂性外溢到 gateway/core 通用层。
  - 必须把“消息标识是谁的真值”讲清楚；不能再默认把 Telegram `message_id` 当成跨 bot 可直接比较的键。
  - 可观测性改造必须优先复用现有 `reply_target_ref` / `channel_turn_id` 承载链；若现有 carrier 足够，不得为了 Telegram 再给 gateway/common 层新增专属 `local_msg_ref` 字段。
  - `reply_target_ref` 自身也必须 one-cut 去除旧隐藏链路残留；不得继续夹带 `lineage_message_id` 一类非直接投递真值字段。
- Out of scope：
  - 不顺手重做群聊 planner 规则本身。
  - 不扩到非 Telegram 渠道。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **bot-local `message_id`**（主称呼）：某个 Telegram bot 账号在自己视角下看到的群消息编号。
  - Why：这是当前缺陷的根源；它只在“这个 bot 的这份 update / reply 视图”内可用。
  - Not：不是整个群的全局消息 id，不是跨 bot 可直接比较的真值；该点已被 2026-03-25 现场 3 bot 同收一条人类消息、但 `message_id` 各不相同的日志直接证实。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：local Telegram message id

- **`local_msg_ref`**（主称呼）：共享运行时里唯一合法的 Telegram 消息引用，严格表达“哪个 bot 在哪个 chat 里看到的哪条本地消息”。
  - Why：如果运行时要保存作者、reply 目标或去重，它至少必须区分“哪个 bot 看到的哪条消息”。
  - Not：不是 session id，不是跨 bot 全局消息 id，不是隐藏协作链 id。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：delivery ref / message ref

- **作者绑定**（主称呼）：`local_msg_ref -> managed_author_account_handle` 的最小运行时事实，用于 reply-to 目标 bot 识别。
  - Why：reply-to 激活只需要知道“这条本地消息是谁发的 bot”，不需要 root、hop、budget 或隐藏链路。
  - Not：不是消息全文上下文，也不是 root lineage。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：message author binding

- **dedupe 命名空间**（主称呼）：去重键所属的独立作用域，至少要区分方向、账号视图和业务对象。
  - Why：去重只能去“同一件事”，不能把不同方向/不同 bot 的消息误判成一件事。
  - Not：不是一个全局大桶。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：dedupe namespace

- **authoritative**：来自 Telegram update / send 回包的真实字段。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给下游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 冻结 Telegram 群共享运行时的唯一消息身份合同：`local_msg_ref = (account_handle, chat_id, message_id)`
- [x] 把 inbound dedupe、outbound plan dedupe、reply-target 作者绑定、group-visible outbound 登记 全部统一到该合同
- [x] 删除 `RootId` / `root_message_id` / `inherited_root_message_id` / `admit_managed_dispatch` / hop limit / dispatch fuse / root budget 整套旧语义与配置入口
- [x] 同步冻结 agent 对话链日志合同，确保 Telegram inbound -> gateway -> agent -> outbound 可结构化串联

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把“bot-local `message_id` 不是 chat-global key”定为硬规则。
  - 必须把 dedupe、message context、message author、reply-target 统一纳入同一份消息标识合同。
  - 必须把 Telegram 群聊协作的唯一真源收敛为“群消息表面语义 + `local_msg_ref`”；不得再保留隐藏 root/lineage 语义。
  - 不得只补 dedupe 表面问题而继续保留 `message_context/root_message_id`、`inherited_root_message_id`、`admit_managed_dispatch` 这类旧链路污染。
  - 不得依赖“不同 bot 看到的 Telegram `message_id` 恰好一致”这种隐含假设。
- 兼容性：按 one-cut 处理；旧的错误键形状不做 silent 兼容。
- 可观测性：
  - 后续实现必须补齐结构化日志，至少能看出 `local_msg_ref`、dedupe namespace、reply-target 解析命中/缺失、错误降级原因。
  - Telegram inbound -> gateway `chat.send` -> `agent run complete` -> `channel reply delivery` -> Telegram outbound 这一整条 agent 对话链，必须能用结构化字段串起来排障，不能依赖读完整正文/回复文本去“猜这是不是同一轮”。
  - 外层日志输出形态必须总体沿用现有 tracing 文本风格：`<timestamp> <LEVEL> <target>: <message> key=value ...`；本单不做日志系统格式迁移，只优化 message 文案与字段集合。
- 测试：
  - 必须坚持核心路径测试覆盖原则：优先覆盖行首点名 dispatch、reply-to dispatch、跨 bot dedupe 撞号、reply-target 作者绑定、日志链路可追踪；不得继续为已删除的 root/hop/fuse 语义堆兼容测试。
- 安全与隐私：日志不得打印完整正文；只记录 chat/account/message/ref 等排障所需坐标。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一条群消息在多 bot 群里会出现“部分 bot 收到、部分 bot 被吞”的现象，表现为 mirror / relay 不稳定。
2) 现场日志已经出现明确的 `tg_dedup_hit` 假阳性：某个 bot 的真实 inbound 群消息，在进入 gateway handoff 前就被当成重复事件丢掉。
3) 该问题不是单纯 mirror/relay 规则误判，而是共享运行时把多个 bot 视角下的 Telegram `message_id` 混进了同一个 key 空间。
4) 最新现场日志进一步证明：同一条人类群消息会被 3 个 bot 正常各自收到、各自 dispatch、各自进入各自 session，但它们看到的 `message_id` 不同；因此问题不在 `chat_id` / `peer_id` / addressed 判定，而在共享运行时错误复用了 bot-local `message_id`。
5) 进一步实验还证明：每个 bot 在同一群里各自维护独立的本地 `message_id` 序列，且该序列由 inbound / outbound 共用；因此“裸 `message_id`”不仅不能跨 bot 复用，连“跨方向共桶”也天然不成立。

### 影响（Impact）
- 用户体验：
  - mirror 消息会丢。
  - relay / 被点名激活会随机失效。
  - 多 bot 协作看起来像“偶现不稳定”。
- 可靠性：
  - 去重会吞掉不该吞的消息。
  - 作者归属、reply 目标识别以及旧 root/fuse 路径都会读到错的上下文。
  - 同一条外部人类消息在多 bot 侧会与不相关消息撞桶。
- 排障成本：
  - 目前日志只看到 `tg_dedup_hit` / `tg_record_context` / `tg_dispatch_promoted_from_record`，但看不出是“同方向重复”还是“跨 bot / 跨方向污染”。
  - 当前 gateway 侧 `chat.send` / `agent run complete` / `channel reply delivery starting` 日志仍偏“全文本+弱关联”：
    - `crates/gateway/src/chat.rs:2505` 直接打印完整 `user_message`
    - `crates/gateway/src/chat.rs:5413` 直接打印完整 `response`
    - `crates/gateway/src/chat.rs:6554` 的 `channel reply delivery starting` 只有 `session_id` / `trigger_id` / `target_count` / `text_len`，没有 `reply_target_ref_hash` / carrier 维度
  - 这导致排障时虽然能“看懂聊天内容”，但仍然很难严谨回答：某条 Telegram inbound 到底触发了哪个 `chat.send`、落到了哪个 agent session、又对应了哪条 outbound reply / group outbound plan。

### 复现步骤（Reproduction）
1. 在同一个 Telegram 群里接入 3 个受管 bot。
2. 让 bot A 发一条 group-visible 消息，触发对 bot B 的 mirror/relay fanout。
3. 之后由人类在群里再发一条新消息，恰好在 bot B 视角下使用到与上一步 fanout 相同的本地 `message_id`。
4. 期望 vs 实际：
   - 期望：这是两件不同的事；真实 inbound 必须继续进入 planner / gateway。
   - 实际：真实 inbound 在 Telegram adapter/gateway handoff 前被打成 `tg_dedup_hit` 直接丢弃。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 现场日志证据：
  - 2026-03-25 现场日志里，`source_account_handle="telegram:8576199590"` 发出的 outbound fanout 曾向 `target_account_handle="telegram:8344017527"` 写入 `message_id="1941"` 的 `telegram.group.outbound_plan`
  - 随后同一群里，`account_handle="telegram:8344017527"` 收到一条真实人类 inbound，日志里它自己的本地 `message_id` 也恰好是 `1941`
  - 该 inbound 紧接着被记录为 `reason_code="tg_dedup_hit"` 并在 gateway handoff 前 drop
  - 同一条人类消息在另外两个 bot 视角下的本地 `message_id` 又分别是 `1944`、`1523`，进一步证明“同一真实群消息在不同 bot 侧并不共享同一个 `message_id`”
  - 2026-03-25 `08:26:48` 的另一组现场日志给出了更直接的 3 bot 并行证据：
    - `account_handle="telegram:8704214186"` 收到 `chat_id=-5288040422`、`message_id=1525`、`peer_id="8454363355"`、`username=Some("Neoragex2002")`、`sender_name=Some("neo")`、`kind=Some(Text)`、`text_len=73`
    - `account_handle="telegram:8576199590"` 收到同一 `chat_id=-5288040422`、但 `message_id=1945`；其余 `peer_id` / `username` / `sender_name` / `kind` / `text_len` 一致
    - `account_handle="telegram:8344017527"` 收到同一 `chat_id=-5288040422`、但 `message_id=1942`；其余 `peer_id` / `username` / `sender_name` / `kind` / `text_len` 一致
  - 同一组日志里，3 路 inbound 随后都被正常判定为 `reason_code="tg_dispatch_line_start_mention"`，且 `bucket_key=group-peer-tgchat.n5288040422` 一致：
    - `telegram:8576199590` -> `session=sess_755340bbdeb242be899bfc991dc0b940`
    - `telegram:8344017527` -> `session=sess_ff8884410db84c939e907d5710773810`
    - `telegram:8704214186` -> `session=sess_77d686882a9a4063966c3dbe33e3b722`
  - 随后 3 路 `chat.send` 的 `user_message` 文本一致，均为 `neo -> you: @cute_alma_bot @fluffy_tomato_bot @lovely_apple_bot 大家好，我是neo`
  - 结论：在当前部署形态下，`chat_id`、`peer_id`、`username`、`sender_name`、`kind`、`text_len`、`bucket_key`、addressed 判定都表现一致；真正分叉的是各 bot 视角下的本地 `message_id`
  - 2026-03-25 `09:12:33` 到 `09:13:39` 的单 bot 点名实验进一步确认了 `message_id` 的序列规则：
    - `telegram:8344017527` 在同一群里的本地序列表现为 `1950(inbound) -> 1951(outbound) -> 1952(inbound) -> 1953(inbound) -> 1954(inbound) -> 1955(outbound)`
    - `telegram:8576199590` 在同一群里的本地序列表现为 `1952(inbound) -> 1953(outbound) -> 1954(inbound) -> 1955(inbound) -> 1956(outbound) -> 1957(inbound)`
    - `telegram:8704214186` 在同一群里的本地序列表现为 `1533(inbound) -> 1534(outbound) -> 1535(inbound) -> 1536(outbound) -> 1537(inbound) -> 1538(inbound)`
  - 该实验直接说明两件事：
    - 每个 bot 在同一群里有自己的本地 `message_id` 序列，序列彼此独立，不存在“群共享全局 id”
    - 对单个 bot 而言，inbound / outbound 共用同一条本地 `message_id` 序列
  - 同一实验还给出了两条双向污染的直接示例：
    - 示例 A（`outbound -> inbound` 污染）：
      - `09:12:45`，`source_account_handle="telegram:8576199590"` 的 outbound 记录向共享运行时写入 `message_id="1953"`
      - `09:13:23`，`account_handle="telegram:8344017527"` 收到一条真实人类 inbound，自己的本地 `message_id=1953`
      - 该真实 inbound 随即被记录为 `reason_code="tg_dedup_hit"` 并 drop
    - 示例 B（`inbound -> outbound` 污染）：
      - `09:13:23`，`account_handle="telegram:8576199590"` 收到一条真实人类 inbound，本地 `message_id=1955`
      - `09:13:39`，`source_account_handle="telegram:8344017527"` 发出 outbound，其本地 `message_id="1955"`
      - 面向 `target_account_handle="telegram:8576199590"` 的 `telegram.group.outbound_plan` 随即被记录为 `reason_code="tg_dedup_hit"` 并 drop
  - 结论补充：当前缺陷不是单向“出站污染入站”，而是入站 / 出站在共享 dedupe 键空间里双向串桶
  - 2026-03-25 `09:32:13` 到 `09:32:49` 的 DM 侧日志给出一条一致性侧证：
    - 同一账号 `telegram:8704214186` 在 DM（`chat_id=8454363355`、`bucket_key=dm-main`）中连续收到 inbound `message_id=1539`、`1541`、`1543`
    - 每次 inbound 都触发同一个 `session=sess_03f1acb495c64c60b1c2d85b759737a1` 的 `chat.send`，并随后发出一条 reply_to 对应 inbound 的 outbound 回复
    - 虽然 outbound send 日志未直接打印 outbound 自身的 `message_id`，但该形态与群聊实验高度一致：inbound 编号之间夹着 outbound 回复，符合“单账号本地序列由 inbound / outbound 共用”的观察
    - 该 DM 样本未出现 dedupe 错杀；它在本单中的作用是验证 `message_id` 的本地序列语义，而不是证明 DM 自身存在同类 bug
- 代码证据（修复前历史问题定位）：
  - `crates/channels/src/plugin.rs:89`：gateway 现有通用入站上下文只有 `bucket_key + reply_target_ref + channel_binding`，没有 Telegram 专属 `local_msg_ref`
  - `crates/gateway/src/channel_events.rs:43`、`crates/gateway/src/channel_events.rs:47`：Telegram inbound 进入 gateway 时，已经把 `(account_handle, chat_id, message_id)` 编进 `reply_target_ref`
  - `crates/gateway/src/channel_events.rs:326`、`crates/gateway/src/channel_events.rs:374`：Telegram adapter -> gateway 的 inbound 文本 `inbound_text` 仍作为真实业务载荷进入 UI 广播与 `chat.send`
  - `crates/gateway/src/channel_events.rs:360`、`crates/gateway/src/channel_events.rs:367`、`crates/gateway/src/state.rs:541`、`crates/gateway/src/state.rs:581`：gateway 已有 `channel_turn_id(trigger_id) -> reply_target_ref` 的会话内承载链
  - `crates/telegram/src/adapter.rs:1023`、`crates/telegram/src/adapter.rs:1052`：Telegram `reply_target_ref` 现状即可无损承载并反解本地 `(account_handle, chat_id, message_id)`，不需要再发明第二套 cross-layer carrier
  - `crates/telegram/src/adapter.rs:970`、`crates/telegram/src/adapter.rs:1004`、`crates/telegram/src/adapter.rs:1066`：`TelegramOutboundTargetRef.lineage_message_id`、`reply_target_ref_for_target_with_lineage()` 与 decode 时的 `v1.lineage_message_id.or(v1.message_id.clone())` 仍是旧隐藏链路残留，需要随本单一并删除
  - `crates/telegram/src/outbound.rs:2311`：gateway -> Telegram 的 outbound 文本仍通过 `send_text_by_reply_target_ref_with_ref(..., text)` 原样作为真实业务载荷发送
  - `crates/telegram/src/outbound.rs:704`：outbound 去重键只包含 `account_handle + chat_id + message_id`
  - `crates/telegram/src/outbound.rs:864`：outbound fanout 在 gateway handoff 前使用上述键做 dedupe
  - `crates/telegram/src/handlers.rs:3860`：inbound 去重键也采用同样形状
  - `crates/telegram/src/handlers.rs:892`：inbound 群消息在 handoff 前同样进入共享 dedupe cache
  - `crates/telegram/src/state.rs:24`、`crates/telegram/src/state.rs:375`：inbound / outbound 共用同一个 per-chat dedupe cache，TTL 10 分钟
  - `crates/telegram/src/state.rs:245`、`crates/telegram/src/state.rs:276`、`crates/telegram/src/state.rs:319`、`crates/telegram/src/state.rs:328`：`register_sent_message_contexts` / `message_context` / `inherited_root_message_id` / `admit_managed_dispatch` 全都仅按 `chat_id + message_id` 操作共享运行时上下文
  - `crates/telegram/src/handlers.rs:3733`：reply-to 目标作者回查也直接拿 `reply.id` 到共享运行时按 `chat_id + message_id` 查作者
  - `crates/telegram/src/outbound.rs:725`：outbound lineage 继承同样只按 `chat_id + lineage_message_id` 查根
  - `crates/gateway/src/chat.rs:2505`：`chat.send` 修复前直接打印完整 `user_message`
  - `crates/gateway/src/chat.rs:5413`：`agent run complete` 修复前直接打印完整 `response`
  - `crates/gateway/src/chat.rs:6554`：`channel reply delivery starting` 修复前缺少 `reply_target_ref_hash` / carrier 维度，无法与 Telegram 侧 `local_msg_ref` 稳定对齐
- 实现后代码证据（2026-03-25）：
  - `crates/telegram/src/state.rs:62`：`GroupLocalMessageRef`
  - `crates/telegram/src/state.rs:205`：`register_message_author()` 改为 observer-scoped `local_msg_ref`
  - `crates/telegram/src/state.rs:241`：`check_and_insert_action()` 统一承载 namespaced dedupe
  - `crates/telegram/src/handlers.rs:377`：inbound 收包日志已输出 `local_msg_ref`
  - `crates/telegram/src/handlers.rs:960`：`telegram inbound dispatched to chat` 已输出 `local_msg_ref + text_preview + decision + policy`
  - `crates/telegram/src/outbound.rs:976`：`telegram group outbound event handed to gateway` 已输出 `local_msg_ref + text_preview`
  - `crates/telegram/src/outbound.rs:1268`：`telegram outbound text send start` 已输出 `text_preview`
  - `crates/telegram/src/outbound.rs:2265`、`crates/telegram/src/outbound.rs:2310`、`crates/telegram/src/outbound.rs:2591`、`crates/telegram/src/outbound.rs:2687`：reply-target 出站已追加 `reply_target_ref_hash + local_msg_ref` 关联日志
  - `crates/telegram/src/adapter.rs:1479`：`reply_target_ref_round_trip_keeps_direct_delivery_fields_only`
  - `crates/telegram/src/adapter.rs:1494`：`reply_target_ref_rejects_legacy_lineage_field`
  - `crates/config/src/schema.rs:1884`、`crates/config/src/validate.rs:1544`：legacy `bot_dispatch_cycle_budget` 形状自然失败
  - `crates/gateway/src/chat.rs:2505`、`crates/gateway/src/chat.rs:5413`、`crates/gateway/src/chat.rs:6554`、`crates/gateway/src/chat.rs:6319`：`chat.send` / `agent run complete` / `channel reply delivery starting` / `chat stream done` 已切到 `channel_turn_id + channel_type + bucket_key + reply_target_ref_hash + text_preview`
- 测试覆盖演进：
  - 修复前缺口（已补齐）：
    - namespaced dedupe 核心路径缺失
    - `local_msg_ref` 作者绑定隔离测试缺失
    - reply-to 以 observer 本地 ref 解析目标 bot 的测试缺失
    - gateway / Telegram 统一关联字段的日志回归测试缺失
  - 修复后新增覆盖：
    - `crates/telegram/src/state.rs`：author binding owner 隔离、inbound/outbound namespaced dedupe、participant 保留
    - `crates/telegram/src/handlers.rs:4986`、`crates/telegram/src/handlers.rs:5033`、`crates/telegram/src/handlers.rs:5114`：reply 作者解析、`outbound -> inbound` 冲撞回归、inbound 日志回归
    - `crates/telegram/src/outbound.rs:3884`、`crates/telegram/src/outbound.rs:3948`、`crates/telegram/src/outbound.rs:4160`：reply-target 出站作者绑定、`inbound -> outbound` 冲撞回归、outbound 日志回归
    - `crates/telegram/src/adapter.rs:1479`、`crates/telegram/src/adapter.rs:1494`：direct-delivery `reply_target_ref` round-trip 与 legacy reject
    - `crates/gateway/src/chat.rs:7666`、`crates/gateway/src/chat.rs:7672`、`crates/gateway/src/chat.rs:7698`：`text_preview`、`reply_target_ref_hash`、`channel_type/bucket_key` 会话链回归

## 根因分析（Root Cause）
- A. 当前 Telegram 群共享运行时把 `message_id` 当成跨 bot 可直接比较的消息真值，但 2026-03-25 现场 3 bot 同收同一人类消息的日志已直接证明：在当前运行形态下，它只是 bot-local 视图值。
- B. inbound 与 outbound 复用了同一个 dedupe cache，但 dedupe key 没有区分方向/来源/视图命名空间，导致真实 inbound 能被之前的 outbound fanout 误杀。
- C. 同样的错误假设还扩散到了 `message_context`、`message_author`、`inherited_root_message_id`、`admit_managed_dispatch` 这些共享运行时索引上，因此问题不是单点，而是一类系统性污染。
- D. 现场日志同时显示 `chat_id`、`peer_id`、sender 元数据与 addressed 判定都一致，只有 `message_id` 在 bot 之间分叉；因此当前问题不是群聊路由字段漂移，而是共享运行时身份模型选错了键。
- E. 最新单 bot 点名实验还证明：对单个 bot 而言，inbound / outbound 共用同一条本地 `message_id` 序列；因此只要共享 dedupe 空间未按“方向 + owner”隔离，就会天然出现双向串桶。
- F. `root_message_id` / root budget / dispatch fuse 这一整套隐藏链路语义并没有 authoritative 外部真源；它们只是建立在错误消息身份模型上的额外复杂度，因此本单应删除而不是继续修补。
- G. 当前测试样本基本都在“单账号、单条消息 id 空间”里跑，缺少多 bot 本地 message id 分叉/撞号场景，导致这类问题一直没暴露。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - Telegram 群共享运行时唯一合法的消息身份必须是 `local_msg_ref = (account_handle, chat_id, message_id)`。
  - 所有跨 bot 共享运行时消费者都必须把 `message_id` 视为 owner-scoped 的本地坐标，而不是 chat-scoped 真值。
  - dedupe key 必须带明确命名空间，至少区分 inbound / outbound、视图 owner、业务对象。
  - Telegram 群聊协作的唯一真源必须是“群消息表面语义（行首点名 / reply-to）+ `local_msg_ref`”；不得再依赖隐藏 root/lineage。
  - gateway/common 层若只需要“把这轮 inbound 与后续 outbound 串起来”，必须优先复用 `reply_target_ref + channel_turn_id(trigger_id)`；不得重复引入第二套 Telegram 专属 cross-layer 身份字段。
  - `reply_target_ref` 的 Telegram 载荷必须只表达直接投递所需真值：`(account_handle, chat_id, thread_id?, message_id?)`；不得继续承载 `lineage_message_id`、root、hop 或其他隐藏链路字段。
  - adapter <-> gateway <-> adapter 的入站/出站**文本载荷必须继续完整存在于业务主路径**；本单不改变消息文本的功能传递，只治理身份模型与日志口径。
  - 行首点名 dispatch 与 reply-to dispatch 必须继续可用。
  - mirror / relay 不得再因为不相关的本地 `message_id` 撞号而被误杀。
  - agent 对话日志必须能把“Telegram inbound 触发了哪次 `chat.send`、哪次 agent run、哪次 channel delivery、哪次 Telegram outbound”结构化串联起来。
- 不得：
  - 不得继续使用裸 `chat_id + message_id` 作为共享运行时 key。
  - 不得继续保留 `root_message_id`、`inherited_root_message_id`、`root_budget`、`dispatch_fuse`、hop limit 等隐藏链路语义。
  - 不得靠“最近消息”“概率匹配正文”“历史 fallback”去猜测跨 bot 的同一外部消息。
  - 不得继续把完整 `user_message` / 完整 `response` 当作主要排障索引；正文只能做短预览、长度或哈希辅助。
- 应当：
  - 应把 reply-to 目标识别收敛为 `local_msg_ref -> managed_author_account_handle` 的最小作者绑定。
  - 应补充专门的结构化日志，能区分 dedupe hit 是“同方向真实重复”还是“跨命名空间冲撞”。
  - 应让 gateway 与 Telegram adapter 的日志字段合同对齐，避免上游有 `reason_code` / `decision` / `policy`，下游却只能靠 `run_id` 和正文猜测。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 在 Telegram 群共享运行时 one-cut 收敛为一个唯一消息身份：`local_msg_ref = (account_handle, chat_id, message_id)`。
  - 保留最小作者绑定：`local_msg_ref -> managed_author_account_handle`，只用于 reply-to 目标 bot 识别。
  - dedupe 全部改为 namespaced `action_key`，按 owner + 方向 + 业务对象隔离。
  - 彻底删除 `root_message_id`、root budget、dispatch fuse、hop limit、隐藏协作链 lineage。
- 优点：
  - 一次切干净同类污染，不留半套旧规则
  - 符合第一性原则：Telegram 真值只剩本地消息坐标；群聊协作真值只剩表面消息语义
  - 符合唯一真源原则：不再引入没有 authoritative 外部真源的 root 概念
- 风险/缺点：
  - 会显式删除 root/hop/fuse 这类隐藏链路能力；这是本方案的设计目标，不是副作用

#### 方案 2（拒绝）
- 核心思路：保留 `root_message_id` / hop limit / dispatch fuse，只补 dedupe key
- 优点：
  - 表面改动较小
- 风险/缺点：
  - `message_context` / `message_author` / `root_message_id` 仍会继续污染
  - 继续保留没有 authoritative 外部真源的隐藏链路语义，违背唯一真源与第一性原则，不能接受

### 最终方案（Chosen Approach）
- 采用方案 1。

#### 行为规范（Normative Rules）
- 规则 1（唯一消息身份）：
  - 共享运行时唯一合法的 Telegram 消息身份是 `local_msg_ref = (account_handle, chat_id, message_id)`
- 规则 2（最小运行时事实）：
  - 共享运行时只保留三类最小事实：
    - `participants`
    - `author binding`（`local_msg_ref -> managed_author_account_handle`）
    - namespaced `dedupe`
- 规则 3（显式协作真源）：
  - Telegram 群聊协作只认两种显式表面语义：
    - 行首点名
    - reply-to 某 bot 消息
- 规则 4（隐藏链路删除）：
  - `root_message_id`、`inherited_root_message_id`、`admit_managed_dispatch`、root budget、dispatch fuse、hop limit 必须整体删除；不得保留兼容尾巴
- 规则 5（dedupe 命名空间）：
  - inbound 和 outbound 必须是不同 namespace
  - 同 namespace 内也必须带足够 owner/source/target 信息，避免跨 bot 视图冲撞

#### 接口与数据结构（Contracts）
- 运行时唯一数据结构：
  - `GroupLocalMessageRef { account_handle, chat_id, message_id }`
  - `participants: HashMap<chat_id, BTreeSet<account_handle>>`
  - `author_bindings: HashMap<GroupLocalMessageRef, managed_author_account_handle>`
  - `dedupe: GroupRuntimeDedupeCache<ActionKey>`
- cross-layer 承载合同：
  - 不新增 Telegram 专属通用字段到 `ChannelInboundContext`
  - gateway/common 继续只认 `ChannelInboundContext.reply_target_ref`
  - Telegram 适配层通过 `reply_target_ref <-> local_msg_ref` 双向编码/解码维持唯一真源
  - `reply_target_ref` 的 Telegram shape 必须同步收敛为“直接投递坐标 only”；不再保留 `lineage_message_id` 旁路字段
  - session 内关联继续复用现有 `channel_turn_id(trigger_id) -> reply_target_ref` 存储链
- 运行时禁止保留的数据结构：
  - `GroupMessageContextSnapshot`
  - `GroupRootBudgetSnapshot`
  - `GroupDispatchAdmission`
  - `GroupRuntimeMessageContextEntry`
  - `GroupRuntimeRootBudgetEntry`
  - `message_contexts`
  - `root_budgets`
- `ActionKey` 必须显式区分业务对象，至少包含以下两类：
  - inbound handoff：`telegram.group.inbound_handoff|observer:{account_handle}|chat:{chat_id}|message:{message_id}`
  - outbound plan：`telegram.group.outbound_plan|source:{source_account_handle}|target:{target_account_handle}|chat:{chat_id}|message:{sent_message_id}`
  - 不允许继续使用当前这种同桶 `telegram.group.action|account:...|chat:...|message:...`
- reply / author：
  - `resolve_reply_to_target_account_handle()` 必须改为按“当前观察 bot 的 `GroupLocalMessageRef`”查 `author_bindings`
  - 其函数签名应显式接收 `observer_account_handle`
  - outbound 成功发送 group-visible 消息后，必须按 `source_account_handle` 为每个成功返回的 `sent_message_id` 登记作者绑定
- 日志合同：
  - Telegram adapter / outbound 侧结构化日志必须补 `local_msg_ref`
  - gateway 侧不强制直接展开 Telegram `local_msg_ref`；优先记录 `reply_target_ref_hash`、`channel_turn_id(trigger_id)`、`session_id/session_key`、`bucket_key`
  - reply 场景必须保证可从 `reply_target_ref` 反解出 Telegram 本地坐标；必要时在 Telegram 侧日志同时打印 `reply_target_ref_hash + local_msg_ref`
  - 文本本身仍在业务载荷中传递；削减的是默认 INFO 级完整正文日志，不是删 inbound/outbound `text`
  - 默认正文字段改为 `text_len` + `text_preview` + 可选 hash；不再把完整正文当主索引
  - `text_preview` 必须足够长以支持人工排障，默认目标为“最多 160 个 Unicode 字符，必要时允许到 240”；不得短到看不出是哪条消息
- 删除项：
  - `register_sent_message_contexts()`、`message_context()`、`root_budget_snapshot()`、`inherited_root_message_id()`、`ensure_external_root_dispatch()`、`admit_managed_dispatch()` 及其调用链必须整体删除
  - `TelegramOutboundTargetRef.lineage_message_id`、`reply_target_ref_for_target_with_lineage()` 以及 decode 时对 `lineage_message_id` 的兼容分支必须一起删除
  - `TelegramChannelsConfig::bot_dispatch_cycle_budget`、`default_bot_dispatch_cycle_budget()`、模板项、validate/schema/server/plugin 接线必须一起删除
  - 旧 `telegram.group.dispatch_fuse` 日志事件及相关 reason code 必须一并删除，不保留空壳

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若缺失 `local_msg_ref` 或作者绑定缺失，必须显式留结构化日志；不得静默改走旧键
  - reply-to 若无法解析出目标 bot，必须不触发 reply-based dispatch；不得猜测目标
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - runtime 旧键空间与新键空间不得长期并行；落地时应 one-cut 清理
  - root/hop/fuse 旧状态必须整体删除；不得在运行时同时保留两套语义

#### 可观测性合同（Observability Contract）
- Telegram adapter 侧：
  - `telegram inbound message received`
  - `telegram inbound dispatched to chat`
  - `telegram.group.plan` / `telegram.group.outbound_plan`
  - `telegram outbound text send start/sent`
- gateway / agent 侧：
  - `chat.send`
  - `agent run complete`
  - `channel reply delivery starting`
- Telegram 侧主关联字段：
  - `account_handle`
  - `chat_id`
  - `message_id`
  - `local_msg_ref`
  - `reply_target_ref_hash`
  - `bucket_key`
  - `reason_code`
  - `decision`
  - `policy`
- gateway / agent 侧主关联字段：
  - `channel_type`
  - `bucket_key`
  - `session_id` / `session_key`
  - `channel_turn_id(trigger_id)`
  - `run_id`
  - `reply_target_ref_hash`
- 关联原则：
  - `reply_target_ref` 是跨层 opaque carrier，默认不在 gateway/common 层直接展开为 Telegram 专属字段
  - `channel_turn_id` 是这轮 inbound 到后续 agent/outbound 的 session 内唯一关联键
  - `reply_target_ref_hash` 用于在 gateway 与 Telegram 出站日志之间做非明文对齐
  - 外层日志前缀继续保持现有样式，例如：`2026-03-25T08:27:03.024959Z INFO moltis_gateway::chat: channel reply delivery starting ...`
- 正文治理：
  - `user_message` / `response` 默认不再打印完整原文
  - 如需辅助排障，优先记录 `text_len`、较长 `text_preview`（默认上限 160 字符）与稳定哈希
  - 结构化日志优先回答“谁触发了谁、为什么触发/被拦截、关联到哪条 inbound/outbound”，而不是堆全文本
- 最小回答能力：
  - 给定任一 Telegram inbound，必须能追到其对应的 `channel_turn_id` / `chat.send` / agent run / outbound reply
  - 给定任一 `tg_dedup_hit`，必须能看出它撞到的是 inbound 还是 outbound、owner 是谁、命中的是哪类 key namespace
  - 给定任一 Telegram outbound，必须能追到它使用的 source `local_msg_ref` / `reply_target_ref_hash`
- 目标日志示例（格式示意，非最终逐字冻结）：
  - `2026-03-25T08:26:48.524651Z INFO moltis_telegram::handlers: telegram inbound dispatched to chat account_handle="telegram:8704214186" chat_id=-5288040422 message_id="1525" local_msg_ref="telegram:8704214186|-5288040422|1525" bucket_key="group-peer-tgchat.n5288040422" addressed=true text_len=73 text_preview="@cute_alma_bot @fluffy_tomato_bot @lovely_apple_bot 大家好，我是neo" reason_code="tg_dispatch_line_start_mention" decision="dispatch" policy="group_record_dispatch_v3"`
  - `2026-03-25T08:26:48.680624Z INFO moltis_gateway::chat: chat.send run_id="66954990-4ffe-4e15-b70e-8e9d661971af" trigger_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" session_id="sess_755340bbdeb242be899bfc991dc0b940" channel_turn_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" channel_type="telegram" bucket_key="group-peer-tgchat.n5288040422" reply_target_ref_hash="ab12..." text_len=73 text_preview="neo -> you: @cute_alma_bot @fluffy_tomato_bot @lovely_apple_bot 大家好，我是neo" model="openai-responses::gpt-5.2" reply_medium=Text`
  - `2026-03-25T08:27:03.024959Z INFO moltis_gateway::chat: chat stream done run_id="66954990-4ffe-4e15-b70e-8e9d661971af" trigger_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" session_id="sess_755340bbdeb242be899bfc991dc0b940" channel_turn_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" channel_type="telegram" bucket_key="group-peer-tgchat.n5288040422" reply_target_ref_hash="ab12..." input_tokens=29027 output_tokens=618 output_text_len=325 output_preview="neo 你好，我是朵朵（fluffy_tomato_bot）。我们这边辩论流程还在跑：R1～R5 已完成..." silent=false`
  - `2026-03-25T08:27:04.768477Z INFO moltis_gateway::chat: agent run complete run_id="66954990-4ffe-4e15-b70e-8e9d661971af" trigger_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" session_id="sess_755340bbdeb242be899bfc991dc0b940" channel_turn_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" channel_type="telegram" bucket_key="group-peer-tgchat.n5288040422" reply_target_ref_hash="ab12..." output_text_len=325 output_preview="neo 你好，我是朵朵（fluffy_tomato_bot）。我们这边辩论流程还在跑：R1～R5 已完成..." silent=false`
  - `2026-03-25T08:27:04.768811Z INFO moltis_gateway::chat: channel reply delivery starting session_id="sess_755340bbdeb242be899bfc991dc0b940" trigger_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" channel_turn_id="trg_01KMJ1MYTHYVTKKG1GBNC5S8JB" channel_type="telegram" bucket_key="group-peer-tgchat.n5288040422" target_count=1 reply_target_ref_hash="ab12..." text_len=325 text_preview="neo 你好，我是朵朵（fluffy_tomato_bot）。我们这边辩论流程还在跑：R1～R5 已完成..." reply_medium=Text`
  - `2026-03-25T08:27:05.210262Z INFO moltis_telegram::outbound: telegram outbound text send start account_handle="telegram:8576199590" chat_id="-5288040422" reply_to="1945" local_msg_ref="telegram:8576199590|-5288040422|1956" reply_target_ref_hash="ab12..." text_len=325 text_preview="neo 你好，我是朵朵（fluffy_tomato_bot）。我们这边辩论流程还在跑：R1～R5 已完成..." silent=false`

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - Telegram 侧只展示 `chat_id`、`account_handle`、`message_id`、`local_msg_ref` 的短形式
  - gateway/common 侧优先展示 `reply_target_ref_hash`，不直接打印原始 `reply_target_ref`
- 禁止打印字段清单：
  - 完整正文
  - token
  - 未脱敏的长上下文

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 已冻结 Telegram 群共享运行时的唯一消息身份：`local_msg_ref`
- [x] 已冻结最小运行时事实为 `participants + author_bindings + namespaced dedupe`
- [x] 已确认 dedupe、作者绑定、reply-target、gateway/telegram 关联日志 的受影响范围
- [x] 已明确 why “bot-local `message_id` 不能再直接当共享 key”并写入 issue Spec
- [x] 已形成 one-cut 实施方案，并明确删除 root/hop/fuse 代码、配置、日志与测试
- [x] 已冻结 observability carrier：gateway/common 复用 `reply_target_ref + channel_turn_id`，Telegram 侧输出 `local_msg_ref`
- [x] 已列出必须补齐的核心路径自动化测试矩阵
- [x] 已明确 legacy 配置 `channels.telegram.bot_dispatch_cycle_budget` 不做兼容迁移

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/telegram/src/state.rs`：`GroupLocalMessageRef` 以 `account_handle + chat_id + message_id` 唯一标识消息；同 `chat_id + message_id` 在不同 bot 下不得互相覆盖
- [x] `crates/telegram/src/state.rs`：namespaced dedupe 对 inbound handoff 与 outbound plan 分桶；必须覆盖 `outbound -> inbound` 与 `inbound -> outbound` 两个冲撞方向
- [x] `crates/telegram/src/state.rs`：reply 作者绑定只按 `GroupLocalMessageRef` 读写；相同 `chat_id + message_id` 在不同 owner 下返回不同作者或空值
- [x] `crates/telegram/src/adapter.rs`：`reply_target_ref` round-trip 只保留直接投递坐标；不得再出现 `lineage_message_id` 旁路字段
- [x] `crates/config/src/{schema.rs,validate.rs}`：删除 `bot_dispatch_cycle_budget` 后，legacy 配置形状自然报错；不做兼容映射

### Integration
- [x] `crates/telegram/src/handlers.rs`：复现“bot A outbound plan 先写入同号 key，bot B 后收到真实 inbound”场景，真实 inbound 不得再被 `tg_dedup_hit` 吞掉
- [x] `crates/telegram/src/outbound.rs` + `crates/telegram/src/handlers.rs`：复现“bot A 真实 inbound 先占用本地 `message_id`，bot B 后续 outbound plan 在 target bot 侧撞同号”场景，outbound plan 不得再被误杀
- [x] `crates/telegram/src/outbound.rs`：group-visible outbound 成功后会为每个 `sent_message_id` 登记 source bot 的作者绑定
- [x] `crates/telegram/src/handlers.rs` + `crates/telegram/src/outbound.rs`：reply-to 作者识别按 observer `local_msg_ref` 正确命中；行首点名与 reply-to 显式协作仍保持正确
- [x] `crates/gateway/src/channel_events.rs` + `crates/gateway/src/chat.rs`：同一轮 Telegram inbound 会把 `reply_target_ref` 绑定到 `channel_turn_id(trigger_id)`，后续 `chat.send` / `agent run complete` / `channel reply delivery` 可沿该链路追踪
- [x] `crates/gateway/src/chat.rs` + `crates/telegram/src/outbound.rs`：gateway 侧输出 `reply_target_ref_hash`，Telegram 出站侧输出同一 hash 与 `local_msg_ref`，两侧可无正文对齐
- [x] `crates/telegram/src/{handlers.rs,outbound.rs,state.rs}`：删除 root/hop/fuse 后，不再产出 `telegram.group.dispatch_fuse` / `root_dispatch_*` 相关日志与状态

### UI E2E（Playwright，如适用）
- [x] 不适用；本单聚焦 Telegram 运行时

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 自动化应覆盖本单核心闭环；不应再把关键正确性留给手工
  - 手工只用于真实 Telegram 三 bot 环境的最终烟测，而不是替代核心测试
- 手工验证步骤：
  1. 建 3 bot 共享群
  2. 连续发起“bot 出站 fanout + 人类新消息 + bot reply-to + 连续几轮 relay”混合链
  3. 检查是否仍出现 `tg_dedup_hit` 吞真实 inbound、mirror 丢失、reply 作者识别错误或 reply-target 串桶
  4. 检查日志能直接串出 `local_msg_ref -> reply_target_ref_hash -> trigger_id/run_id -> outbound`
  5. 检查不再出现任何 root/hop/fuse 相关运行时日志或配置入口

## 发布与回滚（Rollout & Rollback）
- 发布策略：实现后应以单次 one-cut 方式上线；不保留旧键兼容路径
- 回滚策略：如需回滚，只能整体回滚到旧版本；不得在运行时同时保留“`local_msg_ref` 最小模型”和旧 root/hop/fuse 模型
- 上线观测：
  - `telegram.group.plan`
  - `telegram.group.outbound_plan`
  - `tg_dedup_hit`
  - `reply_target_context_missing`
  - `local_msg_ref`
  - `reply_target_ref_hash`
  - `run_id`
  - `trigger_id`

## 实施拆分（Implementation Outline）
- Step 1: `crates/telegram/src/state.rs`
  - 引入 `GroupLocalMessageRef`
  - 把运行时收敛为 `participants + author_bindings + namespaced dedupe`
  - 删除 `message_contexts`、`root_budgets`、root/fuse 相关公开 API 与测试
- Step 2: `crates/telegram/src/handlers.rs`
  - 删除 `apply_group_dispatch_fuse()`
  - 将 inbound dedupe key 改为 inbound namespace
  - 将 `resolve_reply_to_target_account_handle()` 改为显式接收 `observer_account_handle` 并按 observer `local_msg_ref` 查作者绑定
- Step 3: `crates/telegram/src/outbound.rs`
  - 删除 `lineage_message_id -> root_message_id` 继承与 `register_group_visible_outbound_contexts()`
  - 出站成功后按 source `local_msg_ref` 注册作者绑定
  - 将 outbound dedupe key 改为 outbound-plan namespace
  - 删除所有 dispatch fuse 降级路径与相关日志
- Step 3.5: `crates/telegram/src/adapter.rs`
  - 删除 `TelegramOutboundTargetRef.lineage_message_id`
  - 删除 `reply_target_ref_for_target_with_lineage()`
  - 将 `reply_target_ref` Telegram shape 收敛为“direct delivery only”
  - 删除 decode 侧对 `lineage_message_id` 的旧隐藏链路语义
- Step 4: `crates/config/src/{telegram.rs,template.rs,validate.rs,schema.rs}` + `crates/telegram/src/plugin.rs` + `crates/gateway/src/server.rs`
  - 删除 `bot_dispatch_cycle_budget` 字段、默认值、模板、validate/schema、plugin setter 与 server 接线
  - 不加兼容 shim；legacy 配置自然失败
- Step 5: `crates/gateway/src/chat.rs` + `crates/telegram/src/{handlers.rs,outbound.rs}`
  - gateway/common 层复用既有 `reply_target_ref + channel_turn_id(trigger_id)` 作为承载链，不新增 Telegram 专属 generic 字段
  - 把 agent 对话链日志改成“`channel_turn_id` + `channel_type` + `bucket_key` + `reply_target_ref_hash` + 长度/预览”
  - Telegram 出站/入站日志补 `local_msg_ref`，必要时同时补同一个 `reply_target_ref_hash`
  - 去掉完整 `user_message` / `response` 作为主日志载荷
- Step 6: 测试与验收
  - 删除 root/hop/fuse 旧测试
  - 补齐 `state.rs` / `handlers.rs` / `outbound.rs` / `config` / `gateway chat` 的核心路径测试
  - 回到真实 3 bot 群做最终烟测
- 受影响文件：
  - `crates/telegram/src/state.rs`
  - `crates/telegram/src/handlers.rs`
  - `crates/telegram/src/outbound.rs`
  - `crates/telegram/src/plugin.rs`
  - `crates/config/src/telegram.rs`
  - `crates/config/src/template.rs`
  - `crates/config/src/validate.rs`
  - `crates/config/src/schema.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/chat.rs`
  - `issues/issue-v3-telegram-group-execution-plan-record-dispatch-one-cut.md`
  - `issues/issue-telegram-group-dispatch-fuse.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-v3-telegram-group-execution-plan-record-dispatch-one-cut.md`
  - `issues/issue-telegram-group-dispatch-fuse.md`（已被本单 supersede；仅保留历史决策记录）
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - N/A

## 未决问题（Open Questions）
- 无。本单设计已明确拒绝“跨 bot 外部消息自动合并”为系统职责；唯一真源与删除项均已冻结。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

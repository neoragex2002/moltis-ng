# Issue: V3 one-cut 实施前补齐可信性缺口（旧桥内存态 / legacy binding 过渡 / transcript 配置 / 契约文档对齐）

> SUPERSEDED BY:
> - 设计真源：`docs/src/refactor/session-key-bucket-key-one-cut.md`
> - 治理主单：`issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
> - 本单仅保留历史背景与实施证据，不再定义当前实现口径或规范优先级。

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P0
- Updated: 2026-03-22
- Owners: TBD
- Components: gateway/tools/common/channels/telegram/config/ui/onboarding/docs
- Affected providers/models: all

**已实现（如有，写日期）**
- 2026-03-20：已完成一次“one-cut 主单实施可信性”审查，发现当前主单仍缺少若干实施前冻结项与漏网清单：`issues/issue-v3-session-ids-and-channel-boundary-one-cut.md:1`
- 2026-03-20：已将旧桥内存态、`channel_binding/reply_target_ref` 分工、`group_session_transcript_format` 删除口径、`_chanChatKey`/旧 sandbox scope/persona 漏网点，全部并回主单。
- 2026-03-20：已补充并对齐实施文档优先级：当前 C 阶段以主单 + `docs/src/refactor/channel-info-exposure-boundary.md` + `docs/src/refactor/telegram-adapter-boundary.md` 为准；`docs/src/refactor/channel-adapter-generic-interfaces.md` 明确降为 future-facing 参考。
- 2026-03-21：关单复核补齐了两个收尾项：runner hook 上下文已按主单要求由 gateway 显式传入，`docs/src/session-branching.md` 的旧术语残留已清理，主单证据同步更新。
- 2026-03-21：按当时的 one-cut 设计口径补齐了最后一组可观测性收口：旧 `persona_id` 改为显式拒绝，旧 `people/` 命中时改为结构化告警而非静默读空。
- 2026-03-22：后续实现按 strict one-cut 主单完成最终收口：loader 命中旧 `people/` 直接拒绝并记录 `legacy_people_dir_rejected`；legacy `tools.exec.sandbox.scope` 改为硬错误；`scope_key=session_key` 缺少 `_sessionKey` 改为直接失败并记录 `missing_session_key_for_scope_key_session_key`。本单中的兼容窗口表述已被严格口径取代。

**已覆盖测试（如有）**
- 无；本单是实施前补缺/收口单，不直接改运行时代码。

**已知差异/后续优化（非阻塞）**
- 本单不是当前治理主单；当前实现与规范以 `docs/src/refactor/session-key-bucket-key-one-cut.md` 和 `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md` 为准。
- 本单已完成；若本单中的历史设计表述与当前实现冲突，以新设计真源、当前治理主单、运行时代码和最新测试为准。

---

## 背景（Background）
- 场景：对 `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md` 做实施前复核时，发现主单方向基本正确，但仍有几处“范围未写进主单 / 契约未冻结 / 设计文档互相打架 / 漏网清单不完整”的缺口。
- 约束：
  - C 阶段仍不改最终落盘格式。
  - 目标仍是 one-cut：除落盘外，不接受遗留旧桥接路径、旧契约、旧命名尾巴。
  - 实施主单必须在开工前明确：什么必须一起切、哪些 legacy 只允许留在 adapter/helper 内部、哪些文档已过时不得继续指导实现。
- Out of scope：
  - 本单不直接实施运行时代码改造。
  - 本单不做 persistence schema / 历史数据迁移。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`readiness_gap`**（主称呼）：不是“新功能需求”，而是“当前主单若直接开工会导致实现歧义、漏改或反复返工的缺口”。
  - Why：本单目标不是扩 scope，而是把主单开工前必须冻结的事项写透。
  - Not：不是运行时 bug 单，也不是替代主单的新实现单。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：可信性缺口 / 实施前补缺

- **`channel_binding`**（主称呼）：session 级渠道绑定载体，用于 `session_id -> adapter` 的渠道回投入口。
  - Why：在“落盘暂不改”的前提下，它仍是 session 侧唯一稳定绑定。
  - Not：不等于 per-turn 的 reply threading 引用，也不应继续被 gateway/core 到处反序列化成公共渠道结构体。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：legacy binding blob

- **`reply_target_ref`**（主称呼）：adapter 私有的 per-turn/per-delivery opaque 引用。
  - Why：用于 reply-to / topic / thread / typing / edit 等“具体发回哪里”的投递执行。
  - Not：不是 `channel_binding` 的别名，也不是跨层公开字段集合。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：opaque delivery ref

- **authoritative / effective**
  - authoritative：当前代码/持久化里已经存在、实现必须面对的真实载体
  - effective：按当前阶段冻结后的生效口径

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 把“one-cut 主单”未覆盖的旧桥内存态一并纳入范围：包括 `ChannelTurnContext`、pending channel reply/status 等 runtime state。
- [x] 冻结 `channel_binding` 与 `reply_target_ref` 的职责边界、过渡方式与 legacy parse 归属，避免一边说“不改落盘”，一边隐含改掉 binding 格式。
- [x] 明确 `group_session_transcript_format` 在 C 阶段的去留口径：本轮 one-cut 直接删除，不保留跨层或 adapter-local bridge tail。
- [x] 对齐或显式废止仍会误导实现的设计文档，尤其是 `channel-adapter-generic-interfaces.md`。
- [x] 补齐 `_chanChatKey`、旧 sandbox `scope`、旧 persona 命名等漏网清单，更新到主单的实施与测试计划里。
- [x] 消除主单内部自相矛盾的地方（例如 Out of scope 与 Step 列表冲突）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：主单开工前，所有“是否在本单切、切到哪层、谁负责解析 legacy”的问题都要写清。
  - 不得：靠实现时临场判断来决定 `channel_binding`、`reply_target_ref`、`group_session_transcript_format` 的归属。
- 兼容性：在“不改落盘”的前提下，legacy binding 只允许作为 adapter/helper 内部兼容读取，不得继续扩散为跨层公共契约。
- 可观测性：若本单最终决定保留任何 bridge tail，必须在主单里显式标出“为什么允许留、留到哪一层、何时退场”。
- 安全与隐私：补缺文档不得鼓励把 `chat_id/thread_id/account_key/message_id` 重新透传给 UI/hooks/tools。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 主单已经冻结了 Q1/Q2/Q3/Q4/Q5/Q6/Q7，但仍有关键运行时旧桥未被写进范围。
2. 主单已经提出 `reply_target_ref`，但没有写清 `channel_binding` 在 C 阶段到底如何过渡、谁负责解析 legacy blob。
3. Q2 已说“TG adapter 产出最终群聊文本”，但 config/UI 仍有 `group_session_transcript_format`，设计文档之间也仍互相冲突。
4. 主单的清单还没有覆盖所有旧 `_chanChatKey` / 旧 sandbox `scope` / 旧 persona 的实际落点。
5. 主单里存在范围冲突：`docs/src/concepts-and-ids.md` 被标成 out of scope，但 Step 5 又要求更新它。

### 影响（Impact）
- 用户体验：如果实现时临场解释 `channel_binding` 与 `reply_target_ref`，很容易造成 reply routing、location、typing 继续走旧桥，或者把新旧契约混在一起。
- 可靠性：若 `ChannelTurnContext` 等 runtime state 不在 one-cut 范围内，表面接口切干净后，内部仍可能继续靠 `chan_chat_key` 和 `ChannelReplyTarget` 运转。
- 排障成本：若 `group_session_transcript_format`、`NormalizedMessage`、`ChannelReplyTarget` 等不同文档同时指向不同边界，后续 review 很难判断“到底哪份文档才是准绳”。

### 复现步骤（Reproduction）
1. 打开主单：`issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`
2. 对照当前代码继续检索 `_chanChatKey`、`ChannelReplyTarget`、`ChannelMessageMeta.telegram`、`tools.exec.sandbox.scope`、`persona_id`。
3. 期望 vs 实际：
   - 期望：主单已完整列出所有 one-cut 必切路径，并写清 legacy 只允许留在哪一层。
   - 实际：仍有若干 live runtime path、config/UI path、doc path 未被主单覆盖。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/gateway/src/state.rs:92`：`ChannelTurnContext` 仍保存 `chan_chat_key` 与 `Vec<ChannelReplyTarget>`。
  - `crates/gateway/src/state.rs:554`：turn context 的 ensure/push/drain 全链路仍围绕旧字段与旧 reply target 工作。
  - `crates/gateway/src/channel_events.rs:437`：WS/chat payload 仍输出 `chanChatKey`；`crates/gateway/src/channel_events.rs:1476`：状态卡仍展示 `ChanChatKey`。
  - `crates/gateway/src/server.rs:218`：`request_channel_location()` 仍按 `_chanChatKey` / `parse_chan_chat_key()` 兼容归一。
  - `crates/gateway/src/server.rs:254`：gateway 仍直接把 `channel_binding` 反序列化为 `ChannelReplyTarget`。
  - `crates/gateway/src/chat.rs:899`：gateway prompt/debug 上下文仍直接把 `channel_binding` 解析为 `ChannelReplyTarget`。
  - `crates/gateway/src/session.rs:122`：sandbox 仍从 `channel_binding` 反推 `chan_chat_key`。
  - `crates/gateway/src/session.rs:404`：sandbox override 提示文案仍绑定旧 `tools.exec.sandbox.scope`。
  - `crates/telegram/src/adapter.rs:147`：TG adapter 仍把 binding 视为 `ChannelReplyTarget` JSON。
  - `crates/telegram/src/config.rs:112`：`group_session_transcript_format` 仍是 TG 配置字段。
  - `crates/telegram/src/config.rs:180`：snapshot 仍携带 `group_session_transcript_format`。
  - `crates/telegram/src/plugin.rs:113`：TG account snapshot 仍向外围暴露 `group_session_transcript_format`。
  - `crates/gateway/src/channel.rs:80`：gateway 渠道 config 仍接受 `group_session_transcript_format` / `persona_id`。
  - `crates/gateway/src/assets/js/page-channels.js:343`：新增 TG 渠道时仍提交 `group_session_transcript_format`。
  - `crates/gateway/src/assets/js/page-channels.js:553`：TG 渠道设置页仍暴露 transcript format 下拉框。
  - `crates/tools/src/sandbox_packages.rs:444`：`sandbox_packages` 仍优先读取 `_chanChatKey`。
  - `crates/tools/src/process.rs:408`：`process` 工具仍优先读取 `_chanChatKey`。
  - `crates/tools/src/exec.rs:273`：exec 工具仍兼容读取 `_chanChatKey`。
  - `crates/tools/src/spawn_agent.rs:184`：子 agent tool context 仍透传 `_chanChatKey`。
  - `crates/config/src/validate.rs:1017`：配置校验仍接受旧 `tools.exec.sandbox.scope`。
  - `crates/tools/src/sandbox.rs:606`：runtime sandbox 仍使用旧 `scope=session|chat|bot|global`。
  - `crates/gateway/src/assets/js/sandbox.js:46`：前端仍按旧 `tools.exec.sandbox.scope` 文案展示。
  - `crates/gateway/src/assets/js/page-settings.js:329`、`crates/gateway/src/assets/js/persona-utils.js:1`、`crates/config/src/loader.rs:267`、`crates/onboarding/src/service.rs:496`：persona / `people/` UI、loader、onboarding 尾巴仍在。
- 文档证据：
  - `docs/src/refactor/channel-adapter-generic-interfaces.md:308`：仍以 `NormalizedMessage { channel_kind, chat_kind, ingress_mode, body, source_ref }` 作为当前建议接口。
  - `docs/src/refactor/channel-adapter-generic-interfaces.md:736`：仍把 `ChannelReplyTarget` 描述为当前粗接口的一部分。
  - `docs/src/refactor/telegram-adapter-boundary.md:195`：仍把 `group_session_transcript_format` 描述成 bridge 过渡项，与当前 Q2 方向存在张力。
  - `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md:56`：主单把 `docs/src/concepts-and-ids.md` 记为 out of scope。
  - `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md:253`：主单 Step 5 又要求更新 `docs/src/concepts-and-ids.md`。
- 当前测试覆盖：
  - 已有：主单只列出 prompt cache / tool context / hooks / sandbox / location / reply 的目标测试。
  - 缺口：没有覆盖 `ChannelTurnContext` 旧桥退场、`channel_binding` legacy parse 归属、`group_session_transcript_format` 去留、generic interface doc 对齐。

## 根因分析（Root Cause）
- A. 主单主要围绕“对外契约”和“设计方向”展开，未把一部分 live runtime bridge state 和周边工具路径完整纳入。
- B. `channel_binding` 与 `reply_target_ref` 同时存在，但“session 级绑定”和“per-turn 投递引用”的职责边界尚未写透。
- C. 设计文档演进速度不一致：新的边界决策已经前进，但 generic interface / TG boundary / UI config 仍保留旧表述或过渡表述。
- D. 漏网清单基于局部搜索形成，尚未做最后一轮“按关键词全仓库兜底”的 inventory 冻结。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 主单必须显式纳入 `ChannelTurnContext` / pending reply / in-memory turn bridge 的 one-cut 改造。
  - 主单必须写清：C 阶段不改落盘时，`channel_binding` 是 session 级 legacy binding，`reply_target_ref` 是 runtime/per-turn opaque ref；legacy parse 只能收敛在 adapter/helper。
  - 主单必须对 `group_session_transcript_format` 给出明确结论：本单删除，或仅允许作为 adapter-local bridge tail 存在并写明退场条件。
  - 主单必须把 `_chanChatKey`、旧 sandbox `scope`、旧 persona 命名的所有主要消费点补齐到实施与测试计划。
  - 设计文档必须只有一套有效口径；未对齐的文档必须显式标“暂不作为实现依据”。
- 不得：
  - 不得让 gateway/core 继续到处直接 `serde_json::from_str::<ChannelReplyTarget>(channel_binding)`。
  - 不得在主单中同时出现“该文档 out of scope”和“该文档本单必须更新”这类范围冲突。
- 应当：
  - 在主单里加一个“实施前冻结项 / bridge tail 白名单”小节，列清楚哪些 legacy 允许暂存于 adapter/helper，哪些不允许再出现于 core/gateway/tools/hooks/ui。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：保留现有 one-cut 主单作为唯一实现主单；本单只负责把主单和相关设计文档补齐到“可直接按文开工”的状态。
- 优点：
  - 不会再开第二张平行实现单，避免 scope 分裂。
  - 可以把刚才 review 发现的遗漏都沉淀为主单的明确范围、契约和测试点。
  - 便于后续每轮 review 只对主单和代码做一致性校验。
- 风险/缺点：
  - 需要先补文档和清单，不能立刻开切代码。

#### 方案 2（备选）
- 核心思路：把这些 readiness gap 直接并回主单，不单独建补缺单。
- 优点：
  - issue 数量更少。
- 风险/缺点：
  - review 发现的问题容易被新的实现细节淹没，无法单独跟踪“主单何时达到可开工状态”。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（主从关系）：`issues/issue-v3-session-ids-and-channel-boundary-one-cut.md` 仍是唯一实现主单；本单是它的实施前补缺单。
- 规则 2（旧桥内存态入 scope）：`ChannelTurnContext`、pending channel reply/status、任何以 `chan_chat_key` 或 `ChannelReplyTarget` 为 live runtime state 的结构，必须显式纳入主单 one-cut 范围。
- 规则 3（binding/ref 分工冻结）：
  - `channel_binding`：session 级 binding；C 阶段落盘格式不变。
  - `reply_target_ref`：per-turn/per-delivery opaque 引用；用于 reply-to/topic/thread/typing/edit 等投递执行。
  - legacy `channel_binding` 的解析只能由 adapter/helper 负责；gateway/core 不再直接到处反序列化为公共渠道结构体。
- 规则 4（transcript 配置去留必须定案）：
  - 已决策：本轮 one-cut 直接删除 `group_session_transcript_format`；主单必须把 TG config/UI/API/snapshot 的删项一起列入。
- 规则 5（文档有效性冻结）：
  - 当前 C 阶段以主单、`channel-info-exposure-boundary.md`、`telegram-adapter-boundary.md` 为准。
  - `channel-adapter-generic-interfaces.md` 已明确标注为 future-facing；若仍有冲突，不作为当前施工依据。
- 规则 6（inventory 必须闭环）：凡是仍读取 `_chanChatKey`、仍使用旧 sandbox `scope`、仍暴露 `persona_id` 的主要代码点，都必须在主单里有对应实施项或测试项。

#### 接口与数据结构（Contracts）
- session 级：
  - `channel_binding` 继续作为 session metadata 的 binding 载体存在，但在 C 阶段只允许作为 adapter/helper 私有解析入口。
- turn 级：
  - `reply_target_ref` 作为新的正式投递契约进入主单，替代跨层 `ChannelReplyTarget`。
- 文档级：
  - 主单应增加“bridge tail 白名单”：
    - 允许：adapter/helper 内部对 legacy `channel_binding` 的 best-effort 解析
    - 不允许：gateway/core/tools/hooks/ui 再把 `ChannelReplyTarget` 当作跨层正式契约

#### 失败模式与降级（Failure modes & Degrade）
- 若主单未先补齐这些 readiness gap 就直接实施：
  - 很可能出现“外层契约改了，内层 runtime state 仍走旧桥”的半切状态。
  - 很可能出现“以为不改落盘，实际偷偷引入 binding 格式变化”的隐性 scope 膨胀。
- 因此本单关闭前，主单不得进入真正的代码实施阶段。

#### 安全与隐私（Security/Privacy）
- 本单不新增任何需要暴露给 UI/hooks/tools 的渠道私有字段。
- 若讨论 `channelTarget`、`channel_binding`、`reply_target_ref`，必须明确区分“外围可见字段”和“adapter/internal only”。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] 主单已显式纳入 `ChannelTurnContext` / pending channel reply / in-memory turn bridge 的改造范围，并补到实施与测试计划里。
- [x] 主单已明确写清 `channel_binding` 与 `reply_target_ref` 的职责边界，以及 C 阶段 legacy parse 的归属层。
- [x] 主单已对 `group_session_transcript_format` 给出明确结论，并覆盖 config/UI/API/文档的相应影响面。
- [x] 主单已补齐 `_chanChatKey` 主要漏网点、旧 sandbox `scope` 主要漏网点、旧 persona 主要漏网点。
- [x] `channel-adapter-generic-interfaces.md` 已对齐，或已显式标明当前 C 阶段不作为实现依据。
- [x] 主单内部已消除 Out of scope / Step / Acceptance 之间的自相矛盾。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 本单不直接新增单元测试；但主单测试计划中已新增：
  - `ChannelTurnContext` 旧桥退场回归
  - `channel_binding` legacy parse 只由 adapter/helper 持有的回归
  - `group_session_transcript_format` 去留对应的 config/UI 回归

### Integration
- [x] 本单不直接新增集成测试；但主单已把“多桶 reply routing + location + typing + sandbox key”作为联动回归面写清楚。

### UI E2E（Playwright，如适用）
- [x] 若 `group_session_transcript_format` 或 `persona_id` 在 UI 里删改，主单已补对应 `page-channels` / settings E2E 覆盖要求。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：本单本质上是主单实施前的范围/契约/文档补缺，不直接改运行时行为。
- 手工验证步骤：
  1. 对照本单验收项更新主单与相关设计文档。
  2. 再次执行全仓库关键词 review：`_chanChatKey`、`ChannelReplyTarget`、`group_session_transcript_format`、`persona_id`、`tools.exec.sandbox.scope`。
  3. 确认所有主要消费点都已经被主单纳入实施或显式标为允许保留的 adapter/helper bridge tail。

## 发布与回滚（Rollout & Rollback）
- 发布策略：本单无需上线；作为主单实施前的 blocker。
- 回滚策略：无运行时改动，无需单独回滚。
- 上线观测：无；但本单关闭后，主单应在上线观测里新增对 `missing_session_id`、`missing_session_key_for_scope_key_session_key`、`channel_location_not_supported` 等 reason code 的检查。

## 实施拆分（Implementation Outline）
- Step 1: 更新主单，补入旧桥内存态、binding/ref 分工、transcript config 去留、inventory 漏网点。
- Step 2: 更新/标注设计文档，消除 `channel-info-exposure-boundary.md`、`telegram-adapter-boundary.md`、`channel-adapter-generic-interfaces.md` 之间的冲突。
- Step 3: 回到主单，确认其实施计划与验收/测试计划已经可以直接驱动代码改造。
- 受影响文件：
  - `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`
  - `docs/src/refactor/channel-adapter-generic-interfaces.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `docs/src/refactor/channel-info-exposure-boundary.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-v3-session-ids-and-channel-boundary-one-cut.md`
  - `docs/src/refactor/channel-info-exposure-boundary.md`
  - `docs/src/refactor/telegram-adapter-boundary.md`
  - `docs/src/refactor/channel-adapter-generic-interfaces.md`
- Related commits/PRs：
  - 无
- External refs（可选）：
  - 无

## 未决问题（Open Questions）
- 当前无：本单涉及的默认决策已冻结并并回主单，不再需要新增用户决策。

## Close Checklist（关单清单）【不可省略】
- [x] 主单已补齐本单发现的 readiness gap，且口径一致
- [x] 旧桥/legacy parse 的保留边界已写清（只允许留在 adapter/helper 的地方）
- [x] 相关设计文档已同步更新或显式标注优先级
- [x] 主单测试计划已覆盖本单发现的关键漏项
- [x] 不再存在主单内部范围冲突或相互打架的表述

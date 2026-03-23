# Issue: Telegram 群聊 `Dispatch` / `RecordOnly` 正文被按行首点名切片，破坏语义完整性（body_integrity / no_segment_slicing）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-24
- Owners:
- Components: telegram
- Affected providers/models: (n/a)

**已实现（如有，写日期）**
- 2026-03-24：`Dispatch` / `RecordOnly` 命中 target 后统一返回 `body.trim().to_string()`，不再按 target 裁切正文：`crates/telegram/src/adapter.rs:871`
- 2026-03-24：保留 scan-only 的行首点名判定，但删除正文拼接残骸，`TgLineStartMentionGroup` 不再携带 `segment_text`：`crates/telegram/src/adapter.rs:647`
- 2026-03-24：`plan_group_target_action(...)` 只根据 target 命中与 `task_text` 决定 `mode/addressed/reason_code`，正文始终来自统一源正文：`crates/telegram/src/adapter.rs:886`

**已覆盖测试（如有）**
- 完整原文替代旧切片断言：`crates/telegram/src/adapter.rs:1389`
- “规范说明 + 坏例子”保持完整上下文：`crates/telegram/src/adapter.rs:1512`
- 多目标命中收到同一份完整原文：`crates/telegram/src/adapter.rs:1536`
- 仅最外层 `trim()`，内部换行不变：`crates/telegram/src/adapter.rs:1575`
- quote / inline code / fenced code 只影响判定、不影响最终 body：`crates/telegram/src/adapter.rs:1595`

**已知差异/后续优化（非阻塞）**
- 本单按收敛范围完成；无额外后续项。

---

## 背景（Background）
- 场景：Telegram 群聊中，planner 会根据“行首点名”决定某个 bot 是否进入 `Dispatch` 或 `RecordOnly`。
- 约束：
  - 行首点名是 Telegram 群聊中的目标识别信号。
  - Telegram 适配层可以做目标判定，但不应重写正文语义。
  - 当前转写协议若已有外层 envelope（例如 `发送者 -> you:`），本单不顺手改它。
- Out of scope：
  - 不处理 relay cycle budget / 保险丝。
  - 不改 gateway/core 的 handoff 语义。
  - 不改非 Telegram 渠道。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
- **统一源正文**（主称呼）：Telegram adapter 在做目标判定前拿到的那一份原始消息正文；允许的唯一归一化只有最外层 `trim()`。
  - Why：这是 `Dispatch` / `RecordOnly` 下游应看到的同一份语义载荷。
  - Not：不是按 target 派生出来的片段，也不是 planner 重新拼接的摘要。
  - Source/Method：as-sent
- **行首点名判定**（主称呼）：利用行首 mention 判定某个 target 是否命中、是否 `addressed`、以及进入 `Dispatch` 还是 `RecordOnly`。
  - Why：这是 Telegram 群聊 planner 的职责边界。
  - Not：不是正文切段边界，也不是正文重写依据。
  - Source/Method：effective
- **判定扫描正文**（主称呼）：仅供行首点名识别使用的扫描输入，可继续沿用现有 sanitize 规则忽略 quote / fenced code / inline code 中的 mention。
  - Why：避免把示例、引用、代码块中的 mention 误判成正式点名。
  - Not：不是最终下游要看到的正文，也不是允许回传给 gateway 的正文副本。
  - Source/Method：effective
- **语义正文完整性**（主称呼）：同一条 Telegram 原始消息在 adapter handoff 前，其正文语义必须稳定；不同 target 不允许看到不同正文变体。
  - Why：避免“解释消息被变形成指令消息”。
  - Not：不是要求删除或改写既有 envelope。
  - Source/Method：as-sent

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] `Dispatch` / `RecordOnly` 命中的 body 必须保持统一源正文，不再按行首点名切片。
- [x] 多 target 命中时，各 target 的正文必须一致；允许变化的只有 `target`、`addressed`、`mode`、`reason_code`。
- [x] 行首点名仍继续用于目标判定，不降低现有 target eligibility 识别能力。
- [x] 除 `body` 生成口径外，本单不改动既有 `mode` / `addressed` / `reason_code` 判定表。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：planner 只负责判定，不负责重写正文语义。
  - 必须：允许的正文归一化严格收敛为“整条 body 最外层一次 `trim()`”。
  - 不得：按目标分别裁剪正文、重排段落、压缩摘要、删除上下文。
- 兼容性：
  - 不追求兼容旧的“切片正文”行为；该行为本身就是本单要删除的错误语义。
- 可观测性：
  - 本单不新增新的日志/指标要求；重点是冻结 adapter handoff 的 body 语义。
- 安全与隐私：
  - 不新增正文日志；测试中可使用示例文本断言行为。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一条群聊消息中，某个 bot 实际收到的可能只是“命中的那一段”，而不是整条原文。
2) 若消息正文是在解释规范、举反例、或同时出现多个目标，切片后会丢失上层语境，甚至把“坏例子”变成看似直接下发的指令。

### 影响（Impact）
- 用户体验：bot 收到的内容与群里真实语义不一致，容易误解任务。
- 可靠性：同一原始消息对不同 target 产生不同正文变体，行为不可预测。
- 排障成本：群里看到的是原文，gateway/session 看到的是切片文本，人工对比容易误判。

### 复现步骤（Reproduction）
1. 在 Telegram 群里发送一条“规范说明 + 坏例子”消息，其中坏例子里再次出现多个 bot mention。
2. 观察命中的 bot 收到的 dispatch / record。
3. 期望 vs 实际：
   - 期望：body 仍是整条原文，只是目标判定不同。
   - 实际：body 被裁成局部片段，坏例子可能被脱离上下文地下发。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 修复前证据（本单旧实现，见当前 diff 与本单测试红灯）：
  - `crates/telegram/src/adapter.rs`：旧实现曾在 `TgLineStartMentionGroup` 中持有 `segment_text`，并在 `plan_group_target_action(...)` 中按 target 收集 `line_start_segments` 后拼接为返回 body；本单红灯测试已直接复现该旧行为。
- 修复后代码证据：
  - `crates/telegram/src/adapter.rs:647`：`TgLineStartMentionGroup` 仅保留 `task_text` 与 `mentions`，旧的 `segment_text` 已移除。
  - `crates/telegram/src/adapter.rs:886`：`plan_group_target_action(...)` 只根据命中 target 与 `task_text` 决定判定结果，不再收集正文切片。
  - `crates/telegram/src/adapter.rs:901`：`Dispatch` / `RecordOnly` 命中后统一返回 `body.to_string()`，即统一源正文的最外层 `trim()` 结果。
- 当前测试覆盖：
  - `crates/telegram/src/adapter.rs:1388`：既有 `group_target_plan_*` 断言已全部改为完整原文，不再锁死旧切片语义。
  - `crates/telegram/src/adapter.rs:1512`：覆盖“规范说明 + 坏例子”完整上下文保留。
  - `crates/telegram/src/adapter.rs:1536`：覆盖多目标同收完整原文。
  - `crates/telegram/src/adapter.rs:1575`：覆盖仅最外层 `trim()`。
  - `crates/telegram/src/adapter.rs:1595`：覆盖 quote / inline code / fenced code 只影响判定、不影响最终 body。

## 根因分析（Root Cause）
- A. 修复前，planner 把“目标识别”和“正文裁切”耦合在同一套 `extract_line_start_mention_groups(...)` / `line_start_segments` 逻辑中。
- B. 修复前，`plan_group_target_action(...)` 在命中 target 后直接回传局部 `segment_text`，导致 `Dispatch` / `RecordOnly` 下游接收到的是按 target 派生的正文。
- C. 修复前，“判定扫描正文”和“最终回传正文”缺少明确边界，后者被错误复用了前者衍生出来的 segment。
- D. 修复前，测试也在验证“切片正文”，进一步把错误语义固化成了既有实现。

## 设计原则（Design Principles）
1. planner 只负责判定：
   - target eligibility
   - `addressed`
   - `mode`（`Dispatch` 或 `RecordOnly`）
2. planner 不重写正文语义。
3. 行首点名是目标识别信号，不是正文切段边界。
4. 多目标命中的情况下，不允许出现按目标分别裁切的正文变体。
5. 正文语义必须在 Telegram adapter handoff 前就稳定，不能把“切段修补”丢给 gateway。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 本单直接冻结 Telegram adapter handoff 的 body 口径，后续实现只允许在测试/进度层更新，不再反复改语义。

- 必须：
  - `Dispatch` / `RecordOnly` 的 body 必须使用统一源正文。
  - 行首点名只决定 target eligibility、`addressed`、`mode`、`reason_code`。
  - 多 target 命中时，各 target 收到的 body 必须完全一致。
  - 若现有协议已有外层 envelope，本单只要求 envelope 内正文不再被切片，不顺手改 envelope 本身。
  - 允许的唯一正文归一化是对统一源正文做一次最外层 `trim()`；内部段落、换行、mention 顺序、解释性上下文必须保留。
  - 行首点名识别若继续使用 scan-only sanitize，该 sanitize 结果只能用于判定，绝不能作为最终返回 body 的来源。
  - 除正文口径外，相同输入下既有 `mode` / `addressed` / `reason_code` 判定结果应保持不变。
- 不得：
  - 不得按行首 mention 把正文切成多个片段后再回传。
  - 不得因为后文又出现其他 target mention 而删除解释性段落。
  - 不得为不同 target 生成不同正文切片。
  - 不得重排段落、压缩摘要、只保留“命中的那一段”。
- 应当：
  - 目标判定逻辑尽量复用现有行首点名识别能力，但正文载荷必须与判定逻辑解耦。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐，局部收口）
- 核心思路：
  - 保留现有“行首点名 -> 命中 target / 是否有 task / 是否 addressed”的判定能力。
  - 删除 `Dispatch` / `RecordOnly` 上依赖 `line_start_segments.join(...)` 生成 body 的语义，统一改为返回 `body.trim().to_string()`。
  - 保留 scan-only 判定所需的 `task_text` 等最小信息；若 `segment_text` 不再参与任何行为，必须一并删除，避免残留半套旧概念。
- 优点：
  - 改动集中在 `crates/telegram/src/adapter.rs`，边界清楚。
  - 不把 Telegram 专属复杂性外溢到 gateway/core。
  - 与本单目标严格对齐，不引入新概念。
- 风险/缺点：
  - 需要同步改写现有旧测试断言，否则会继续锁死旧行为。

#### 方案 2（不推荐，下游补丁）
- 核心思路：维持 adapter 切片逻辑不变，在 gateway/core handoff 后再尝试恢复/替换正文。
- 风险/缺点：
  - 错误边界外溢。
  - 无法可靠恢复“统一源正文”。
  - 与“适配层负责冻结正文语义”的收敛方向相反。

### 最终方案（Chosen Approach）
- 采用方案 1。

#### 行为规范（Normative Rules）
- 规则 1：`plan_group_target_action(...)` 在完成 target 判定后，若该 target 命中 `Dispatch` 或 `RecordOnly`，其 `body` 一律取统一源正文的最外层 `trim()` 结果。
- 规则 2：是否 `Dispatch` 仍由“命中该 target + 是否存在 task + dispatch 配置”决定。
- 规则 3：是否 `RecordOnly` 仍由“命中该 target 但不满足 dispatch 条件 / reply_to / 上下文记录”等现有规则决定。
- 规则 4：不同 target 的可变维度仅限于 `mode`、`addressed`、`reason_code`，不包含正文。
- 规则 5：`sanitize_for_group_dispatch_scan(...)` 这类扫描辅助逻辑若保留，只能服务于“命中谁、是否有 task”的判定；不得再产出或参与组装最终 `body`。

#### 示例场景（Examples）
- 示例 A：规范说明 + 坏例子
  - 输入：
    - `@cute_alma_bot ...（前文规范说明）`
    - `我会刻意避免的错误写法（示例）`
    - `@cute_alma_bot @lovely_apple_bot 我先说下：我做了一半，等会再补。`
  - 要求：
    - 目标识别仍可判断 `@lovely_apple_bot` 被命中。
    - 下游 body 必须保持整条原始消息正文，从而保留“这是坏例子”的上层语境。
- 示例 B：同一条消息命中多个目标
  - 输入：
    - `@bot_a 你负责日志`
    - `@bot_b 你负责配置`
    - `下面是统一背景、边界和注意事项...`
  - 要求：
    - `@bot_a` 与 `@bot_b` 的判定结果可以不同。
    - 但两者拿到的 body 都必须是同一份完整原文，不能按目标切成不同片段。

#### 接口与数据结构（Contracts）
- API/RPC：
  - 无新增接口；保持现有 `TgGroupTargetAction` 结构不变。
- 存储/字段兼容：
  - 无字段变更；仅修正 `body` 的生成口径。
- UI/Debug 展示（如适用）：
  - 下游若展示 `body`，将自然看到完整原文；本单不新增 UI 字段。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 本单不新增降级逻辑；命中 target 时直接返回统一源正文。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无新增状态。

#### 安全与隐私（Security/Privacy）
- 默认不新增正文日志。
- 禁止为了调试本单而打印完整 Telegram 正文到结构化日志。

## 适配层归属（Boundary Ownership）
- 这个修复严格属于 Telegram 适配层 / 运行时边界。
- 归属面：
  - Telegram 入站 / 出站规划
  - Telegram adapter helper
  - Telegram 测试
- 明确不允许：
  - 在 gateway/core 中新增 Telegram 群聊正文修补逻辑
  - adapter handoff 后再做按目标切正文

## 验收标准（Acceptance Criteria）【不可省略】
- [x] “规范说明 + 坏例子”类消息在 `Dispatch` / `RecordOnly` 下不再被切片，body 保持完整原文。
- [x] 多目标命中时，各 target 收到同一份 body，不存在按 target 裁出的不同正文变体。
- [x] `Dispatch` 与 `RecordOnly` 的差异只体现在 `mode` / `addressed` / `reason_code`，不影响 body 是否保持原文。
- [x] 仅允许最外层 `trim()`；内部段落、换行、mention 顺序保持不变。
- [x] 行首点名扫描若继续跳过 quote / code block / inline code，仅影响“是否命中”的判定，不影响最终返回 body 继续保留这些原文内容。
- [x] 与旧切片语义绑定的死字段/死路径被清理，不留下 `segment_text` 这类无行为归属的残余结构。
- [x] 除 `body` 外，现有 `mode` / `addressed` / `reason_code` 行为不发生意外漂移。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/telegram/src/adapter.rs`：改写现有 `group_target_plan_*` 测试，使其断言 body 为完整原文，而非切片文本。
- [x] `crates/telegram/src/adapter.rs`：新增“规范说明 + 坏例子”用例，验证坏例子不会脱离上层语境地下发。
- [x] `crates/telegram/src/adapter.rs`：新增“多 target 命中收到同一份原文”用例。
- [x] `crates/telegram/src/adapter.rs`：新增“仅最外层 trim，不破坏内部换行/段落”用例。
- [x] `crates/telegram/src/adapter.rs`：新增“quote / code block 中的 mention 不参与命中，但原文仍完整保留在返回 body 中”用例。
- [x] `crates/telegram/src/adapter.rs`：若删除 `segment_text` 等旧结构，同步以编译与测试覆盖保证不存在残余正文拼接路径。
- [x] `crates/telegram/src/adapter.rs`：保留并必要时改写既有 `mode` / `addressed` / `reason_code` 断言，确保本单未引入判定漂移。

### Integration
- [x] 暂不要求新增跨 crate 集成测试；本单逻辑闭环集中在 Telegram adapter 单测。

### UI E2E（Playwright，如适用）
- [x] 不适用。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：无。
- 手工验证步骤：无；以 adapter 单测闭环为主。

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随 Telegram adapter 修复发布；不加 feature flag。
- 回滚策略：若需回滚，仅回滚 `crates/telegram/src/adapter.rs` 及对应测试；无配置/数据迁移风险。
- 上线观测：重点观察 Telegram 群聊 planner 相关测试与真实群聊中 dispatch/record body 是否与群内原文一致。

## 实施拆分（Implementation Outline）
- Step 1: 在 `crates/telegram/src/adapter.rs` 收口 `plan_group_target_action(...)` 的 body 生成逻辑，移除按 target 拼接 `line_start_segments` 的正文路径。
- Step 2: 保留现有 target 判定能力，但显式分离“判定扫描正文”和“最终返回正文”；前者可 sanitize，后者只能来自统一源正文。
- Step 3: 删除与旧切片语义绑定的死字段/死路径（如不再需要的 `segment_text`）。
- Step 4: 改写/补齐 adapter 单测，冻结完整原文语义。
- 受影响文件：
  - `crates/telegram/src/adapter.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - （原 `docs/plans/2026-03-23-telegram-group-body-integrity-spec.md` 内容已并入本 issue）

## 未决问题（Open Questions）
- Q1: 无。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

# Issue: Skills discovery 边界混乱与事实源打架（one_cut / single_source_of_truth / skills）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Updated: 2026-03-24
- Owners: <TBD>
- Components: skills / agents/prompt / gateway/ui / sandbox
- Affected providers/models: 所有会注入 `<available_skills>` 或展示 skills 列表的会话与页面

**已实现（如有，必须逐条写日期）**
- 2026-03-24：已完成一次上游审查，确认当前 skills 系统把 discovery 事实、启用事实、运行时投影事实混在同一套结构与枚举里，导致 source/path/identity 三类口径互相污染：`crates/skills/src/discover.rs:42`、`crates/skills/src/prompt_gen.rs:31`、`crates/gateway/src/server.rs:4529`
- 2026-03-24：已补充部署视角审查，确认 `project-local skills` 当前依赖进程 `cwd` 与代码仓存在；这不是 deploy-stable 事实源，且曾在现场暴露出 `/home/luy/.moltis/.moltis/skills` 这类错误口径：`crates/config/src/loader.rs:265`

**已覆盖测试（如有）**
- `FsSkillDiscoverer::default_paths()` 覆盖四类默认扫描目录：`crates/skills/src/discover.rs:304`
- registry mixed formats 当前行为测试存在，但它本身也暴露了 source 语义污染：`crates/skills/src/discover.rs:384`

**已知差异/后续优化（非阻塞）**
- 本单是上游边界/owner 收口单，不直接替代 `issues/issue-sandbox-data-mount-root-and-available-skills-path-contract.md`
- 本单先备档与冻结问题边界；实现前可继续增补证据与方案，但不得在代码里继续扩散混合语义

---

## 背景（Background）
- 场景：当前 skills 系统同时承担“发现有哪些 skill”“哪些 skill 处于启用态”“如何向 UI / prompt / registry 投影 skill”三类职责。
- 约束：
  - 必须遵循第一性原则：同一类事实只能有一个 owner。
  - 必须遵循不后向兼容原则：一旦确定新的边界模型，不保留旧的混合 source/path 语义。
  - 必须遵循唯一事实来源原则：不得继续让 `SkillMetadata` 同时承载 discovery、prompt、UI 三套口径。
  - 必须遵循关键路径测试覆盖原则：至少覆盖 discover、catalog build、prompt projection、UI list projection、name collision/identity 几条主路径。
- Out of scope：
  - 本单不直接实现 sandbox path bug 修复；该问题单独由 `issues/issue-sandbox-data-mount-root-and-available-skills-path-contract.md` 收口。
  - 本单不重做 skill authoring UX，不重做安装来源（git/registry）产品流程。
  - 本单不扩展新的 skill 类型。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **discovery 事实**（主称呼）：系统在宿主机侧识别出的“某个 skill 条目存在”及其最小宿主机读取信息。
  - Why：这是 skills 系统的源头事实。
  - Not：它不是 prompt 下发路径，也不是 UI 显示模型，更不是 agent 运行时可读路径。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：host discovery entry / discovered skill

- **启用事实**（主称呼）：某个 skill 条目是否应进入当前 catalog 的 authoritative 判定。
  - Why：local skill 和 installed skill 当前走不同启用链路，这正是混乱源之一。
  - Not：它不是“发现到了就一定启用”，也不是 prompt 是否最终下发。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：enabled state / activation state

- **运行时投影**（主称呼）：为了某个消费面（prompt/UI/registry load/debug）从 discovery 事实导出的视图模型。
  - Why：不同消费面需要不同字段与路径口径。
  - Not：它不是 discovery 源头事实本身。
  - Source/Method：[effective|as-sent]
  - Aliases（仅记录，不在正文使用）：projection / consumer view

- **内容形态**（主称呼）：skill 内容是 `SKILL.md` 目录形态，还是 plugin/adapted 文档形态。
  - Why：这是“如何读取/渲染”的问题，不是“来自哪里”的问题。
  - Not：它不是 source owner。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：content kind / format kind

- **source owner**（主称呼）：skill 条目的来源 owner，例如 project-local、personal-local、installed。
  - Why：它回答“这条 discovery 事实归谁管、由谁启用”。
  - Not：它不是内容形态，也不是 prompt path 分支开关。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：origin / authority

- **deploy-stable 来源**（主称呼）：在“没有代码仓、只有部署产物”的运行环境里仍然成立的 skills 来源。
  - Why：部署用户不应依赖某个 repo cwd 恰好存在，才能得到“系统可用技能”。
  - Not：它不是 workspace-local / cwd-local 的临时来源。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：deployment-stable source / runtime-stable source

- **`Personal` 命名债**（主称呼）：当前 `SkillSource::Personal` 实际表示 `data_dir/skills` 这一类 data-owned local source，而不是真正意义上的“个人技能”产品模型。
  - Why：名字会误导后续设计，把 data-dir local 与 future personal/product semantics 混在一起。
  - Not：它不是已经完成的“个人级 skill owner”定义。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：personal-label debt / misleading personal source

- **技能标识**（主称呼）：系统内唯一定位某个 skill discovery 条目的标识。
  - Why：仅靠 `name` 无法表达跨 source 的唯一性。
  - Not：它不等同于展示名。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：skill identity / discovery key

- **authoritative**：来自真实 discovery / manifest / filesystem 的权威值。
- **estimate**：本地推导或猜测值，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置或 manifest 原始值
  - effective：合并、过滤、默认后的生效值
  - as-sent：最终发给 agent/provider/UI 的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] skills 系统必须把 discovery 事实、启用事实、运行时投影三层职责拆开。
- [ ] `source owner` 与 `内容形态` 必须拆开建模，不得继续共用一个脏枚举语义。
- [ ] UI、prompt、registry load 必须从同一个 authoritative catalog 投影，而不是各自再拼一套逻辑。
- [ ] skill identity 不得继续只用 `name` 作为唯一 key。
- [ ] deploy-stable 来源与 workspace-local 来源必须显式分层；部署运行时不得把 cwd/project-local 误当系统级稳定来源。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须先冻结 authoritative catalog，再派生 prompt/UI/registry 视图。
  - 不得继续让 `SkillMetadata.path` 同时承担 host path、prompt path、UI path 三种语义。
  - 不得继续让“registry 的非 SKILL.md 项”伪装成 `Plugin` source。
  - 不得在 gateway、skills、agents 三层各写半套 discover/enable/project 逻辑。
  - 不得继续把 `SkillSource::Personal` 这个现有命名误当 future personal 模型；要么重命名，要么冻结为 data-dir local 语义并禁止复用。
- 兼容性：这是内部 one-cut 重构；不要求保留旧的混合枚举/字段语义。
- 可观测性：
  - catalog 过滤、投影过滤、identity 冲突必须有结构化日志。
  - 日志至少带 `event`、`reason_code`、`decision`、`skill_name`；必要时带 `skill_id`、`source_owner`、`content_kind`。
- 安全与隐私：
  - prompt/UI/debug 不得误泄露不属于该消费面的宿主机内部路径。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 同一个 `SkillMetadata.path` 既被当 discovery host path，又被 prompt 直接下发给 agent。
2) `SkillSource` 既像来源 owner，又像内容形态，还被当 prompt path 分支开关。
3) UI skills 列表、prompt 注入、registry load 分别从不同入口拼自己的技能视图。
4) registry / project / personal 如果重名，当前 registry 构建会直接被后插入覆盖，缺少唯一身份模型。
5) `project-local skills` 当前实质上绑定 `cwd/.moltis/skills`；一旦运行环境不是代码仓工作区，这个来源就失去稳定含义，现场甚至出现过 `/home/luy/.moltis/.moltis/skills` 这类错误口径。
6) `SkillSource::Personal` 名称看起来像“真正 personal owner”，但代码实际指的是 `data_dir/skills`，命名已经开始误导上游讨论与配置理解。

### 影响（Impact）
- 用户体验：同一个 skill 在 UI、prompt、sandbox、CLI 中表现不一致。
- 可靠性：一处修复会牵动多处语义，回归风险高。
- 排障成本：很难回答“哪个字段是真相、哪个 source 表示什么、为什么这个 skill 被启用/下发/覆盖”。
- 部署可用性：部署用户即使没有代码仓，也会被当前 project-local 发现模型裹挟；这不符合稳定运行时事实源原则。

### 复现步骤（Reproduction）
1. 准备 project skill、personal skill，以及一个 installed registry skill。
2. 观察 discover 结果、UI skills 列表、`<available_skills>` prompt 注入、registry `load_skill(name)` 行为。
3. 期望 vs 实际：
   - 期望：所有消费面从同一个 authoritative catalog 投影，各自语义清楚。
   - 实际：discover / UI / prompt / registry 各自拼装，source/path/identity 口径不一致。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/config/src/loader.rs:265`：`project_local_dir()` 直接把当前 `cwd` 下的 `.moltis` 当作 project root，说明 project-local 来源天然依赖工作区存在。
  - `crates/skills/src/discover.rs:42`：默认 discovery 同时扫描 project、personal、installed-skills、installed-plugins 四类目录。
  - `crates/skills/src/discover.rs:44`：project source 直接取 `project_root.join("skills")`，没有“部署运行时是否允许该来源”的边界。
  - `crates/skills/src/discover.rs:162`：`discover_registry()` 对非 `SKILL.md` 格式的 registry 项直接伪装成 `SkillSource::Plugin`，把来源 owner 和内容形态混为一层。
  - `crates/skills/src/prompt_gen.rs:31`：prompt 注入当前直接拿 `SkillSource::Plugin` 决定 path 拼法，说明 source 已沦为运行时分支开关。
  - `crates/agents/src/prompt.rs:295`、`crates/agents/src/prompt.rs:698`、`crates/agents/src/prompt.rs:988`：三个 prompt 发射点直接复用同一个 `generate_skills_prompt()`，但上游并没有 authoritative projection 层。
  - `crates/skills/src/registry.rs:44`：`InMemoryRegistry::from_discoverer()` 直接用 `name` 做 HashMap key，重名条目会被覆盖。
  - `crates/skills/src/registry.rs:74`：`load_skill(name)` 默认按 `meta.path/SKILL.md` 读取，说明 registry load 也隐含假设“路径就是宿主机 SKILL.md 目录”。
  - `crates/gateway/src/services.rs:793`：UI service 直接消费 raw discover 结果并输出 `path/source`。
  - `crates/gateway/src/server.rs:4529`：web skills API 先从 manifest 拿一份 enabled skills，再追加 discovered personal/project skills，说明 UI 列表不是来自单一 catalog。
- 现场证据：
  - 已出现 `/home/luy/.moltis/.moltis/skills` 这类“把 project-local 错绑到配置/home 目录下”的错误口径，说明 project source 的边界对操作者并不清晰。
- 当前测试覆盖：
  - 已有：default paths、部分 registry mixed formats 行为
  - 缺口：没有测试冻结“source owner vs content kind 分层”“catalog identity”“各消费面共用同一 projection 主路径”

## 根因分析（Root Cause）
- A. skills 系统没有先定义“authoritative catalog 是什么”，就把 discover 结果直接喂给 prompt/UI/registry。
- B. `SkillSource` 设计时把“来源 owner”和“内容形态”揉成一个枚举，后续又被拿来控制 prompt path 分支。
- C. `SkillMetadata.path` 没有边界定义，被不同消费面反复借用，最终变成多义字段。
- D. identity 只靠 `name`，没有单独的 discovery key，所以跨 source 重名天然不稳。
- E. gateway 与 skills crate 没有一个单点 catalog builder，导致 manifest + discover + projection 在多处散写。
- F. project-local 来源当前直接依附 `cwd`，缺少“workspace-only”与“deploy-stable”边界，所以它会误进入系统级可用技能语义。
- G. `Personal` 命名没有和真实 owner 模型对齐，导致 data-dir local 与 future personal/product semantics 被提前混在一起。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - skills 系统必须先构建 authoritative catalog，再为 prompt/UI/registry 各自投影。
  - source owner、内容形态、host load path、runtime projection path 必须分层建模。
  - skill identity 必须独立于展示名 `name`。
  - project-local 来源若保留，必须被明确标记为 workspace-only；不得默认冒充 deploy-stable 来源。
  - `SkillSource::Personal` 这类误导性命名必须在重构中消除，或冻结为严格的 data-dir local 语义并从产品语义上去歧义。
- 不得：
  - 不得继续让 `SkillSource` 同时表达 owner、format、prompt branch 三种语义。
  - 不得继续让 UI 与 prompt 分别从不同入口拼出各自的“可用技能列表”。
  - 不得继续以 `name` 覆盖式聚合跨 source skill 条目。
- 应当：
  - 应当让本地 skill（project/personal）与 installed skill 共享同一 catalog 入口，但保留各自 owner 语义。
  - 应当把 prompt/UI/registry 视图明确标为 projection，而不是直接暴露 raw metadata。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：在 `crates/skills` 内新增单一 authoritative catalog 层，先把 discovery + enable 统一成 normalized entries，再由 prompt/UI/registry 各自做投影。
- 优点：
  - 高内聚，边界清楚。
  - 可以一刀切清理 `SkillSource` 与 `SkillMetadata.path` 的多义污染。
  - 后续 sandbox path、UI 列表、registry load 都有统一上游。
- 风险/缺点：
  - 需要一次性动到 skills、gateway、agents 三个消费面。

#### 方案 2（不推荐）
- 核心思路：保留现有 discover 结构，只在各消费面分别补 mapping/shim。
- 风险/缺点：
  - 继续扩散复杂度。
  - 继续保留多套 owner/path/source 语义。
  - 不符合 one-cut 与唯一事实来源原则。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：authoritative catalog 必须成为 skills 系统的唯一事实源；discoverer 只负责采集原始条目，不直接服务 prompt/UI。
- 规则 2：`source owner` 与 `内容形态` 必须拆分为两个独立字段；旧 `SkillSource` 语义不得原样保留。
- 规则 3：host load path 只用于宿主机读取；prompt path / UI 展示 path 必须由 projection 层单独生成。
- 规则 4：`SkillMetadata` 若继续保留，只能承载单一层级事实；不得再被多个消费面跨层复用。
- 规则 5：skill identity 必须独立存在；任何 catalog 构建都不得仅以 `name` 作为唯一 key。
- 规则 6：gateway skills API、prompt `<available_skills>`、registry load 都必须从同一 authoritative catalog 出发，不允许多入口各自再 discover。
- 规则 7：project-local 来源必须显式标记为 workspace-only；部署运行时若没有对应 workspace 事实，不得把它当“系统可用 skill 来源”。
- 规则 8：现有 `Personal` 命名必须在实现前冻结去向；不得一边保留旧标签，一边再引入新的真正 personal owner 语义。

#### 接口与数据结构（Contracts）
- API/RPC：
  - UI skills 列表必须来自 catalog projection，而不是 raw discover + manifest ad-hoc merge。
  - prompt `<available_skills>` 必须来自 prompt projection，而不是 raw metadata path 拼接。
- 存储/字段兼容：
  - 允许内部结构 one-cut 重命名/拆分，不保留旧的混合字段语义。
- UI/Debug 展示（如适用）：
  - debug 应能区分 `source_owner`、`content_kind`、`host_path`、`prompt_path(method=as-sent)`。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若发现 identity 冲突：必须记录 `skill_identity_conflict`，禁止 silent overwrite。
  - 若某消费面请求了当前 content kind 不支持的投影：必须记录 `unsupported_skill_projection_for_content_kind`。
  - 若 catalog entry 缺 authoritative owner 或 host path：必须记录 `invalid_skill_catalog_entry`。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无额外队列；旧 ad-hoc projection 路径必须删除，不保留 fallback。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - UI / prompt 默认不得泄露不属于该消费面的宿主机内部路径。
  - 结构化日志可打印 `skill_id`、`skill_name`、`source_owner`、`content_kind`，不得打印 skill 正文。
- 禁止打印字段清单：
  - skill 正文
  - 无关宿主机目录清单
  - token / secret

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] skills 系统存在单一 authoritative catalog，prompt/UI/registry 都从它投影。
- [ ] `source owner` 与 `内容形态` 已分层建模，不再共用脏枚举。
- [ ] `host load path` 与 `prompt path` 已分层建模，不再共用一个多义字段。
- [ ] catalog identity 不再仅靠 `name`，重名场景不会 silent overwrite。
- [ ] gateway skills API 不再做 manifest + discover 的 ad-hoc 双拼。
- [ ] `issue-sandbox-data-mount-root-and-available-skills-path-contract` 可以作为该 catalog 的一个下游投影规则实现，而不是继续单独兜路径。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/skills`：catalog builder 覆盖 local / installed / plugin-adapted 三类主路径。
- [ ] 覆盖 `source owner` 与 `内容形态` 分层，不再出现 registry entry 被伪装成 plugin source。
- [ ] 覆盖 identity 冲突会被显式拒绝/记录，而不是覆盖写入。
- [ ] 覆盖 host load projection 与 prompt projection 分离。

### Integration
- [ ] `crates/agents/src/prompt.rs`：prompt skills 列表来自 catalog projection，而不是 raw discover 结果。
- [ ] `crates/gateway/src/server.rs` / `crates/gateway/src/services.rs`：UI skills 列表来自同一 catalog projection，不再双拼。
- [ ] `crates/skills/src/registry.rs`：load path 仅消费 host-side projection，不再偷读 prompt path 语义。

### UI E2E（Playwright，如适用）
- [ ] 不适用；当前主单聚焦后端边界与契约

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若短期内无法把 UI 与 prompt 双消费面的端到端链路统一自动化，可先以 catalog + projection 集成测试为主。
- 手工验证步骤：
  1. 同时准备 project、personal、installed skill。
  2. 打开 skills UI，确认列表来源一致、无重复/无覆盖。
  3. 触发一轮带 `<available_skills>` 的会话，确认和 UI 中看到的是同一组 authoritative entries。
  4. 在宿主机直接读取某个 skill，确认 load path 与 prompt path 不再混用。

## 发布与回滚（Rollout & Rollback）
- 发布策略：作为 one-cut 内部重构一次性切换，无 feature flag。
- 回滚策略：回退整组 catalog/projection 重构；风险是恢复当前 source/path/identity 混乱。
- 上线观测：
  - `skill_identity_conflict`
  - `unsupported_skill_projection_for_content_kind`
  - `invalid_skill_catalog_entry`

## 实施拆分（Implementation Outline）
- Step 1: 冻结 authoritative catalog 的最小字段集与 identity 模型。
- Step 2: 拆开 `source owner`、`内容形态`、`host_path`、projection path。
- Step 3: 让 prompt projection、UI projection、registry host-load projection 统一从 catalog 出发。
- Step 4: 删除旧的 ad-hoc merge / raw metadata 直出路径。
- 受影响文件：
  - `crates/skills/src/types.rs`
  - `crates/skills/src/discover.rs`
  - `crates/skills/src/registry.rs`
  - `crates/skills/src/prompt_gen.rs`
  - `crates/agents/src/prompt.rs`
  - `crates/gateway/src/services.rs`
  - `crates/gateway/src/server.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-sandbox-data-mount-root-and-available-skills-path-contract.md`
  - `issues/done/issue-sandbox-public-data-view-skills.md`
- Related commits/PRs：
  - `21dd4ae`：sandbox 下已先行收口 `data_dir/skills` 可见路径契约，作为本单的下游投影约束
- External refs（可选）：
  - <N/A>

## 未决问题（Open Questions）
- Q1：authoritative catalog 应放在 `crates/skills` 的独立 builder，还是直接替换现有 `discover + registry` 边界？
- Q2：identity 是用显式 `skill_id`，还是 `(source_owner, locator)` 结构键；需要在实现前冻结。
- Q3：project-local 来源在 one-cut 后是否只允许作为 workspace-only source 存在，并从 deployed runtime 的默认 catalog 中移除？
- Q4：现有 `SkillSource::Personal` 是直接重命名为 data-dir local，还是由新的 owner 模型彻底替换；实现前必须冻结。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

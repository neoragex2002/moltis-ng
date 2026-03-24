# Issue: sandbox `data_mount_source` 根口径错误 + `<available_skills>` host 路径泄露导致 `data_dir/skills` 不可读（sandbox / data_mount / skills / prompt）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-24
- Owners: <TBD>
- Components: tools/sandbox / skills / agents/prompt / config
- Affected providers/models: 所有启用 sandbox exec 且会注入 `<available_skills>` 的会话

**已实现（如有，必须逐条写日期）**
- 2026-03-12：sandbox 公开数据视图已支持复制 `data_dir/skills` 白名单目录：`crates/tools/src/sandbox.rs:403`
- 2026-03-24：已完成现场复核，确认当前“sandbox skill 不可读”不是单点 bug，而是两段契约同时断裂：`data_mount_source` 根口径错误导致 `data_dir/skills` 根本未进入 `/moltis/data`；即使进入后，`<available_skills>` 仍会继续下发 host path
- 2026-03-24：用户已把 live 配置改回 `data_mount_source = "/home/luy/.moltis/data"`；当前运行中的 `dm-main` sandbox 已能看到 `/moltis/data/skills`，说明第 1 层问题是“错误配置未被 strict reject”而不是复制逻辑本身恒定失效
- 2026-03-24：bind 模式新增 strict reject；`data_mount_source` 若不解析到 effective `data_dir`，直接拒绝并写结构化日志：`crates/tools/src/sandbox.rs:1030`
- 2026-03-24：sandbox skill path contract 已收口到 prompt 组装单点；sandbox 下只把 `SkillSource::Personal` 投影到 `/moltis/data/skills/<basename>/SKILL.md`，其他 source 直接过滤：`crates/agents/src/prompt.rs:268`、`crates/skills/src/prompt_gen.rs:74`
- 2026-03-24：`skills_md`、legacy prompt、OpenAI Responses runtime snapshot 已统一复用同一 resolver，不再在 sandbox 下泄露 host path：`crates/agents/src/prompt.rs:310`、`crates/agents/src/prompt.rs:711`、`crates/agents/src/prompt.rs:1001`
- 2026-03-24：配置模板与系统提示文档已同步更新 sandbox contract：`crates/config/src/template.rs:240`、`docs/src/system-prompt.md:172`

**已覆盖测试（如有）**
- strict reject + 结构化拒绝日志：`crates/tools/src/sandbox.rs:2982`
- 规范化后等价路径不误拒：`crates/tools/src/sandbox.rs:3018`
- public view 复制 `skills/` 现状回归：`crates/tools/src/sandbox.rs:3110`
- sandbox guest path 投影与 basename 规则：`crates/skills/src/prompt_gen.rs:207`
- sandbox source 过滤：`crates/skills/src/prompt_gen.rs:228`
- legacy prompt 的 sandbox guest path / filter / reason_code 日志：`crates/agents/src/prompt.rs:1389`
- non-sandbox host-path 保持不变：`crates/agents/src/prompt.rs:1456`
- `skills_md` 注入后的 canonical Type4 prompt 使用 guest path：`crates/agents/src/prompt.rs:1933`
- OpenAI Responses runtime snapshot 在 sandbox 下不再泄露 host path：`crates/agents/src/prompt.rs:1761`
- OpenAI Responses runtime snapshot 在“全部 source 被过滤”时会正确回落为“（无）”，不再留下空技能段：`crates/agents/src/prompt.rs:1990`

**已知差异/后续优化（非阻塞）**
- 本单只聚焦 bind 模式下 `data_dir/skills` 这一类本地 skills 的 sandbox 可读性闭环；不顺手重做 skills discovery 总体边界
- `project` / `installed-skills` / `installed-plugins` 在 sandbox 下的可读性不在本单恢复；当前统一按 strict filter 处理

---

## 背景（Background）
- 场景：agent 在 sandbox 中执行时，需要通过 `/moltis/data` 读取 `data_dir/skills` 下的本地 skill 文件，同时从 system prompt 的 `<available_skills>` 获得可读路径。
- 约束：
  - 必须遵循第一性原则：`data_mount_source` 的 bind 根口径只能有一个 authoritative 解释。
  - 必须遵循不后向兼容原则：若当前 bind source 不是 effective `data_dir`，不得继续“猜老布局/猜 home root”。
  - 必须遵循唯一事实来源原则：sandbox guest 目录与 prompt as-sent path 都必须从同一 authoritative runtime contract 推出。
  - 必须遵循关键路径测试覆盖原则：至少覆盖 bind source 校验、public view 复制、prompt path projection、strict reject 四条主路径。
- Out of scope：
  - 本单不恢复 `installed-skills` / `installed-plugins` 在 sandbox 中的可读性。
  - 本单不重做整个 skills discovery 系统；该上游问题由 `issues/issue-skills-discovery-boundary-and-source-of-truth-one-cut.md` 单独收口。
  - 本单不改用户如何安装/管理 skill。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **sandbox data mount 根**（主称呼）：bind 模式下用于生成 sandbox public data view 的宿主机根目录。
  - Why：`prepare_public_data_view()` 会直接从这个根目录复制 `USER.md`、`PEOPLE.md`、`skills/` 等内容。
  - Not：它不是任意 host 路径，更不是“看起来像 Moltis home root 的目录”。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：bind source root / public-view source root

- **effective `data_dir`**（主称呼）：当前运行时真正生效的 Moltis 数据目录。
  - Why：sandbox bind 模式要挂进去的公共数据只能来自这里。
  - Not：它不是 `~/.moltis` home root，也不是 config 目录。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：runtime data dir

- **公开数据视图**（主称呼）：宿主机侧生成的 `.sandbox_views/<sandbox_key>` 白名单目录树，最终挂载到 guest `/moltis/data`。
  - Why：sandbox 读到的是它，而不是原始 host data dir。
  - Not：它不是整个 `data_dir` 的完整镜像。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：public data view

- **本地 data-dir skills**（主称呼）：当前存放在 effective `data_dir/skills` 下、可作为正式运行时能力暴露给 agent 的本地 skills。
  - Why：这是本单唯一承诺在 sandbox 下闭环的一类 skills。
  - Not：它不包含 project-local skills，也不包含 installed registry/plugin skills。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：data-owned local skills / runtime local skills

- **技能路径契约**（主称呼）：`<available_skills>` 中写给 agent 的 skill 文件路径口径。
  - Why：agent 会按这个路径直接读 `SKILL.md` 或插件文档。
  - Not：它不是 discovery 阶段的 host path。
  - Source/Method：[as-sent]
  - Aliases（仅记录，不在正文使用）：skill path contract

- **宿主机路径**（主称呼）：skill 在 gateway/发现进程所在文件系统中的真实路径。
  - Why：skill discovery 与 host-side load 使用它。
  - Not：它不保证在 sandbox 容器内可见。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：host path

- **沙盒可见路径**（主称呼）：agent 在 sandbox 内实际可访问的 skill 路径。
  - Why：最终执行读取是按这个路径发生的。
  - Not：它不是宿主机路径字符串复用。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：guest path / runtime-visible path

- **技能目录 basename**（主称呼）：`SkillMetadata.path` 指向目录的最后一级名称。
  - Why：guest 可见路径必须对齐磁盘上的目录层级。
  - Not：它不等同于 `SkillMetadata.name`。
  - Source/Method：[authoritative]
  - Aliases（仅记录，不在正文使用）：directory basename / skill dir name

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] bind 模式下，sandbox `data_dir/skills` 必须真实进入 `/moltis/data`。
- [x] sandbox 模式下，`<available_skills>` 中每个下发条目的路径必须是 agent 运行时可读路径。
- [x] 当前 guest 不可读的 skill source 不得继续伪装成可用 skill 下发。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把 bind 模式下 `data_mount_source` 的 authoritative 语义冻结为 effective `data_dir`（比较口径是“规范化后指向同一目录”，不是字符串猜测旧布局）。
  - 不得继续接受“home root 也能凑合跑”“猜测旧布局”的兼容尾巴。
  - 必须以单点规则完成 sandbox 路径映射或过滤。
  - 不得在 sandbox、prompt、discovery 三层分别硬编码半套 rewrite 规则。
- 兼容性：不兼容当前错误的 bind source 口径；命中错误根目录时直接报错并给 remediation。
- 可观测性：
  - strict reject 与 projection filter 都必须记录结构化日志，至少带 `event`、`reason_code`、`decision`、`policy`。
  - 日志不得打印 skill 正文。
- 安全与隐私：
  - sandbox prompt 中不得泄露宿主机私有绝对路径。
  - public view 不得因为修复而扩大到整个 home root。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 当 bind source 被配置成 home root（例如 `/home/luy/.moltis`）而不是 effective `data_dir` 时，sandbox guest `/moltis/data` 只会出现 `USER.md` 与 `PEOPLE.md`，不会出现 `skills/` 或 `.moltis/skills`。
2) `prepare_public_data_view()` 却按“source 根目录下直接存在 `skills/` 与 `.moltis/skills`”复制，导致历史错误配置现场中的 `data_dir/skills` 没有进入 guest。
3) 用户已把 live 配置改回 `data_mount_source = "/home/luy/.moltis/data"`；当前运行中的 `dm-main` 容器里 `/moltis/data/skills` 已恢复可见，证明第 1 层断裂依赖于错误 bind source。
4) 即使 `data_dir/skills` 进入 guest，`<available_skills>` 当前仍直接下发 host path，agent 在 sandbox 中依然读不到。

### 影响（Impact）
- 用户体验：`data_dir/skills` 明明存在于 host data dir，但 sandbox 中完全不可见或路径错误，行为非常反直觉。
- 可靠性：所有依赖 `data_dir/skills` 本地文件的 sandbox 流程都会失败。
- 排障成本：问题表象像“skills 没挂载”或“prompt 路径错了”，实际是两段契约同时断裂。

### 复现步骤（Reproduction）
1. 配置 bind 模式 sandbox，`tools.exec.sandbox.data_mount_source = "/home/luy/.moltis"`。
2. 在 host `data_dir=/home/luy/.moltis/data/skills` 下准备本地 skills。
3. 进入 sandbox 容器查看 `/moltis/data`。
4. 触发一轮注入 `<available_skills>` 的会话。
5. 期望 vs 实际：
   - 期望：`/moltis/data/skills/...` 可见，prompt 中下发 guest-readable path。
   - 实际：guest 中没有 skills 目录；prompt 仍下发 host path。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 现场证据：
  - 当前 live 配置已改正：`~/.moltis/config/moltis.toml:239`、`~/.moltis/config/moltis.toml:244`
  - 当前 live bind source：`data_mount_source = "/home/luy/.moltis/data"`
  - 当前 live mount source：容器 `moltis-jarvis-sandbox-dm-main` 绑定 `/home/luy/.moltis/data/.sandbox_views/dm-main -> /moltis/data`
  - 当前 live public view：`/home/luy/.moltis/data/.sandbox_views/dm-main` 已包含 `skills/`
  - 历史错误配置复现现场：当 bind source 曾指向 `/home/luy/.moltis` 时，生成出的 `.sandbox_views/dm-main` 只有 `USER.md` 与 `PEOPLE.md`
- 代码证据：
  - `crates/config/src/template.rs:233`：配置模板已声明这里挂的是 Moltis `data_dir`。
  - `crates/tools/src/sandbox.rs:398`：`prepare_public_data_view()` 把 `base_data_dir` 当成包含 `USER.md`、`PEOPLE.md`、`skills/` 的根目录。
  - `crates/tools/src/sandbox.rs:1026`：bind 模式直接把 `data_mount_source` 传给 `prepare_public_data_view()`，没有校验它是否等于 effective `data_dir`。
  - `crates/agents/src/prompt.rs:295`、`crates/agents/src/prompt.rs:370`：`<available_skills>` 不是独立旁路文本，而是先生成 `skills_md` 模板变量，再注入 Type4 system prompt。
  - `crates/skills/src/prompt_gen.rs:31`：`<available_skills>` 当前仍直接把 discovery host path 转成 prompt `path`，未区分 host vs sandbox。
- 当前测试覆盖：
  - 已有：public view 复制 `data_dir/skills` 的单元测试，但测试传入的是“正确的 data_dir 根”
  - 缺口：没有测试覆盖“bind source 指错根目录时必须 strict reject”，也没有覆盖“sandbox path 必须是 guest path”

## 根因分析（Root Cause）
- A. bind 模式下，`data_mount_source` 缺少“必须等于 effective `data_dir`”的强约束，现场配置可以指向 `~/.moltis` home root。
- B. `prepare_public_data_view()` 按 data-dir 布局复制白名单内容，但运行时没有验证传入根目录是否符合该布局。
- C. 因此在错误 bind source 场景下，public view 只复制出空的 `USER.md` / `PEOPLE.md`，真实 `data_dir/skills` 没有进入 guest；而当 bind source 改正为 effective `data_dir` 后，这一层会恢复正常。
- D. 同时，prompt 组装阶段把 discovery 的宿主机路径原样写进 `<available_skills>`，没有根据 runtime 是否 sandbox 做 path contract 转换。
- E. 两层断裂叠加后，现场表现为“skills 目录没进容器 + prompt 路径也错”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - bind 模式下，`tools.exec.sandbox.data_mount_source` 在规范化后必须与 effective `data_dir` 指向同一目录。
  - sandbox 模式下，`<available_skills>` 只能暴露 guest 中真实可读的 `data_dir/skills` 路径。
  - prompt 组装必须成为 skill path 口径切换的单一收口点。
  - `skills_md` 作为 system prompt 模板变量注入时，不得继续把 host path 带入最终 Type4 prompt。
- 不得：
  - 不得继续接受 home root / 旧布局 / 任意 host 根目录作为 bind 模式 source。
  - 不得让 agent 看到“名义可用、实际不可读”的 skill 条目。
  - 不得在 discovery、prompt、sandbox 三层分别各写一套 rewrite 逻辑。
- 应当：
  - 应当为 strict reject 与 projection filter 留下结构化日志，方便排障。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 第 1 步：在 bind 模式下硬性校验 `data_mount_source` 与 effective `data_dir` 规范化后指向同一目录；不满足直接报错。
  - 第 2 步：保留 discovery 继续产出宿主机路径；在 prompt 组装阶段根据 runtime 是否 sandbox，把 `data_dir/skills` 条目映射为 guest path，其他 source 直接过滤。
- 优点：
  - 根因闭环，不靠猜测。
  - 符合 strict one-cut，不保留旧布局 compat。
  - 复杂度收敛在 sandbox contract + prompt projection 两个单点。
- 风险/缺点：
  - 现有错误配置会直接失败，需要明确 remediation。

#### 方案 2（不推荐）
- 核心思路：在 `prepare_public_data_view()` 里兼容 home root 与 data_dir 两种根布局，同时在 prompt 里继续补 path rewrite。
- 风险/缺点：
  - 继续把错误配置合法化。
  - 保留双布局语义，违反 one-cut 与唯一事实来源原则。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：bind 模式下，`tools.exec.sandbox.data_mount_source` 与 effective `data_dir` 在规范化后必须解析到同一目录；不满足时直接失败并记录结构化拒绝日志。
- 规则 2：`prepare_public_data_view()` 继续只接受 data-dir 布局根目录，不新增 home-root fallback。
- 规则 3：当 runtime 为 sandbox 时：
  - `data_dir/skills` 这一类本地条目必须映射到 `/moltis/data/skills/<技能目录 basename>/SKILL.md`。
  - `SkillSource::Project` / `SkillSource::Registry` / `SkillSource::Plugin` / `source=None` 必须直接过滤，不得继续下发。
- 规则 4：当 runtime 非 sandbox 时，继续保持现有 host-path 行为，不借本单改变 project / registry / plugin 的可见性与路径口径。
- 规则 5：sandbox 路径解析只允许基于 `SkillSource` + `SkillMetadata.path` 的目录 basename + 冻结的 guest 路径契约决定；不得新增 guest 文件探测、host 前缀猜测、legacy root fallback 等兼容性分支。
- 规则 6：runtime-sensitive 的 resolve / filter 必须只在 prompt 组装单点完成；discovery 继续产出 host metadata，`skills_md` 模板变量必须复用同一个 resolver 结果；不得在模板组装阶段绕过 resolver 再次拼接 host path，也不得把 sandbox runtime 判断散落进 discovery。

#### `SkillSource × runtime` 冻结规则表

| Runtime | SkillSource | Prompt path / decision | Why |
| --- | --- | --- | --- |
| non-sandbox | Personal | `<host>/skills/<技能目录 basename>/SKILL.md` | 当前进程直接读宿主机 |
| non-sandbox | Project | `<host>/<project>/.moltis/skills/<技能目录 basename>/SKILL.md` | 保持当前 host 行为；不在本单改动 |
| non-sandbox | Registry | `<host>/installed-skills/.../SKILL.md` | 当前进程直接读宿主机 |
| non-sandbox | Plugin | `<host>/installed-plugins/.../*.md`（path as-is） | plugin 本就不是 `SKILL.md` 目录 |
| sandbox | Personal | `/moltis/data/skills/<技能目录 basename>/SKILL.md` | `data_dir/skills` 经 public view 进入 guest |
| sandbox | Project | filter | 当前单子不处理 project source |
| sandbox | Registry | filter | 当前 guest 不可见 |
| sandbox | Plugin | filter | 当前 guest 不可见 |

> 上表是本单唯一权威规则。实现不得另起第二套“先试试看 guest 有没有这个文件”的分支。

#### 接口与数据结构（Contracts）
- API/RPC：无对外 API 变更。
- 存储/字段兼容：无持久化 schema 变更。
- Prompt Contract：
  - `<available_skills>.path` 定义为“agent 当前运行环境中可直接读取的实际路径”。
  - `path` 不再等同于 discovery 原始路径。
  - `skills_md` 作为模板变量注入后，其内部 `<available_skills>` 内容必须与上述 as-sent path 契约一致。
- 配置契约：
  - bind 模式下 `tools.exec.sandbox.data_mount_source` 在规范化后必须与 effective `data_dir` 指向同一目录。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若 bind 模式 `data_mount_source != effective data_dir`：记录 `event="sandbox_data_mount_rejected"`、`reason_code="sandbox_bind_source_must_equal_data_dir"`、`decision="reject"`、`policy="sandbox_data_mount_contract"`，并给 remediation。
  - 若 `SkillSource::Project` / `SkillSource::Registry` / `SkillSource::Plugin` / `source=None` 在 sandbox 下被过滤：记录 `event="skills_prompt_entry_filtered"`、`reason_code="skill_source_not_exposed_in_sandbox"` 或 `unknown_skill_source_for_prompt_path`、`decision="filter"`、`policy="sandbox_skill_path_contract"`。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无额外队列；旧错误配置路径必须自然失败，不保留 fallback。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - sandbox prompt 中不得出现宿主机私有绝对路径。
  - 日志可打印 `skill_name`、`skill_source`、`configured_source`、`effective_data_dir`，不得打印 skill 正文。
- 禁止打印字段清单：
  - skill 正文
  - token / secret
  - 无关宿主机目录清单

## 验收标准（Acceptance Criteria）【不可省略】
- [x] bind 模式下，错误的 `data_mount_source` 会被直接拒绝，而不是继续生成残缺 public view。
- [x] bind 模式下，正确的 effective `data_dir` 会把 `data_dir/skills/` 复制进 guest `/moltis/data`。
- [x] sandbox 模式下，`<available_skills>` 不再出现 `/home/...` 这类宿主机 skill 路径。
- [x] sandbox 模式下，最终 Type4 system prompt 中经 `skills_md` 注入的 `<available_skills>` 也不再出现 `/home/...` 这类宿主机 skill 路径。
- [x] `data_dir/skills` 这类本地条目在 sandbox 模式下输出 `/moltis/data/skills/<技能目录 basename>/SKILL.md` 并可被 agent 读取。
- [x] `project` / `registry` / `plugin` / `source=None` 在 sandbox 模式下不会继续作为“可用 skill”下发给 agent。
- [x] non-sandbox 模式下，现有 host-path 行为保持不变；本单不得顺手改变 project / registry / plugin 的 prompt 暴露规则。
- [x] strict reject 与 projection filter 都有结构化日志，且无 silent degrade。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/tools/src/sandbox.rs`：bind 模式下 `data_mount_source != effective data_dir` 直接报错，并校验 `sandbox_bind_source_must_equal_data_dir`。
- [x] `crates/tools/src/sandbox.rs`：bind 模式下对 `data_mount_source` 与 effective `data_dir` 做规范化后比较，等价路径不误拒。
- [x] `crates/tools/src/sandbox.rs`：bind 模式下传入正确 `data_dir` 时，public view 复制 `skills/`。
- [x] `crates/skills/src/prompt_gen.rs` 或对应 resolver：覆盖 sandbox / non-sandbox 两种路径生成。
- [x] 覆盖 `data_dir/skills` 条目在 sandbox 下映射为 guest path。
- [x] 覆盖 project/registry/plugin/source=None 在 sandbox 下被过滤并留下对应 `reason_code`。
- [x] 覆盖 `SkillMetadata.name` 与目录 basename 不同的用例，冻结“路径取 basename，不取 name”。

### Integration
- [x] `crates/agents/src/prompt.rs`：`build_canonical_system_prompt_v1()` 在 sandbox 模式下生成的 `<available_skills>` 不出现宿主机 skill 绝对路径。
- [x] `crates/agents/src/prompt.rs`：`build_system_prompt_with_session_runtime()` 在 sandbox 模式下同样不出现宿主机 skill 绝对路径。
- [x] `crates/agents/src/prompt.rs`：通过 `skills_md` 模板变量注入后的最终 Type4 prompt，不出现宿主机 skill 绝对路径。

### UI E2E（Playwright，如适用）
- [ ] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若短期内没有稳定的真实 sandbox 端到端测试链路，可先用 sandbox public-view + prompt projection 集成测试闭环。
- 手工验证步骤：
  1. 将 bind 模式 `data_mount_source` 改为 effective `data_dir`。
  2. 重建或重启对应 sandbox 容器。
  3. 在容器内确认 `/moltis/data/skills` 存在。
  4. 发起一轮注入 `<available_skills>` 的对话。
  5. 检查 as-sent prompt 或日志，确认 skill path 为 guest path，而不是 host path。

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随代码发布，无 feature flag。
- 回滚策略：回退整组 sandbox bind source 校验 + prompt projection 逻辑；风险是恢复当前“残缺 public view + host path 泄露”双问题。
- 上线观测：
  - `sandbox_bind_source_must_equal_data_dir`
  - `skills_prompt_entry_filtered`
  - 仍出现 `/home/` skill path 的 prompt/调试日志

## 实施拆分（Implementation Outline）
- Step 1: 在 sandbox bind 合同处冻结“`data_mount_source` 与 effective `data_dir` 规范化后指向同一目录”，错误配置直接拒绝。
- Step 2: 确保 public view 只从正确 `data_dir` 根复制 `data_dir/skills` 白名单内容。
- Step 3: 在 prompt 组装单点新增 skill path resolve / filter 逻辑。
- Step 4: 补齐 sandbox + prompt 两侧测试，并同步更新相关文档与 remediation 文案。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/skills/src/prompt_gen.rs`
  - `crates/agents/src/prompt.rs`
  - `crates/config/src/template.rs`
  - `docs/src/system-prompt.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-skills-discovery-boundary-and-source-of-truth-one-cut.md`
  - `issues/done/issue-sandbox-public-data-view-skills.md`
  - `issues/done/issue-sandbox-fixed-data-dir-mountpoint.md`
- Related commits/PRs：
  - <pending>
- External refs（可选）：
  - <N/A>

## 未决问题（Open Questions）
- Blocking：无。代码、测试、文档已落地；本单可关。
- Non-blocking：
  - volume 模式下 local skills 的 sandbox 可读性是否要单独出单收口；本单不扩 scope。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

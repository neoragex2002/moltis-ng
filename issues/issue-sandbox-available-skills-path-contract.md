# Issue: sandbox 下 `<available_skills>` 泄露 host 路径导致 skill 文件不可读（sandbox / skills / prompt）

## 实施现状（Status）【增量更新主入口】
- Status: TODO
- Priority: P1
- Updated: 2026-03-24
- Owners: <TBD>
- Components: agents/prompt, skills, tools/sandbox
- Affected providers/models: 所有会注入 `<available_skills>` 且启用 sandbox exec 的会话

**已实现（如有，写日期）**
- 2026-03-12：sandbox 公开数据视图已同步 personal / project skills 白名单目录：`crates/tools/src/sandbox.rs:396`
- 当前 `<available_skills>` 已向 prompt 注入 skill `path` 字段：`crates/skills/src/prompt_gen.rs:29`

**已覆盖测试（如有）**
- sandbox 公开数据视图包含 `skills/` 与 `.moltis/skills/`：`crates/tools/src/sandbox.rs:2953`
- prompt 生成会输出 `SKILL.md` 路径：`crates/skills/src/prompt_gen.rs:67`

**已知差异/后续优化（非阻塞）**
- 当前 issue 先聚焦 `<available_skills>` 的路径契约；registry/plugin skill 在 sandbox 中的可读性需一并核查，但不得以局部 hardcode 方式扩散复杂度。

---

## 背景（Background）
- 场景：agent 在 sandbox 中执行时，会从 system prompt 的 `<available_skills>` 读取 skill 路径，再尝试打开对应 `SKILL.md`。
- 约束：
  - sandbox 内数据目录固定为 `/moltis/data`。
  - sandbox 可见的是公开数据视图，不是宿主机原始绝对路径。
  - `<available_skills>` 里的 `path` 必须是 agent 运行时真实可读的路径，而不是发现阶段的宿主机路径。
- Out of scope：
  - 本单不重做整个 skill 发现体系。
  - 本单不改用户如何安装/管理 skill。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **技能路径契约**（主称呼）：`<available_skills>` 中写给 agent 的 skill 文件路径口径。
  - Why：agent 会按这个路径直接读 `SKILL.md`。
  - Not：它不是后台发现时的宿主机扫描路径，也不是仅供 UI 展示的字符串。
  - Source/Method：[as-sent] 由 prompt 组装阶段最终写入 system prompt。
  - Aliases（仅记录，不在正文使用）：skill path contract

- **宿主机路径**（主称呼）：skill 在 gateway/发现进程所在文件系统中的真实路径。
  - Why：skill discovery 当前先在宿主机侧完成。
  - Not：它不保证在 sandbox 容器内可见。
  - Source/Method：[authoritative] 由本地文件系统发现结果给出。
  - Aliases（仅记录，不在正文使用）：host path

- **沙盒可见路径**（主称呼）：agent 在 sandbox 内实际可访问的 skill 路径。
  - Why：agent 最终读取文件是按这个路径执行。
  - Not：它不是宿主机绝对路径的简单字符串复用。
  - Source/Method：[effective] 由 sandbox mount 与公开数据视图共同决定。
  - Aliases（仅记录，不在正文使用）：guest path / runtime-visible path

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] sandbox 模式下，`<available_skills>` 中的每个 skill 路径必须是 agent 运行时可读路径。
- [ ] sandbox 模式下，不得再把宿主机绝对路径直接写入 `<available_skills>`。
- [ ] 若某类 skill 在 sandbox 中不可读，则不得继续把它伪装成可用 skill 暴露给 agent。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须保证“发现路径”和“prompt 下发路径”语义分离。
  - 不得在多个模块分别硬编码半套 path rewrite 规则。
  - 必须以单点规则完成 sandbox 路径映射或过滤。
- 兼容性：不要求兼容旧的错误 prompt 路径；命中旧口径时直接改为新口径。
- 可观测性：
  - 当 skill 因 sandbox 不可读而被过滤时，应记录结构化日志，至少带 `event`、`reason_code`、`decision`、`policy`。
  - 日志不得打印 skill 正文。
- 安全与隐私：不得在 prompt 中泄露宿主机私有绝对路径。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) sandbox 会话中，agent 按 `<available_skills>` 提示去读取 skill 文件时，访问到了宿主机路径，例如：
   - `/home/luy/.moltis/data/skills/template-skill/SKILL.md`
2) 该路径在 sandbox 内不存在，进而报：
   - `ls: cannot access '/home/luy/.moltis/data/skills/template-skill': No such file or directory`
3) 用户直觉上会误判为“skill 没挂载进去”，但实际 mount 已存在，错的是 prompt 里的路径。

### 影响（Impact）
- 用户体验：skill 明明已发现、已挂载，agent 却读不到，行为非常反直觉。
- 可靠性：所有依赖 `<available_skills>` 打开本地 skill 文件的 sandbox 流程都会失效。
- 排障成本：问题表象像 mount 丢失，实际根因在 prompt contract，定位成本高。

### 复现步骤（Reproduction）
1. 在 personal skills 下放置 `template-skill`。
2. 开启 sandbox exec。
3. 触发一轮会注入 `<available_skills>` 的 agent 会话。
4. 观察 agent/工具日志读取的 skill 路径。
5. 期望 vs 实际：
   - 期望：读取 sandbox 可见路径，例如 `/moltis/data/skills/template-skill/SKILL.md`
   - 实际：读取宿主机路径 `/home/luy/.moltis/data/skills/template-skill/SKILL.md`

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/skills/src/discover.rs:42`：skill 默认发现路径直接产出宿主机目录（project / personal / installed / plugin）。
  - `crates/skills/src/prompt_gen.rs:35`：`<available_skills>` 对非 plugin skill 直接输出 `skill.path.join("SKILL.md")`，未区分 host vs sandbox。
  - `crates/agents/src/prompt.rs:405`：prompt 其他数据目录在 sandbox 模式下已明确使用 `/moltis/data`。
  - `crates/tools/src/sandbox.rs:324`：sandbox 数据目录固定挂载到 `/moltis/data`。
  - `crates/tools/src/sandbox.rs:396`：sandbox 公开数据视图负责暴露可发现 skill 的白名单目录。
- 日志/现象证据：
  - 运行时实际报错：`ls: cannot access '/home/luy/.moltis/data/skills/template-skill': No such file or directory`
- 当前测试覆盖：
  - 已有：公开数据视图包含 skill 目录；prompt 会输出 skill 路径。
  - 缺口：没有测试覆盖“sandbox 模式下 `<available_skills>` 输出的必须是 guest-visible path”。

## 根因分析（Root Cause）
- A. skill discovery 产出的是宿主机路径，这本身没问题。
- B. prompt 组装阶段把 discovery 的宿主机路径原样写进 `<available_skills>`，没有根据 runtime 是否 sandbox 做路径契约转换。
- C. agent 真正执行读取时运行在 sandbox 文件系统里，因此 host path 与 guest path 语义断裂，最终表现为“文件不存在”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - sandbox 模式下，`<available_skills>` 只能暴露 sandbox 内真实可读的 skill 路径。
  - 非 sandbox 模式下，`<available_skills>` 保持输出宿主机真实路径。
  - prompt 组装必须成为 skill 路径口径切换的单一收口点。
- 不得：
  - 不得继续把宿主机绝对路径泄露给 sandbox 内 agent。
  - 不得让 agent 看到“名义可用、实际不可读”的 skill 条目。
  - 不得在 discovery、prompt、sandbox 三层分别各写一套 rewrite 逻辑。
- 应当：
  - 应当按 `SkillSource` 与 runtime 环境做集中式映射/过滤。
  - 应当为被过滤的 skill 留下结构化拒绝日志，方便排障。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：保留 discovery 继续产出宿主机路径；在 prompt 组装阶段，根据 runtime 是否 sandbox，把 skill metadata 统一转换为“沙盒可见路径”或直接过滤不可读条目。
- 优点：
  - 单点收口，边界清楚。
  - 不污染 discovery 层。
  - 与现有 `USER.md` / `PEOPLE.md` 的 sandbox prompt 口径一致。
- 风险/缺点：
  - 需要明确每类 `SkillSource` 在 sandbox 下的可见性规则。

#### 方案 2（备选）
- 核心思路：在 discovery 阶段直接产出 sandbox 路径。
- 风险/缺点：
  - discovery 不知道当前 runtime 是否 sandbox。
  - 会把运行时语义错误下沉到静态扫描层，边界混乱。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：`SkillMetadata.path` 继续表示 discovery 侧的宿主机路径，不改变其现有语义。
- 规则 2：新增单点的“skill prompt path resolve”逻辑，仅在 prompt 组装时根据 runtime 解析出最终 as-sent path。
- 规则 3：当 runtime 为 sandbox 时：
  - personal skill 必须映射到 `/moltis/data/skills/<name>/SKILL.md`。
  - 其他 source 必须先判断 sandbox 中是否可读；可读则映射为对应 guest path，不可读则从 `<available_skills>` 过滤。
- 规则 4：当 runtime 非 sandbox 时，继续输出宿主机真实路径。
- 规则 5：任何因 sandbox 不可读而被过滤的 skill，必须记录结构化日志，禁止 silent degrade。

#### 接口与数据结构（Contracts）
- API/RPC：无对外 API 变更。
- 存储/字段兼容：无持久化 schema 变更。
- Prompt Contract：
  - `<available_skills>` 中的 `path` 定义为“agent 当前运行环境中可直接读取的实际路径”。
  - `path` 不再等同于 discovery 原始路径。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若某 skill 在 sandbox 下不可读：从 `<available_skills>` 过滤，并记录 `skill_unavailable_in_sandbox`。
  - 若路径映射逻辑遇到未知 source：直接过滤并记录 `unknown_skill_source_for_sandbox_path`。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无额外队列或状态。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - prompt 中不得出现宿主机私有绝对路径。
  - 日志可打印 skill 名称与 source，不打印 skill 正文。
- 禁止打印字段清单：
  - skill 正文
  - token / secret
  - 无关宿主机目录清单

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] sandbox 模式下，`<available_skills>` 不再出现 `/home/...` 这类宿主机 skill 路径。
- [ ] personal skill 在 sandbox 模式下输出 `/moltis/data/skills/<name>/SKILL.md` 并可被 agent 读取。
- [ ] 非 sandbox 模式下，skill 路径行为不回归。
- [ ] 不可在 sandbox 中读取的 skill source 不会继续作为“可用 skill”下发给 agent。
- [ ] 命中过滤分支时有结构化日志，且无 silent degrade。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/skills/src/prompt_gen.rs` 或对应 prompt resolver：覆盖 sandbox / non-sandbox 两种路径生成。
- [ ] 覆盖 personal skill 在 sandbox 下映射为 `/moltis/data/skills/<name>/SKILL.md`。
- [ ] 覆盖不可读 source 在 sandbox 下被过滤且留下 `reason_code`。

### Integration
- [ ] 增加一条 prompt 组装集成测试：sandbox 模式下 `<available_skills>` 中不出现宿主机 skill 绝对路径。

### UI E2E（Playwright，如适用）
- [ ] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：若短期内没有稳定的真实 sandbox 端到端测试链路，可先用 prompt 组装测试 + 手工验收闭环。
- 手工验证步骤：
  1. 创建 `template-skill` personal skill。
  2. 开启 sandbox exec。
  3. 发起一轮会注入 `<available_skills>` 的对话。
  4. 检查 as-sent prompt 或相关日志，确认 skill path 为 `/moltis/data/skills/template-skill/SKILL.md`。
  5. 在 sandbox 中实际读取该文件，确认成功。

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随代码发布，无 feature flag。
- 回滚策略：回退 prompt path resolve 逻辑即可；风险是重新暴露 host-path 泄露与 sandbox 不可读问题。
- 上线观测：
  - 关注 `skill_unavailable_in_sandbox`
  - 关注仍出现 `/home/` skill path 的 prompt/调试日志

## 实施拆分（Implementation Outline）
- Step 1: 明确 `SkillSource` × runtime 的 guest-path / filter 规则，并冻结 `<available_skills>.path` 语义。
- Step 2: 在 prompt 组装单点新增 skill path resolve / filter 逻辑。
- Step 3: 补齐单元测试与 prompt 组装测试，覆盖 sandbox / non-sandbox 主路径。
- Step 4: 补结构化日志，确保过滤/拒绝可观测。
- 受影响文件：
  - `crates/skills/src/prompt_gen.rs`
  - `crates/agents/src/prompt.rs`
  - `crates/skills/src/discover.rs`（仅在需要补类型注释/辅助方法时最小增量修改）
  - `crates/tools/src/sandbox.rs`（仅在需要复用公开数据视图路径规则时）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-sandbox-public-data-view-skills.md`
  - `issues/done/issue-sandbox-fixed-data-dir-mountpoint.md`
- Related commits/PRs：
  - <pending>
- External refs（可选）：
  - <N/A>

## 未决问题（Open Questions）
- Q1: registry / plugin skill 在 sandbox 下是否已有稳定 guest 可见路径；若没有，是否统一按“过滤而非伪暴露”处理？
- Q2: 是否需要在 debug 面板里直接展示 `<available_skills>` 的 as-sent path，方便排障？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

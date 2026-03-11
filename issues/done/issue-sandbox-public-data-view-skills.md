# Issue: sandbox 公开数据视图未暴露 skills 目录（sandbox / skills）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-12
- Owners: Neo
- Components: tools/sandbox, skills
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- 2026-03-12：sandbox 公开数据视图增加白名单目录刷新与递归复制能力：`crates/tools/src/sandbox.rs:341`
- 2026-03-12：`prepare_public_data_view()` 现在同步 personal/project skills 到 sandbox 视图：`crates/tools/src/sandbox.rs:389`
- 2026-03-12：新增 `walkdir` 依赖以支持技能目录递归复制：`crates/tools/Cargo.toml:33`

**已覆盖测试（如有）**
- 公开数据视图包含 `skills/` 与 `.moltis/skills/`：`crates/tools/src/sandbox.rs:3001`
- 公开数据视图刷新时会清除已删除技能的残留副本：`crates/tools/src/sandbox.rs:3082`
- Docker run args 仍挂载 `.sandbox_views/<key>` 到 `/moltis/data`：`crates/tools/src/sandbox.rs:2972`

**已知差异/后续优化（非阻塞）**
- 公开视图会完整复制技能目录；若某些 skill 附带较大参考资料，后续可考虑按需裁剪或增加缓存策略。
- 当前仍未覆盖“真实 Docker sandbox 内读取 `/moltis/data/skills/...`”的集成测试；现阶段由单元测试和现有 mount 测试共同兜底。

---

## 背景（Background）
- 场景：Moltis 后台可从 `data_dir/skills` 与 `data_dir/.moltis/skills` 发现个人技能和项目技能，但 sandboxed exec 只能读取 `/moltis/data`。
- 约束：
  - bind mount 并不是直接把整个 `data_dir` 暴露给 sandbox，而是先通过 `prepare_public_data_view()` 生成一个受限的公开视图。
  - 公开视图原本只复制 `USER.md` 和 `PEOPLE.md`，因此沙箱内看不到 skills。
- Out of scope：
  - 不在本单中重做技能发现逻辑。
  - 不在本单中把整个 `data_dir` 原样暴露给 sandbox。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **公开数据视图**（主称呼）：为 sandbox bind mount 预先生成的白名单目录树。
  - Why：避免把整个 `data_dir` 原样暴露给 sandbox。
  - Not：它不是完整的 `data_dir` 镜像，也不是 UI 的技能发现入口。
  - Source/Method：[effective] 由 `prepare_public_data_view()` 每次创建或刷新。
  - Aliases（仅记录，不在正文使用）：public data view / `.sandbox_views/<key>`

- **个人技能目录**（主称呼）：`<data_dir>/skills`
  - Why：后台会从这里发现 personal skills。
  - Not：它不等于 repo 内 `skills/` 版本镜像目录。
  - Source/Method：[configured/effective] 由 `moltis_config::data_dir()` 决定。
  - Aliases（仅记录，不在正文使用）：personal skills

- **项目技能目录**（主称呼）：`<data_dir>/.moltis/skills`
  - Why：后台会从这里发现 project skills。
  - Not：它不是已安装 repo skills 清单。
  - Source/Method：[configured/effective]
  - Aliases（仅记录，不在正文使用）：project skills

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] sandbox 公开数据视图必须包含可发现的个人技能目录。
- [x] sandbox 公开数据视图必须包含可发现的项目技能目录。
- [x] 公开数据视图刷新时不得残留已删除技能的旧副本。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须只暴露白名单公共文件和技能目录。
  - 不得顺手把 `people/`、session 数据或其他私有目录暴露进 sandbox。
- 兼容性：现有 `/moltis/data` 挂载点和 Docker run args 行为保持不变。
- 可观测性：遇到技能目录中的符号链接时记录带 `reason_code` 的 warning，避免静默跟随到意外路径。
- 安全与隐私：不得通过这次修复扩大到整个 `data_dir` 的暴露范围。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 后台技能页能看到 `~/.moltis/skills/...` 下的技能。
2. sandbox 内 `/moltis/data` 只能看到 `USER.md` 和 `PEOPLE.md`，看不到 `skills/`。
3. 依赖本地 skill 文件的沙箱内流程会出现“后台可见、运行时不可见”的断裂。

### 影响（Impact）
- 用户体验：技能在后台看起来已安装，但沙箱环境读不到，行为不一致。
- 可靠性：依赖 sandboxed exec 读取 skill 文件的流程会失败或退化。
- 排障成本：表面像“路径配错”或“技能没装上”，实际根因在公开视图裁剪层。

### 复现步骤（Reproduction）
1. 在 `~/.moltis/skills/<skill_name>/` 下放入一个本地 skill。
2. 打开 Moltis 后台，确认技能已被发现。
3. 在 sandboxed exec 中查看 `/moltis/data`。
4. 期望 vs 实际：
   - 期望：`/moltis/data/skills/<skill_name>/...` 可见。
   - 实际：只有 `USER.md` 和 `PEOPLE.md`。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/gateway/src/server.rs:4503`：后台会从 `data_dir/skills` 和 `data_dir/.moltis/skills` 发现 personal/project skills。
  - `crates/tools/src/sandbox.rs:389`：sandbox bind mount 前会构造公开数据视图。
  - `crates/tools/src/sandbox.rs:399`：修复前后，公开视图始终保留 `USER.md` / `PEOPLE.md`。
  - `crates/tools/src/sandbox.rs:401`：修复后先清理公开视图里的旧技能目录，再按白名单重新同步。
- 当前测试覆盖：
  - 已有：`crates/tools/src/sandbox.rs:3001`、`crates/tools/src/sandbox.rs:3082`、`crates/tools/src/sandbox.rs:2972`
  - 缺口：尚无真实 Docker 集成测试直接验证容器内 `ls /moltis/data/skills`

## 根因分析（Root Cause）
- A. 后台技能发现和 sandbox 挂载读取走的是两条不同链路。
- B. sandbox 的 `prepare_public_data_view()` 只复制 `USER.md` 与 `PEOPLE.md`，没有同步技能目录。
- C. bind mount 最终挂进去的是 `.sandbox_views/<key>`，所以沙箱只能看到这份被裁剪过的视图，而不是完整 `data_dir`。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - sandbox 公开数据视图必须包含后台可发现的个人技能目录和项目技能目录。
  - 公开数据视图每次刷新时必须移除旧的技能副本，保证与源目录一致。
  - 对技能目录中的符号链接必须跳过，不得跟随复制到 sandbox 公开视图。
- 不得：
  - 不得因为修复 skills 可见性而把整个 `data_dir` 暴露给 sandbox。
  - 不得改变现有 `/moltis/data` 挂载点契约。
- 应当：
  - 应当把异常路径行为记录为结构化 warning，便于排障。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：保持公开视图白名单机制不变，只把 `skills/` 与 `.moltis/skills/` 两个公开技能目录加入白名单同步。
- 优点：
  - 改动面小。
  - 不破坏现有 `/moltis/data` 挂载契约。
  - 与后台现有技能发现路径保持一致。
- 风险/缺点：
  - 技能目录较大时，刷新成本会相应增加。

#### 方案 2（备选）
- 核心思路：直接把整个 `data_dir` bind mount 给 sandbox。
- 风险/缺点：
  - 暴露范围明显扩大。
  - 与现有“公开数据视图”安全边界相冲突。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：公开视图保留现有 `USER.md`、`PEOPLE.md` 白名单。
- 规则 2：公开视图额外同步 `<data_dir>/skills` 与 `<data_dir>/.moltis/skills`。
- 规则 3：每次刷新前，先定向删除公开视图中的旧技能目录，再从源目录重建，避免残留。
- 规则 4：遇到符号链接仅记录 warning，不复制内容。

#### 接口与数据结构（Contracts）
- API/RPC：无对外接口变更。
- 存储/字段兼容：无持久化 schema 变更。
- UI/Debug 展示（如适用）：无变更；后台继续按原逻辑发现技能。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 公开视图生成失败时沿用现有 `anyhow` 错误链路。
  - 符号链接不会导致失败，仅记录 `sandbox_public_data_symlink_skipped` warning。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 仅清理公开视图中的 `skills/` 与 `.moltis/skills/`，其余公开文件保留。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：warning 只打印本地路径，不打印敏感正文。
- 禁止打印字段清单：无 token、无 skill 正文内容、无其他私有目录清单。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] sandbox 公开视图包含 `USER.md`、`PEOPLE.md`、`skills/`、`.moltis/skills/`。
- [x] `people/` 等非白名单目录不进入公开视图。
- [x] 删除源技能目录后，再次刷新公开视图时，旧副本不会残留。
- [x] 现有 `/moltis/data` Docker mount 参数测试不回归。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `test_prepare_public_data_view_copies_public_files_and_skills`：`crates/tools/src/sandbox.rs:3001`
- [x] `test_prepare_public_data_view_refresh_prunes_removed_skills`：`crates/tools/src/sandbox.rs:3082`
- [x] `test_docker_run_args_includes_data_dir_env`：`crates/tools/src/sandbox.rs:2972`

### Integration
- [ ] 后续可补真实 Docker sandbox 集成测试，验证容器内直接读取 `/moltis/data/skills/...`

### UI E2E（Playwright，如适用）
- [ ] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：当前没有稳定的 CI 容器运行时前提来验证真实 Docker sandbox 内文件可见性。
- 手工验证步骤：
  1. 在 `~/.moltis/skills/<name>/` 下创建本地 skill。
  2. 触发一次 sandboxed exec。
  3. 在沙箱内检查 `/moltis/data/skills/<name>/SKILL.md` 是否可读。

## 发布与回滚（Rollout & Rollback）
- 发布策略：直接随代码发布，无 feature flag。
- 回滚策略：回退 `crates/tools/src/sandbox.rs` 与 `crates/tools/Cargo.toml` 的本次增量改动即可。
- 上线观测：关注 sandbox 相关日志中是否出现异常公开视图构建失败，或 `sandbox_public_data_symlink_skipped` 高频告警。

## 实施拆分（Implementation Outline）
- Step 1: 定位后台技能发现路径与 sandbox 公开视图裁剪点。
- Step 2: 给公开视图增加白名单技能目录同步与刷新前清理逻辑。
- Step 3: 补测试，覆盖技能可见性、删除后不残留、现有 mount 契约不回归。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/tools/Cargo.toml`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-sandbox-fixed-data-dir-mountpoint.md`
- Related commits/PRs：
  - <pending>
- External refs（可选）：
  - <N/A>

## 未决问题（Open Questions）
- Q1: 后续是否需要对公开视图的技能目录复制增加缓存或大小控制？
- Q2: 是否需要为真实 Docker sandbox 增加一条集成测试链路？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

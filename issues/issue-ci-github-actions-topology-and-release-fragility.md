# Issue: GitHub Actions CI 拓扑失衡 + release 流水线高脆弱（ci / release / github-actions）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Updated: 2026-03-24
- Owners: <TBD>
- Components: ci/workflows/release/docs
- Affected providers/models: N/A

**已实现（如有，写日期）**
- 2026-03-24：PR 检查已改为“等待本地 local status 回传”模式，而不是在 GitHub runner 上直接执行 fmt/lint/test：`.github/workflows/ci.yml:22`、`.github/workflows/e2e.yml:27`
- 2026-03-24：release workflow 已覆盖 `.deb`、`.rpm`、Arch、AppImage、Snap、Homebrew binaries、Docker、SBOM、Homebrew tap 更新、deploy tag 更新：`.github/workflows/release.yml:171`、`.github/workflows/release.yml:248`、`.github/workflows/release.yml:325`、`.github/workflows/release.yml:414`、`.github/workflows/release.yml:525`、`.github/workflows/release.yml:576`、`.github/workflows/release.yml:649`、`.github/workflows/release.yml:805`、`.github/workflows/release.yml:865`、`.github/workflows/release.yml:919`、`.github/workflows/release.yml:1017`
- 2026-03-24：repo 文档已经明确要求 PR 存在时运行 `./scripts/local-validate.sh <PR_NUMBER>` 以发布 commit statuses：`docs/src/local-validation.md:20`、`CLAUDE.md:947`

**已覆盖测试（如有）**
- <N/A>

**已知差异/后续优化（非阻塞）**
- `coverage`、`homebrew`、`deploy tag` 等路径是否在当前 GitHub 仓库设置下具备所需 secret / branch protection 权限，仍需真实仓库配置佐证。
- 本 issue 只收敛现状、失败面与后续修复目标；不在本轮直接拆分多子 issue。

---

## 背景（Background）
- 场景：当前仓库在 push 到 GitHub 后，会同时触发 PR 校验、main 分支校验、定时任务、release 打包、文档发布、Homebrew tap 更新等多类 workflow。
- 调查目标：先基于本地仓库只读证据梳理“这些 workflow 实际在做什么、哪些失败是确定性的、哪些是高概率环境问题、哪些必须看 GitHub 日志/仓库设置才能确认”。
- 约束：
  - 本轮不改代码、不改 workflow，只沉淀 issue 文档。
  - 按单 issue 模板收敛为一个主单，后续修复再按需要拆分。
- Out of scope：
  - 本轮不直接修复 workflow。
  - 本轮不引入新的 CI 平台，不讨论迁移到外部构建系统。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **PR 状态门**（主称呼）：PR workflow 不在 GitHub runner 上执行实际检查，而是轮询 PR head commit 上是否已经存在 `local/*` 成功状态。  
  - Why：这是当前 PR 是否通过的首要机制。
  - Not：它不是 hosted CI 真正跑出的 fmt/lint/test/e2e 结果。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：local validation gate / local status gate

- **release 大流水线**（主称呼）：tag push 后在单个 workflow 中串起校验、跨平台打包、签名、镜像发布、SBOM、外部 tap 更新、主分支 deploy tag 更新的整套流程。  
  - Why：它决定 release 成败，也承载了当前最多的失败面。
  - Not：它不是单纯的“构建一个发布包”。
  - Source/Method：authoritative
  - Aliases（仅记录，不在正文使用）：release pipeline / packaging pipeline

- **环境失配**（主称呼）：workflow 所需 runner、系统依赖、secret、权限与 job 实际声明不一致，导致任务在 GitHub 环境中高概率失败。  
  - Why：当前大量失败并非业务逻辑错误，而是 workflow 假设与执行环境不一致。
  - Not：它不等同于单一代码 bug。
  - Source/Method：estimate
  - Aliases（仅记录，不在正文使用）：env mismatch / infra mismatch

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 梳理当前所有 GitHub Actions workflow 的触发条件、主要 job、依赖关系与职责边界。
- [ ] 明确区分“确定会失败 / 高概率失败 / 需要 GitHub 实际日志确认”的失败面。
- [ ] 为后续修复提供单一事实来源，避免在多个讨论里重复推断 CI 结构。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须把 PR / main / schedule / release / docs / manual workflow 的职责边界写清楚。
  - 必须把“配置错误”和“环境错误”分开，不得混成一句“CI 不稳定”。
  - 不得在没有本地证据时把 GitHub secret / branch protection / runner 存在性写成确定事实。
- 兼容性：本 issue 不要求兼容当前混合拓扑；后续修复可做硬切换收敛。
- 可观测性：后续修复时，至少要让 workflow 失败原因能从 job 名称/日志直接定位到“PR gate / runner 缺失 / 系统依赖 / 打包配置错误 / token/secret 缺失”。
- 安全与隐私：issue 不记录 secret 值，只记录是否存在 secret 依赖。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. PR 上的 `fmt` / `clippy` / `test` / `e2e` 看起来像 GitHub check，实际并不在 GitHub runner 上执行，而是在等待本地开发机先回传 commit status。
2. release workflow 一次性串起大量互不相同的目标，导致任何单点配置错误或环境错误都会阻断整条发版链路。
3. 多个 hosted job 直接构建默认 `moltis` 二进制，但系统依赖 provisioning 并不一致；只有部分 job 显式安装了 `cmake` / `clang` / `libclang-dev` / `pkg-config` / `git` 等构建依赖。
4. release 中至少存在 1 处确定性错误：RPM 打包命令使用了错误的 package 选择。

### 影响（Impact）
- 用户体验：
  - PR 作者如果不了解 `local-validate.sh` 机制，会看到 GitHub check 长时间 pending 后失败，误以为 GitHub 本身在跑测试但“不稳定”。
- 可靠性：
  - release 成功依赖 self-hosted runner、系统依赖、secret、仓库权限、外部资产上传顺序，多环节耦合，稳定性差。
- 排障成本：
  - 当前 workflow 过多且职责重叠，失败时需要先判断“这是 PR 状态门没喂数据，还是 hosted CI 真挂了，还是 release 打包配置写错了，还是 GitHub 仓库设置不满足”。

### 复现步骤（Reproduction）
1. 创建一个 PR，但不运行 `./scripts/local-validate.sh <PR_NUMBER>`。
2. 观察 `CI` 与 `E2E Tests` workflow：GitHub 只会等待 `local/fmt`、`local/biome`、`local/zizmor`、`local/lint`、`local/test`、`local/e2e` 等 status。
3. 推送一个 release tag，观察 `Build Packages` workflow 会串行/并行进入 lint、test、e2e、各种打包、Docker、SBOM、tap 更新、deploy tag 更新。
4. 期望 vs 实际：
   - 期望：PR 在 GitHub 上直接执行最基本的验证；release 只承担发版相关且自洽的步骤。
   - 实际：PR 依赖本地人工动作；release 承载过多目标且含确定性配置错误。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `.github/workflows/ci.yml:22`：PR 下只运行 `local-validation` 矩阵，检查 `local/fmt`、`local/biome`、`local/zizmor`、`local/lint`、`local/test`。
  - `.github/workflows/e2e.yml:27`：PR 的 E2E workflow 只检查 `local/e2e`。
  - `scripts/check-local-status.py:10`：workflow 通过 GitHub API 轮询 commit status；若超时或缺失则失败。
  - `docs/src/local-validation.md:20`：文档明确要求 PR 模式运行 `./scripts/local-validate.sh <PR_NUMBER>` 才会发布 commit statuses。
  - `.github/workflows/ci.yml:103`：`rust-ci` 依赖 `[self-hosted, Linux, X64]` runner 和 CUDA container。
  - `.github/workflows/release.yml:65`：release 的 `clippy` 同样依赖 self-hosted CUDA runner。
  - `.github/workflows/release.yml:303`：RPM job 使用 `cargo generate-rpm -p crates/cli --target "$BUILD_TARGET"`。
  - `crates/cli/Cargo.toml:1`：CLI package 名称是 `moltis`，不是 `crates/cli`。
  - `crates/cli/Cargo.toml:67` + `crates/gateway/Cargo.toml:81`：默认 `moltis` feature 会带上 `moltis-gateway` 默认 feature，其中包含 `local-llm`。
  - `.github/workflows/ci.yml:120`：只有 `rust-ci` 显式安装 `curl git cmake build-essential clang libclang-dev pkg-config ca-certificates`。
  - `.github/workflows/ci.yml:229`：E2E job 直接 `cargo build --bin moltis`，但没有安装上述构建依赖。
  - `.github/workflows/release.yml:105`、`.github/workflows/release.yml:138`、`.github/workflows/release.yml:221`、`.github/workflows/release.yml:298`、`.github/workflows/release.yml:360`、`.github/workflows/release.yml:448`、`.github/workflows/release.yml:610`：release 下多个 hosted job 都直接构建默认 `moltis`，但依赖 provisioning 不一致。
- 配置/协议证据（必要时）：
  - `.config/nextest.toml:7`：`--profile ci` 配置存在，本地未发现 nextest profile 缺失。
  - `CLAUDE.md:912`：文档明确要求 release tag 与 workspace version 保持一致，说明 release workflow 依赖严格版本纪律。
- 当前测试覆盖：
  - 已有：workflow 文件、local validation 文档、nextest profile、Cargo workspace/feature 声明可本地直接核查。
  - 缺口：self-hosted runner 是否存在、`CODECOV_TOKEN` / `HOMEBREW_TAP_TOKEN` 是否配置、branch protection 是否允许 workflow 更新 `main`，都需要 GitHub 仓库设置或真实日志确认。

## 根因分析（Root Cause）
- A. PR 校验目标与手段不匹配：
  - PR 的 GitHub check 被设计成“等待本地开发机上报状态”，不是 hosted CI 直接执行。
  - 这让 PR 成败依赖开发者本地环境、手工动作和 token 权限，而不是仓库可重复执行的自动化环境。
- B. release 目标耦合过重：
  - 同一条 release workflow 同时承担验证、跨平台打包、签名、镜像发布、SBOM、外部仓库更新、主分支 deploy tag 更新。
  - 任一子目标失败都会拖死整条链，职责边界不清。
- C. 构建环境口径不统一：
  - 默认 `moltis` 构建链包含较重 feature，但并不是所有 job 都 provision 了需要的系统依赖。
  - 部分关键 job 还依赖 self-hosted CUDA runner，这与其他 hosted runner 路径形成混杂拓扑。
- D. 存在确定性配置错误：
  - RPM job 使用错误 package selector（`-p crates/cli`），无需等待 GitHub 日志即可判定为错误配置。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - PR 校验必须在 GitHub 上直接执行最小必要检查，不能把成功前提绑定到开发者本地手工上报。
  - release workflow 必须只承担发版闭环所需步骤；验证、打包、分发、外部仓库更新应当有清晰边界。
  - 确定性的 workflow 配置错误必须先清零，再讨论“偶发不稳定”。
  - 每个构建 job 所需 runner 与系统依赖必须自洽，不能隐含依赖某些 job 或某些环境“碰巧装好”。
- 不得：
  - 不得继续把 PR gate、main 验证、release 构建、外部仓库更新混成一个不可分辨的大系统。
  - 不得在缺少 self-hosted runner / secret / branch permission 证据时假定这些前提一定存在。
- 应当：
  - 应当把 workflow 分层为“PR 快速反馈”、“main/schedule 深验证”、“release 打包分发”、“docs 发布”、“手动运维类任务”。
  - 应当让 job 名称与职责一一对应，失败时能直接看懂是在“等本地状态”、“缺 runner”、“缺依赖”、“打包配置错”、“缺 token/权限”中的哪一类。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：先收敛拓扑，再分层修复
- 核心思路：
  - 保留当前 issue 作为主单；
  - 先修掉确定性错误与明显拓扑问题；
  - 再按 PR/main/release/docs/manual 几类职责拆小步收敛。
- 优点：
  - 风险可控，便于把“先止血”和“再瘦身”分开。
  - 不需要一口气重写全部 workflow。
- 风险/缺点：
  - 需要后续拆分时保持边界纪律，避免再次膨胀。

#### 方案 2（备选）：一次性重写所有 workflow
- 核心思路：直接整体重做 CI 拓扑。
- 优点：理论上最干净。
- 风险/缺点：范围过大，容易在不了解 GitHub 仓库实际设置时误删必要路径，不符合当前“先筹备 issue、后逐步修复”的节奏。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（PR）：PR workflow 后续必须以 hosted CI 为主，本地验证只能作为附加开发者工具，不能作为 merge 前置唯一路径。
- 规则 2（release）：release 先清理确定性配置错误，再把验证与打包/分发职责分层。
- 规则 3（环境）：每个 job 自己声明并满足自己的 runner / 依赖前提；不能依赖“另一条 job 已经装过”。
- 规则 4（权限）：涉及 secret、跨仓库 push、更新 `main` 的 job，必须显式标注权限前提与失败模式。

#### 接口与数据结构（Contracts）
- API/RPC：N/A
- 存储/字段兼容：N/A
- UI/Debug 展示（如适用）：N/A

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - PR gate 缺本地 status
  - self-hosted runner 不可用
  - 系统依赖缺失导致构建失败
  - workflow 配置错误
  - secret / GitHub permission / branch protection 不满足
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 本 issue 阶段不涉及运行态队列清理。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 只记录 secret 是否被依赖，不记录 secret 内容。
- 禁止打印字段清单：
  - `CODECOV_TOKEN`
  - `HOMEBREW_TAP_TOKEN`
  - 任意 GitHub token 实值

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 有一份单一 issue 文档，能让后续修复者直接看懂当前 workflow 全貌、主要 failure classes 与证据来源。
- [ ] 文档明确点出至少 1 个确定性错误、至少 2 类高概率环境问题、以及需要 GitHub 实际日志/仓库设置确认的证据缺口。
- [ ] 后续修复应以本 issue 为主单推进，避免再次出现“边修边猜当前 CI 到底在干什么”的情况。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] N/A：本 issue 仅为调查与文档沉淀，不涉及代码单测。

### Integration
- [ ] 后续修复时按 workflow 分类验证：
  - PR：GitHub hosted runner 上直接执行最小校验
  - release：逐个打包 job 验证输入、输出与权限前提

### UI E2E（Playwright，如适用）
- [ ] N/A

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - self-hosted runner 存在性、仓库 secret、branch protection、GHCR/Pages/tap push 权限均不在本地仓库内，必须看 GitHub 实际配置或 workflow 日志。
- 手工验证步骤：
  1. 打开 GitHub Actions 历史运行记录。
  2. 分别检查 PR、push main、tag release、docs、homebrew manual run 的最近一次执行。
  3. 对照本 issue 中的 failure classification，确认每个失败是否归类正确。

## 发布与回滚（Rollout & Rollback）
- 发布策略：
  - 先以本 issue 为主单收敛问题。
  - 后续按“先修确定性错误，再收敛拓扑，再补 runner/依赖/权限文档”的顺序推进。
- 回滚策略：
  - 本 issue 本身是文档，无需回滚；后续具体修复需各自定义回滚方案。
- 上线观测：
  - 关注 PR check 从“等待本地状态”向 hosted CI 迁移后的稳定性。
  - 关注 release 打包 job 的失败率是否明显下降。

## 实施拆分（Implementation Outline）
- Step 1: 修正 release 中已确认的确定性错误（例如 RPM package selector）。
- Step 2: 收敛 PR workflow，使其不再依赖本地 status gate 作为唯一前置。
- Step 3: 逐个梳理并统一 hosted job 的构建依赖与 runner 前提。
- Step 4: 将 release 中“验证 / 打包 / 外部更新”拆出清晰边界，并补齐 secret / permission 前提文档。
- 受影响文件：
  - `.github/workflows/ci.yml`
  - `.github/workflows/e2e.yml`
  - `.github/workflows/release.yml`
  - `.github/workflows/docs.yml`
  - `.github/workflows/homebrew.yml`
  - `scripts/check-local-status.py`
  - `scripts/local-validate.sh`
  - `docs/src/local-validation.md`
  - `CLAUDE.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/local-validation.md`
  - `docs/src/e2e-testing.md`
  - `CLAUDE.md`
- Related commits/PRs：<TBD>
- External refs（可选）：N/A

## 未决问题（Open Questions）
- Q1: PR 是否彻底取消 `local/*` gate，还是保留为非阻塞辅助状态？
- Q2: release 是否应继续保留“更新 Homebrew tap + 更新 deploy template tags”这类跨仓库/跨主分支写操作？
- Q3: 默认 `moltis` 构建链是否应在打包 job 中剥离重 feature，还是统一给所有 job 补足系统依赖？
- Q4: self-hosted CUDA runner 在当前仓库是否稳定可用，是否应继续作为 release 前置硬依赖？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

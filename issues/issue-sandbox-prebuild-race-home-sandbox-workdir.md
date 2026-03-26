# SUPERSEDED BY `issues/issue-sandbox-runtime-image-and-container-lifecycle-one-cut.md`
#
# 2026-03-26 决策更新：
# - Moltis sandbox 运行模型已 hard-cut 收敛为“唯一运行镜像 + 容器生命周期”。
# - 旧的 prebuild / build / override / provision 模型不再作为实现方向。

# Issue: sandbox 默认 `/home/sandbox` workdir 假设不成立 + 预构建镜像切换不触发重建（sandbox / exec）

## 实施现状（Status）【增量更新主入口】
- Status: SUPERSEDED（不再推进；以新主单为唯一准绳）
- Priority: P1
- Updated: 2026-03-26
- Owners: <TBD>
- Components: tools/sandbox, tools/exec, gateway
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- 2026-02-xx：sandbox 镜像预构建在 gateway 启动后台执行（并通过 `SandboxRouter::set_global_image()` 切换默认 image）：`crates/gateway/src/server.rs`
- 2026-02-xx：exec tool 在 “sandboxed + 容器后端” 下默认 working_dir 为 `/home/sandbox`：`crates/tools/src/exec.rs`

**核实结果（2026-03-26）**
- 复现确认：基础镜像 `ubuntu:25.10` 不包含 `/home/sandbox`，且对“未创建该目录”的容器执行 `docker exec -w /home/sandbox ...` 会稳定报错（与本单症状一致）：`OCI runtime exec failed ... chdir to cwd (\"/home/sandbox\") ... no such file or directory`。
- 代码确认：`DockerSandbox::docker_run_args()` 启动容器时未设置工作目录、也未在非 prebuilt image 路径创建 `/home/sandbox`：`crates/tools/src/sandbox.rs`；因此只要容器不是 prebuilt image（或 prebuild 尚未完成），`ExecTool` 的默认 `working_dir=/home/sandbox` 会触发上述 OCI chdir 失败。
- 代码确认：`container_contract_matches()` 当前只校验 env + mounts，不校验运行中容器的 image/tag/ImageID：`crates/tools/src/sandbox.rs:1078`；因此 prebuild 完成后的 `set_global_image()` 不会强制已存在容器重建（image 切换不会生效），存在“预构建完成但仍继续复用旧容器”的漂移风险。
- 命令确认：存在“会话之外”的预构建入口（满足本单目标之一）：`moltis sandbox build/list/clean/remove`：`crates/cli/src/sandbox_commands.rs:1`。
- 实测确认：`moltis sandbox build` 当前实际构建出的 image tag 为 `msb:<hash>`（由 `DockerSandbox::image_repo() == "msb"` 决定）：`crates/tools/src/sandbox.rs:965`；且 `moltis sandbox list/clean` 仅处理 `*-sandbox:*`，不会列出/清理 `msb:*`（运维/可观测 UX 存在缺口，且 CLI build 输出的 `Tag:` 与实际 build tag 不一致）。
- 风险确认：`process` 工具当前硬编码 `working_dir=/home/sandbox`（且 host fallback 也带该 working_dir），仍可能在无容器后端或未创建目录时触发 ENOENT/OCI cwd 类错误：`crates/tools/src/process.rs:129`。

**已覆盖测试（如有）**
- <N/A>

**已知差异/后续优化（非阻塞）**
- `process` 工具目前也硬编码 `working_dir=/home/sandbox`（且 host fallback 也会带上该 working_dir），可能会引入额外 ENOENT/OCI cwd 错误；建议另立 issue 拆分处理：`crates/tools/src/process.rs:128`

---

## 背景（Background）
- 场景：本机以进程方式启动 Moltis（非容器化 gateway），启用 `[tools.exec.sandbox] mode="all"`。
- 常见配置：`packages` 非空（触发预构建沙盒镜像），且启动后可能很快触发第一次 exec（WebUI、channel、heartbeat 等路径）。
- 关键澄清：该问题并不严格依赖 “prebuild race”。即便 `packages=[]`（不触发预构建），只要使用基础镜像启动 Docker sandbox 容器且容器内不存在 `/home/sandbox`，仍可能必现 OCI `chdir` 失败。
- 约束：
  - sandbox 默认 working_dir 需要是**容器内可写**目录（避免写入只读的 `/moltis/data` data mount）。
  - 沙盒容器生命周期受 `scope`/`idle_ttl_secs` 影响；当 `idle_ttl_secs=0` 时，容器可能长期复用。
- 环境因素（可能放大时序窗口）：
  - Docker 镜像/层缓存位于机械硬盘，I/O 性能较差，镜像构建与解包可能显著变慢。
  - 镜像构建过程包含外网拉取（例如 `apt-get`、`curl` 等），若需要翻墙/VPN 或网络抖动，会导致预构建耗时拉长，从而更容易在“预构建完成前”触发首次 exec。
- Out of scope：
  - 本 issue 不处理 “Docker daemon 不可用导致回退 host 执行” 的场景（属于部署/权限问题）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **预构建沙盒镜像**（主称呼）：gateway 在后台把 `packages` bake 进镜像后生成的 `msb:<hash>` 镜像（当前 Docker backend 实现），并通过 `SandboxRouter::set_global_image()` 设为默认。  
  - Why：避免每次 `docker run` 都安装包；提升启动/执行速度。
  - Not：它不是 `[tools.exec.sandbox].image` 显式指定的基础镜像（例如 `ubuntu:25.10`）。
  - Source/Method：effective（运行态 override）
  - Aliases（仅记录，不在正文使用）：prebuilt image / baked image

- **基础镜像**（主称呼）：`DockerSandbox::image()` 默认返回的基础镜像（例如 `ubuntu:25.10`）。  
  - Why：预构建完成前，可能被用来启动沙盒容器。
  - Not：它不保证包含 `/home/sandbox` 目录/WORKDIR。
  - Source/Method：configured/effective
  - Aliases（仅记录，不在正文使用）：base image

- **container contract**（主称呼）：`container_contract_matches()` 用于判断“已有容器是否可复用”的契约（当前仅 env + mounts）。  
  - Why：避免复用旧容器导致 mount/env 不符合预期。
  - Not：当前不覆盖 image/tag 与容器内目录存在性。
  - Source/Method：effective（代码逻辑）
  - Aliases（仅记录，不在正文使用）：contract check

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] sandbox 开启时，exec/process 在容器后端下**不得**因为默认 working_dir 不存在而报 OCI `chdir` 错误。
- [ ] 预构建镜像完成后，沙盒容器应当可靠地使用“包含 `/home/sandbox`”的 image（或确保该目录存在）。
- [ ] 提供“会话之外”的可选预构建机制：允许通过命令/运维动作提前预构建沙盒镜像（而不是被动等待用户会话触发首次 exec 后才逐步进入稳定状态）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：默认 working_dir 在容器中始终存在且可写（`/home/sandbox` 或等价目录）。
  - 不得：依赖“预构建后台任务一定先于首次 exec 完成”的时序假设。
- 可观测性：
  - 需要可定位日志：当前容器 image、是否复用、contract 不匹配原因（至少包含 image/tag 差异）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 用户在启动后很快触发 exec（或 process/tmux）时，出现：
   - `OCI runtime exec failed: exec failed: unable to start container process: chdir to cwd ("/home/sandbox") set in config.json failed: no such file or directory`
2) 启动日志中可观察到：
   - 先出现 `exec tool invoked ... working_dir=Some("/home/sandbox")`
   - 后出现 `sandbox image pre-build complete ... tag="msb:<hash>"`
   - 说明首次 exec 可能发生在预构建完成之前。
3) 在 “镜像构建较慢（机械硬盘 + 需要翻墙/网络慢）” 的环境中，上述时序窗口会被显著放大，使该问题更容易复现且更具持续性。
4) 附带 UX 问题：在 WebUI chat 中输入类似 `hello` 这样的短字符串，也可能被 runner 的 “direct shell command” 启发式误判为命令，从而触发一次强制 exec（进一步放大“启动后立刻触发 exec”的概率）。
5) 观测陷阱：日志 `sandbox ensure_ready ... image=<tag>` 打印的是 **resolved image**，并不保证“当前复用的已存在容器”确实运行在该 image 上；当 container contract 未包含 image/tag 校验时，预构建完成后的 image 切换可能不会触发容器重建。

### 影响（Impact）
- 用户体验：首次/早期 exec 不稳定，且失败信息偏底层（OCI chdir），对用户不友好。
- 可靠性：在 `idle_ttl_secs=0` 且容器复用时，错误可能持续存在（旧容器不被自动替换）。
- 排障成本：需要用户理解“预构建镜像 vs 基础镜像”与容器复用契约，成本高。

### 复现步骤（Reproduction）
1. 本机以进程方式启动 gateway，配置 `[tools.exec.sandbox] mode="all"`（Docker daemon 可用）。
2. 在启动后尽快触发一次 `exec`（例如在 chat 里运行一个简单命令）。
3. 若此时 sandbox 容器镜像不包含 `/home/sandbox`，则 `docker exec -w /home/sandbox ...` 可能报 OCI `chdir` 失败。
4. 变体 A（放大窗口）：`packages` 非空且预构建较慢（机械盘/翻墙），更容易在预构建完成前触发首次 exec。
5. 变体 B（不依赖预构建）：将 `packages=[]`，仍可能触发同样的 OCI `chdir` 失败（基础镜像不保证 `/home/sandbox` 存在）。
6. 变体 C（误触发 exec）：在 WebUI 里清空会话后输入 `hello`（无标点、无换行），可能被误判为命令并触发强制 exec：`crates/agents/src/runner.rs:153`。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/tools/src/exec.rs`：sandbox + 容器后端下默认 `working_dir=/home/sandbox`（backend="none" 时会回退到 host data_dir，避免假设 `/home/sandbox` 存在）。
  - `crates/tools/src/sandbox.rs`：`DockerSandbox::exec()` 若设置了 `working_dir`，会传递给 `docker exec -w <dir>`。
  - `crates/tools/src/sandbox.rs`：`DockerSandbox::ensure_ready()` 启动容器未保证创建 `/home/sandbox`（基础镜像不保证存在）。
  - `crates/tools/src/sandbox.rs`：当 `container_contract_matches()` 为 true 时，`ensure_ready()` 直接复用既有容器，不会因 image/tag 变化而重建。
  - `crates/tools/src/sandbox.rs:1078`：`container_contract_matches()` 仅校验 env + mounts，不校验 image/tag，也不校验 `/home/sandbox` 是否存在。
  - `crates/tools/src/sandbox.rs`：`DockerSandbox::build_image()` 生成的 Dockerfile 会 `mkdir -p /home/sandbox` 并设置 `WORKDIR /home/sandbox`（因此仅 prebuilt image 路径保证该目录存在）。
  - `crates/tools/src/sandbox.rs:965`：Docker backend 的 prebuilt image repo 固定为 `msb`（`DockerSandbox::image_repo()`），因此预构建 tag 形如 `msb:<hash>`。
  - `crates/gateway/src/server.rs`：预构建镜像在后台任务完成后才 `set_global_image()`。
  - `crates/cli/src/sandbox_commands.rs`：已有 CLI 支持构建/列出/清理 sandbox images（`moltis sandbox build|list|clean`），但当前 list/clean 仅覆盖 `*-sandbox:*`，不覆盖 `msb:*`。
  - `crates/agents/src/runner.rs:153`：direct-shell-command 启发式对 `hello` 这类输入也会返回 Some，从而可能强制触发一次 exec（放大时序 race 的触发概率）。
  - `crates/tools/src/exec.rs`：`image = router.resolve_image(...)`（resolved image），以及 `sandbox ensure_ready ... image` 日志打印 resolved image（不等同于“当前容器实际 image”）。
- 日志关键词（本地可 grep）：
  - `exec tool invoked`（字段 `working_dir="/home/sandbox"`）
  - `sandbox image pre-build complete`（字段 `tag="msb:<hash>"`）

## 根因分析（Root Cause）
- A. 逻辑前提不成立（workdir）：
  - 在容器后端下，exec 默认 `working_dir=/home/sandbox`，但基础镜像（例如 `ubuntu:25.10`）不保证存在该目录；`ensure_ready()` 也未保证创建该目录。
  - 这使得问题在某些配置下是“必现”，而不仅是 “prebuild race 才会发生”。
- B. 契约不完整（image/tag）：
  - 预构建完成后，resolved image 会切换到 `msb:<hash>`，但 `container_contract_matches()` 未校验 image/tag，导致旧容器可能继续复用而不重建。
- C. 触发窗口放大（prebuild 慢 + 输入误判）：
  - gateway 启动后预构建在后台异步执行；预构建耗时受磁盘 I/O 与网络（apt/pull/翻墙）影响，窗口更长，更容易在预构建完成前触发首次 exec。
  - WebUI 的 direct-shell-command 误判（例如 `hello`）会额外提高“启动后立刻触发 exec”的概率。
- D. 下游表现：
  - `docker exec -w /home/sandbox ...` 或等价 OCI exec 在容器内 `chdir` 失败，报 “no such file or directory”。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - sandbox 容器内默认工作目录必须存在且可写（无论是否已完成预构建）。
  - 当默认 image（resolved）发生变化时，必须保证后续 exec 不会继续复用“不满足工作目录/镜像契约”的旧容器。
- 应当：
  - 支持“会话之外”的预构建（例如 CLI / 管理入口），让运维/用户可以在启动后、使用前提前完成镜像构建与缓存预热，避免把系统稳定性绑定在“首次用户会话 exec 的时机”上。
- 不得：
  - 不得依赖后台预构建任务完成顺序。
- 应当：
  - 发生降级/重建时输出结构化日志（至少包含 resolved image、container 实际 image、是否重建 + 原因码）。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：把 image/tag 纳入 contract + 必要时重建容器
- 核心思路：`container_contract_matches()` 增加镜像（`.Config.Image` 或 ImageID）校验；当 resolved image 与运行中容器不一致时重建。
- 优点：从根源解决“预构建切换后仍复用旧容器”的问题；契约更完整。
- 风险/缺点：需要谨慎处理 per-session image override / skill image；可能导致更多重建（但符合预期）。

#### 方案 2（备选）：启动容器时强制创建 `/home/sandbox`
- 核心思路：`docker run ...` 的 entry command 改为 `mkdir -p /home/sandbox && ...`（或在 `ensure_ready` 后 `docker exec` 一次创建目录）。
- 优点：简单直接；即便使用基础镜像也不会缺目录。
- 风险/缺点：仍可能继续复用旧容器（但至少不会再因为 `/home/sandbox` 不存在失败）。

#### 方案 3（备选）：更换默认 working_dir 到稳定存在路径
- 核心思路：默认工作目录改为 `/` 或 `/tmp` 等基础镜像必定存在路径，并显式设置 `HOME` 或提供可写目录。
- 优点：规避目录不存在。
- 风险/缺点：可能影响工具对 HOME/缓存目录的假设；且 `/moltis/data` 常为只读，不适合作为默认 cwd。

#### 方案 4（备选）：提供“会话之外”的预构建/预热路径（不依赖用户会话）
- 核心思路：
  - 明确支持/文档化 `moltis sandbox build` 作为“预构建镜像”的运维入口。
  - 可选：增加配置策略，让 gateway 在启动时选择“阻塞等待预构建完成后再对外服务”或“先对外服务但对首次 exec 做更强兜底”（两者二选一或可配置）。
- 优点：减少“用户第一次使用时踩坑”的概率；把耗时的镜像构建从用户路径中移出。
- 风险/缺点：需要定义清楚阻塞策略与 UX（启动变慢 vs 首次 exec 变慢）。

### 最终方案（Chosen Approach）
- <TBD>

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] 在 `tools.exec.sandbox.mode != "off"` 且容器后端可用时，首次 exec 不会出现 OCI `chdir /home/sandbox` 失败。
- [ ] 预构建镜像完成后，后续 exec 使用与 resolved image 一致的容器（不复用旧 image 造成的坏状态）。
- [ ] 有至少 1 条自动化测试覆盖“image 变化触发重建/或 workdir 兜底创建”的关键路径（或记录缺口 + 手工验收）。
- [ ] “会话之外预构建”入口与运行态口径一致：`moltis sandbox build` 构建出的 tag 与 gateway/runtime prebuild 使用一致（当前为 `msb:<hash>`），且 `moltis sandbox list/clean` 能列出/清理该类 tag（避免“建了但看不到/清不掉”）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] `crates/tools/src/sandbox.rs`：新增 contract 检查包含 image/tag 的单测（或新增 workdir init 的单测）。

### Integration
- [ ] 手工：启动 gateway 后立刻触发 exec；确认无 OCI chdir 错误；等待预构建完成后再次触发 exec；确认容器被重建/使用正确 image。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：CI 环境可能不稳定提供 docker daemon；可保持 unit 测试 + 手工验收。
- 手工验证步骤：见 Integration。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认开启（修复 bug）；必要时可通过配置/feature flag 控制“image 变更是否触发重建”（如担心影响）。
- 回滚策略：恢复旧 contract 行为；风险是重新暴露该时序 bug。
- 上线观测：监控 `docker exec failed` / `OCI runtime exec failed` 与容器重建相关日志。

## 实施拆分（Implementation Outline）
- Step 1: 将 resolved image/tag 纳入 Docker container contract（并补齐日志字段）。
- Step 2: 确保 `/home/sandbox` 在容器内存在（必要时）。
- Step 3: 补齐单测/手工验收说明。
- Step 4（可选）: 文档化/强化“会话之外预构建”入口（例如 CLI `moltis sandbox build`），并评估是否需要提供“启动阻塞预构建”的配置选项。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/tools/src/exec.rs`（如需调整默认 working_dir 策略）
  - `crates/gateway/src/server.rs`（如需同步 prebuild 与首次容器创建）
  - `crates/cli/src/sandbox_commands.rs`（如需增强 build/list 输出或 UX）
  - `docs/src/sandbox.md`（如需补充运维/预热说明）

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-sandbox-fixed-data-dir-mountpoint.md`
  - `docs/src/sandbox.md`
- Related commits/PRs：<TBD>

## 未决问题（Open Questions）
- Q1: contract 校验 image 时，使用 `.Config.Image`（tag）还是 ImageID（更可靠但更复杂）？
- Q2: 当 per-session override / skill image 存在时，重建策略是否需要更细粒度日志与原因码？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

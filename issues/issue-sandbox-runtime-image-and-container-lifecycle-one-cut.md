# Issue: sandbox 唯一运行镜像与容器生命周期 one-cut（sandbox / docker）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-26
- Checklist discipline: 每次增量更新除补“已实现 / 已覆盖测试”外，必须同步勾选正文里对应的 checklist；禁止出现文首已完成、正文 TODO 未更新的漂移
- Owners: tools/sandbox（primary） + gateway/ui + config
- Components: tools/sandbox, tools/exec, tools/process, gateway/ui, cli, config
- Affected providers/models: <N/A>

**已实现（如有，必须逐条写日期）**
- 2026-03-26：完成现状盘点并确认旧方案存在多真源/多路径混用；历史证据保留在 superseded 旧单：`issues/issue-sandbox-prebuild-race-home-sandbox-workdir.md:1`
- 2026-03-26：sandbox one-cut 收敛为“唯一运行镜像 + 容器生命周期”（删除 build/pull/provision 路径，启动期同步校验 + startup 策略）：`crates/tools/src/sandbox.rs:1`
- 2026-03-26：`exec`/`process` 默认目录合同统一为 `/moltis/workdir`（含 `HOME`/`TMPDIR`）：`crates/tools/src/exec.rs:1`、`crates/tools/src/process.rs:1`
- 2026-03-26：配置 one-cut：拒绝 legacy `backend`/`packages`/`container_prefix`/`scope`，并新增 `startup_container_policy`：`crates/config/src/schema.rs:1`、`crates/config/src/validate.rs:1`
- 2026-03-26：UI/RPC/CLI one-cut：移除 images build / per-session sandbox override / `moltis sandbox` 镜像管理命令组：`crates/gateway/src/assets/js/page-images.js:1`、`crates/gateway/src/session.rs:1`、`crates/cli/src/main.rs:1`
- 2026-03-26：同步更新 sandbox 文档为 Docker-only + no build/pull：`docs/src/sandbox.md:1`

**已覆盖测试（如有）**
- 单测：schema one-cut 拒绝 legacy/alias，镜像缺失/模式非法 fail-fast：`crates/tools/src/sandbox.rs:1683`
- 单测：sandbox 开启但无容器后端时 exec fail-fast：`crates/tools/src/exec.rs:1097`
- 单测：配置校验拒绝 `mode="non_main"`：`crates/config/src/validate.rs:1506`
- UI E2E：Images 页不再提供 build/default-image 控件：`crates/gateway/ui/e2e/specs/images.spec.js:1`
- Playwright 全量已通过（2026-03-26，本机 runner；无需 sudo）：`just ui-e2e`（136 passed，3 skipped）
- Rust 全量已通过（2026-03-26）：`cargo test --workspace`

**已知差异/后续优化（非阻塞）**
- 本单只收敛 Docker sandbox 的运行镜像与容器生命周期；`no_network`、`resource_limits`、外部 mounts 的细节语义只要求并入运行合同，不在本单另做泛化设计。
- 本单先冻结容器内目录职责：`/moltis/data` 只承担实例数据路径语义，`/moltis/workdir` 只承担默认可写工作区语义；`bind`/`volume`/`ro`/`rw` 的进一步收敛另开专项治理，不在本单扩写。

---

## 背景（Background）
- 场景：Moltis 需要在 Docker sandbox 内执行 `exec` / `process` / tmux，但当前实现把“镜像构建、镜像选择、容器复用、请求执行”混在一起，导致时序、命名、可观测性和故障语义均不稳定。
- 约束：
  - Moltis **不负责 build 镜像**、不负责 pull 镜像；运行前置条件是：配置里的 sandbox 运行镜像已经存在于本地 Docker 镜像库。
  - 本单 hard-cut 到 Docker：配置中不再保留 `backend` 选择项；sandbox 启用即表示“使用 Docker sandbox”，Docker 不可用时直接失败。
  - 请求主路径**绝不允许** build 镜像、pull 镜像、apt-get provision、运行时切换 image。
  - 必须保留一个启动期容器处理选项，允许用户选择“启动时全部删除旧容器”或“仅复用完全匹配当前合同的旧容器”。
  - 本单只冻结容器内 guest path 语义，不在本单重新设计宿主机映射拓扑；实现时只允许围绕“实例数据路径”和“默认工作区路径”收口，不得顺手扩展第三套目录语义。
- Out of scope：
  - 本单不设计外部镜像生产流程（例如 CI build / docker build / docker pull / 镜像仓库发布）。
  - 本单不扩展新的通用抽象层，不引入跨后端框架层。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **运行镜像**（主称呼）：`[tools.exec.sandbox]` 中配置的唯一 Docker 本地镜像引用，例如 `moltis-sandbox:20260326`。
  - Why：它是 sandbox 运行环境的唯一事实来源。
  - Not：它不是 base image、不是“默认 image override”、不是 per-session image。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：runtime image / sandbox image

- **运行合同**（主称呼）：当前配置下容器必须满足的最小运行条件集合，至少包含 `运行镜像`、数据挂载、关键环境变量、工作目录语义、网络/资源限制语义。
  - Why：只有完全满足合同的容器才允许复用。
  - Not：它不是“尽量兼容”的启发式检查，不允许 silent degrade。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：container contract

- **工作目录**（主称呼）：容器内唯一默认可写目录，固定为 `/moltis/workdir`。
  - Why：请求执行、tmux、临时文件、`HOME` 都统一落在这一个目录，避免 `/home/sandbox`、`/tmp`、随机 cwd 散落。
  - Not：它不是实例数据目录，也不是由外部镜像自行决定的任意目录。
  - Source/Method：effective / as-sent
  - Aliases（仅记录，不在正文使用）：sandbox workdir

- **实例数据目录**（主称呼）：容器内暴露 Moltis 实例数据的固定路径，固定为 `/moltis/data`。
  - Why：它把“实例级事实数据”与“命令执行工作区”硬分离，避免用户命令把实例数据目录误当成默认 cwd、临时目录或构建输出目录。
  - Not：它不是默认工作目录、不是 `HOME`、不是临时文件目录、不是额外的第三工作区。
  - Source/Method：effective / as-sent
  - Aliases（仅记录，不在正文使用）：sandbox data dir / guest data dir

- **实例容器标签**（主称呼）：Moltis 创建 sandbox 容器时写入的 Docker labels，用于唯一识别“本实例自己管理的容器”。
  - Why：启动阶段删除或复用旧容器时，必须依赖显式标签，而不是名字猜测或模糊前缀。
  - Not：它不是给用户看的业务字段，也不是容器命名约定的替代品。
  - Source/Method：as-sent
  - Aliases（仅记录，不在正文使用）：managed container labels

- **启动容器策略**（主称呼）：服务启动时如何处理本实例上一次运行遗留的 sandbox 容器。
  - Why：用户必须显式选择“全部删除旧容器”还是“只复用完全匹配当前运行合同的旧容器”。
  - Not：它不影响运行镜像事实，不允许启动后再动态切换。
  - Source/Method：configured
  - Aliases（仅记录，不在正文使用）：startup container policy

- **旧容器**（主称呼）：由当前 Moltis 实例创建并管理、但不是本次请求新创建的 sandbox 容器。
  - Why：启动阶段和请求阶段都只能处理“本实例自己管理的容器”，不能误伤其它 Docker 容器。
  - Not：它不是所有 Docker 容器，也不是所有名字里带 `sandbox` 的容器。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：managed sandbox container

- **坏容器**（主称呼）：请求到来时，该 scope 对应容器存在但不可继续执行的状态。仅限以下情形：容器不在 `running` 状态；`docker exec` 返回“容器不存在/未运行/OCI 无法进入”类错误；或运行合同检查明确失败。
  - Why：请求路径只允许对“坏容器”做删除后重建，不允许借此修配置漂移或运行时切镜像。
  - Not：它不是“看起来不顺眼的旧容器”，也不是配置变更的补丁入口。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：broken container

- **空闲回收**（主称呼）：按 `idle_ttl_secs` 删除长时间未使用的 sandbox 容器。
  - Why：它只控制容器缓存寿命。
  - Not：它不参与镜像选择、不参与镜像构建、不参与配置切换。
  - Source/Method：configured / effective
  - Aliases（仅记录，不在正文使用）：TTL cleanup

- **scope 生命周期锁**（主称呼）：同一 `scope_key + scope_value` 对应容器的生命周期串行化约束。
  - Why：它保证同一 scope 不会被并发请求或 TTL 清理同时创建、删除、重建，避免并发下出现双容器、误删活容器、状态撕裂。
  - Not：它不是新的跨模块框架层，也不是全局大锁；只服务于单个 scope 的容器生命周期串行化。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：per-scope lifecycle lock

- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并默认值、校验、策略判断后的生效值
  - as-sent：最终传给 Docker CLI 的实参

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] sandbox 运行环境只认配置里的唯一 `运行镜像`；Moltis 自身不再 build/pull/provision 镜像。
- [x] 启动阶段必须在对外服务前完成：配置校验、本地镜像存在性校验、运行合同校验、按 `启动容器策略` 处理旧容器。
- [x] 请求阶段只允许四种结果：新建容器、复用容器、删除坏容器后重建、直接失败；请求阶段绝不 build 镜像。
- [x] `exec` 与 `process` 的默认工作目录语义必须完全一致，并由运行镜像合同保证。
- [x] UI / CLI / 配置 /日志 /测试口径统一到“唯一运行镜像 + 容器生命周期”；旧的 build/prebuild/override 入口全部删除或直接失败。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：`[tools.exec.sandbox].image` 表示**最终运行镜像**，且必须是本地 Docker 可 inspect 的镜像引用（例如 `moltis-sandbox:20260326`），不要求也不默认使用远端仓库 URL。
  - 必须：sandbox 配置不再保留 `backend` 字段；命中 legacy `backend` 配置即启动失败并要求用户删除。
  - 必须：若本地不存在配置镜像，或镜像不满足运行合同，启动直接失败；不得先启动再慢慢补环境。
  - 必须：当 sandbox 启用时，`data_mount` 不允许为 `none`；`data_mount_type` 与 `data_mount_source` 必须完整、合法并通过配置校验。实例数据目录 `/moltis/data` 不能是“有时存在、有时不存在”的可选语义。
  - 必须：当 sandbox 启用时，legacy 构建字段或入口（例如 `packages`、build/prebuild/override 相关配置/API/CLI/UI）命中即失败，并给出明确 remediation；不得静默忽略。
  - 必须：`startup_container_policy` 只允许 `reset` / `reuse` 两个值，默认 `reset`；其它值配置校验直接失败。
  - 必须：`startup_container_policy = "reuse"` 只复用“本实例创建 + 镜像相同 + 运行合同相同 + 容器状态正常”的旧容器；其余一律删除。
  - 必须：容器内固定路径只保留两个：实例数据目录 `/moltis/data` 与可写工作目录 `/moltis/workdir`；`HOME` 必须等于 `/moltis/workdir`。
  - 必须：`/moltis/data` 只承担实例数据路径语义，不能再被当作默认 cwd、`HOME`、临时文件目录或命令输出目录；`/moltis/workdir` 才是唯一默认可写工作区。
  - 必须：`TMPDIR` 必须固定为 `/moltis/workdir/tmp`；不得再落回 `/tmp` 或其它第三默认目录。若目录不存在，运行路径必须先创建再执行请求。
  - 必须：同一 `scope_key + scope_value` 的容器生命周期操作必须串行化；请求路径、坏容器重建、TTL 清理不能并发地对同一 scope 执行 create/delete/rebuild。
  - 不得：保留 `packages`、prebuild、on-demand build、`set_global_image()`、per-session image override、UI 默认镜像切换、容器内 apt-get provision。
  - 不得：请求阶段 build/pull/provision/切换 image/host fallback。
- 兼容性：严格 one-cut；命中 legacy 配置或入口直接失败并给 remediation，不做 alias / shim / 双读 / 双写。
- 可观测性：启动与请求阶段的所有策略决策都必须补结构化日志，至少包含 `event`、`reason_code`、`decision`、`policy`，并带 `image`、`container_name`、`startup_container_policy`、`session_id`（如适用）。
- 安全与隐私：日志不得打印完整命令正文、环境敏感值、token；命令只允许 preview/hash/len。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1. 当前 sandbox 同时存在 gateway 后台 prebuild、CLI build、UI build/default-image、请求时容器内 provision 等多条路径，用户无法判断哪一条才是真正生效路径。
2. 请求路径既负责选镜像，又负责修补容器，又可能触发 provision，导致“请求执行”不再是单一主路径。
3. UI 还把 `moltis-cache/...` 的 cached image 与 sandbox 运行镜像混在一起展示和选择，形成第二语义。
4. 容器内部默认目录目前散落在 `/home/sandbox`、`/moltis/data` 等多处，目录职责不统一。

### 影响（Impact）
- 用户体验：第一次请求是否稳定、是否需要等待、是否命中旧容器都不确定。
- 可靠性：镜像与容器语义漂移，导致 `/home/sandbox`、image 切换、容器复用等问题反复出现。
- 排障成本：必须理解 prebuild / build / default image / cached image / provision 多套概念，违反第一性原则。

### 复现步骤（Reproduction）
1. 启用 Docker sandbox，并保留当前 build/prebuild/override/UI 入口。
2. 修改 sandbox 相关配置或在 UI/CLI 中切换 image/build image。
3. 观察启动、首次请求、再次请求的容器行为与日志。
4. 实际：镜像 owner、请求路径副作用、UI 语义混杂，均无法用单一规则解释。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - 旧模型（prebuild/build/provision/override）的历史证据保留在 superseded 旧单：`issues/issue-sandbox-prebuild-race-home-sandbox-workdir.md:1`
  - one-cut 后的运行镜像与启动期合同校验：`crates/tools/src/sandbox.rs:1023`
  - one-cut 后的容器合同（workdir/HOME/TMPDIR/labels/mounts/network）：`crates/tools/src/sandbox.rs:785`
  - 配置校验拒绝 legacy 字段与 alias：`crates/config/src/validate.rs:907`
  - 启动期同步 `startup_ensure_ready()`（ready 前校验镜像+合同+按策略清理旧容器）：`crates/gateway/src/server.rs:1565`
  - UI 仅展示 runtime info（不再提供 build/default-image/per-session override）：`crates/gateway/src/assets/js/page-images.js:1`
  - docs 已对齐为 Docker-only + no build/pull：`docs/src/sandbox.md:1`
- 当前测试覆盖：
  - 已有：见文首“已覆盖测试”；包含 Rust 单测与 Playwright 全量跑通证据。
  - 缺口：Docker daemon 级别的集成覆盖仍以手工验收为准（见 Test Plan 的 Manual Integration）。

## 根因分析（Root Cause）
- A. 事实源分裂：配置、runtime override、per-session override、gateway prebuild、CLI/UI build 同时在影响“运行镜像”。
- B. 请求主路径被污染：请求阶段既选镜像又修容器又可能 provision，违反单一职责。
- C. 产品语义混杂：sandbox 运行镜像与 cached tool image、UI default image、CLI build/list/clean 混为一谈。
- D. 生命周期漂移：启动、请求、TTL、手动操作各自有一套容器处理逻辑，导致行为不可预测。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - sandbox 运行环境只认配置中的唯一 `image`，且该 image 必须已经存在于本地 Docker 镜像库。
  - 启动阶段必须在 ready 之前完成：校验配置、校验本地镜像存在性、校验镜像运行合同、按 `startup_container_policy` 处理旧容器。
  - 请求阶段只允许：新建容器、复用容器、删除坏容器后重建、直接失败。
  - `idle_ttl_secs` 只影响空闲容器删除，不影响镜像、配置或请求动作语义。
  - 默认工作目录必须统一为 `/moltis/workdir`，并由 Docker run 参数与环境变量保证，而不是依赖镜像自行预创建 `/home/sandbox`。
  - `TMPDIR` 必须固定为 `/moltis/workdir/tmp`，从而把默认临时文件语义收口到同一工作区。
  - `/moltis/data` 与 `/moltis/workdir` 必须职责硬分离：前者只承载实例数据路径语义，后者只承载默认可写工作区语义；两者不得互相兼任。
  - 同一 scope 上的请求、坏容器删除后重建、TTL 清理必须遵循单一串行顺序；不得因为竞态产生第二个同 scope 容器或删掉正在执行请求的容器。
- 不得：
  - 不得在 Moltis 内 build 镜像、pull 镜像、根据 `packages` 组装镜像、在容器内 apt-get provision。
  - 不得保留 runtime image override、per-session image override、UI 默认 image 设置、sandbox image build/check-packages/default-image API、整个 `moltis sandbox` 镜像管理命令组。
  - 不得在请求阶段补做配置收敛、镜像切换、旧方案兼容。
- 应当：
  - 启动默认策略应优先 `reset`（全部删除本实例旧容器），`reuse` 作为显式可选策略。
  - 运行合同应覆盖：镜像引用、`/moltis/workdir` 可用、`HOME=/moltis/workdir`、`TMPDIR=/moltis/workdir/tmp`、mounts、网络/资源限制语义。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（淘汰，不推荐）：继续在旧 prebuild/build/provision 机制上补丁收敛
- 核心思路：保留 `packages`、prebuild、CLI/UI build，只补 `image` 合同和更多 guard。
- 优点：短期改动看起来更小。
- 风险/缺点：继续保留多真源与请求期副作用，无法满足第一性原则与唯一真源原则。

#### 方案 2（推荐）：运行镜像唯一真源 + Moltis 只管理容器生命周期
- 核心思路：删除所有镜像构建与运行时 image override 语义；配置里只保留唯一 `image`；启动期做镜像与旧容器处理；请求期只做容器生命周期。
- 优点：概念最少、状态源唯一、请求路径最短、测试面最清晰。
- 风险/缺点：是硬切 breaking change，需要明确移除旧配置/API/UI/CLI。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：`[tools.exec.sandbox].image` 是唯一运行镜像配置，语义为“最终运行镜像”，不是 base image。
  - 命中 legacy `packages` / `container_prefix` / `backend` / runtime image override / per-session image override 时，配置校验直接失败。
- 规则 2：Moltis 启动时只校验本地镜像存在性与运行合同，不 build、不 pull；校验失败直接启动失败。
  - 镜像运行合同的最小验证必须包含：镜像可被 Docker 本地 inspect；容器可用 `sh -lc` 执行；以 `-w /moltis/workdir -e HOME=/moltis/workdir -e TMPDIR=/moltis/workdir/tmp` 启动时可稳定进入工作目录。
  - 启动阶段必须先完成“配置 + 镜像 + 运行合同”校验，校验全部通过后才允许执行 `startup_container_policy`；任何前置校验失败都不得先删旧容器。
- 规则 3：Moltis 创建的 sandbox 容器必须带显式 Docker labels，至少包含：
  - `moltis.managed=true`
  - `moltis.role=sandbox`
  - `moltis.instance_id=<stable instance id>`
  - `moltis.image_ref=<configured image>`
  - `moltis.contract_version=sandbox_contract_v1`
  - `moltis.workdir=/moltis/workdir`
  - `moltis.tmpdir=/moltis/workdir/tmp`
  - `moltis.instance_id` 必须由**当前 Moltis 实例的 data_dir（canonicalized）**稳定派生，并带固定 schema/version 做哈希；不得引入额外用户可写状态。
- 规则 4：`startup_container_policy = "reset"` 时，启动阶段删除当前实例标签匹配的全部旧 sandbox 容器；`"reuse"` 时，只保留完全匹配当前运行合同且状态正常的旧容器，其余删除。
  - 启动阶段凡是命中“应删除”的目标容器，只要 `docker rm -f` 失败，启动必须直接失败；不得部分删除后继续 ready。
- 规则 5：请求阶段只允许：
  - **新建容器**：该 scope 没有容器时，基于当前唯一 `image` 创建；
  - **复用容器**：已有容器存在、运行正常且合同匹配时直接使用；
  - **重建坏容器**：容器存在但已坏时，删除后按同一 `image` 重建；
  - **直接失败**：创建/重建失败或 Docker daemon 不可用时，当前请求报错结束。
- 规则 5.1：请求阶段若命中坏容器但删除失败，当前请求必须直接失败；不得跳过删除继续创建第二个并存容器，也不得 silent degrade。
- 规则 5.2：同一 scope 的请求路径必须受 `scope 生命周期锁` 保护；在锁持有期间，TTL 清理不得删除该 scope 容器，第二个并发请求不得并发 create/rebuild 同一 scope 容器。
- 规则 6：请求阶段绝不 build/pull/provision 镜像；命中 legacy build/config/override 路径直接失败并输出 remediation。
- 规则 7：`exec` 与 `process` 的默认工作目录统一为 `/moltis/workdir`，并通过 Docker run / exec 参数与 `HOME=/moltis/workdir`、`TMPDIR=/moltis/workdir/tmp` 保证；不能再由不同工具各自硬编码不同语义。
- 规则 8：`/moltis/data` 只暴露 Moltis 实例数据，不能再被任何执行路径当作默认 cwd、`HOME`、临时目录或命令输出目录；若确需写入工作文件，只能落到 `/moltis/workdir`。
- 规则 9：本单不扩展新的 guest path，不引入 `/home/sandbox`、额外 project dir、额外 temp root 等第三默认目录语义；目录收口只允许保留 `/moltis/data` 与 `/moltis/workdir`。若配置了外部 mounts，它们也只能作为显式附加挂载存在，不能成为默认 cwd、`HOME`、`TMPDIR` 或实例数据目录的替代品。

#### 接口与数据结构（Contracts）
- 配置：
  - 保留：`mode`、`scope_key`、`image`、`startup_container_policy`、`idle_ttl_secs`、数据挂载字段、`no_network`、`resource_limits`、外部 mounts。
  - 删除：`backend`、`packages`、`container_prefix`、任何 image build/prebuild/override 相关配置与持久化字段。
  - 约束：当 `mode != "off"` 时，`data_mount` 只能是 `ro` 或 `rw`；`none` 视为配置错误并直接失败。
  - 约束：外部 mounts 允许保留，但只作为显式附加挂载；不得借由 mounts 重新引入第三套“默认工作目录 / 默认临时目录 / 默认实例数据目录”语义。
- API/RPC：
  - 删除：`/api/images/build`、`/api/images/check-packages`、`/api/images/default`。
  - 删除：session 级 `sandboxImage` patch/selector。
  - UI 只展示 configured image / startup policy / readiness，不再提供运行时 image 管理入口。
- CLI：
  - 删除整个 `moltis sandbox` 镜像管理命令组；镜像的 build / pull / inspect / clean 统一回归 Docker CLI。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 配置缺失/非法：启动失败，报 `SANDBOX_CONFIG_INVALID`
  - 命中 legacy 构建配置或入口：启动失败或请求失败，报 `SANDBOX_LEGACY_BUILD_PATH_REMOVED`
  - 命中 legacy `backend` 字段：启动失败，报 `SANDBOX_LEGACY_BACKEND_REMOVED`
  - Docker daemon 不可用：启动失败，报 `SANDBOX_BACKEND_UNAVAILABLE`
  - 本地镜像不存在：启动失败，报 `SANDBOX_IMAGE_MISSING`，并提示先用 `docker image inspect <image>` 自查
  - 运行镜像不满足合同：启动失败，报 `SANDBOX_IMAGE_CONTRACT_INVALID`
  - 启动阶段旧容器清理失败：启动失败，报 `SANDBOX_CONTAINER_CLEANUP_FAILED`
  - 请求期容器创建/重建失败：当前请求失败，报 `SANDBOX_CONTAINER_CREATE_FAILED` / `SANDBOX_CONTAINER_REBUILD_FAILED`
  - 请求期坏容器删除失败：当前请求失败，报 `SANDBOX_CONTAINER_DELETE_FAILED`
  - 同一 scope 生命周期并发冲突：必须被串行化吸收，不允许因为竞态额外暴露第二套容器；这属于内部串行化语义，不单独引入新的业务失败码。
- 队列/状态清理：
  - 启动阶段按策略删除旧容器时必须只作用于“当前实例标签匹配的 sandbox 容器”
  - TTL 清理只删除空闲容器，不删除镜像；命中正在被请求路径持有 `scope 生命周期锁` 的容器时必须跳过
  - 请求失败后不做 host fallback、不做 silent degrade

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：日志只输出 image ref、container_name、reason_code、policy、session_id；命令正文只允许 preview/hash/len。
- 禁止打印字段清单：完整命令正文、完整环境变量值、token、用户文件正文。

## 验收标准（Acceptance Criteria）【不可省略】
> 已完成项必须同步勾选；若本区块只想保留历史快照而不维护勾选状态，请改成普通 bullet，不要保留假 TODO。
- [x] Moltis 不再负责 sandbox 镜像 build/pull/provision；命中旧 build/prebuild/override 入口直接失败或代码已删除。
- [x] `tools.exec.sandbox.image` 成为唯一运行镜像配置；请求路径不再有 `resolve_image()` 多优先级镜像选择语义。
- [x] 启动阶段在 ready 之前完成：本地镜像校验、运行合同校验、按 `startup_container_policy` 处理实例标签匹配的旧容器；镜像缺失或合同无效时启动失败。
- [x] 请求阶段只存在“新建容器 / 复用容器 / 重建坏容器 / 直接失败”四种结果，且请求阶段绝不 build 镜像。
- [x] `exec` 与 `process` 的默认工作目录语义完全一致，并固定为 `/moltis/workdir`；`HOME=/moltis/workdir`、`TMPDIR=/moltis/workdir/tmp`；`/moltis/data` 只承担实例数据路径语义，不再兼任默认 cwd / `HOME` / 临时目录。
- [x] `idle_ttl_secs` 只影响空闲容器回收，不再影响镜像选择、配置切换或请求动作语义。
- [x] 同一 scope 的并发请求与 TTL 清理不会产生双容器、误删活容器或状态撕裂；竞态语义已按主路径串行化冻结。
- [x] UI / CLI / 配置 /日志 /测试口径全部切到新模型，旧单不再作为实现依据。

## 测试计划（Test Plan）【不可省略】
> 已完成且有证据的测试项必须同步勾选；未勾选项表示当前仍未补到自动化证据或手工验收说明。
### Unit
- [x] schema->tools one-cut：拒绝 legacy 字段（backend/packages/container_prefix/scope）、拒绝 `mode="non_main"`、sandbox 启用必须配置 `image`：`crates/tools/src/sandbox.rs:1683`
- [x] sandbox 开启但无容器后端时 exec fail-fast：`crates/tools/src/exec.rs:1097`
- [x] 配置校验拒绝 `mode="non_main"`：`crates/config/src/validate.rs:1506`
- [x] Docker run args 合同：`-w /moltis/workdir` + `HOME` + `TMPDIR` + 合同 labels：`crates/tools/src/sandbox.rs:1689`

### Manual Integration（Docker）
- 验证 `SANDBOX_IMAGE_MISSING`：配置不存在的 `tools.exec.sandbox.image`，启动应失败并提示 `docker image inspect <image>`。
- 验证 `SANDBOX_IMAGE_CONTRACT_INVALID`：配置一个缺少 `sh`/不支持 `-w /moltis/workdir` 的镜像，启动应失败。
- 验证 `startup_container_policy=reset|reuse`：准备两类 managed 容器（合同匹配/不匹配），重启 gateway 后观察删除/复用行为与日志。
- 验证请求期 only-load：触发 `exec`/`process`，只允许新建/复用/删除坏容器后重建/失败四种动作，且不会出现 build/pull/provision 日志。

### UI E2E（Playwright）
- [x] `crates/gateway/ui/e2e/specs/images.spec.js`：Images 页不再提供 build/default-image/per-session image 入口，只展示 configured image / startup policy / readiness。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：Docker 相关集成测试在 CI 上可能受 daemon 可用性限制。
- 手工验证步骤：
  - 预先准备本地镜像 `docker image inspect <image>` 成功；
  - 启动 Moltis，确认未出现 sandbox build/prebuild 日志，且只有在配置、镜像、运行合同全部通过后，才按 `startup_container_policy` 处理实例标签匹配的旧容器；
  - 发起 `exec` / `process` 请求，确认只发生新建/复用/删除坏容器后重建/失败四种动作，且默认进入 `/moltis/workdir`，`HOME=/moltis/workdir`，`TMPDIR=/moltis/workdir/tmp`；
  - 设置 `idle_ttl_secs > 0`，确认只删除空闲容器，不删除镜像。

## 发布与回滚（Rollout & Rollback）
- 发布策略：hard-cut 发布；删除 legacy build/prebuild/override 模型，不做兼容尾巴。
- 回滚策略：整体回滚到旧版本；风险是重新引入多真源和请求期副作用。
- 上线观测：重点监控 `SANDBOX_IMAGE_MISSING`、`SANDBOX_IMAGE_CONTRACT_INVALID`、`SANDBOX_CONTAINER_CLEANUP_FAILED`、`SANDBOX_CONTAINER_DELETE_FAILED`、`SANDBOX_CONTAINER_CREATE_FAILED`、`SANDBOX_CONTAINER_REBUILD_FAILED`、TTL 删除日志。

## 实施拆分（Implementation Outline）
- Step 1: 配置模型 one-cut：删除 `backend` / `packages` / build/prebuild/override 语义，冻结 `image` + `startup_container_policy`。
- Step 2: 启动阶段实现“本地镜像校验 + 运行合同校验 + 基于实例标签按策略处理旧容器”。
- Step 3: 请求阶段收敛为“新建容器 / 复用容器 / 重建坏容器 / 直接失败”，删除 build/provision 分支。
- Step 4: UI / CLI / API /日志 /测试同步删除 legacy 入口并补齐可观测性。
- 受影响文件：
  - `crates/config/src/schema.rs`
  - `crates/config/src/validate.rs`
  - `crates/tools/src/sandbox.rs`
  - `crates/tools/src/exec.rs`
  - `crates/tools/src/process.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/assets/js/page-images.js`
  - `crates/gateway/src/assets/js/sandbox.js`
  - `crates/cli/src/sandbox_commands.rs`
  - `docs/src/sandbox.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-sandbox-prebuild-race-home-sandbox-workdir.md`（已 superseded；仅保留旧机制问题证据）
  - `issues/done/issue-sandbox-fixed-data-dir-mountpoint.md`
  - `docs/src/sandbox.md`
- Related commits/PRs：
  - `e7cf1a3` One-cut sandbox runtime image & container lifecycle
  - `2ed917a` docs+ui: harden sandbox one-cut contracts
- External refs（可选）：Docker 本地镜像与容器 inspect 行为

## 未决问题（Open Questions）
- <N/A>：本轮 review 已将阻塞实施的系统性未决问题全部并回正文；后续若再发现新增范围，必须先回写主单再实施。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

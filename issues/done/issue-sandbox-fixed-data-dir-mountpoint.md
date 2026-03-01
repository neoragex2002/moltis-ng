# Issue: 固定 sandbox 内 data_dir 挂载点为 `/moltis/data`（sandbox / agents）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Owners: Neo
- Components: tools/sandbox, agents/prompt, config
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- 2026-02-27：固定 sandbox guest data_dir 为 `/moltis/data`（Docker mount point + env）：`crates/tools/src/sandbox.rs:323`
- 2026-02-27：Docker sandbox data mount（bind/volume）与 fail-fast 校验：`crates/tools/src/sandbox.rs:869`
- 2026-02-27：`backend=apple-container` 明确不支持（fail-fast + remediation）：`crates/tools/src/sandbox.rs:1393`
- 2026-02-27：对外配置键名收敛到 `tools.exec.sandbox.data_mount*`（schema + template + validate）：`crates/config/src/schema.rs:1279`
- 2026-02-27：persona/system prompt People 引用改为真实可达路径 `/moltis/data/PEOPLE.md`：`crates/agents/src/prompt.rs:125`
- 2026-02-27：gateway debug/context 输出收敛为 `dataMount`：`crates/gateway/src/chat.rs:3606`

**已覆盖测试（如有）**
- Docker data_mount args（bind/volume + invalid/missing fail-fast）：`crates/tools/src/sandbox.rs:2744`
- Docker run args 注入 `MOLTIS_DATA_DIR=/moltis/data`：`crates/tools/src/sandbox.rs:2842`
- `backend=apple-container` fail-fast：`crates/tools/src/sandbox.rs:3444`
- prompt People 路径为 `/moltis/data/PEOPLE.md`：`crates/agents/src/prompt.rs:1182`
- config validate：`backend=apple-container` 报错（早失败）：`crates/config/src/validate.rs:1826`

**已知差异/后续优化（非阻塞）**
- CI/单测层面未增加 “真实启动 Docker sandbox 并验证 `/moltis/data` 可访问” 的集成测试（原因：CI/container runtime 不稳定）；保留手工验收步骤即可。

---

## 背景（Background）
- 场景：agent/工具在 sandbox 容器内需要读取 instance 数据文件（例如 `PEOPLE.md`、persona 文件、session state 等）。
- 目的（Goal）：在 system prompt / persona 等“提示性文本”中，能够**稳定地引用 sandbox 容器内可访问的 Moltis data 目录路径**，避免根据部署环境（宿主机路径、容器内路径、volume 等）动态拼接/泄露宿主机绝对路径；并确保该引用对 agent 是“可执行的真实路径”，而不是概念占位符。
- 约束：
  - sandbox 容器内默认 `HOME`/用户不可假定与宿主机一致（容器通常以 root 运行，且 `HOME` 可能被设为 `/home/sandbox`）。
  - 宿主机 data_dir 可随启动用户/`MOLTIS_DATA_DIR`/CLI 参数变化。
- 现有 Docker sandbox 采用 “host_path:guest_path 1:1” 挂载，会把宿主机绝对路径暴露为容器内路径（不可移植、不可稳定引用）。
- gateway 进程可能运行在容器内并通过 `/var/run/docker.sock` 创建 sandbox 容器：此时 gateway “看到的路径”（例如容器内 `/moltis/data`）不等价于 Docker daemon “看到的路径”（宿主机文件系统/volume）。因此不能把 gateway 的 `data_dir()` 直接当作 `docker run -v <source>:...` 的 `<source>`。
- Out of scope：
  - 本 issue 不重做整体 sandbox 架构（只做 data_dir 的固定挂载点与 prompt 口径收敛）。
  - 不在本 issue 内引入新的同步机制（如 rsync/镜像 bake-in）。
  - 本 issue 直接不支持 Apple Container backend；当用户选择 `backend=apple-container` 时必须明确报错并提示改用 Docker。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **data_mount**（主称呼，配置项名）：是否将 **data_mount_source** 挂载进 sandbox 容器（固定挂载点 `/moltis/data`；mode=`none|ro|rw`）。
  - Why：agent/工具需要稳定且可执行的容器内真实路径（`/moltis/data/...`），避免泄露/绑定宿主机绝对路径。
  - Not：它不是“项目仓库目录挂载”；也不是 `sandbox.mounts[]`（外部目录 allowlist 挂载）。
  - Source/Method：configured → effective（来自 sandbox config；合并默认后生效）
  - Aliases（仅记录，不在正文使用）：<None>

- **data_mount_type**（主称呼，配置项名）：data_mount_source 的类型：`bind|volume`。
  - Why：明确 Docker daemon 如何解析 `<mount_source>`（绝对路径 vs volume 名）。
  - Not：它不是 gateway data_dir 的“路径形态”，也不是容器内路径类型。
  - Source/Method：configured → effective
  - Aliases（仅记录，不在正文使用）：<None>

- **data_mount_source**（主称呼，配置项名）：创建 sandbox 容器时用于挂载到 `/moltis/data` 的 `<mount_source>`（Docker host 视角）。
  - bind：Docker daemon 宿主机可见的**绝对路径**（例如 `/srv/moltis-data`）
  - volume：Docker volume 名（例如 `moltis-data`）
  - Why：容器化部署下 gateway 的 `data_dir()` 往往是“容器内路径”，不能拿来当 `docker run -v <source>:...` 的 `<source>`。
  - Not：它不等同于 gateway data_dir（进程视角）。
  - Source/Method：configured → effective
  - Aliases（仅记录，不在正文使用）：mount backing / host-visible source

- **data_dir**（主称呼）：Moltis instance 的数据目录根（包含 `PEOPLE.md`、personas、sessions、memory 等）。
  - Why：工具/agent 需要确定性路径读取/写入这些文件。
  - Not：它不是容器内的 `HOME`，也不等价于 `~/.moltis`。
  - Source/Method：configured（由 CLI/环境变量/默认值解析得到）
  - Aliases（仅记录，不在正文使用）：`~/.moltis` / “数据目录”

- **gateway data_dir（进程视角）**（主称呼）：gateway 进程自身读写数据使用的目录（在“gateway 所在的文件系统”内可达即可）。
  - Why：容器化部署时，gateway data_dir 常是容器内路径（如 `/moltis/data`），它对 gateway 可达，但对 Docker daemon 并不一定可达。
  - Not：它不保证能作为 `docker run -v <source>:...` 的 source。
  - Source/Method：configured（`MOLTIS_DATA_DIR`/CLI/默认值）
  - Aliases（仅记录，不在正文使用）：runtime data dir

- **sandbox 固定 data_dir 挂载点**（主称呼）：容器内固定使用 `/moltis/data` 作为 data_dir 的访问入口。
  - Why：避免把宿主机绝对路径写入 system prompt；避免依赖 `~`；跨用户/跨机器一致。
  - Not：它不是宿主机路径；它只是容器内约定的“入口点”。
  - Source/Method：configured（由 sandbox runtime 创建容器时注入）
  - Aliases（仅记录，不在正文使用）：guest data dir mountpoint

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] Docker sandbox：将 **data_mount_source** 挂载到容器内固定路径 `/moltis/data`（ro/rw 由 `data_mount` 决定；source 可为宿主机绝对路径或 Docker volume）。
- [x] Docker sandbox：创建容器时注入 `MOLTIS_DATA_DIR=/moltis/data`，确保容器内运行的代码调用 `moltis_config::data_dir()` 时解析到固定挂载点。
- [x] Agents persona/system prompt：将 `PEOPLE.md` 等引用从占位符 `<data_dir>/...` 改为真实可达路径 `/moltis/data/...`（至少覆盖 `PEOPLE.md`）。本单采取 fail-fast，不存在“mount 不可用但继续执行”的降级态。
- [x] Apple Container backend：明确为不支持（fail-fast），并给出“改用 Docker”的 remediation（避免引入自动切换/降级策略）。
- [x] 定义并实现 sandbox data mount backing 配置（用于容器化/volume 场景，且只保留一套命名）：
  - `tools.exec.sandbox.data_mount_type`：`bind` | `volume`
  - `tools.exec.sandbox.data_mount_source`：
    - type=`bind`：Docker daemon 宿主机可见的绝对路径（例如 `/srv/moltis-data`）
    - type=`volume`：Docker volume 名（例如 `moltis-data`）

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：system prompt 里出现的文件路径应当是“容器内可直接访问的真实路径”，不得依赖隐含概念（如 “data_dir” 未解析）。
  - 不得：不得将宿主机用户名相关的绝对路径写死在 prompt（例如 `/home/<user>/.moltis`）。
- 兼容性：
  - 宿主机主进程（gateway）对 `MOLTIS_DATA_DIR`/默认值的解析保持不变；仅 sandbox 容器内路径口径改变。
  - 行为变化（破坏性）：Docker sandbox 不再隐式使用 `data_dir()` 作为挂载 source；升级后必须显式配置 `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source`，否则 fail-fast。
  - 已存在且被复用的 sandbox 容器可能缺少新 env/mount，需要给出迁移/重建策略。
  - 容器化部署兼容：gateway 容器内 data_dir 统一使用 `/moltis/data`；backing 支持 bind mount 或 named volume（通过 `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source` 显式指定）。
  - 术语收敛：对外只存在 `tools.exec.sandbox.data_mount`/`tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source`（不提供 alias / 后向兼容）。
- 可观测性：
  - 在 sandbox 启动日志中打印一次性摘要（backend、mountpoint、ro/rw、是否启用）。
- 安全与隐私：
  - 默认避免把宿主机绝对路径写进 prompt；日志中也避免打印宿主机 data_dir 真实路径（如确需打印，应脱敏或仅在 debug）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) system prompt 中引用 `<data_dir>/PEOPLE.md`，agent 无法理解/定位 `<data_dir>` 是什么目录，导致读取 roster 的指引不可执行。
2) Docker sandbox 当前挂载策略为 `host_abs_path:host_abs_path`，prompt 若要给出真实路径，会泄露并绑定宿主机用户名/路径结构，且在换用户/换机器时不稳定。
3) 当 gateway 运行在容器内并通过 docker.sock 创建 sandbox 容器时，gateway 的 `MOLTIS_DATA_DIR` 往往是容器内路径（如 `/moltis/data`）。若 sandbox mount 仍基于该路径拼接 `-v /moltis/data:...`，Docker daemon 将在宿主机侧解析该路径并导致挂载失败（或挂载到错误位置）。

### 影响（Impact）
- 用户体验：agent 依据 prompt 查找文件失败，产生误导性指引与额外排障成本。
- 可靠性：工具/agent 在 sandbox 内的“读取 data_dir 文件”能力缺少稳定契约，后续扩展（更多 data_dir 引用）会反复踩坑。
- 排障成本：问题表现为“文件不存在/路径不对”，根因是“约定缺失”，容易反复被误修。

### 复现步骤（Reproduction）
1. 运行任意会注入 persona/system prompt 的流程。
2. 观察 persona 中的 People 参考路径为 `<data_dir>/PEOPLE.md`。
3. 期望 vs 实际：
   - 期望：给出容器内可直接访问的路径（如 `/moltis/data/PEOPLE.md`）。
   - 实际：给出概念占位符，agent 无法解析。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 本节以“修复后现状”为准；历史问题已体现在 Problem Statement 与 Symptoms。

- 代码证据：
  - `crates/tools/src/sandbox.rs:323`：sandbox 容器内固定 data_dir 挂载点常量为 `/moltis/data`。
  - `crates/tools/src/sandbox.rs:869`：Docker sandbox data_mount（bind/volume）严格校验，缺配置直接 fail-fast（仅提 `tools.exec.sandbox.*` 键名）。
  - `crates/tools/src/sandbox.rs:1393`：`backend=apple-container` 直接 fail-fast（明确 remediation：改用 Docker）。
  - `crates/agents/src/prompt.rs:125`：persona People reference 输出 `/moltis/data/PEOPLE.md`（真实可达路径）。
  - `crates/config/src/schema.rs:1279`：对外配置键名收敛为 `tools.exec.sandbox.data_mount*`。
- 配置/文档证据（部署示例与心智模型口径）：
  - `docs/src/sandbox.md:24`：明确 data directory mount 的容器内固定路径为 `/moltis/data`（并说明 env 注入）。
  - `docs/src/docker.md:15`：docker 部署示例统一使用 `-e MOLTIS_DATA_DIR=/moltis/data -v ...:/moltis/data`。
- 当前测试覆盖：
  - Docker args 构造与 fail-fast（bind/volume + invalid/missing）：`crates/tools/src/sandbox.rs:2744`
  - Docker run args 注入 `MOLTIS_DATA_DIR=/moltis/data`：`crates/tools/src/sandbox.rs:2842`
  - `backend=apple-container` fail-fast：`crates/tools/src/sandbox.rs:3444`
  - prompt People reference 路径：`crates/agents/src/prompt.rs:1182`
  - config validate（`backend=apple-container`）：`crates/config/src/validate.rs:1826`

## 根因分析（Root Cause）
- A. prompt 层将 data_dir 作为“人类概念”写入 system prompt，但未把它绑定到任何可达路径契约。
- B. sandbox 层将 “gateway data_dir（进程视角）” 与 “data_mount_source（Docker host 视角）” 混为一谈：容器化部署下 `data_dir()` 往往是容器内路径或 volume 挂载点，不能直接用于 `docker run -v <source>:...`。
- C. sandbox 层缺少稳定的容器内访问入口点（当前 1:1 映射到宿主机绝对路径），导致 prompt 无法引用一个跨用户/跨机器稳定的路径。
- D. 容器内 `HOME`/用户与宿主机不一致，使得 `~/.moltis` 也不是可靠路径。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当 sandbox data_mount 启用时，容器内必须存在固定可达路径 `/moltis/data` 指向宿主机 data_dir 内容。
  - 容器创建时必须设置 `MOLTIS_DATA_DIR=/moltis/data`，使容器内运行的 Moltis 代码与脚本统一解析到该路径。
  - sandbox data_mount 的 source 必须显式可配置为 `bind`（宿主机绝对路径）或 `volume`（Docker volume 名），不得隐式依赖 gateway 的 `MOLTIS_DATA_DIR` 恰好等于 Docker host 可见路径。
  - system prompt 中引用的 `<data_dir>` 文件路径必须使用 `/moltis/data/<file>` 形式；不得出现占位符（如 `<data_dir>/...`）。
  - 本单采取 fail-fast：若 backend 不支持或配置不满足条件，则不得创建 sandbox 容器，必须直接报错并给出 remediation（避免“降级态”与误导性输出）。
- 不得：
  - 不得在 system prompt 中使用未定义的占位符目录（如 `<data_dir>/...`）。
  - 不得在 system prompt 中写死宿主机用户名相关的绝对路径。
- 应当：
  - 对不支持挂载的 backend（如 Apple Container），应当明确报错并提示改用 Docker。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：固定容器内挂载点 + env 注入
- 核心思路：把 sandbox 容器内 data_dir 入口固定为 `/moltis/data`，并在容器环境中设置 `MOLTIS_DATA_DIR=/moltis/data`；挂载的 source 通过 `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source` 显式指定（bind/volume），避免依赖 gateway 的进程视角路径。
- 优点：
  - 路径契约稳定、跨用户一致、prompt 可直接引用真实路径。
  - 容器内运行的代码无需特殊判断（`moltis_config::data_dir()` 自动正确）。
- 风险/缺点：
  - 需要处理“旧容器复用”导致 env/mount 不更新的问题（迁移/重建策略）。
  - Apple Container backend 当前不支持外部 mounts：本单直接不支持该 backend（Linux/Docker 为主路径）。

#### 方案 2（不推荐）：prompt 输出宿主机解析后的绝对路径
- 核心思路：prompt 直接写 `data_dir()` 的 resolved absolute path（如 `/home/alice/.moltis/PEOPLE.md`），依赖 Docker 的 1:1 bind mount。
- 缺点：泄露宿主机路径；换用户/路径变化导致不稳定；不适用于 Apple Container（当前无 mount）。

#### 方案 3（备选）：启动时复制必要文件到容器内固定目录
- 核心思路：每次 exec 前把 `PEOPLE.md`/personas 复制进容器（不做 bind mount）。
- 缺点：同步复杂、易不一致；写回/状态类文件难处理；更像临时 workaround。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：容器内固定 data_dir 为 `/moltis/data`（source=constant）。
- 规则 2：容器创建时必须注入 `MOLTIS_DATA_DIR=/moltis/data`（source=as-sent 到 container runtime）。
- 规则 3：system prompt 中任何“可执行的 <data_dir> 路径指引”必须使用 `/moltis/data/...`，不得出现 `<data_dir>/...` 占位符。
- 规则 4（Docker 前置条件，已决策）：backend 为 Docker 时，必须同时满足：
  - `data_mount != none`（语义：data_dir mount；ro/rw 任选其一）
  - `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source` 配置完整且通过语法校验
  - 否则必须 fail-fast（明确报错 + remediation），不得创建 sandbox 容器。
- 规则 5（Apple Container 前置条件，已决策）：backend 解析为 `apple-container` 时必须 fail-fast（含 `backend=auto` 在 macOS 命中 Apple Container 的情况；明确报错 + remediation），不得创建 sandbox 容器。
- 规则 6（配置命名收敛，已决策）：只保留一套对外配置键名（`tools.exec.sandbox.*`），且报错只提这一套键名：
  - `tools.exec.sandbox.data_mount` = `"none"` | `"ro"` | `"rw"`（规范上 Docker 必须为 `ro|rw`）
  - `tools.exec.sandbox.data_mount_type` = `"bind"` | `"volume"`
  - `tools.exec.sandbox.data_mount_source` = `"/srv/moltis-data"` | `"moltis-data"`
- 规则 7（错误口径，已决策）：fail-fast 必须返回明确错误码与 remediation，且不泄露宿主机真实路径。

#### prompt 输出规则（冻结）
> 目标：prompt 只输出“对 agent 可执行的真实路径”。本单采取 fail-fast，因此能进入 sandbox 的前提就是 mount 已可靠建立。

- 当 sandbox 启用时，prompt 中所有 data_dir 引用必须使用 `/moltis/data/<file>`（例如 `/moltis/data/PEOPLE.md`）。

#### 接口与数据结构（Contracts）
- Sandbox（Docker）：
  - `docker run` 必须包含：
    - `-v <mount_source>:/moltis/data:(ro|rw)`（`<mount_source>` 由 `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source` 决定：bind=宿主机绝对路径；volume=Docker volume 名）
    - `-e MOLTIS_DATA_DIR=/moltis/data`
- Sandbox（Apple Container）：
  - 本单直接不支持 Apple Container，必须 fail-fast，并返回明确错误：
    - error code（建议）：`SANDBOX_BACKEND_UNSUPPORTED`
    - message（必须包含 remediation）：`backend=apple-container is not supported; set tools.exec.sandbox.backend=docker`
- Agents persona：
  - People roster 路径（mount 可用时）：`/moltis/data/PEOPLE.md`
  - 由于本单采取 fail-fast，不存在“sandbox 启用但 mount 不可用”的降级态。

#### Quick Config（Examples）
> 目标：让部署方“照抄即可”，且不引入额外策略分支；示例只使用 `tools.exec.sandbox.*` 一套命名（报错也只提这一套）。

**Case 1：裸机/VM（gateway 在宿主机上跑）+ Docker bind mount**
```toml
[tools.exec.sandbox]
enabled = true
backend = "docker"
mode = "all"
scope = "session"
idle_ttl_secs = 0
no_network = false

# sandbox image / packages（与 mount 无关的核心项）
image = "ubuntu:25.10"
packages = ["git", "curl"]

# data_dir mount（本 issue 的核心）
data_mount = "ro"
data_mount_type = "bind"
data_mount_source = "/srv/moltis-data" # Docker daemon 宿主机可见的绝对路径
```

**Case 2：gateway 容器部署 + Docker bind mount（需要“宿主机绝对路径”，不是容器内路径）**
```toml
[tools.exec.sandbox]
enabled = true
backend = "docker"
mode = "all"
scope = "session"
idle_ttl_secs = 0
no_network = false

# sandbox image / packages（与 mount 无关的核心项）
image = "ubuntu:25.10"
packages = ["git", "curl"]

data_mount = "ro"
data_mount_type = "bind"
data_mount_source = "/srv/moltis-data" # 仍然是 Docker host 的绝对路径
```
> 说明：即使 gateway 容器内 `MOLTIS_DATA_DIR=/moltis/data`，这里也**不能**填 `/moltis/data`；`docker run -v <source>:...` 的 `<source>` 必须能被 Docker daemon 在宿主机视角解析。
>
> gateway 容器内 data_dir 路径统一使用 `/moltis/data`（即 `-e MOLTIS_DATA_DIR=/moltis/data -v ...:/moltis/data`）。

**Case 3（推荐）：gateway 容器部署 + Docker named volume**
```toml
[tools.exec.sandbox]
enabled = true
backend = "docker"
mode = "all"
scope = "session"
idle_ttl_secs = 0
no_network = false

# sandbox image / packages（与 mount 无关的核心项）
image = "ubuntu:25.10"
packages = ["git", "curl"]

data_mount = "ro"
data_mount_type = "volume"
data_mount_source = "moltis-data" # Docker volume 名（与 gateway 容器内路径无关）
```

**Case 4：不支持/配置不完整（必须 fail-fast，示例用于对照报错口径）**
- Apple Container（不支持）：
  ```toml
  [tools.exec.sandbox]
  enabled = true
  backend = "apple-container"
  ```
  - 期望错误：`SANDBOX_BACKEND_UNSUPPORTED`（remediation：改用 `tools.exec.sandbox.backend=docker`）
- Docker（缺 data_mount 配置）：
  ```toml
  [tools.exec.sandbox]
  enabled = true
  backend = "docker"
  data_mount = "none"
  ```
  - 期望错误：`SANDBOX_DATA_MOUNT_REQUIRED`（remediation：补齐 `tools.exec.sandbox.data_mount=ro|rw` + `tools.exec.sandbox.data_mount_type` + `tools.exec.sandbox.data_mount_source`）

**Case 1 / 2 / 3 的配置差异**
- 仅在 `data_mount_type` 与 `data_mount_source`：
  - bind：`data_mount_source` 为 Docker host 绝对路径
  - volume：`data_mount_source` 为 volume 名

#### 失败模式与降级（Failure modes & Degrade）
- 旧容器复用（缺少新 env/mount）：
  - 方案：检测版本/签名不一致则删除并重建；或在发布说明中要求清理旧 sandbox 容器。
- 配置缺失/无效：
  - 条件：`data_mount=none` 或 `tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source` 缺失/无效。
  - 方案：必须 fail-fast，并返回明确错误：
    - error code（建议）：`SANDBOX_DATA_MOUNT_REQUIRED`
    - message（必须包含 remediation）：`docker sandbox requires data_dir mount; set tools.exec.sandbox.data_mount=ro|rw and set tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source`
    - 不得创建 sandbox 容器。
- backend 不支持：
  - 条件：`backend=apple-container`
  - 方案：必须 fail-fast（见 Contracts 的错误码/文案），不得创建 sandbox 容器。

#### 安全与隐私（Security/Privacy）
- prompt 与默认日志中避免打印宿主机 data_dir 绝对路径。
- 仅在 debug 日志中（可选）打印 mount enabled 与 ro/rw，不打印 host 路径。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] Docker sandbox 启用 data_mount 且 data mount 配置完整时，容器内 `/moltis/data` 存在且可访问宿主机 data_dir 内容。
- [x] 容器内运行的任意 Moltis 代码调用 `moltis_config::data_dir()` 时解析到 `/moltis/data`（通过单测或集成验证证明）。
- [x] system prompt 不再出现 `<data_dir>/PEOPLE.md`；sandbox 场景下输出 `/moltis/data/PEOPLE.md`。
- [x] `backend=apple-container` 时 sandbox 明确报错（含 remediation），不得创建 sandbox 容器。
- [x] Docker 下当 `data_mount=none` 或 data mount 配置缺失/无效时 sandbox 明确报错（含 remediation），不得创建 sandbox 容器。
- [x] 容器化部署兼容（至少覆盖两类常见形态）：
  - [x] gateway 容器内 data_dir 为 `/moltis/data` 且 backing 为 bind mount：通过 `tools.exec.sandbox.data_mount_type=bind` 生效。
  - [x] gateway 容器内 data_dir 为 `/moltis/data` 且 backing 为 named volume：通过 `tools.exec.sandbox.data_mount_type=volume` 生效。
- [x] 回归测试覆盖：至少包含 Docker args 构造（bind/volume 两条路径）与 persona 输出路径断言（`/moltis/data/PEOPLE.md`）。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] Docker sandbox data_mount args（bind/volume）：`crates/tools/src/sandbox.rs:2744`
- [x] mount source 校验与 fail-fast（未配置/无效配置）：`crates/tools/src/sandbox.rs:2787`
- [x] Docker sandbox run env 注入（`MOLTIS_DATA_DIR=/moltis/data`）：`crates/tools/src/sandbox.rs:2842`
- [x] persona People 路径：`crates/agents/src/prompt.rs:1182`（断言输出 `/moltis/data/PEOPLE.md`）

### Integration
- [ ] （可选）在可用 Docker 的 CI/本地环境中，启动 sandbox 并 `ls /moltis/data/PEOPLE.md` 验证 mount（若现有测试框架允许）。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：CI 可能无法稳定提供 container runtime。
- 手工验证步骤：
  1. 启动 gateway 并触发一次 sandbox 工具执行。
  2. `docker exec <sandbox_container> ls -la /moltis/data`
  3. 验证 prompt 输出路径与容器内文件一致。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认启用 fail-fast（仅在配置完整时允许 sandbox），并提供迁移说明。
- 回滚策略：
  - 回滚代码变更后，旧行为恢复为 `host:host` 挂载与 `<data_dir>/...` 占位符（不建议长期保留）。
  - 回滚风险：已更新 prompt 的路径会与旧挂载策略不一致。
- 上线观测：
  - 关注 “sandbox mount missing / file not found: /moltis/data/...” 相关日志。
  - 关注 “sandbox data mount required” / “sandbox backend unsupported” 相关错误回执（确认 remediation 清晰且不会误导）。

## 实施拆分（Implementation Outline）
- Step 1: 定义常量 `SANDBOX_GUEST_DATA_DIR=/moltis/data`（tools/sandbox）。
- Step 2: 增加 sandbox data mount backing 配置与解析（`tools.exec.sandbox.data_mount_type`/`tools.exec.sandbox.data_mount_source`）。
- Step 3: 新增 `tools.exec.sandbox.data_mount`（`none|ro|rw`），并将其作为 Docker sandbox 的必填前置条件（不提供 alias / 后向兼容）。
- Step 4: Docker data mount args（`data_mount_args()` / `docker_run_args()`）改为挂载到固定 guest path `/moltis/data`（ro/rw 语义来自 `data_mount`）；source 由 `data_mount_type/source` 决定，并做严格校验与 fail-fast（缺一不可就报错）。
- Step 5: Docker `run` args 增加 `-e MOLTIS_DATA_DIR=/moltis/data`（创建时一次性注入）。
- Step 6: 旧 sandbox 容器迁移策略落地（推荐自动检测不匹配则删除重建，避免升级后行为不一致）。
- Step 7: Apple Container backend：当 `backend=apple-container` 时 fail-fast（错误码+明确 remediation），不做自动切换（避免策略复杂化）。
- Step 8: 更新 `crates/agents/src/prompt.rs`：
  - `<data_dir>/PEOPLE.md` → `/moltis/data/PEOPLE.md`
  - 更新/新增单测断言
- Step 9: 更新相关文档：解释 “gateway data_dir（进程视角）” 与 “data_mount_source（Docker host 视角）” 的区别，给出 3 个可用部署示例（Case 1/2/3）+ 1 个 fail-fast 对照（Case 4）。
- Step 10: 补齐 unit tests +（可选）integration smoke。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/agents/src/prompt.rs`
  - `docs/src/<相关文档>`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-persona-profiles-ui-default-persona-and-session-labels.md`
  - `docs/src/docker.md`（容器部署示例；建议同步更新为 data_dir=`/moltis/data`）
  - `docs/src/cloud-deploy.md`（部署示例；建议同步更新为 data_dir=`/moltis/data`）
- Related commits/PRs：<TBD>

## 未决问题（Open Questions）
- Q1: 是否也需要为 `MOLTIS_CONFIG_DIR` 引入类似固定挂载点（如 `/moltis/config`）？还是保持只修 data_dir（最小化）？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

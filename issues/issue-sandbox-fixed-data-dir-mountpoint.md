# Issue: 固定 sandbox 内 data_dir 挂载点为 `/moltis/data`（sandbox / agents）

## 实施现状（Status）【增量更新主入口】
- Status: SURVEY
- Priority: P1
- Owners: <可选>
- Components: tools/sandbox, agents/prompt, config
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- <None>

**已覆盖测试（如有）**
- <None>

**已知差异/后续优化（非阻塞）**
- Apple Container backend 当前未实现 host `data_dir` 的 bind mount（仅 Docker 实现了 workspace mount）；需要决定是否补齐或强制 fallback：`crates/tools/src/sandbox.rs:1666`。

---

## 背景（Background）
- 场景：agent/工具在 sandbox 容器内需要读取 instance 数据文件（例如 `PEOPLE.md`、persona 文件、session state 等）。
- 目的（Goal）：在 system prompt / persona 等“提示性文本”中，能够**稳定地引用 sandbox 容器内可访问的 Moltis data 目录路径**，避免根据部署环境（宿主机路径、容器内路径、volume 等）动态拼接/泄露宿主机绝对路径；并确保该引用对 agent 是“可执行的真实路径”，而不是概念占位符。
- 约束：
  - sandbox 容器内默认 `HOME`/用户不可假定与宿主机一致（容器通常以 root 运行，且 `HOME` 可能被设为 `/home/sandbox`）。
  - 宿主机 data_dir 可随启动用户/`MOLTIS_DATA_DIR`/CLI 参数变化。
  - 现有 Docker sandbox 采用 “host_path:guest_path 1:1” 挂载，会把宿主机绝对路径暴露为容器内路径（不可移植、不可稳定引用）。
  - gateway 进程可能运行在容器内并通过 `/var/run/docker.sock` 创建 sandbox 容器：此时 gateway “看到的路径”（例如容器内 `/data`）不等价于 Docker daemon “看到的路径”（宿主机文件系统/volume）。因此不能把 gateway 的 `data_dir()` 直接当作 `docker run -v <source>:...` 的 `<source>`。
- Out of scope：
  - 本 issue 不重做整体 sandbox 架构（只做 data_dir 的固定挂载点与 prompt 口径收敛）。
  - 不在本 issue 内引入新的同步机制（如 rsync/镜像 bake-in），除非 Apple Container 无法支持 volume mount。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **data_dir**（主称呼）：Moltis instance 的数据目录根（包含 `PEOPLE.md`、personas、sessions、memory 等）。
  - Why：工具/agent 需要确定性路径读取/写入这些文件。
  - Not：它不是容器内的 `HOME`，也不等价于 `~/.moltis`。
  - Source/Method：configured（由 CLI/环境变量/默认值解析得到）
  - Aliases（仅记录，不在正文使用）：`~/.moltis` / “数据目录”

- **gateway data_dir（进程视角）**（主称呼）：gateway 进程自身读写数据使用的目录（在“gateway 所在的文件系统”内可达即可）。
  - Why：容器化部署时，gateway data_dir 常是容器内路径（如 `/data`），它对 gateway 可达，但对 Docker daemon 并不一定可达。
  - Not：它不保证能作为 `docker run -v <source>:...` 的 source。
  - Source/Method：configured（`MOLTIS_DATA_DIR`/CLI/默认值）
  - Aliases（仅记录，不在正文使用）：runtime data dir

- **sandbox 固定 data_dir 挂载点**（主称呼）：容器内固定使用 `/moltis/data` 作为 data_dir 的访问入口。
  - Why：避免把宿主机绝对路径写入 system prompt；避免依赖 `~`；跨用户/跨机器一致。
  - Not：它不是宿主机路径；它只是容器内约定的“入口点”。
  - Source/Method：configured（由 sandbox runtime 创建容器时注入）
  - Aliases（仅记录，不在正文使用）：guest data dir mountpoint

- **sandbox data mount source（Docker host 视角）**（主称呼）：创建 sandbox 容器时用于挂载到 `/moltis/data` 的 “source”。
  - Why：`docker run -v <source>:/moltis/data` 的 `<source>` 必须由 Docker daemon 在其宿主机环境中解析（可能是宿主机绝对路径，也可能是 Docker volume 名）。
  - Not：它不等同于 gateway data_dir（进程视角）。
  - Source/Method：configured（新增配置/环境变量显式指定）
  - Aliases（仅记录，不在正文使用）：mount backing / host-visible source

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] Docker sandbox：将 **sandbox data mount source** 挂载到容器内固定路径 `/moltis/data`（ro/rw 由 `workspace_mount` 决定；source 可为宿主机绝对路径或 Docker volume）。
- [ ] Docker sandbox：创建容器时注入 `MOLTIS_DATA_DIR=/moltis/data`，确保容器内运行的代码调用 `moltis_config::data_dir()` 时解析到固定挂载点。
- [ ] Agents persona/system prompt：将 `PEOPLE.md` 等引用从占位符 `data_dir/...` 改为真实可达路径 `/moltis/data/...`（至少覆盖 `PEOPLE.md`），并且必须按能力 gate：当 sandbox 未挂载 data_dir 时不得输出会误导的“可执行路径”。
- [ ] 明确 Apple Container backend 行为：支持相同挂载点，或在需要 workspace mount 时强制选择 Docker（含文档/日志提示）。
- [ ] 定义并实现 **双配置变量**（用于容器化/volume 场景）：
  - `MOLTIS_SANDBOX_DATA_MOUNT_TYPE`：`bind` | `volume`
  - `MOLTIS_SANDBOX_DATA_MOUNT_SOURCE`：
    - type=`bind`：Docker daemon 宿主机可见的绝对路径（例如 `/srv/moltis-data`）
    - type=`volume`：Docker volume 名（例如 `moltis-data`）

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：system prompt 里出现的文件路径应当是“容器内可直接访问的真实路径”，不得依赖隐含概念（如 “data_dir” 未解析）。
  - 不得：不得将宿主机用户名相关的绝对路径写死在 prompt（例如 `/home/<user>/.moltis`）。
- 兼容性：
  - 宿主机主进程（gateway）对 `MOLTIS_DATA_DIR`/默认值的解析保持不变；仅 sandbox 容器内路径口径改变。
  - 已存在且被复用的 sandbox 容器可能缺少新 env/mount，需要给出迁移/重建策略。
  - 容器化部署兼容：支持 gateway 容器内 data_dir 为 `/data` + backing 为 bind mount 或 named volume 的常见部署形态（通过 `MOLTIS_SANDBOX_DATA_MOUNT_*` 显式指定）。
- 可观测性：
  - 在 sandbox 启动日志中打印一次性摘要（backend、mountpoint、ro/rw、是否启用）。
- 安全与隐私：
  - 默认避免把宿主机绝对路径写进 prompt；日志中也避免打印宿主机 data_dir 真实路径（如确需打印，应脱敏或仅在 debug）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) system prompt 中引用 `data_dir/PEOPLE.md`，agent 无法理解/定位 `data_dir` 是什么目录，导致读取 roster 的指引不可执行。
2) Docker sandbox 当前挂载策略为 `host_abs_path:host_abs_path`，prompt 若要给出真实路径，会泄露并绑定宿主机用户名/路径结构，且在换用户/换机器时不稳定。
3) 当 gateway 运行在容器内并通过 docker.sock 创建 sandbox 容器时，gateway 的 `MOLTIS_DATA_DIR` 往往是容器内路径（如 `/data`）。若 sandbox mount 仍基于该路径拼接 `-v /data:...`，Docker daemon 将在宿主机侧解析 `/data` 并导致挂载失败（或挂载到错误位置）。

### 影响（Impact）
- 用户体验：agent 依据 prompt 查找文件失败，产生误导性指引与额外排障成本。
- 可靠性：工具/agent 在 sandbox 内的“读取 data_dir 文件”能力缺少稳定契约，后续扩展（更多 data_dir 引用）会反复踩坑。
- 排障成本：问题表现为“文件不存在/路径不对”，根因是“约定缺失”，容易反复被误修。

### 复现步骤（Reproduction）
1. 运行任意会注入 persona/system prompt 的流程。
2. 观察 persona 中的 People 参考路径为 `data_dir/PEOPLE.md`。
3. 期望 vs 实际：
   - 期望：给出容器内可直接访问的路径（如 `/moltis/data/PEOPLE.md`）。
   - 实际：给出概念占位符，agent 无法解析。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/agents/src/prompt.rs:123`：persona 写入 `- data_dir/PEOPLE.md`。
  - `crates/config/src/loader.rs:249`：`data_dir()` 的解析优先级包含 `MOLTIS_DATA_DIR`。
  - `crates/tools/src/sandbox.rs:829`：Docker sandbox `workspace_args()` 将宿主机 `data_dir()` 以 `host:host` 方式挂载（非固定入口点）。
  - `crates/tools/src/sandbox.rs:1666`：Apple Container backend 当前不支持外部 mounts，且未实现 workspace mount。
- 配置/文档证据（容器化部署的常见形态）：
  - `docs/src/docker.md:182`：示例使用 `-e MOLTIS_DATA_DIR=/data -v ./data:/data`（gateway data_dir 为容器内路径）。
  - `docs/src/docker.md:142`：示例使用 named volume 将数据挂到容器内用户目录（同样属于“容器内路径 ≠ Docker host 路径/volume 名”的典型）。
- 当前测试覆盖：
  - 已有：`crates/agents/src/prompt.rs:1176` 断言包含 `data_dir/PEOPLE.md`（会随本变更更新）。
  - 缺口：缺少 “sandbox 固定挂载点 + env 注入” 的单测覆盖与回归测试。

## 根因分析（Root Cause）
- A. prompt 层将 data_dir 作为“人类概念”写入 system prompt，但未把它绑定到任何可达路径契约。
- B. sandbox 层将 “gateway data_dir（进程视角）” 与 “sandbox data mount source（Docker host 视角）” 混为一谈：容器化部署下 `data_dir()` 往往是容器内路径或 volume 挂载点，不能直接用于 `docker run -v <source>:...`。
- C. sandbox 层缺少稳定的容器内访问入口点（当前 1:1 映射到宿主机绝对路径），导致 prompt 无法引用一个跨用户/跨机器稳定的路径。
- D. 容器内 `HOME`/用户与宿主机不一致，使得 `~/.moltis` 也不是可靠路径。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - 当 sandbox workspace mount 启用时，容器内必须存在固定可达路径 `/moltis/data` 指向宿主机 data_dir 内容。
  - 容器创建时必须设置 `MOLTIS_DATA_DIR=/moltis/data`，使容器内运行的 Moltis 代码与脚本统一解析到该路径。
  - sandbox workspace mount 的 source 必须显式可配置为 `bind`（宿主机绝对路径）或 `volume`（Docker volume 名），不得隐式依赖 gateway 的 `MOLTIS_DATA_DIR` 恰好等于 Docker host 可见路径。
  - system prompt 中引用的 data_dir 文件路径必须使用 `/moltis/data/<file>` 形式，并且必须按能力 gate：只有在“配置完整且 backend 能力支持”的前提下才输出该路径；否则必须输出明确的不可用说明或省略该引用（避免误导）。
- 不得：
  - 不得在 system prompt 中使用未定义的占位符目录（如 `data_dir/...`）。
  - 不得在 system prompt 中写死宿主机用户名相关的绝对路径。
- 应当：
  - 对不支持挂载的 backend（如 Apple Container 若确实无法 volume mount），应当明确降级策略与用户提示（prefer docker / disable workspace mount / 明确限制）。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：固定容器内挂载点 + env 注入
- 核心思路：把 sandbox 容器内 data_dir 入口固定为 `/moltis/data`，并在容器环境中设置 `MOLTIS_DATA_DIR=/moltis/data`；挂载的 source 通过 `MOLTIS_SANDBOX_DATA_MOUNT_*` 显式指定（bind/volume），避免依赖 gateway 的进程视角路径。
- 优点：
  - 路径契约稳定、跨用户一致、prompt 可直接引用真实路径。
  - 容器内运行的代码无需特殊判断（`moltis_config::data_dir()` 自动正确）。
- 风险/缺点：
  - 需要处理“旧容器复用”导致 env/mount 不更新的问题（迁移/重建策略）。
  - Apple Container backend 是否支持 volume mount 需确认；不支持则需要 fallback 或替代方案。

#### 方案 2（不推荐）：prompt 输出宿主机解析后的绝对路径
- 核心思路：prompt 直接写 `data_dir()` 的 resolved absolute path（如 `/home/alice/.moltis/PEOPLE.md`），依赖 Docker 的 1:1 bind mount。
- 缺点：泄露宿主机路径；换用户/路径变化导致不稳定；不适用于 Apple Container（当前无 mount）。

#### 方案 3（备选）：启动时复制必要文件到容器内固定目录
- 核心思路：每次 exec 前把 `PEOPLE.md`/personas 复制进容器（不做 bind mount）。
- 缺点：同步复杂、易不一致；写回/状态类文件难处理；更像临时 workaround。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：容器内固定 data_dir 为 `/moltis/data`（source=configured）。
- 规则 2：容器创建时必须注入 `MOLTIS_DATA_DIR=/moltis/data`（source=as-sent 到 container runtime）。
- 规则 3：system prompt 中任何 data_dir 引用必须使用 `/moltis/data/...`，不得出现 `data_dir/...` 占位符。
- 规则 4：当 sandbox data mount 不可用（未配置/配置无效/backend 不支持）时，必须禁用 workspace mount 并对 prompt 做能力降级，避免输出误导性的可执行路径。
- 规则 5（默认策略，已决策）：当 `MOLTIS_SANDBOX_DATA_MOUNT_*` 未配置时，默认尝试把 gateway 的 `data_dir()` 作为 `bind` source，但仅当其为非空绝对路径且通过基本校验时才启用；否则禁用 workspace mount 并给出明确告警与配置指引。
- 规则 6（配置入口，已决策）：`MOLTIS_SANDBOX_DATA_MOUNT_TYPE`/`MOLTIS_SANDBOX_DATA_MOUNT_SOURCE` 同时支持：
  - 配置文件字段（推荐部署用，便于集中管理）
  - 环境变量覆盖（env override 优先于 config）
  - 字段命名建议（避免与 `workspace_mount` 混淆）：
    - `sandbox.data_mount_type` = `"bind"` | `"volume"`
    - `sandbox.data_mount_source` = `"/srv/moltis-data"` | `"moltis-data"`

#### prompt 能力 gate（Capability Gate）
> 目标：prompt 只输出“对 agent 可执行的真实路径”。此 gate 不要求在 prompt 生成时验证 mount 一定成功（容器化场景下 gateway 进程未必能在本地文件系统验证 host path），但必须至少保证“配置完整且能力支持”，否则降级。

- 认为 sandbox data mount “可用”的最低条件（静态判定）：
  - `workspace_mount != None`
  - backend 支持 workspace mount（Docker 支持；Apple Container 未支持则视为不可用并触发 fallback/禁用）
  - mount source 配置完整且通过语法校验：
    - type=`bind`：source 为非空绝对路径字符串
    - type=`volume`：source 为非空 volume 名字符串
- 任何条件不满足：
  - prompt 不得输出 `/moltis/data/<file>` 这类可执行路径
  - 必须输出明确降级提示（例如 “sandbox data-dir mount is not available in this deployment”）

#### 接口与数据结构（Contracts）
- Sandbox（Docker）：
  - `docker run` 必须包含：
    - `-v <mount_source>:/moltis/data:(ro|rw)`（`<mount_source>` 由 `MOLTIS_SANDBOX_DATA_MOUNT_TYPE`/`MOLTIS_SANDBOX_DATA_MOUNT_SOURCE` 决定：bind=宿主机绝对路径；volume=Docker volume 名）
    - `-e MOLTIS_DATA_DIR=/moltis/data`
- Sandbox（Apple Container）：
  - 若 CLI 支持 volume mount：对齐 Docker 合同（同挂载点、同 env）。
  - 若不支持：明确规则——当 `workspace_mount != None` 时强制选择 Docker backend，并在日志/配置文档中说明。
- Agents persona：
  - People roster 路径（mount 可用时）：`/moltis/data/PEOPLE.md`
  - mount 不可用时：不得输出会误导的可执行路径；输出明确说明（例如 “sandbox data-dir mount is not available in this deployment”）。

#### 失败模式与降级（Failure modes & Degrade）
- 旧容器复用（缺少新 env/mount）：
  - 方案：检测版本/签名不一致则删除并重建；或在发布说明中要求清理旧 sandbox 容器。
- 容器化部署误配（gateway data_dir 为 `/data`，但未指定 sandbox mount source）：
  - 方案：默认禁用 workspace mount（避免创建失败/错误挂载），记录清晰日志提示如何配置 `MOLTIS_SANDBOX_DATA_MOUNT_*`；prompt 必须降级（不输出 `/moltis/data/...` 路径）。
- mount source 校验失败（bind 路径不存在/非绝对路径，volume 不存在等）：
  - 方案：禁用 workspace mount + 清晰错误信息（不得静默失败）；prompt 同步降级。
- backend 不支持挂载：
  - 方案：prefer docker / failover 到 docker（可复用现有 failover 机制，或新增 capability gate）。

#### 安全与隐私（Security/Privacy）
- prompt 与默认日志中避免打印宿主机 data_dir 绝对路径。
- 仅在 debug 日志中（可选）打印 mount enabled 与 ro/rw，不打印 host 路径。

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] Docker sandbox 启用 workspace mount 时，容器内 `/moltis/data` 存在且可访问宿主机 data_dir 内容。
- [ ] 容器内运行的任意 Moltis 代码调用 `moltis_config::data_dir()` 时解析到 `/moltis/data`（通过单测或集成验证证明）。
- [ ] system prompt 不再出现 `data_dir/PEOPLE.md`；当 mount 可用时输出 `/moltis/data/PEOPLE.md`，当 mount 不可用时输出明确降级说明或省略该指引。
- [ ] Apple Container backend 的行为明确且可预测：要么同样支持 `/moltis/data`，要么在需要挂载时自动/明确切换到 Docker。
- [ ] 容器化部署兼容（至少覆盖两类常见形态）：
  - [ ] gateway 容器内 data_dir 为 `/data` 且 backing 为 bind mount：通过 `MOLTIS_SANDBOX_DATA_MOUNT_TYPE=bind` 生效。
  - [ ] gateway 容器内 data_dir 为 `/data` 且 backing 为 named volume：通过 `MOLTIS_SANDBOX_DATA_MOUNT_TYPE=volume` 生效。
- [ ] 回归测试覆盖：至少包含 Docker args 构造（bind/volume 两条路径）与 persona 输出能力 gate 断言。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] Docker sandbox workspace mount args（bind/volume）：`crates/tools/src/sandbox.rs:<新增测试行>`
- [ ] mount source 校验与降级（未配置/无效配置）：`crates/tools/src/sandbox.rs:<新增测试行>`
- [ ] Docker sandbox run env 注入（`MOLTIS_DATA_DIR=/moltis/data`）：`crates/tools/src/sandbox.rs:<新增测试行>`（建议把 run args 构造提取为 pure helper 便于测试）
- [ ] persona People 路径能力 gate：`crates/agents/src/prompt.rs:<新增测试行>`

### Integration
- [ ] （可选）在可用 Docker 的 CI/本地环境中，启动 sandbox 并 `ls /moltis/data/PEOPLE.md` 验证 mount（若现有测试框架允许）。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：CI 可能无法稳定提供 container runtime。
- 手工验证步骤：
  1. 启动 gateway 并触发一次 sandbox 工具执行。
  2. `docker exec <sandbox_container> ls -la /moltis/data`
  3. 验证 prompt 输出路径与容器内文件一致。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认开启（属于契约修复），但需要迁移说明。
- 回滚策略：
  - 回滚代码变更后，旧行为恢复为 `host:host` 挂载与 `data_dir/...` 占位符（不建议长期保留）。
  - 回滚风险：已更新 prompt 的路径会与旧挂载策略不一致。
- 上线观测：
  - 关注 “sandbox mount missing / file not found: /moltis/data/...” 相关日志。
  - 关注 “workspace mount disabled due to missing/invalid `MOLTIS_SANDBOX_DATA_MOUNT_*`” 相关日志。

## 实施拆分（Implementation Outline）
- Step 1: 定义常量 `SANDBOX_GUEST_DATA_DIR=/moltis/data`（tools/sandbox）。
- Step 2: 增加 sandbox mount source 配置与解析（双配置变量）：
  - `MOLTIS_SANDBOX_DATA_MOUNT_TYPE`（bind/volume）
  - `MOLTIS_SANDBOX_DATA_MOUNT_SOURCE`
- Step 3: Docker `workspace_args()` 改为挂载到固定 guest path（并保留 ro/rw 语义）；source 由 mount type/source 决定，并做严格校验与降级策略。
- Step 4: Docker `run` args 增加 `-e MOLTIS_DATA_DIR=/moltis/data`（创建时一次性注入）。
- Step 5: 旧 sandbox 容器迁移策略落地（推荐自动检测不匹配则删除重建，避免升级后行为不一致）。
- Step 6: Apple Container backend：
  - 评估 `container run` 是否支持 volume mount；
  - 若支持：实现与 Docker 同合同；
  - 若不支持：在 router 选择上对 “需要 workspace mount” 的场景强制用 Docker，并写清楚文档/错误信息。
- Step 7: 更新 `crates/agents/src/prompt.rs`：
  - `data_dir/PEOPLE.md` →（mount 可用时）`/moltis/data/PEOPLE.md`
  - 增加能力 gate（mount 不可用时的降级文本）
  - 更新/新增单测断言
- Step 8: 更新相关文档：解释 “gateway data_dir（进程视角）” 与 “sandbox data mount source（Docker host 视角）” 的区别，给出三种部署示例（裸机 / bind mount / named volume）。
- Step 9: 补齐 unit tests +（可选）integration smoke。
- 受影响文件：
  - `crates/tools/src/sandbox.rs`
  - `crates/agents/src/prompt.rs`
  - `docs/src/<相关文档>`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-persona-profiles-ui-default-persona-and-session-labels.md`
  - `docs/src/docker.md`（主容器使用 `/data`/`/config` 的约定）
  - `docs/src/cloud-deploy.md`（示例中 data_dir 常用 `/data`）
- Related commits/PRs：<TBD>

## 未决问题（Open Questions）
- Q1: <已决策> Apple Container backend 暂不实现 workspace mount；当需要 workspace mount（`workspace_mount != None`）时强制选择 Docker backend，并输出明确日志/文档说明。
- Q2: 是否也需要为 `MOLTIS_CONFIG_DIR` 引入类似固定挂载点（如 `/moltis/config`）？还是保持只修 data_dir（最小化）？
- Q3: <已决策> 旧 sandbox 容器复用采用自动迁移：启动时检测 env/mount 版本不匹配则删除并重建，避免升级后出现随机不一致行为。
- Q4: <已决策> `MOLTIS_SANDBOX_DATA_MOUNT_*` 未配置时，默认采用 “尽力使用 `data_dir()` 作为 bind source（仅当为绝对路径且通过校验）否则禁用并告警”。
- Q5: <已决策> `MOLTIS_SANDBOX_DATA_MOUNT_*` 同时支持配置文件字段与环境变量覆盖；env 优先于 config。

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

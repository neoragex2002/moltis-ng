# Issue: Sandbox agent 通过挂载 host SSH 配置免密回连 host（sandbox ssh / host ssh）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P2
- Updated: 2026-03-18
- Owners:
- Components: tools / sandbox / agents / ops
- Affected providers/models: (any)

**已实现（如有，写日期）**
- 2026-03-18：将 host `~/.ssh` 以只读方式挂载到 sandbox 普通外部挂载路径，而不是错误挂到 `~/.ssh` 目标位：`/home/luy/.config/moltis/moltis.toml:256`
- 2026-03-18：为 sandbox 增加短命令 wrapper `ssh-host`，避免 agent 直接触达敏感路径 `~/.ssh` 或记忆长参数：`ssh-host:1`

**已覆盖测试（如有）**
- 自动化测试：无（本单为本机运行环境 / host SSH / sandbox 联动配置）
- 手工验收：sandbox agent 已可免密 SSH 回连 host（用户现场确认）

**已知差异/后续优化（非阻塞）**
- 当前方案直接复用 host 当前用户的 `~/.ssh` 身份边界，简单可用，但隔离性较弱。
- 当前 `ssh-host` 固定回连 alias `z87x`；后续如需多 host/多别名，可再抽象，但本单不扩 scope。

---

## 背景（Background）
- 场景：agent 运行在 Docker sandbox 内，用户希望 agent 能回连 host 执行命令。
- 约束：必须使用免密公钥 SSH；不接受密码登录；方案必须尽量简单，不引入额外账号/复杂降权设计。
- Out of scope：不在本单引入专用低权用户、受限 key、host wrapper 服务、多 host 调度。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **host SSH 映射**（主称呼）：将 host 当前用户的 `~/.ssh` 目录以只读方式挂载进 sandbox 的普通外部挂载路径。
  - Why：复用 host 已有免密 SSH 能力，最短路径打通 sandbox → host。
  - Not：不是把 key 写进镜像；也不是给 sandbox 单独创建一套 SSH 身份。
  - Source/Method：[configured]
  - Aliases（仅记录，不在正文使用）：挂载 `.ssh` / 复用 host SSH 配置

- **SSH wrapper**（主称呼）：sandbox 内调用的短命令脚本，内部封装 `ssh -F/-i/UserKnownHostsFile` 等参数。
  - Why：避免 agent 直接访问 `~/.ssh` 路径或输出冗长命令。
  - Not：不是新的 SSH 协议层；只是命令封装。
  - Source/Method：[as-sent]
  - Aliases（仅记录，不在正文使用）：`ssh-host`

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] sandbox agent 必须能够通过免密公钥 SSH 回连 host。
- [x] agent 侧调用入口必须足够短，不要求 agent 直接处理 `~/.ssh` 路径或长命令。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须复用 host 现有免密 SSH alias `z87x`。
  - 必须使用合法 sandbox 外部挂载路径。
  - 不得修改用户其他 Moltis 配置。
- 兼容性：保留原有 `/home/luy/dev -> /mnt/host/dev` 挂载不变，仅追加 SSH 相关映射。
- 可观测性：本单以配置与现场手工验收为主，不新增运行时日志。
- 安全与隐私：不在 issue 中记录私钥内容；只记录目录与调用方式。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 用户在 host 上 `ssh z87x` 已免密可用，但 sandbox agent 仍无法直接复用。
2) 先前方案错误地把 guest 挂载目标写成 `/home/sandbox/.ssh`，不符合 sandbox 外部挂载约束。
3) 即使挂载存在，agent 也会对 `~/.ssh` 这类敏感路径产生安全顾虑，不适合直接要求其访问。

### 影响（Impact）
- 用户体验：agent 无法短路径回连 host；需要人工解释长 SSH 命令。
- 可靠性：错误 guest 挂载目标会导致配置不符合实现约束，行为不稳定。
- 排障成本：同时混杂“挂载路径错误”和“模型不愿直接碰 `.ssh`”两类问题，容易误判。

### 复现步骤（Reproduction）
1. host 已具备 `ssh z87x` 免密登录能力。
2. 在 Moltis 中将 host `~/.ssh` 映射到 `/home/sandbox/.ssh`。
3. 让 sandbox agent 直接执行 `ssh z87x`。
4. 期望 vs 实际：期望 agent 直接回连；实际出现 guest 挂载路径不符合约束，且 agent 对 `~/.ssh` 路径存在安全顾虑。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/tools/src/sandbox.rs:1247`：外部挂载 `guest_dir` 必须位于 `/mnt/host/...` 之下。
  - `crates/tools/src/sandbox_packages.rs:46`：sandbox 已包含 `openssh-client`。
- 配置/协议证据（必要时）：
  - `/home/luy/.ssh/config:1`：alias `z87x` 指向 `10.0.0.3`，可作为 sandbox 回连目标。
  - `/home/luy/.config/moltis/moltis.toml:256`：allowlist 已包含 `/home/luy/.ssh`。
  - `/home/luy/.config/moltis/moltis.toml:257`：SSH 目录最终映射到 `/mnt/host/ssh`。
- 当前测试覆盖：
  - 已有：现场手工验证已通过（用户确认 sandbox agent 已能免密 SSH 访问 host）。
  - 缺口：无自动化环境可稳定覆盖本机 host SSH + Docker sandbox 联动。

## 根因分析（Root Cause）
- A. 触发条件已具备：host 自身 `ssh z87x` 已免密通，但 Moltis 侧尚未正确复用这套配置。
- B. 初版方案将 guest 挂载目标设为 `/home/sandbox/.ssh`，违反了当前 sandbox 外部挂载实现约束。
- C. 直接让 agent 触碰 `~/.ssh` 路径会触发模型侧敏感目录顾虑，导致即使网络/密钥正常也不利于稳定使用。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - sandbox 必须通过只读外部挂载复用 host 当前用户的 `~/.ssh`。
  - guest 挂载目标必须位于 `/mnt/host/...`。
  - agent 必须可以通过短 wrapper 命令完成回连，不依赖直接访问 `~/.ssh`。
- 不得：
  - 不得要求 agent 使用密码 SSH。
  - 不得为了本单改动无关 Moltis 配置。
- 应当：
  - 应当复用 host 已可用的 alias `z87x`，避免重复维护 HostName/User/IdentityFile。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：将 host `~/.ssh` 只读挂到 `/mnt/host/ssh`，再提供短 wrapper `ssh-host` 供 sandbox agent 使用。
- 优点：最短路径；无需改 host 现有 SSH alias；agent 入口短。
- 风险/缺点：复用 host 当前用户 SSH 身份，边界较宽。

#### 方案 2（备选）
- 核心思路：直接让 agent 使用长命令 `ssh -F ... -i ... -o UserKnownHostsFile=... z87x`。
- 风险/缺点：命令过长，不适合作为稳定 agent 入口；仍暴露 `.ssh` 细节。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（明确 source/method）：host SSH 身份来源于 host 当前用户现有 `~/.ssh`（configured）。
- 规则 2：sandbox 只读挂载该目录到 `/mnt/host/ssh`，不映射到 sandbox `~/.ssh`。
- 规则 3：agent 只调用短 wrapper `ssh-host`，wrapper 内部再显式指定 SSH config / key / known_hosts。

#### 接口与数据结构（Contracts）
- API/RPC：无变更。
- 存储/字段兼容：仅修改本机运行配置 `/home/luy/.config/moltis/moltis.toml`，不涉及持久化 schema。
- UI/Debug 展示（如适用）：无新增 UI 字段。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 若 host alias 失效：`ssh-host` 直接返回 SSH 错误。
  - 若 `known_hosts` / key 缺失：`ssh-host` 返回标准 SSH 失败信息。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 不涉及额外队列与状态机。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：issue 不记录私钥内容，仅记录路径与流程。
- 禁止打印字段清单：私钥正文、`authorized_keys` 全文。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] Moltis sandbox 配置仅做最小修改，新增 `~/.ssh -> /mnt/host/ssh` 只读挂载。
- [x] sandbox agent 存在一个短命令入口，不需要记忆长 SSH 参数。
- [x] sandbox agent 已能免密 SSH 回连 host。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] 无（本单为环境配置与 wrapper 落地，不涉及稳定 unit 边界）

### Integration
- [x] 无（缺少可在 CI 中稳定复刻的 host SSH / Docker sandbox 环境）

### UI E2E（Playwright，如适用）
- [x] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：依赖本机 host `sshd`、现有 SSH alias `z87x`、本机用户 `~/.ssh`、Docker sandbox 运行态，CI 不具备该上下文。
- 手工验证步骤：
  1. 确认 host 上 `ssh z87x` 已免密可用。
  2. 重启 Moltis / gateway，使新的 sandbox mount 生效。
  3. 在 sandbox 中执行 `/mnt/host/dev/moltis/ssh-host 'hostname && whoami'`。
  4. 返回 host 机器信息，即视为通过。

## 发布与回滚（Rollout & Rollback）
- 发布策略：本机配置生效；重启 Moltis / gateway 后切换到新 sandbox。
- 回滚策略：删除 `/home/luy/.config/moltis/moltis.toml:256` 中的 `/home/luy/.ssh` allowlist 项、删除 `/home/luy/.config/moltis/moltis.toml:257` 中的 SSH mount，并移除 `ssh-host` 文件。
- 上线观测：以手工执行 `ssh-host` 成功为准。

## 实施拆分（Implementation Outline）
- Step 1: 核查 host 现有 SSH alias `z87x` 与 `sshd` 监听状态。
- Step 2: 以最小差异更新 Moltis sandbox 配置，追加 `/home/luy/.ssh -> /mnt/host/ssh` 只读挂载。
- Step 3: 新增短 wrapper `ssh-host`，将长 SSH 参数封装到脚本内。
- 受影响文件：
  - `/home/luy/.config/moltis/moltis.toml`
  - `ssh-host`
  - `/home/luy/.ssh/config`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/template/TEMPLATE-issue-single.md`
  - `docs/agent-file-and-git-safety-rules.md`
- Related commits/PRs：
  - N/A（本单包含本机配置与工作区脚本落地）
- External refs（可选）：
  - OpenSSH client behavior（project default）

## 未决问题（Open Questions）
- Q1: 是否后续需要把 `ssh-host` 变成更通用的多 alias wrapper？
- Q2: 是否后续需要收缩 SSH 身份边界，避免直接复用 host 当前用户的整套 `~/.ssh`？

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

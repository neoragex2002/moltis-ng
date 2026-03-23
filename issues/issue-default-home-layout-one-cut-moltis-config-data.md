# Issue: 默认 home 目录布局 one-cut 切换到 `~/.moltis/{config,data}`（paths / one-cut）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-23
- Owners: 待定
- Components: config/cli/gateway/oauth
- Affected providers/models: all

**已实现（如有，写日期）**
- 2026-03-23：默认路径主语义已切到 `~/.moltis/{config,data}`，并保持项目根 `./moltis.*` 配置发现优先：`crates/config/src/loader.rs:129`
- 2026-03-23：project-local hooks/skills 已从错误的 `data_dir().join(".moltis/...")` 收回到 `<cwd>/.moltis/...`，避免双层旧路径：`crates/skills/src/discover.rs:30`、`crates/plugins/src/hook_discovery.rs:38`、`crates/gateway/src/services.rs:1513`
- 2026-03-23：默认 config 主落点已统一到 `~/.moltis/config`（CLI help / TLS / provider key store / OAuth）：`crates/cli/src/main.rs:41`、`crates/gateway/src/tls.rs:57`、`crates/gateway/src/provider_setup.rs:105`、`crates/oauth/src/config_dir.rs:3`
- 2026-03-23：清理 gateway/oauth/cli 调用侧 `".moltis/config"` dead fallback，改为 fail-fast：`crates/gateway/src/tls.rs:60`、`crates/gateway/src/server.rs:1144`、`crates/gateway/src/server.rs:1281`、`crates/gateway/src/provider_setup.rs:120`、`crates/gateway/src/provider_setup.rs:666`、`crates/oauth/src/config_dir.rs:11`
- 2026-03-23：修复 `moltis hooks list` 空提示双层 `.moltis`，并补最小单测冻结文案：`crates/cli/src/hooks_commands.rs:10`、`crates/cli/src/hooks_commands.rs:157`
- 2026-03-23：README / Docker / compose / docs 已同步到新默认布局：`README.md:132`、`docs/src/docker.md:9`、`Dockerfile:56`、`examples/docker-compose.yml:24`

**已覆盖测试（如有）**
- 2026-03-23：`cargo test -p moltis-skills -p moltis-plugins -p moltis-oauth -p moltis-voice` 通过；覆盖 project/user 路径拆分与相关默认路径文案/配置逻辑：`crates/skills/src/discover.rs:296`、`crates/plugins/src/hook_discovery.rs:164`
- 2026-03-23：`cargo test -p moltis-config` 通过；覆盖默认 home 布局、project-root 优先与 override 隔离：`crates/config/src/loader.rs:1743`、`crates/config/src/loader.rs:1775`
- 2026-03-23：`cargo test -p moltis hooks_commands::tests::no_hooks_hint_does_not_double_moltis -- --exact` 通过：`crates/cli/src/hooks_commands.rs:157`
- 2026-03-23：`cargo test -p moltis tests::cli_parses_config_and_data_dir_flags -- --exact` 通过：`crates/cli/src/main.rs:473`
- 2026-03-23：`cargo test -p moltis-gateway --no-run`、`cargo test -p moltis-oauth --no-run`、`cargo test -p moltis --no-run` 通过（编译验证）。
- 2026-03-23：`rg -n 'PathBuf::from\\(\"\\.moltis/config\"\\)|\"\\.moltis/config\"' crates/cli crates/gateway crates/oauth` 输出为空（无 `".moltis/config"` fallback）。
- 2026-03-23：`rg -n 'join\\(\"\\.moltis/hooks\"\\)|join\\(\"\\.moltis/skills\"\\)|PathBuf::from\\(\"\\.moltis/hooks\"\\)|PathBuf::from\\(\"\\.moltis/skills\"\\)' crates/cli crates/gateway crates/plugins crates/skills` 输出为空（无旧 project-local path-building literal）。
- 2026-03-23：手工验收（temp HOME + 保留 rustup/cargo home）：`timeout 2s cargo run -p moltis -- gateway ...` 能在 `~/.moltis/{config,data}` 落盘并生成 `config/moltis.toml`。
- 2026-03-23：手工验收（override）：`MOLTIS_CONFIG_DIR=/tmp/... MOLTIS_DATA_DIR=/tmp/... timeout 2s cargo run -p moltis -- gateway ...` 能在 override 目录落盘并生成 `moltis.toml`。

**已知差异/剩余收尾**
- 暂无。

---

## 背景（Background）
- 场景：变更前默认配置目录是 `~/.config/moltis`，默认数据目录是 `~/.moltis`；用户明确要求将默认布局硬切换为单一根目录 `~/.moltis/`，内部再划分 `config/` 与 `data/`。
- 约束：
  - 本次为 strict one-cut / 硬切换。
  - **不考虑后向兼容**。
  - **不做自动数据迁徙**。
  - 不得为旧默认布局额外增加专门的 fallback、alias、探测或拒绝机制。
- 变更范围必须收敛在“默认路径语义 + 直接依赖这些默认路径的 runtime 消费代码/测试”，不得顺手扩展为更大规模的路径系统重构。
- Out of scope：
  - 不重命名 `MOLTIS_CONFIG_DIR` / `MOLTIS_DATA_DIR`。
  - 不改变 `--config-dir` / `--data-dir` CLI 语义。
  - 不提供自动 copy / move / migrate 工具。
  - 不处理用户显式传入自定义目录时的布局统一问题。
  - 不在本单重构 `config_dir()` / `user_global_config_dir()` 的 public API 形状。
  - 不把 `user_global_config_dir()` / `user_global_config_dir_if_different()` / `find_user_global_config_file()` 扩散成第四套 runtime 基目录；它们只保留 loader/discovery 读路径职责。
  - 不新增新的路径 helper / 路径中心；本单只认现有 `config_dir()` / `data_dir()` / `project_local_dir()` 这 3 个基目录入口。
  - 不处理 `crates/tools/src/sandbox.rs` 的 sandbox project view 路径；该处 `.moltis/skills` 属于 sandbox 视图语义，不计入本单 `runtime_consumer_code`。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`home_root`**（主称呼）：默认 home 级 Moltis 根目录。
  - Why：本 issue 目标是把默认目录心智模型收敛到单一根。
  - Not：不是用户通过 env/CLI 显式传入的 override 目录。
  - Source/Method：effective
  - Aliases（仅记录，不在正文使用）：workspace root / home workspace

- **`config_dir`**（主称呼）：Moltis 的配置目录。
  - What：默认应为 `~/.moltis/config`。
  - Why：承载 `moltis.toml`、TLS 证书、OAuth/provider 配置等低频配置文件。
  - Not：不是 sessions、memory、logs 的存储目录。
  - Source/Method：effective

- **`data_dir`**（主称呼）：Moltis 的数据目录。
  - What：默认应为 `~/.moltis/data`。
  - Why：承载 persona/people 文档、数据库、memory、logs、hooks、skills、models 等运行期状态。
  - Not：不是默认配置发现目录。
  - Source/Method：effective

- **`runtime_consumer_code`**（主称呼）：运行期真正消费默认路径语义的业务代码。
  - What：例如 gateway / oauth / cli / plugins / tls 里的路径消费点。
  - Why：本单要求清理的默认路径 literal 只针对这层，避免误把 docs、测试 fixture、sandbox 视图路径也算进 runtime 收口。
  - Not：不是文档示例、测试数据、注释性字符串、sandbox project view 路径，也不是 `user_global_config_dir*()` 这类 loader/discovery 辅助函数本身。
  - Source/Method：effective

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 默认路径语义只允许收口在 3 个基目录入口：`config_dir()`、`data_dir()`、`project_local_dir()`。
- [x] 配置发现仍保持“项目根 `./moltis.*` 优先，其次用户级默认配置目录”。
- [x] `runtime_consumer_code` 的默认路径只能从这 3 个基目录函数的返回结果派生，再做相对路径 `join(...)`。
- [x] `runtime_consumer_code` 移除 `".moltis/config"` 这类默认路径 fallback；hooks/skills 仅允许从 `project_local_dir()` / `data_dir()` 派生（UI/文档里的相对路径提示不算路径解析逻辑）。
- [x] 显式 `MOLTIS_CONFIG_DIR` / `MOLTIS_DATA_DIR` 与 `--config-dir` / `--data-dir` override 语义保持不变。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：默认路径语义单一且稳定，消费层不得各写半套。
  - 必须：不得出现双层 `.moltis` 路径拼接。
  - 不得：不得为旧默认目录保留 fallback、alias、兼容读取或 silent ignore。
  - 不得：不得为这次收口再新增一批 leaf path helper，把路径接口面继续做大。
- 兼容性：明确 breaking change；旧默认目录退出默认语义，但显式 override 仍按用户指定目录生效。
- 可观测性：
  - 不为旧默认布局新增专门的 legacy 拒绝日志。
  - 仅保留路径切换后各模块原本的正常失败/初始化日志。
- 安全与隐私：日志不得打印敏感配置正文/token。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 默认 home 布局主语义已经切到 `~/.moltis/{config,data}`，但 `moltis hooks list` 仍把 project-local 提示打印成 `<cwd>/.moltis/.moltis/hooks/...`。
2) gateway / oauth 若干 config consumers 仍散落 `".moltis/config"` fallback literal，代码语义没有完全收口到单一默认路径。
3) 主路径切换虽然已落地，但目前没有把“消费层只能基于 3 个基目录 join 相对路径”写成硬规则，后续很容易再散。

### 影响（Impact）
- 用户体验：用户按 CLI 提示创建 project-local hooks 时会放到错误目录，随后继续看到 “No hooks found.”。
- 可靠性：死 fallback 分支虽然当前多半走不到，但保留它们会让后续修改继续误以为还存在另一套默认配置根。
- 排障成本：主语义已切换、尾巴却未完全删除，后续 review/维护者很难一眼判断哪些路径语义已经冻结、哪些仍有旧逻辑残留。

### 复现步骤（Reproduction）
1. 在没有 hooks 的项目目录运行 `moltis hooks list`。
2. 观察空结果提示。
3. 期望 vs 实际：
   - 期望：提示 `<cwd>/.moltis/hooks/<name>/HOOK.md`
   - 实际：提示 `<cwd>/.moltis/.moltis/hooks/<name>/HOOK.md`
4. 再检查 `crates/gateway/src/tls.rs`、`crates/gateway/src/server.rs`、`crates/gateway/src/provider_setup.rs`、`crates/oauth/src/config_dir.rs`。
5. 观察：
   - 期望：调用侧不再自带 `".moltis/config"` fallback literal
   - 实际：仍有散落死分支

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/config/src/loader.rs:172`：`find_config_file()` 当前按“项目根 `./moltis.*` 优先，再查 `~/.moltis/config`”工作。
  - `crates/config/src/loader.rs:207`：`config_dir()` 当前默认收口到 `~/.moltis/config`。
  - `crates/config/src/loader.rs:252`：`data_dir()` 当前默认收口到 `~/.moltis/data`。
  - `crates/config/src/loader.rs:265`：`project_local_dir()` 当前口径是 `<cwd>/.moltis`。
  - `crates/cli/src/hooks_commands.rs:10`：CLI hooks 空提示已统一走单点 helper，project-local 文案不再重复拼接 `/.moltis`。
  - `crates/gateway/src/tls.rs:60`：TLS cert 路径只从 `config_dir()` 派生，不再带旧 fallback。
  - `crates/gateway/src/server.rs:1144`：gateway 启动时的 config dir 创建已改为 fail-fast，不再落回 `".moltis/config"`。
  - `crates/gateway/src/provider_setup.rs:664`：provider config dir 只从 `config_dir()` 派生，不再保留旧 fallback。
  - `crates/oauth/src/config_dir.rs:9`：OAuth config dir 只从 `config_dir()` 派生。
- 配置/协议证据（必要时）：
  - `crates/cli/src/main.rs:41`：CLI help 已切到 `~/.moltis/config/`。
  - `crates/cli/src/main.rs:473`：CLI test 已冻结 `--config-dir` / `--data-dir` 解析语义。
- 当前测试覆盖：
  - 已有：`crates/config/src/loader.rs` 的默认根/配置发现测试、`crates/cli/src/hooks_commands.rs` 的空提示测试、`crates/cli/src/main.rs` 的 CLI flag 解析测试、以及 `rg` 针对 fallback/path-building literal 的 targeted 断言。
  - 缺口：已由 targeted tests、编译验证与手工启动验收关闭。

## 根因分析（Root Cause）
- A. 默认路径主语义已经切换成功，但最后一层用户可见提示和调用侧 literal 清理没有跟着完全收尾。
- B. `project_local_dir()` 已经是 `<cwd>/.moltis`，但部分消费者仍把它当成“工作区根目录”，于是重复手拼 `/.moltis/...`。
- C. `config_dir()` 仍是 `Option<PathBuf>` 形状，调用侧因此保留了“万一没有 config dir 就退回 `".moltis/config"`”的旧习惯，即便当前默认实现不会走到该分支。
- D. 当前缺的是“只认 3 个基目录、消费层只做 join 相对路径”的硬性实施规则，导致尾巴容易散回去。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 默认 `config_dir` 必须是 `~/.moltis/config`。
  - 默认 `data_dir` 必须是 `~/.moltis/data`。
  - `project_local_dir()` 必须是 `<cwd>/.moltis`。
  - 默认配置发现必须仍然保持：先 `./moltis.{toml,yaml,yml,json}`，再 `~/.moltis/config/moltis.{...}`。
  - 显式 `MOLTIS_CONFIG_DIR` / `MOLTIS_DATA_DIR` 与 `--config-dir` / `--data-dir` 必须继续覆盖默认值。
  - 消费层默认路径必须只从 `config_dir()` / `data_dir()` / `project_local_dir()` 的返回结果派生，再做相对路径 `join(...)`。
- 不得：
  - 不得从 `~/.config/moltis` 或旧 `~/.moltis` 继续做默认读取。
  - 不得自动复制/迁移旧目录内容到新布局。
  - 不得为旧默认布局增加专门探测并拦截的额外机制。
  - 不得在 `runtime_consumer_code` 里保留任何 `".moltis/config"` / `".moltis/hooks"` / `".moltis/skills"` 默认路径 literal 或 fallback。
- 应当：
  - 应当保留现有 env/CLI override 入口，作为显式 operator override，而不是兼容默认逻辑的一部分。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：把默认路径语义严格冻结在 `config_dir()` / `data_dir()` / `project_local_dir()` 这 3 个基目录；仓库其它地方只能基于这 3 个入口做 `join(...)`，并删除散落的默认路径 literal 与 fallback。
- 优点：
  - 范围最小，只收根语义，不扩接口面。
  - 符合 strict one-cut：旧默认路径语义只能死在中心入口之外。
  - review 成本低：只要扫消费层还有没有默认路径 literal 即可。
- 风险/缺点：
  - 需要逐个清掉消费层尾巴，但都是小改动。

#### 方案 2（备选）
- 方案：继续在 gateway / oauth / cli 各处保留自己的默认路径 literal，只把主入口默认值改掉。
- 不选原因：
  - 这不叫收口，只是换了默认值，尾巴仍然散一地。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1（默认配置根）：`config_dir()` 默认值收口到 `~/.moltis/config`。
- 规则 2（默认数据根）：`data_dir()` 默认值收口到 `~/.moltis/data`。
- 规则 3（项目基目录）：`project_local_dir()` 是唯一合法的 project-local 基目录入口，口径固定为 `<cwd>/.moltis`。
- 规则 4（配置发现）：`find_config_file()` 保持项目根优先，用户级默认配置目录改为 `~/.moltis/config`。
- 规则 5（消费层约束）：`runtime_consumer_code` 中的默认路径必须从 `config_dir()` / `data_dir()` / `project_local_dir()` 的返回结果派生，再做相对路径 `join(...)`；`user_global_config_dir*()` 仅限 loader/discovery 场景，不作为第四套 runtime 基目录。
- 规则 6（禁止 literal）：`runtime_consumer_code` 中不得再出现 `".moltis/config"`、`".moltis/hooks"`、`".moltis/skills"` 这类默认路径 literal / fallback。
- 规则 7（override 不变）：用户显式传入的 `config_dir` / `data_dir` 仍按 override 处理，不因本次 one-cut 被额外限制。
- 规则 8（one-cut 语义）：旧默认布局不再具有默认语义，但不额外为其引入专门探测/专门拒绝逻辑。

#### 接口与数据结构（Contracts）
- API/RPC：
  - 无新增 API。
- 存储/字段兼容：
  - 无自动迁移。
  - 旧默认目录仅退出默认语义；显式 override 不受本次变更影响。
- UI/Debug 展示（如适用）：
  - 所有展示默认路径的文案必须改为 `~/.moltis/config` 与 `~/.moltis/data`。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 路径切换后如因找不到配置、找不到数据或首次初始化产生失败，沿用现有正常错误路径，不新增 legacy 专用拒绝分支。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无自动清理；旧目录内容保持原样，不触碰。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 日志不得打印敏感配置正文/token；仅记录必要的路径与错误信息。
- 禁止打印字段清单：
  - token、provider secret、OAuth token、用户正文。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `config_dir()` 默认返回 `~/.moltis/config`。
- [x] `data_dir()` 默认返回 `~/.moltis/data`。
- [x] `project_local_dir()` 默认返回 `<cwd>/.moltis`。
- [x] `find_config_file()` 的用户级默认搜索路径改为 `~/.moltis/config`，且项目根 `./moltis.*` 仍优先。
- [x] 显式 `MOLTIS_CONFIG_DIR` / `MOLTIS_DATA_DIR` 与 `--config-dir` / `--data-dir` override 行为保持不变。
- [x] `moltis hooks list` 等用户可见 project-local 路径提示，与真实 `<cwd>/.moltis/...` 目录完全一致，不再出现双层 `.moltis`。
- [x] gateway / oauth / cli 等 `runtime_consumer_code` 不再保留 `".moltis/config"` fallback；hooks/skills 的 project-local 路径构造也不再散落 `".moltis/hooks"` / `".moltis/skills"` path-building literal。
- [x] README、CLI help、`docs/` 内所有引用旧默认路径的位置全部同步到新默认布局；不得只更新局部页面。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/config/src/loader.rs`：默认 `config_dir()` / `data_dir()` 路径断言更新。
- [x] `crates/config/src/loader.rs`：`project_local_dir()` 路径断言更新。
- [x] `crates/config/src/loader.rs`：`find_config_file()` 用户级默认目录断言更新。
- [x] `crates/config/src/loader.rs:1743`：项目根 `./moltis.*` 仍优先于用户级默认配置目录。
- [x] `crates/config/src/loader.rs:1775`：显式 override（`set_config_dir`）会隔离 config discovery，不再落回 project/user-global。
- [x] `crates/cli/src/main.rs:473`：`--config-dir` / `--data-dir` CLI flags 解析保持不变。
- [x] `crates/cli/src/hooks_commands.rs:157`：空 hooks 提示使用 `<cwd>/.moltis/hooks/<name>/HOOK.md`，不再额外拼接 `/.moltis`。
- [x] `rg`/targeted assertions：`crates/cli`、`crates/gateway`、`crates/oauth` 无 `".moltis/config"` fallback；`crates/cli`、`crates/gateway`、`crates/plugins`、`crates/skills` 无 `join(\".moltis/hooks\")` / `join(\".moltis/skills\")` 之类旧 path-building literal。

### Integration
- [x] 全新空目录启动：默认创建 `~/.moltis/config` 与 `~/.moltis/data`，并把配置/数据分别写入正确位置。
- [x] 显式 override 启动：当用户传入自定义 `config_dir` / `data_dir` 时，系统仍按指定目录运行。

### UI E2E（Playwright，如适用）
- [x] N/A：本单不改 UI 的默认路径展示；当前变更点已由 CLI help / docs / unit tests 覆盖。

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：home 目录真实用户环境与 CI 临时目录行为可能不同。
- 手工验证步骤：
  1. 清空测试 home，运行 `cargo run`。
  2. 确认生成 `~/.moltis/config/moltis.toml` 与 `~/.moltis/data/...`。
  3. 确认 CLI hooks 空提示与实际 project-local 目录一致。
  4. 确认 docs、UI 提示与 CLI help 全部指向新默认布局。

## 发布与回滚（Rollout & Rollback）
- 发布策略：breaking change，默认直接切换，不加 feature flag。
- 回滚策略：回滚到旧版本；或由 operator 显式设置 `MOLTIS_CONFIG_DIR` / `MOLTIS_DATA_DIR` 指向自定义目录。
- 上线观测：
  - 关注启动失败率与 onboarding 首次启动路径创建日志

## 实施拆分（Implementation Outline）
- Step 1：冻结 3 个基目录入口：`config_dir()`、`data_dir()`、`project_local_dir()`；保持它们的当前 one-cut 语义不变。
- Step 2：清理消费层默认路径 literal / fallback；只允许从这 3 个入口的返回结果派生路径再 `join(...)`，不新增 leaf helper。
- Step 3：修正 `moltis hooks list` 空结果提示，确保 project-local 文案直接基于 `project_local_dir()` 展示。
- Step 4：在 `crates/config/src/loader.rs` 补 project-root 优先与 override 语义测试；在 `crates/cli/src/hooks_commands.rs` 补 CLI hooks 空提示断言；再加 `crates/cli` / `crates/gateway` / `crates/oauth` 的 targeted literal 断言；仅在确有 CLI 参数层缺口时再触 `crates/cli/src/main.rs`。
- Step 5：执行一次全新 home 启动手工验收，确认 `~/.moltis/config` 与 `~/.moltis/data` 初始化落点正确。
- 受影响文件：
  - `crates/config/src/loader.rs`
  - `crates/cli/src/hooks_commands.rs`
  - `crates/gateway/src/tls.rs`
  - `crates/gateway/src/server.rs`
  - `crates/gateway/src/provider_setup.rs`
  - `crates/oauth/src/config_dir.rs`
  - 注：docs/README 的主路径文案已同步完成；除非本轮实施发现新的错位证据，否则本单剩余收尾不再扩展到额外文档重排。

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/configuration.md`
  - `docs/src/config-reset-and-recovery.md`
- Related commits/PRs：
  - 无
- External refs（可选）：
  - 无

## 未决问题（Open Questions）
- 无。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（N/A）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] breaking change 口径已写清（无自动迁移、无 legacy 特判）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

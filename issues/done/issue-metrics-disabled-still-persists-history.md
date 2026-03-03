# Issue: `metrics.enabled=false` 仍写入 `metrics_history` 导致慢 SQL 噪声（metrics / gateway）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P2
- Owners: <TBD>
- Components: gateway/metrics, metrics/store, config
- Affected providers/models: <N/A>

**已实现（如有，写日期）**
- metrics 采集开关：`[metrics].enabled`：`crates/config/src/schema.rs:874`
- gateway 启动时初始化 metrics recorder（enabled=false 走 disabled handle）：`crates/gateway/src/server.rs:2190`
- `metrics.enabled=false` 时不初始化 metrics store（避免写入 `metrics.db`）：`crates/gateway/src/server.rs:2216`
- `metrics.enabled=false` 时不启动 metrics history 周期任务（避免 `metrics_history` INSERT）：`crates/gateway/src/server.rs:2964`
- `metrics.enabled=true` 时保留既有行为：每 10s 生成 snapshot 并持久化到 store：`crates/gateway/src/server.rs:2970`
- SQLite store 持久化点：`INSERT INTO metrics_history ...`：`crates/metrics/src/store.rs:163`

**已覆盖测试（如有）**
- gateway 单测通过（回归保护）：`cargo test -p moltis-gateway`（本地）

**已知差异/后续优化（非阻塞）**
- `metrics.enabled=false` 时仍能通过 API 获取空/零值 snapshot（是否保留该行为需要明确）。

---

## 背景（Background）
- 场景：用户在 `moltis.toml` 里将 `[metrics] enabled=false`，认为将完全关闭 metrics 相关开销与持久化。
- 现状：仍周期性写入 `~/.moltis/metrics.db` 的 `metrics_history`，并可能触发 sqlx 的慢语句 WARN（例如机械硬盘、IO 忙、SQLite lock）。
- Out of scope：
  - 不讨论是否需要 Prometheus `/metrics` endpoint（由 `prometheus_endpoint` 控制）。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **metrics.enabled**（主称呼）：是否启用 metrics 采集。  
  - Why：期望为“关闭后无持久化、无周期任务”的总开关。
  - Not：它不是仅关闭 `/metrics` endpoint（那是 `prometheus_endpoint`）。
  - Source/Method：configured → effective
  - Aliases（仅记录，不在正文使用）：metrics toggle

- **metrics history persistence**（主称呼）：将 metrics 快照写入 SQLite `metrics_history` 表，用于 UI 图表/API 历史点查询。  
  - Why：提供历史趋势。
  - Not：它不是实时 Prometheus scrape。
  - Source/Method：effective（gateway 周期任务）

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [ ] 当 `metrics.enabled=false` 时，不应启动 metrics history 周期任务，也不应初始化/写入 metrics store（避免 `metrics_history` INSERT）。
- [ ] 保持 `metrics.enabled=true` 时现有行为不变（历史点、清理等）。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：`metrics.enabled=false` 语义清晰，关闭后不产生写盘/慢 SQL 噪声。
  - 不得：关闭 metrics 后仍周期性写入 `metrics.db`。
- 可观测性：
  - 启动日志应明确说明 metrics history 是否启用（不仅仅是 recorder enabled/disabled）。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) 配置为：
   - `moltis.toml: [metrics] enabled=false`
2) 仍出现 sqlx 慢语句 WARN：
   - `slow statement: INSERT INTO metrics_history ... elapsed > 1s`

### 影响（Impact）
- 用户体验：关闭 metrics 仍持续输出 WARN，噪声大且容易误判为故障。
- 性能/资源：周期性写盘（尤其是机械硬盘、低 IOPS 环境）会造成额外延迟与抖动。
- 排障成本：用户需要阅读代码/日志才能理解为何“关了还在写”。

### 复现步骤（Reproduction）
1. 在配置中设置 `[metrics] enabled=false`。
2. 启动 gateway，等待一段时间。
3. 观察日志中出现 `INSERT INTO metrics_history` 的 slow statement。

## 现状核查与证据（As-is / Evidence）【不可省略】
- 代码证据：
  - `crates/metrics/src/recorder.rs:53`：`enabled=false` 返回 disabled handle（仍可 render 空 metrics）。
  - `crates/gateway/src/server.rs:2216`：`metrics.enabled=false` 时 metrics store 初始化被 gated（store=None）。
  - `crates/gateway/src/server.rs:2964`：`metrics.enabled=false` 时不 spawn metrics history 周期任务。
  - `crates/gateway/src/server.rs:2970`：`metrics.enabled=true` 时仍每 10s 生成 snapshot/写入 store。
  - `crates/metrics/src/store.rs:163`：SQLite 写入 `metrics_history` 的 INSERT 语句。
- 配置证据：
  - `~/.config/moltis/moltis.toml:492`：`[metrics] enabled=false`。

## 根因分析（Root Cause）
- A. gating 颗粒度不一致：
  - recorder 受 `metrics.enabled` 影响（会打印 disabled），但 store 初始化与 history 周期任务并未严格以 enabled 为总开关。
- B. 逻辑分支：history 任务目前只检查 `metrics_handle.is_some()`，而 `enabled=false` 仍会返回 Some(handle)（disabled handle）。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
- 必须：
  - `metrics.enabled=false` 时：不初始化 store、不启动 history 周期任务、不写 `metrics.db`。
- 应当：
  - 启动日志输出一条类似：`metrics history disabled (metrics.enabled=false)`。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）：以 `metrics.enabled` 作为总开关
- 核心思路：
  - 仅当 `config.metrics.enabled=true` 时初始化 `metrics_store`。
  - 仅当 `config.metrics.enabled=true` 时启动 metrics history 周期任务。
- 优点：语义直观，符合用户预期，最少改动。
- 风险/缺点：关闭 metrics 后 UI 的历史图表可能变为空（符合关闭的预期）。

#### 方案 2（备选）：增加独立开关（如 `metrics.history_enabled`）
- 核心思路：保留 recorder/endpoint 与 history persistence 的独立控制。
- 优点：更灵活。
- 风险/缺点：需要扩展 schema/validate/docs，配置更复杂。

### 最终方案（Chosen Approach）
#### 方案 1：以 `metrics.enabled` 作为总开关（已实现）
- 仅当 `config.metrics.enabled=true` 时初始化 `metrics_store`
- 仅当 `config.metrics.enabled=true` 时启动 metrics history 周期任务

## 验收标准（Acceptance Criteria）【不可省略】
- [ ] `metrics.enabled=false` 时不再出现 `INSERT INTO metrics_history`（且不会初始化 metrics store）。
- [ ] `metrics.enabled=true` 时行为保持：history 仍每 10s 推送并落库，且清理逻辑不变。

## 测试计划（Test Plan）【不可省略】
### Unit
- [ ] 增加 gateway 层测试（或最小化单测）验证 `metrics.enabled=false` 不初始化 `metrics_store` 且不启动 history task（可通过注入/计数器验证）。

### 手工验证步骤
- 设置 `[metrics] enabled=false`，启动后观察不再出现 metrics_history INSERT/slow statement。

## 发布与回滚（Rollout & Rollback）
- 发布策略：默认开启（bugfix）。
- 回滚策略：恢复旧行为（会重新引入噪声与写盘）。

## 实施拆分（Implementation Outline）
- Step 1: 将 metrics store 初始化 gated 到 `config.metrics.enabled`。
- Step 2: 将 metrics history 周期任务 gated 到 `config.metrics.enabled`（或显式 `history_enabled`）。
- Step 3: 补齐测试与启动日志。
- 受影响文件：
  - `crates/gateway/src/server.rs`
  - `crates/metrics/src/recorder.rs`（如需调整 enabled=false handle 语义）
  - `crates/metrics/src/store.rs`（通常不需改）
  - `crates/config/src/schema.rs` / `crates/config/src/validate.rs`（若新增独立开关）
  - `docs/src/metrics-and-tracing.md`

## 交叉引用（Cross References）
- Related issues/docs：
  - `docs/src/metrics-and-tracing.md`

## 未决问题（Open Questions）
- Q1: `metrics.enabled=false` 时是否仍要保留 `/api/metrics` 返回空 snapshot（无需持久化）？
- Q2: 是否需要新增 `metrics.history_enabled` 来兼容“只关持久化”的需求？

## Close Checklist（关单清单）【不可省略】
- [ ] 行为已按 Spec 实现（口径一致）
- [ ] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [ ] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [ ] 文档/配置示例已同步更新（避免断链）
- [ ] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [ ] 安全隐私检查通过（敏感字段不泄露）
- [ ] 回滚策略明确

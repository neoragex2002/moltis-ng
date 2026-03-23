# Issue: `load_soul()` 空文件语义错误导致误 reseed，且测试锁 poison 放大失败面（config / tests）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-23
- Owners: 待定
- Components: config
- Affected providers/models: all

**已实现（如有，写日期）**
- 2026-03-23：`load_soul()` 正确区分 “canonical 缺失” 与 “canonical 存在但为空（显式清空）”；清空后稳定返回 `None`，不再伪造 reseed：`crates/config/src/loader.rs:731`
- 2026-03-23：`DATA_DIR_TEST_LOCK` 获取改为 poison-safe，避免单测 panic 造成后续连坐：`crates/config/src/loader.rs:1704`

**已覆盖测试（如有）**
- 2026-03-23：`cargo test -p moltis-config` 通过；确认 soul 语义、legacy strict reject 与 poison-safe test lock 行为整体闭环。

**已知差异/后续优化（非阻塞）**
- 暂无。

---

## 背景（Background）
- 场景：`SOUL.md` 的设计有两种合法状态：
  - 文件不存在：表示“用户还没写”，系统应 seed `DEFAULT_SOUL`
  - 文件存在但为空：表示“用户明确清空”，系统不得自动 reseed
- 约束：
  - 本单是 bugfix，不涉及默认目录 one-cut 路径切换本身。
  - 需要区分真实行为 bug 与测试基础设施放大的次生失败，避免误判范围。
- Out of scope：
  - 不改默认目录布局。
  - 不重写 `USER.md` / `PEOPLE.md` / workspace markdown 的业务语义，除非复核后证明存在独立真实 bug。
  - 不把本单扩成通用 markdown loader 重构或测试框架重构。
  - 不修改 `read_markdown_raw()` 的共享返回契约；避免把 “空 vs 缺失” 的特判扩散到其它 markdown loader。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **`missing_soul_file`**（主称呼）：`SOUL.md` 路径不存在。
  - Why：这是“允许 seed 默认值”的唯一正常入口。
  - Not：不是文件存在但内容为空。
  - Source/Method：effective

- **`cleared_soul_file`**（主称呼）：`SOUL.md` 路径存在，但内容为空或清洗后为空。
  - Why：这表示用户明确清空，不应被系统自动恢复默认值。
  - Not：不是首次启动状态。
  - Source/Method：effective

- **`poison_cascade`**（主称呼）：某个测试 panic 后把共享 `Mutex` poison，导致后续无关测试在 `lock().unwrap()` 处连带失败。
  - Why：会把单点失败伪装成多点失败，污染评估。
  - Not：不是业务逻辑真实失败面。
  - Source/Method：effective

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] `load_soul()` 必须正确区分 `missing_soul_file` 与 `cleared_soul_file`。
- [x] `save_soul(None)` 或空字符串后，后续 `load_soul()` 必须返回 `None`，不得 reseed。
- [x] 现有 legacy `people/default/SOUL.md` strict reject 语义必须保持不变。
- [x] 测试锁 poison 不得继续把无关测试伪装成业务失败。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须：只有在 canonical `SOUL.md` 不存在且未命中现有 legacy reject 分支时，才允许 seed `DEFAULT_SOUL`。
  - 不得：文件已存在但为空时，不得返回 `Some(DEFAULT_SOUL)`。
  - 不得：不得因修本 bug 顺手放松 legacy `people/` 路径拒绝语义。
  - 不得：测试基础设施不得放大单点失败为批量假失败。
- 兼容性：不涉及外部接口或持久化 schema 变更。
- 可观测性：失败测试与真实行为 bug 需要能被清晰区分。
- 安全与隐私：不涉及敏感信息输出变更。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) `loader::tests::save_soul_none_prevents_reseed` 单独运行时稳定失败。
2) 全量跑 `cargo test -p moltis-config` 时，后续多个 `USER.md` / `PEOPLE.md` / workspace markdown 测试会一起炸成 `PoisonError`。

### 影响（Impact）
- 用户体验：
  - 用户明确清空 `SOUL.md` 后，系统仍可能偷偷恢复默认 soul，违背显式意图。
- 可靠性：
  - 测试结果失真，容易误以为多个逻辑都坏了。
- 排障成本：
  - 真 bug 与连坐失败混在一起，复盘和回归成本升高。

### 复现步骤（Reproduction）
1. 运行：
   - `cargo test -p moltis-config loader::tests::save_soul_none_prevents_reseed -- --exact --nocapture`
2. 观察：
   - 期望：`load_soul()` 返回 `None`
   - 实际：返回 `Some(DEFAULT_SOUL)`
3. 再运行：
   - `cargo test -p moltis-config`
4. 观察：
   - 首个真实失败后，多个后续测试在 `DATA_DIR_TEST_LOCK.lock().unwrap()` 处报 `PoisonError`

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 代码证据：
  - `crates/config/src/loader.rs:731`：`load_soul()` 先区分 canonical 是否存在：存在但为空 → 返回 `None`；缺失且未命中 legacy reject → seed `DEFAULT_SOUL`。
  - `crates/config/src/loader.rs:569`：legacy `people/` 等价路径仍会被结构化告警并拒绝；不会 seed 新 canonical 文件。
  - `crates/config/src/loader.rs:607`：`read_markdown_raw()` 仍保持 “空/清洗后为空 → `None`” 的通用契约，本单只在 `load_soul()` 局部补齐 “空 vs 缺失” 判定。
  - `crates/config/src/loader.rs:1704`：测试锁获取已改为 poison-safe（`unwrap_or_else(|e| e.into_inner())`），避免 poison cascade。
- 当前测试覆盖：
  - 已有：`save_soul_none_prevents_reseed`、`load_soul_creates_default_when_missing`、`load_soul_reseeds_after_deletion`
  - 缺口：已由 `cargo test -p moltis-config` 全量回归关闭。

## 根因分析（Root Cause）
- A. `save_soul(None)` 与 `load_soul()` 之间约定不一致：前者把空文件定义为“显式清空”，后者却把空文件等同于“无内容可读”。
- B. `load_soul()` 当前用 `read_markdown_raw()` 的 `Option` 结果直接决定是否 reseed，但这个结果无法区分“文件不存在”与“文件存在但为空”。
- C. `write_default_soul()` 在文件已存在时是 no-op，但 `load_soul()` 仍无条件返回 `Some(DEFAULT_SOUL.to_string())`，造成“磁盘为空、返回值却像已 reseed”这种语义错乱。
- D. 同一逻辑里还叠着 legacy `people/` strict reject 分支，所以修复时必须只修“空 vs 缺失”的判定，不得误伤 reject 语义。
- E. 测试使用共享 `Mutex` 且直接 `unwrap()`，一旦首个测试 panic，后续就被 poison 连坐，掩盖真实失败面。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - `load_soul()` 必须只在 canonical `SOUL.md` 缺失且未命中 legacy reject 时 seed 默认 soul。
  - `cleared_soul_file` 必须稳定返回 `None`。
  - legacy `people/default/SOUL.md` 命中时必须继续直接拒绝，且不得 seed 新文件。
  - 全量测试失败时，应当优先暴露真实首因，而不是大量 poison 次生噪声。
- 不得：
  - 不得把“空文件”与“文件缺失”混为一谈。
  - 不得在文件已存在但为空时伪造 `Some(DEFAULT_SOUL)` 返回值。
- 应当：
  - 应当把测试锁获取改成 poison-safe 处理；本单不再引入新的测试 helper/抽象层。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：
  - 在 `load_soul()` 内显式区分：
    - canonical 文件不存在且未命中 legacy reject → seed 默认值
    - 文件存在但清洗后为空 → 返回 `None`
    - 文件存在且非空 → 返回内容
  - 同时把测试锁获取改成 poison-safe，避免次生误报。
- 优点：
  - 修的是根因，不是补测试。
  - 行为语义与测试/注释一致。
  - 后续测试信号更干净。
- 风险/缺点：
  - 需要小心别误伤 legacy `people/` 路径拒绝逻辑。

#### 方案 2（不选）
- 方案：只改测试预期，让空文件也被视为“允许 reseed”。
- 不选原因：
  - 这会直接推翻 `save_soul()` 当前注释与显式设计意图。

### 最终方案（Chosen Approach）
#### 行为规范（Normative Rules）
- 规则 1：只有 canonical `SOUL.md` 缺失且未命中 legacy reject 时，`load_soul()` 才能调用 seed 分支。
- 规则 2：命中 legacy `people/default/SOUL.md` 时，`load_soul()` 继续返回 `None` 且不得 seed。
- 规则 3：`SOUL.md` 存在但为空时，`load_soul()` 返回 `None`。
- 规则 4：`DATA_DIR_TEST_LOCK` 不得因单个 panic 让后续无关测试统一误报。
- 规则 5：实现必须收敛在 `crates/config/src/loader.rs` 现有逻辑内，优先在 `load_soul()` 本地补 `exists()` / legacy 判定；不得把 `read_markdown_raw()` 改成新的 tri-state 通用契约，也禁止顺手抽新的测试 helper/框架层。

#### 接口与数据结构（Contracts）
- API/RPC：
  - 无新增接口。
- 存储/字段兼容：
  - 继续沿用“空 `SOUL.md` 表示显式清空”的现有语义。
- UI/Debug 展示（如适用）：
  - 无。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 无新增用户侧回执。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 无。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：
  - 不涉及敏感字段。
- 禁止打印字段清单：
  - 无新增。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `save_soul(None)` 后 `load_soul()` 返回 `None`。
- [x] `SOUL.md` 缺失时 `load_soul()` 仍会 seed `DEFAULT_SOUL`。
- [x] legacy `people/default/SOUL.md` 命中时 `load_soul()` 仍返回 `None`，且不得 seed 新 canonical 文件。
- [x] `SOUL.md` 已有非空内容时 `load_soul()` 不覆盖现有内容。
- [x] `cargo test -p moltis-config` 不再因为测试锁 poison 产生批量假失败。
- [x] `crates/config/src/loader.rs` 测试中不再残留 `DATA_DIR_TEST_LOCK.lock().unwrap()`。
- [x] `USER.md` / `PEOPLE.md` / workspace markdown 相关测试在全量下不再被首个 soul 测试连带打爆。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `crates/config/src/loader.rs:2221`：`save_soul_none_prevents_reseed`
- [x] `crates/config/src/loader.rs:2051`：`load_soul_creates_default_when_missing`
- [x] `crates/config/src/loader.rs:2093`：`load_soul_reseeds_after_deletion`
- [x] `crates/config/src/loader.rs:2253`：`save_soul_some_overwrites_default`
- [x] `crates/config/src/loader.rs:2275`：`load_soul_rejects_legacy_people_default_path`
- [x] `rg`/targeted assertions：`crates/config/src/loader.rs` 不再包含 `DATA_DIR_TEST_LOCK.lock().unwrap()`
- [x] `crates/config/src/loader.rs:1942`：`save_user_does_not_delete_file_when_empty_and_preserves_body`
- [x] `crates/config/src/loader.rs:2015`：`workspace_markdown_ignores_leading_html_comments`
- [x] `crates/config/src/loader.rs:2035`：`workspace_markdown_comment_only_is_treated_as_empty`

### Integration
- [x] `cargo test -p moltis-config`

### UI E2E（Playwright，如适用）
- [x] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：
  - 无明显自动化缺口。
- 手工验证步骤：
  1. 在临时 `data_dir` 下首次调用 `load_soul()`，确认自动生成默认 SOUL。
  2. 调用 `save_soul(None)` 后再次 `load_soul()`，确认返回 `None`。
  3. 写入 legacy `people/default/SOUL.md`，确认 `load_soul()` 仍拒绝且不生成 canonical `SOUL.md`。
  4. 再跑 `cargo test -p moltis-config`，确认不再出现 poison 连坐。

## 发布与回滚（Rollout & Rollback）
- 发布策略：普通 bugfix，直接合入。
- 回滚策略：回滚 `crates/config/src/loader.rs` 的相关行为修复即可。
- 上线观测：主要依赖单测回归。

## 实施拆分（Implementation Outline）
- Step 1：修正 `load_soul()` 对空文件 / 缺失文件的判定逻辑。
- Step 2：保持 legacy `people/default/SOUL.md` strict reject 语义不变。
- Step 3：保持 `save_soul()` 的“空文件表示显式清空”语义不变。
- Step 4：把测试锁获取改成 poison-safe，避免 poison 连坐；不新增测试 helper。
- Step 5：跑 `moltis-config` 全量测试并更新 issue 状态。
- 受影响文件：
  - `crates/config/src/loader.rs`

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/issue-default-home-layout-one-cut-moltis-config-data.md`
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
- [x] 文档/配置示例已同步更新（N/A）
- [x] 兼容性/迁移说明已写清（N/A）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确

# Issue: 代码库 UTF-8 边界不安全切片总览（char boundary / string slicing）

## 实施现状（Status）【增量更新主入口】
- Status: DONE
- Priority: P1
- Updated: 2026-03-18
- Owners:
- Components: tools / gateway / sessions / browser / plugins / voice
- Affected providers/models: (any)

**已实现（如有，写日期）**
- 2026-03-18：在 `moltis-common` 新增共享 UTF-8 helper，统一收敛 `floor_char_boundary` / `truncate_utf8` / `truncate_utf8_with_suffix`：`crates/common/src/text.rs`
- 2026-03-18：`tools::exec` / `tools::sandbox` 改为共享安全截断，并把多 backend 输出裁剪收敛到 `ExecResult::from_process_output`：`crates/tools/src/exec.rs`、`crates/tools/src/sandbox.rs`
- 2026-03-18：`plugins::session_memory`、`browser::manager`、`gateway::chat` 的预览/摘要/URL/命令显示裁剪已统一改为共享 helper：`crates/plugins/src/bundled/session_memory.rs`、`crates/browser/src/manager.rs`、`crates/gateway/src/chat.rs`
- 2026-03-18：`web_fetch` 已改为字符语义扫描，消除跨字符串 offset、`&html[i..]` 和 `bytes[i] as char`：`crates/tools/src/web_fetch.rs`
- 2026-03-18：`sessions::store` 已消除 lower/原串 offset 复用；`voice::tts::google` 已补语言前缀 guard：`crates/sessions/src/store.rs`、`crates/voice/src/tts/google.rs`

**已覆盖测试（如有）**
- Telegram 出站 4096 UTF-8 边界 panic 已有历史修复单与覆盖：`issues/done/issue-telegram-outbound-utf8-char-boundary-panic-4096.md:1`
- 本单新增自动化覆盖：
  - 共享 helper 边界矩阵：`crates/common/src/text.rs`
  - `exec` / `sandbox` 多字节输出裁剪：`crates/tools/src/exec.rs`、`crates/tools/src/sandbox.rs`
  - `web_fetch` Unicode HTML 路径：`crates/tools/src/web_fetch.rs`
  - `session_memory` 长 Unicode 消息裁剪：`crates/plugins/src/bundled/session_memory.rs`
  - `browser::manager` / `gateway::chat` UTF-8 预览：`crates/browser/src/manager.rs`、`crates/gateway/src/chat.rs`
  - `sessions::store` Unicode 搜索片段：`crates/sessions/src/store.rs`
  - `voice::tts::google` language code guard：`crates/voice/src/tts/google.rs`

**已知差异/后续优化（非阻塞）**
- 历史上已存在于其他 crate 的局部 helper（如 telegram / agents / gateway::session）本次未强行并表；后续若继续收敛，只允许向 `moltis-common` 归并，不得新增新分支。
- 本单仍只覆盖已核实的 UTF-8 边界风险点，不扩展到所有字符串处理风格统一。

---

## 背景（Background）
- 场景：`tmux` 中运行的 `server:1.1`（`target/debug/moltis`）出现过 `thread 'tokio-runtime-worker' panicked at crates/tools/src/web_fetch.rs:301:44` 一类 `byte index ... is not a char boundary` 异常。
- 约束：Rust `&str` 切片使用的是 UTF-8 **字节索引**，任何 `[..N]` / `[start..end]` 都必须落在字符边界；跨字符串复用 byte offset 同样危险。
- Out of scope：本单不处理与 UTF-8 边界无关的普通逻辑 bug；也不要求一次性统一所有文本处理 API 风格。

## 概念与口径（Glossary & Semantics）【概念收敛/避免歧义】
> 只允许在这里声明别名；正文统一使用“主称呼”。

- **UTF-8 边界切片 bug**（主称呼）：对 `&str` 使用 byte range 切片，但索引未保证落在字符边界，导致 panic 或文本损坏。
  - Why：这是本单需要统一收敛的根问题。
  - Not：不是普通 `Vec<u8>` 切片；不是所有 `[..N]` 都有问题，前提是来源字符串与索引边界不安全。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：char boundary bug / string slicing bug

- **跨字符串 offset 复用**（主称呼）：在 A 字符串上得到的 byte offset，被拿去切 B 字符串（尤其 B 是 `to_lowercase()` 等变换后的新串）。
  - Why：这是本单已确认的一类高危根因。
  - Not：不是“同一字符串上 `find()` 返回 offset 再切同一字符串”的场景。
  - Source/Method：[effective]
  - Aliases（仅记录，不在正文使用）：offset drift / lower-case offset mismatch

- **高风险点**（主称呼）：已确认会 panic、已出现现场 panic、或在非 ASCII 输入下高度可复现的问题点。
  - Why：实施应优先处理。
  - Not：不是低概率理论风险或已证明同串安全切片点。
  - Source/Method：[estimate]

- **authoritative**：来自 provider 返回（例如 usage）或真实请求回包的权威值。
- **estimate**：本地推导/启发式估算（必须标注 method），用于提前评估风险，不能当真值使用。
- **configured / effective / as-sent**：
  - configured：配置文件原始值
  - effective：合并/默认/clamp 后的生效值
  - as-sent：最终写入请求体、实际发送给上游的值

## 需求与目标（Requirements & Goals）
### 功能目标（Functional）
- [x] 汇总代码库中已确认与 UTF-8 边界切片相关的高风险/中风险点。
- [x] 冻结修复口径：直接字节截断、跨字符串 offset 复用、按字节当字符写入三类问题分别如何收敛。
- [x] 为后续实施提供明确范围，避免一边修 panic 一边扩大到无关字符串处理。

### 非功能目标（Non-functional）
- 正确性口径（必须/不得）：
  - 必须区分“已确认高风险”与“已审排除/低风险”。
  - 不得把“同串 `find/rfind` 得到的合法边界切片”误报成同类 bug。
  - 必须优先复用已有 UTF-8 安全 helper，而不是新增多个重复 helper。
- 兼容性：修复应保持现有文本语义基本不变；只修正 panic 与错误截断。
- 可观测性：如新增护栏/降级，需有明确 reason code 或至少不引入无声数据损坏。
- 安全与隐私：问题分析不涉及敏感数据打印。

## 问题陈述（Problem Statement）
### 现象（Symptoms）
1) `server:1.1` 中出现 `byte index ... is not a char boundary` 型 panic，已定位到 `crates/tools/src/web_fetch.rs:301`。
2) 代码库中仍存在多处直接使用 `&text[..N]` / `&s[..N]` 的 byte 截断，用于 UI 摘要、日志预览、持久化裁剪等路径。
3) `tools::exec` / `tools::sandbox` 还存在对 `String` 直接 `.truncate(max_output_bytes)` 的逻辑；`max_output_bytes` 是字节上限，不保证字符边界。
4) 另有部分逻辑把 `to_lowercase()` 结果上的位置或原串上的 byte offset 混用，存在越界或错误片段风险。

### 影响（Impact）
- 用户体验：消息预览、工具结果、URL 摘要在中文/emoji/非 ASCII 输入下可能 panic 或乱码。
- 可靠性：confirmed panic 已发生在运行服务上；部分路径位于高频日志/摘要/会话存储流程，另一些路径位于 `exec`/sandbox 基础设施层。
- 排障成本：如果不系统清点，容易“修一个再炸一个”。

### 复现步骤（Reproduction）
1. 输入包含中文/emoji/非 ASCII 的 HTML/消息/URL/工具输出。
2. 命中相关裁剪/切片逻辑。
3. 期望 vs 实际：期望安全截断；实际可能 panic、错误片段、或乱码。

## 现状核查与证据（As-is / Evidence）【不可省略】
> 必须至少给出 1 条可定位证据：`path/to/file:line` / 测试 / 日志关键词。

- 现场/日志证据：
  - `tmux server:1.1` 中已出现：`thread 'tokio-runtime-worker' panicked at crates/tools/src/web_fetch.rs:301:44`
  - 关键词：`byte index ... is not a char boundary`

- 代码证据（已确认高风险）：
  - `crates/tools/src/web_fetch.rs:301`：`let tag_start = &html_lower[i..];`
  - `crates/tools/src/web_fetch.rs:286`
  - `crates/tools/src/web_fetch.rs:289`
  - `crates/tools/src/web_fetch.rs:292`
  - `crates/tools/src/web_fetch.rs:295`
  - `crates/tools/src/web_fetch.rs:336`
  - `crates/tools/src/web_fetch.rs:359`
  - `crates/tools/src/exec.rs:114`
  - `crates/tools/src/exec.rs:118`
  - `crates/tools/src/sandbox.rs:1483`
  - `crates/tools/src/sandbox.rs:1487`
  - `crates/tools/src/sandbox.rs:1656`
  - `crates/tools/src/sandbox.rs:1660`
  - `crates/tools/src/sandbox.rs:2087`
  - `crates/tools/src/sandbox.rs:2091`
  - `crates/plugins/src/bundled/session_memory.rs:119`
  - `crates/browser/src/manager.rs:845`
  - `crates/gateway/src/chat.rs:4849`
  - `crates/gateway/src/chat.rs:4949`
  - `crates/gateway/src/chat.rs:6348`
  - `crates/gateway/src/chat.rs:8055`
  - `crates/gateway/src/chat.rs:8076`
  - `crates/gateway/src/chat.rs:8101`
  - `crates/sessions/src/store.rs:296`
  - `crates/sessions/src/store.rs:299`

- 代码证据（条件性中风险 / 取决于输入域约束）：
  - `crates/voice/src/tts/google.rs:96`：`&self.language_code[..2]` 依赖 `language_code` 至少 2 字节且位于 ASCII 域；默认配置通常满足，但代码未就长度/字符集显式兜底。

- 代码证据（已审排除 / 当前不列入同类修复范围）：
  - `crates/tools/src/image_cache.rs:75`：来源字符串是十六进制 ASCII，切片安全。
  - `crates/tools/src/process.rs:36`：UUID hex 字符串，切片安全。
  - `crates/config/src/loader.rs:932`
  - `crates/config/src/loader.rs:946`
  - `crates/onboarding/src/wizard.rs:168`
  - `crates/onboarding/src/service.rs:296`
  - `crates/onboarding/src/service.rs:318`
  - `crates/gateway/src/server.rs:405`
  - `crates/voice/src/tts/mod.rs:183`
  - `crates/voice/src/tts/mod.rs:355`
  - `crates/voice/src/tts/mod.rs:374`
  - `crates/agents/src/runner.rs:300`
  - `crates/agents/src/runner.rs:413`
  - `crates/agents/src/runner.rs:462`
  - `crates/agents/src/runner.rs:517`
    - 这些位置的索引来自同一字符串上的 `find()`/`rfind()` / `char_indices()` 等安全来源，当前不属于本单关注的跨字符串 offset 或固定字节截断问题。
  - `crates/tools/src/web_search.rs:451`
  - `crates/tools/src/web_search.rs:465`
  - `crates/tools/src/web_search.rs:467`
    - 这些位置来自同一字符串上的 `find/rfind` 结果，当前不属于“跨字符串 offset 复用”。

- 当前测试覆盖：
  - 已有：Telegram 出站 UTF-8 边界修复已有 precedent：`issues/done/issue-telegram-outbound-utf8-char-boundary-panic-4096.md:1`
  - 已补：本单列出的高风险点已补齐共享 helper、调用点和局部语义修复的自动化回归。

- 实施条件证据：
  - `crates/tools/Cargo.toml`
  - `crates/gateway/Cargo.toml`
  - `crates/sessions/Cargo.toml`
  - `crates/browser/Cargo.toml`
  - `crates/plugins/Cargo.toml`
    - 上述 crate 已依赖 `moltis-common`，共性 UTF-8 helper 放入 `moltis-common` 具备直接复用条件。
  - `crates/voice/Cargo.toml`
    - `voice` 当前尚未依赖 `moltis-common`，因此 `voice` 项应视为条件性单点，不应反向驱动整套方案发散。

## 根因分析（Root Cause）
- A. **直接字节截断**：`&s[..N]` / `&text[..N]` 假定 `N` 总是字符边界，在 Unicode 输入下不成立。
- A1. **直接 `String::truncate(byte_limit)`**：`truncate()` 同样要求字符边界；当上限来自“字节预算”时，会在多字节字符上 panic。
- B. **跨字符串 offset 复用**：例如原串上推进的 byte index，被拿去切 `to_lowercase()` 后的新串；或 lower 串上的位置拿去切原串。
- C. **按字节当字符写入**：扫描 `bytes` 后直接 `bytes[i] as char`，不会 panic，但会在非 ASCII 文本上生成损坏内容。

## 期望行为（Desired Behavior / Spec）【尽量冻结】
> 用“必须/不得/应当”写清楚最终口径；后续更新优先改“实现/测试/进度”，避免频繁改 Spec。

- 必须：
  - 任意 `&str` 截断必须在 UTF-8 字符边界进行。
  - 任意“搜索位置 → 切片”必须保证 offset 与被切字符串是同一串、同一编码布局。
  - 任意文本扫描若要逐字符输出，必须使用字符语义而不是单字节转 `char`。
- 不得：
  - 不得在未知字符集输入上直接使用 `&s[..N]`。
  - 不得把 `to_lowercase()` / `trim` / 其他变换后字符串的 offset 与原串 offset 混用。
- 应当：
  - 应当优先复用现有 `floor_char_boundary` / `truncate_utf8_to_bytes` 一类 helper。
  - 应当在高频裁剪点统一使用同一 helper，减少未来复发面。

## 方案（Proposed Solution）
### 方案对比（Options，可选）
#### 方案 1（推荐）
- 核心思路：按“**共性 helper** + **局部重写**”两层收敛：
  1. 在 `moltis-common` 增加唯一共享的 UTF-8 边界 helper；
  2. 所有“固定字节截断/输出裁剪/预览摘要”统一改用该 helper；
  3. 少数无法靠 helper 解决的逻辑（`web_fetch` 扫描器、`sessions::store` offset 复用）做局部重写。
- 优点：方案内聚、复用面大、后续复扫标准统一。
- 风险/缺点：需要先做一次 helper 落点收敛，再做调用点替换。

#### 方案 2（不推荐）
- 核心思路：逐点散修，哪里 panic 修哪里。
- 风险/缺点：缺乏统一口径，容易遗漏相邻高风险点。

### 最终方案（Chosen Approach）
#### 方案收敛原则（Convergence Rules）
- 原则 1：**共性问题只允许一个共性方案。**
  - 所有“按字节上限做 `&str` 预览/裁剪”的点，统一收敛到一个共享 helper；不得在 `tools/gateway/browser/plugins/sessions` 各自再写一套。
- 原则 2：**局部逻辑问题只在局部重写。**
  - `web_fetch` 的扫描器和 `sessions::store` 的 offset 复用，不强行抽象成通用 helper；在原模块内按语义修正。
- 原则 3：**条件性单点不反向带偏主方案。**
  - `voice` 的 `language_code[..2]` 若纳入修复，应走语义解析（如语言子标签提取），而不是为了它扩张通用 helper 范围。

#### 行为规范（Normative Rules）
- 规则 1：所有纯“摘要/预览/日志裁剪”场景统一走共享 UTF-8 安全截断 helper。
- 规则 2：所有 `String::truncate(byte_limit)` 必须替换为“先安全截断，再追加后缀”的共享实现。
- 规则 3：所有依赖 `to_lowercase()` / `find()` 位置的逻辑，必须确保 offset 不跨字符串复用。
- 规则 4：对 `web_fetch` 这类手写扫描器，优先修成不依赖 `String` byte 切片的实现。
- 规则 5：除 `moltis-common` 外，不再新增第二套同义 UTF-8 截断 helper；调用点只允许复用共享 helper 或做局部语义修正。
- 规则 6：测试也要收敛；UTF-8 边界矩阵集中在共享 helper 覆盖，下游调用点只补代表性回归，不做多份重复矩阵。

#### 接口与数据结构（Contracts）
- API/RPC：无新增外部契约。
- 存储/字段兼容：无 schema 变更。
- UI/Debug 展示（如适用）：截断后文本可保持现有附注形式（如 `...` / `…` / `[truncated ...]`），但必须 UTF-8 安全。
- 共享 helper（推荐落点）：
  - 推荐在 `moltis-common` 新增单一模块，最小 API 仅覆盖：
    - `floor_char_boundary(text, max_bytes) -> usize`
    - `truncate_utf8(text, max_bytes) -> &str`
    - `truncate_utf8_with_suffix(text, max_bytes, suffix) -> String`
      - 语义固定为：`max_bytes` 约束“保留前缀”，仅在发生截断时追加 `suffix`；默认不改变现有调用点“后缀附加后总长度可大于前缀预算”的行为口径。
  - 不在各 crate 新增同义 helper 名称/实现。

#### 修复分桶（Fix Buckets）
- Bucket A（共享基础设施截断）：
  - `crates/tools/src/exec.rs`
  - `crates/tools/src/sandbox.rs`
  - 目标：统一消除 `String::truncate(byte_limit)`。
- Bucket B（共享应用层预览/摘要）：
  - `crates/plugins/src/bundled/session_memory.rs`
  - `crates/browser/src/manager.rs`
  - `crates/gateway/src/chat.rs`
  - 目标：统一消除 `&str[..N] + suffix` 型预览截断。
- Bucket C（局部扫描器重写）：
  - `crates/tools/src/web_fetch.rs`
  - 目标：一次性消除跨字符串 offset、`&html[i..]`、`bytes[i] as char` 三类问题；不引入新 crate。
- Bucket D（局部搜索片段修正）：
  - `crates/sessions/src/store.rs`
  - 目标：确保 snippet 边界只基于原串自身计算，不复用 lower 串 offset。
- Bucket E（条件性单点 guard，非首批阻塞）：
  - `crates/voice/src/tts/google.rs`
  - 目标：如纳入，仅补长度/语义 guard；不改变共享 helper 设计。

#### 失败模式与降级（Failure modes & Degrade）
- 错误分类与用户回执（脱敏）：
  - 修复后不应再出现 `byte index ... is not a char boundary` panic。
  - 极长文本仅允许被安全裁剪，不得触发 panic。
- 队列/状态清理（必须 drain/必须删除/必须保留）：
  - 不涉及额外状态清理。

#### 安全与隐私（Security/Privacy）
- 默认展示/日志是否脱敏：不改变现有脱敏策略。
- 禁止打印字段清单：无新增敏感字段。

## 验收标准（Acceptance Criteria）【不可省略】
- [x] `web_fetch` confirmed panic 根因点完成修复，并补最小测试覆盖 Unicode/中文输入。
- [x] `tools::exec` / `tools::sandbox` 的输出裁剪不再使用 `String::truncate(byte_limit)`。
- [x] 所有已确认 `&str[..N]` 高风险摘要点改为 UTF-8 安全截断。
- [x] `sessions::store` 的 lower/原串 offset 复用风险被消除。
- [x] 首批实施未在 `tools/gateway/browser/plugins/sessions` 新增新的 crate-local UTF-8 截断 helper。
- [x] 本单列出的“已审排除”项不被误修、不扩大 scope。

## 测试计划（Test Plan）【不可省略】
### Unit
- [x] `moltis-common` 共享 helper：覆盖 ASCII / 中文 / emoji / suffix 口径，作为唯一边界矩阵主测试
- [x] `crates/tools/src/web_fetch.rs`：Unicode HTML 输入不 panic，且中文不乱码
- [x] `crates/tools/src/exec.rs` / `crates/tools/src/sandbox.rs`：多字节输出在 `max_output_bytes` 截断下不 panic
- [x] `crates/plugins/src/bundled/session_memory.rs`：长中文消息截断不 panic
- [x] `crates/browser/src/manager.rs`：长非 ASCII URL 截断不 panic
- [x] `crates/gateway/src/chat.rs`：stdout/stderr/summary/query preview 截断不 panic
- [x] `crates/sessions/src/store.rs`：大小写搜索片段提取在 Unicode 输入下不 panic
- [x] `crates/voice/src/tts/google.rs`：异常/短 `language_code` 配置不触发切片 panic

### Integration
- [x] 以 `html_to_text` 真实中文 HTML 样本覆盖核心解析路径；未新增依赖外网的脆弱 integration case。

### UI E2E（Playwright，如适用）
- [x] 不适用

### 自动化缺口（如有，必须写手工验收）
- 缺口原因：未新增依赖外网/远端页面稳定性的 integration case；`web_fetch` 的核心逻辑已由纯本地单测覆盖。
- 手工验证步骤：
  1. 复现含中文/emoji 的 `web_fetch` 输入；
  2. 复现含中文/emoji 的工具输出与日志预览；
  3. 确认不再出现 `byte index ... is not a char boundary`；
  4. 确认预览文本未出现明显乱码。

## 发布与回滚（Rollout & Rollback）
- 发布策略：按“共享 helper → 共性替换 → 局部重写 → 条件性单点”四步推进，避免并行散改。
- 回滚策略：若某个点行为回退，仅允许回滚该点实现；不得回滚已验证的 UTF-8 安全 helper。
- 上线观测：关注 `panic`、`byte index`、`char boundary` 相关日志关键词。

## 实施拆分（Implementation Outline）
- Batch A（共性基础设施）：
  - 在 `moltis-common` 落唯一共享 UTF-8 helper，并补完整边界矩阵单测；
  - 先替换 `tools::exec` / `tools::sandbox` 的 `String::truncate(byte_limit)`。
- Batch B（共性应用层预览）：
  - 统一替换 `plugins::session_memory`、`browser::manager`、`gateway::chat` 的 `&str[..N] + suffix`。
- Batch C（局部语义修复）：
  - 重写 `web_fetch` 的扫描逻辑，去掉跨字符串 offset、`&html[i..]`、`bytes[i] as char`；
  - 修正 `sessions::store` 的 lower/原串 offset 复用。
- Batch D（收口）：
  - 评估是否纳入 `voice::tts::google` 的单点 guard；默认不阻塞主线；
  - 补代表性回归单测并做一次全局复扫，确认无漏点、无新增分散 helper。
- 受影响文件：
  - `crates/common/*`（新增共享 helper 模块）
  - `crates/tools/src/web_fetch.rs`
  - `crates/tools/src/exec.rs`
  - `crates/tools/src/sandbox.rs`
  - `crates/plugins/src/bundled/session_memory.rs`
  - `crates/browser/src/manager.rs`
  - `crates/gateway/src/chat.rs`
  - `crates/sessions/src/store.rs`
  - `crates/voice/src/tts/google.rs`

## 实施准备度（Readiness）
- 共享落点已明确：`moltis-common` 现有依赖关系可直接承接，无需新增跨层依赖。
- 范围已收敛：共性点收敛为 Bucket A/B，局部语义问题收敛为 Bucket C，`voice` 保持非阻塞条件项。
- 外部影响可控：无 schema 迁移、无协议改动、无额外运维前置。
- 测试路径清晰：共享 helper 做主矩阵，下游调用点只补代表性回归。
- 结论：本单已具备直接实施条件，建议严格按 Batch A → B → C → D 顺序推进。

## 交叉引用（Cross References）
- Related issues/docs：
  - `issues/done/issue-telegram-outbound-utf8-char-boundary-panic-4096.md`
- Related commits/PRs：
  - N/A
- External refs（可选）：
  - Rust `str::is_char_boundary`

## 未决问题（Open Questions）
- 当前无阻塞性未决问题。
- 说明 1：共享 helper 已按 `floor_char_boundary` + `truncate_utf8` + `truncate_utf8_with_suffix` 落地。
- 说明 2：`web_fetch` 已按局部字符语义修复落地，未保留半修字节扫描方案。
- 说明 3：`voice::tts::google` 的条件性单点已一并落地，不再阻塞关单。

## Close Checklist（关单清单）【不可省略】
- [x] 行为已按 Spec 实现（口径一致）
- [x] authoritative vs estimate 边界清晰（且 UI/日志标注 method/source）
- [x] 已补齐/更新自动化测试（或记录缺口 + 手工验收）
- [x] 文档/配置示例已同步更新（避免断链）
- [x] 兼容性/迁移说明已写清（如涉及持久化/字段变更）
- [x] 安全隐私检查通过（敏感字段不泄露）
- [x] 回滚策略明确
- [x] 未新增分散的 crate-local UTF-8 helper / workaround

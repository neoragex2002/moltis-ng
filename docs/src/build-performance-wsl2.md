# WSL2 编译性能笔记（Moltis）

本文档汇总了在 **Windows 11 + WSL2** 环境下，为加速本地 Rust 构建（`cargo run` / `cargo build` / 增量迭代）所做的：

- **测量过程**（如何定位最慢的组件）
- **关键结论**（哪些 crate 真正耗时、为何耗时）
- **不改代码的优化步骤**（只动环境/配置，不改 `Cargo.toml`）

## 范围 / 约束

- 范围：本地开发构建（`cargo run`、`cargo build`、增量构建体验）
- 约束：**不修改仓库代码**（不改 `Cargo.toml`、不重构 feature），允许通过环境变量/本机配置加速

## 环境快照（实测）

- 仓库路径：`/mnt/c/dev/moltis`
- 仓库目录文件系统类型：`v9fs`（DrvFS/9p）
- `/mnt/c` 挂载类型：`9p`
- 工具链：
  - `rustc 1.93.0`（stable）
  - `cargo 1.93.0`（stable）
  - 为生成 Cargo timings 安装了 `nightly-2025-11-30`
- WSL2 VM 资源：22 核，约 23GiB 内存
- 已安装构建工具：`clang`、`cmake`、`ninja`、`pkg-config`

为什么这点非常关键：Rust/Cargo 会做**海量的小文件读写 + metadata 操作**（fingerprint、incremental、dep-info、rlib、obj 等）。在 `/mnt/c`（9p/DrvFS）上做这些操作，通常就是“WSL2 下 Rust 编译慢到离谱”的根因之一。

## 测量方法

### 1) 使用 Cargo Timings（仅 nightly 可用）

Cargo 的 `--timings` 在 stable 上是 unstable 功能，因此需要使用 pinned nightly：

```bash
rustup toolchain install nightly-2025-11-30 --profile minimal
```

以 `moltis` 包为例生成 timings：

```bash
export CARGO_TARGET_DIR="$HOME/.cache/moltis-target"
cargo +nightly-2025-11-30 build -Z unstable-options -p moltis --timings=html,json
```

timings 报告输出目录：

```text
$CARGO_TARGET_DIR/cargo-timings/
```

### 2) 从 `--timings=json` 提取 `timing-info` 统计

`--timings=json` 会输出机器可读的 `timing-info` 记录。把这些记录按 crate 汇总后，可以直接得到：

- 哪些 crate 总耗时最高
- 耗时主要来自“Rust 编译本身”还是“build script（C/C++ 现场编译）”

## 关键发现（最耗时热点）

来自一次完整的 `cargo +nightly-2025-11-30 build -p moltis --timings=html,json` 结果，按总耗时排序如下（近似值）：

| 排名 | Crate | 总耗时 | 主要原因 |
|---:|---|---:|---|
| 1 | `llama-cpp-sys-2@0.1.133` | ~630.8s（~10.5 分钟） | build script（C/C++） |
| 2 | `chromiumoxide_cdp@0.8.0` | ~435.2s（~7.3 分钟） | Rust 编译本身 |
| 3 | `openssl-sys@0.9.111` | ~393.7s（~6.6 分钟） | build script（vendored OpenSSL） |
| 4 | `aws-lc-sys@0.37.0` | ~375.2s（~6.3 分钟） | build script（C/C++） |

更细的“build script vs Rust 编译”拆分：

- `llama-cpp-sys-2`：`custom-build` 目标累计 ~629.7s；`run-custom-build` ~617.1s
- `openssl-sys`：`custom-build` 目标累计 ~385.6s；`run-custom-build` ~375.4s
- `aws-lc-sys`：`custom-build` 目标累计 ~373.2s；`run-custom-build` ~358.7s
- `chromiumoxide_cdp`：几乎没有 `custom-build` 时间（主要是 Rust 编译成本）

### 依赖来源（为什么会编这些东西）

- `chromiumoxide_cdp` 来源：
  - `chromiumoxide_cdp -> chromiumoxide -> moltis-browser -> moltis-gateway -> moltis`
  - 当前 `moltis-gateway` 对 `moltis-browser` 是无条件依赖（`crates/gateway/Cargo.toml`），所以默认构建会带上整条浏览器栈。

- `openssl-sys` 来源：
  - workspace 级依赖把 `openssl` 固定为 `vendored`（`Cargo.toml`），会触发 `openssl-src` 从源码编译 OpenSSL。
  - 另有多条传递依赖也会拉 `openssl-sys`（如 `native-tls`、`curl` 等）。

- `aws-lc-sys` 来源：
  - 通过 TLS client 栈（`rustls` / `hyper-rustls` 的 feature 组合）引入，且可能由 metrics/prometheus 相关依赖触发。

- `llama-cpp-sys-2` 来源：
  - `llama-cpp-sys-2 -> llama-cpp-2 -> moltis-agents -> moltis -> ...`
  - 在本次构建里它是通过默认 feature 路径被启用。

### 一个非常关键的实测现象：增量构建可以很快

当把 `CARGO_TARGET_DIR` 设置到 `$HOME` 下（WSL ext4）后，“无改动二次构建（no-op rebuild）”能做到 **~2–3 秒**。

这意味着：最大的问题通常不是“Rust 永远很慢”，而是：

- 一次性编译的大型 C/C++ 依赖（`*-sys` build script）成本很高；以及
- `/mnt/c`（9p/DrvFS）会显著放大 Cargo 的 I/O 成本，影响缓存/增量体验。

## 不改代码的优化步骤（按收益优先级）

### 第 1 步（ROI 最高）：把构建产物从 `/mnt/c` 挪走

把 `target/` 放到 WSL 的 ext4 文件系统（比如 `$HOME`）：

```bash
export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/moltis"
```

建议写进 `~/.bashrc`，确保 `cargo run` 总是受益。

替代方案（全局 Cargo 配置、无需每个 shell export）：在 `$HOME/.cargo/config.toml` 里设置 `build.target-dir`。

### 第 2 步（ROI 次高）：把仓库从 `/mnt/c` 挪到 WSL 文件系统

把仓库 clone/copy 到 WSL ext4（例如 `~/src/moltis`）并在该路径下开发构建。

这通常会带来“质变”，因为 Rust 的构建模式对小文件与 metadata 操作非常敏感。

### 第 3 步：给 Rust + C/C++ 编译加缓存（sccache）

安装并启用 `sccache`：

```bash
cargo install sccache
export RUSTC_WRAPPER=sccache
```

同时把 CMake 系的 native build 接入缓存（对 `aws-lc-sys`、很多情况下也对 `llama-cpp-sys-2` 有帮助）：

```bash
export CMAKE_C_COMPILER_LAUNCHER=sccache
export CMAKE_CXX_COMPILER_LAUNCHER=sccache
```

验证缓存命中：

```bash
sccache --show-stats
```

### 第 4 步：最大化 native build 并行度

```bash
export CMAKE_GENERATOR=Ninja
export CMAKE_BUILD_PARALLEL_LEVEL="$(nproc)"
export MAKEFLAGS="-j$(nproc)"
```

### 第 5 步：尽可能避免 vendored OpenSSL（能省掉分钟级编译）

安装系统 OpenSSL 开发包：

```bash
sudo apt-get update && sudo apt-get install -y pkg-config libssl-dev
```

尝试禁用 vendored：

```bash
export OPENSSL_NO_VENDOR=1
```

如果生效，可减少 `openssl-src` 源码编译带来的分钟级成本。

### 第 6 步：降低链接时间（可选）

如果你发现“链接阶段”在日常增量迭代里占比很高，可安装更快的 linker（`mold`/`lld`）并通过 `RUSTFLAGS` 或 `.cargo/config.toml` 配置。

建议顺序：优先做完第 1–3 步，再评估是否有必要。

### 第 7 步：Windows Defender 排除项（可选，但在 Windows 上常见）

如果满足以下任一情况，实时扫描可能拖慢构建：

- 仓库仍在 Windows 文件系统上
- `CARGO_TARGET_DIR` 指向 Windows-backed 路径
- 你通过 `\\wsl$\...` 访问/编译这些文件

建议：排除项尽量只覆盖构建产物与缓存目录（`target/`、Cargo registry/git cache、sccache cache），不要粗暴排除整盘；并定期复审。

## 外部参考链接

- WSL 文件系统与性能建议（在 `/mnt/c` 上用 Linux 工具构建项目通常更慢）：
  - https://learn.microsoft.com/windows/wsl/filesystems
- Cargo 配置参考（`build.target-dir`、`rustflags` 等）：
  - https://doc.rust-lang.org/cargo/reference/config.html
- `openssl-sys` 的 vendoring 相关（`OPENSSL_NO_VENDOR` 等）：
  - https://github.com/sfackler/rust-openssl
- `sccache` 使用说明：
  - https://github.com/mozilla/sccache
- 快速 linker：`mold`（以及 Cargo 配置 linker 的模式）：
  - https://github.com/rui314/mold
- WSL2 全局 VM 配置（`%UserProfile%\.wslconfig`）：
  - https://learn.microsoft.com/windows/wsl/wsl-config

## 每步完成后的验证清单

1) 重新生成 timings（便于前后对比）：

```bash
export CARGO_TARGET_DIR="$HOME/.cache/moltis-target"
cargo +nightly-2025-11-30 build -Z unstable-options -p moltis --timings=html
```

2) 测量 no-op rebuild（第二次应接近“秒级”）：

```bash
time cargo build -p moltis
time cargo build -p moltis
```

3) 如果 `openssl-sys` / `aws-lc-sys` / `llama-cpp-sys-2` 仍在反复重编，优先排查：

- `target/` 是否仍在 `/mnt/c`
- 每次启动 shell 时环境变量是否一致
- 是否有脚本/习惯性操作触发 `cargo clean` 或清理 `target/`

## 超出本文范围（需要改代码才能获得的结构性收益）

这些往往是“最大结构性胜利”，但需要改 workspace manifest / feature 设计：

- 让 `llama-cpp-*` 不再出现在默认 dev build 路径（仅在需要时显式开启）。
- 让浏览器能力（`moltis-browser` / `chromiumoxide_cdp`）变为可选 feature。
- 重新评估本地开发是否必须默认启用 vendored OpenSSL（确保 CI/发布与本地的可控差异）。

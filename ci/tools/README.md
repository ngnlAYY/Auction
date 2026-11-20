# 构建工具

[docs/README.md](../../docs/README.md) · [ci/README.md](../README.md) · [docs/development.md](../../docs/development.md)

开发、CI 与发布用到的工具由 [mise](https://mise.jdx.dev) 统一安装，全部取上游发布的原始版本：仓库不携带工具补丁，也不维护工具的独立依赖锁。

| 文件 | 内容 |
| --- | --- |
| [mise.toml](../../mise.toml) | 工具版本、构建选项与命令入口（任务）；版本只在这里维护 |
| [mise.lock](../../mise.lock) | 各工具在 Linux x86_64 上的下载地址与校验和，由 `mise lock` 生成，不手工编辑 |
| [rust-toolchain.toml](../../rust-toolchain.toml) | Rust 的版本、组件与编译目标；mise 读取该文件，不在 `mise.toml` 重复 |
| `ci/tools/rustup.sh` | 安装满足证书修复基线的 rustup |
| `ci/tools/blaze.sh` | 本地 Blaze 编译试验入口（`mise run blaze`） |

## 安装与使用

```bash
mise trust           # 首次：确认信任本仓库的 mise.toml
mise install         # 按 mise.toml 与 mise.lock 安装或补齐全部工具
mise tasks           # 列出命令入口
mise run verify      # 运行某个入口
```

- mise 的最低版本见 `mise.toml` 的 `min_version`。
- `mise run <任务>` 总在这些工具的环境里执行，不要求 shell 先激活 mise。直接调用 `cargo`、`rg` 等命令时，先激活 mise（`mise activate`）或写成 `mise exec -- <命令>`。
- 首次 `mise install` 会从源码编译三个 Cargo 工具（其中 `dx` 最慢），需要 C 编译器、CMake、`pkg-config` 与 OpenSSL 开发文件。
- 只支持 Linux x86_64，锁文件只记录这一个平台。
- PostgreSQL 18、Perl、brotli、bash 与 git 由系统包提供，不经 mise。
- `mise run test-wasm` 使用本机已安装的 Firefox。用例范围见[开发文档的测试入口](../../docs/development.md#测试)。

## 工具清单

| 工具 | 来源 | 用途 |
| --- | --- | --- |
| Rust | rustup，按 `rust-toolchain.toml` | 编译、Clippy、rustfmt 与 wasm32 目标 |
| `rg`（ripgrep） | GitHub 发布包 | 策略门禁的文本检索 |
| `sccache` | GitHub 发布包 | 发布构建的编译缓存 |
| `cargo-audit` | crates.io 源码，按发布包自带的 `Cargo.lock` 构建 | `dependency-audit`（联网读取 RustSec） |
| `cargo-machete` | 同上 | `dependency-unused` |
| `dx`（dioxus-cli） | 同上；关闭默认 feature，启用 `disable-telemetry` 与 `no-downloads` | 页面构建 |
| `wasm-bindgen`、`wasm-bindgen-test-runner` | 同一个 GitHub 发布包 | 分别供 `dx` 和 `mise run test-wasm` 使用。发布包版本必须与工作区 `Cargo.toml` 的 `wasm-bindgen` 依赖同步升级 |
| `geckodriver` | GitHub 发布包 | `mise run test-wasm` 用它驱动本机 Firefox |
| `wasm-opt`（binaryen） | GitHub 发布包 | `dx` 的配套工具 |
| `esbuild` | npm 上的 `@esbuild/linux-x64` 原生二进制包 | `dx` 的配套工具，运行时不需要 Node |
| `bun` | GitHub 发布包 | `web/` 下 TypeScript 实现的依赖安装、类型检查、Biome、测试与构建（依赖版本由 `web/bun.lock` 固定）；`mise run web-prepare` 安装依赖，`mise run verify` 的 `web` 项运行类型检查、Biome 与包边界，`mise run web-test` 运行行为测试；包边界由 `web/tooling/check-boundaries.ts` 使用 `web/` 的 TypeScript 编译器检查；`bun audit` 供 `web-dependency-audit` 审计 JS 依赖。Rust 版页面不经过 bun；Solid 发布构建直接调用本地 `web/node_modules/.bin/vite`，不自动安装依赖 |

升级工具：改 `mise.toml` 里的版本，运行 `mise install` 与 `mise lock`，把两个文件一并提交。`mise.lock` 也记录 Rust 的版本、组件与目标，所以改 `rust-toolchain.toml` 后同样要重新 `mise lock`；CI 以 `--locked` 安装，锁文件与请求不一致时直接失败。升级 mise 自身时，同步 `mise.toml` 的 `min_version` 与工作流里 `jdx/mise-action` 的 `version`。

## 发布构建如何取用工具

发布入口（`ci/release/release.sh`、`ci/release/web-bundle.sh`）经 [ci/release/build-identity.sh](../release/build-identity.sh) 选择工具：

- 用 `mise which` 解析原生 Rust 编译工具 `sccache`。有 Dioxus 页面就解析 `dx`、`wasm-bindgen`、`wasm-opt`、`esbuild`，有 Solid 页面就解析 `bun`。每个路径都必须位于 `mise where` 返回的对应安装目录下。固定版本缺失时直接失败。
- 把这些路径放到 `PATH` 前部并设置 `NO_DOWNLOADS=1`，`dx` 不会在构建时自行下载工具；同时把 `DX_HOME` 指到 `target/dx-home`，`dx` 因此不会改用它在用户目录里缓存过的旧工具，也不读取用户级 `dx` 设置。
- 这些二进制的 SHA-256 构成工具身份，参与构建环境指纹；`mise.lock` 参与构建输入指纹。同版本工具重装后字节变化也会让已有制品失效。
- `sccache` 只在环境变量显式设置 `RUSTC_WRAPPER=sccache` 时启用，并换成 mise 安装的版本；Cargo 配置文件里的包装器一律清空。
- 任一 surface 使用 Solid 时，工具闭集加入 mise 提供的 `bun`（版本与二进制摘要），发布路径直接调用 `web/node_modules/.bin/vite`，不执行 `bun x` 或临时下载。依赖只能经 `mise run web-prepare` 安装；该任务把 `web/bun.lock` 的 SHA-256 与 bun 版本写入 `web/node_modules/.auction-prepared`，`web-bundle.sh` 的 Solid 构建阶段核对该凭据而不扫描安装树，不联网。
- `build-release`、`build-single`、`release-check`、`package`、`verify-artifacts`、`verify-single` 与 `run-public`/`run-admin` 都经同一份逐 surface 技术栈选择：构建与门禁任务用 `web_cargo_features` 得出 cargo feature，运行任务经 `release.sh run-web public|admin` 先得到 feature；Solid surface 还从 `WEB-BUILD-INFO` 读取 `build_id` 并以 `AUCTION_BUILD_ID` 传给编译。正式发行（`GITHUB_ACTIONS=true` 且 `GITHUB_REF_TYPE=tag`）在任一端为 Solid 时拒绝。

## 依赖审计的范围

CI 的 `security-audit` 同时审计应用的 `Cargo.lock`（默认分支与最近一个发布 tag）与 `web/bun.lock`（`bun audit`）。JS 审计需要联网读取 npm 公告源，离线或镜像不可达时失败关闭，不当作没有漏洞。工具自身的依赖由各上游发布包的 `Cargo.lock` 或预编译包决定，仓库不再逐项处理其中的公告；需要时升级到上游修复后的版本。

应用的直接依赖有两层检查：Cargo 内置的 `unused_dependencies` 与 `unused_workspace_dependencies`（`Cargo.toml` 的 `[workspace.lints.cargo]`，设为 `deny`）在每次编译时按实际编译的构型报错——普通依赖在 `check`/Clippy 时、开发依赖在编译测试时；CI 的 `dependency-unused`（`cargo machete`）按清单文本检查，覆盖没被编译到的构型。上游分别要求不同版本的传递依赖时，不为减少锁文件条目而跨不兼容版本替换。

## rustup.sh

```bash
bash ci/tools/rustup.sh
```

已安装的 rustup 版本不低于 1.29.3 时直接退出；否则在排除已失信 CA 的临时信任集合下下载固定版本的 `rustup-init`，校验 SHA-256 后安装。不改变仓库固定的工具链（由 `rust-toolchain.toml` 决定）。mise 通过 rustup 安装 Rust，CI 在 mise 之前先执行本脚本。

## blaze.sh

```bash
mise run blaze status
mise run blaze build -p auction-cli
mise run blaze test -p auction-domain --lib
mise run blaze stop
```

调用机器上已安装的 `blaze` 做本地原生编译试验：强制 `--locked`，拒绝 `--release`、`--profile`、`--target`、`--target-dir` 与 `--config` 覆盖；产物、缓存与 daemon 运行目录都在 `target/blaze/` 下。它不属于 `mise run verify`、Web 打包或发布流程，结果仍须以固定工具链的 Cargo 门禁为准。

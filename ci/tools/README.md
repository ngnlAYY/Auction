# 构建工具

[docs/README.md](../../docs/README.md) · [ci/README.md](../README.md) · [docs/development.md](../../docs/development.md)

发布与审计用到的工具（Dioxus CLI、sccache、wasm-bindgen、wasm-opt、cargo-audit）不使用机器上任意版本，而是由本目录的脚本从固定来源、固定版本、独立锁文件构建或下载，并写入安装回执；发布入口在每次运行前校验回执。

| 文件 | 内容 |
| --- | --- |
| `ci/tools/bootstrap.sh` | 受控工具的安装、校验与身份 |
| `ci/tools/rustup.sh` | 安装满足证书修复基线的 rustup |
| `ci/tools/blaze.sh` | 本地 Blaze 编译试验入口（`just blaze`） |
| `ci/tools/locks/*.Cargo.lock` | 各工具的独立依赖锁：`dioxus-cli`、`sccache`、`cargo-audit` |
| `ci/tools/patches/*.patch` | 对工具源码的补丁：dioxus-cli 受控工具与禁止下载、sccache 的 zstd/缓存权限/object feature |

## bootstrap.sh

```bash
bash ci/tools/bootstrap.sh install [release|audit]    # 安装（默认 release）
bash ci/tools/bootstrap.sh verify  [release|audit]    # 只校验回执
bash ci/tools/bootstrap.sh fingerprint [release|audit] # 输出配方指纹
bash ci/tools/bootstrap.sh identity release           # 校验回执后输出工具身份（用于 sccache daemon 隔离）
```

| 档位 | 安装目录 | 工具 |
| --- | --- | --- |
| `release` | `target/build-tools/bin` | `dx`（dioxus-cli）、`sccache`、`wasm-bindgen`、`wasm-opt`（binaryen）、`esbuild` |
| `audit` | `target/audit-tools/bin` | `cargo-audit` |

- 各工具的版本、下载来源与 SHA-256 只在 `ci/tools/bootstrap.sh` 中维护；`wasm-bindgen` 与工作区 `Cargo.toml` 的同名依赖同步升级。
- crate 源码从 `static.crates.io` 下载，wasm-bindgen 与 binaryen 预构建二进制从 GitHub release 下载，esbuild 从 npm registry 下载，全部以固定 SHA-256 校验；构建使用仓库内的独立锁文件（`cmp` 确认未变化）并应用补丁。
- TLS 依赖（rustls、rustls-webpki、webpki-roots/root-certs）必须达到已审阅的版本基线，否则拒绝构建。
- 安装先写入临时目录，完成后原子替换；回执记录配方指纹（脚本、锁文件、补丁、工具链、Cargo 配置）与每个二进制的摘要。配方变化后旧安装校验失败，需要重新安装。
- 首次从源码构建需要 C 编译器、CMake、pkg-config、OpenSSL 开发文件与 `patch`。
- 只支持 Linux x86_64。

发布入口（`ci/release/release.sh`、`ci/release/web-bundle.sh`）会把 `target/build-tools/bin` 放到 `PATH` 前部并设置 `NO_DOWNLOADS=1`，dx 不会在构建时自行下载工具。

## 工具锁已知公告

CI 的 `security-audit` 只审计应用 `Cargo.lock`（默认分支与最近一个发布 tag），不审计 `ci/tools/locks/` 下的工具锁。手动复核：`bash ci/tools/bootstrap.sh install audit` 后把 `target/audit-tools/bin` 放到 `PATH` 前部，对每个锁文件执行 `cargo audit --file <锁文件> --deny warnings`。

下表是受控 cargo-audit 0.23.0 对公告库提交 `02e2517f457a359fc6d3440ea4560c8ea188fee8`（2026-11-15）的复核结果，按包版本计数，包含锁图中的可选及其他平台依赖，不代表每项都能由实际构建路径触发。

| 锁文件 | 漏洞匹配 | 警告 | 结果 |
| --- | --- | --- | --- |
| `ci/tools/locks/cargo-audit.Cargo.lock` | 0 | 0 | 通过 |
| `ci/tools/locks/dioxus-cli.Cargo.lock` | 15 | 11 | 未通过 |
| `ci/tools/locks/sccache.Cargo.lock` | 5 | 10 | 未通过 |

Dioxus CLI 的 `libz-sys` 已从 1.1.28 单项升级到 1.1.31，消除 [RUSTSEC-2026-0418](https://rustsec.org/advisories/RUSTSEC-2026-0418.html) 匹配；其他依赖版本不变。原锁中的 `cc 1.2.61` 未满足公告所述 `cc >= 1.6.0` 触发条件，该升级不能作为既有缓存已泄露环境变量的证据。

三把锁的 `quinn-proto` 都单项升级到 0.11.19、`quinn-udp` 升级到 0.5.16，消除 [RUSTSEC-2026-0523](https://rustsec.org/advisories/RUSTSEC-2026-0523.html)、[RUSTSEC-2026-0524](https://rustsec.org/advisories/RUSTSEC-2026-0524.html) 与 [RUSTSEC-2026-0538](https://rustsec.org/advisories/RUSTSEC-2026-0538.html) 匹配；cargo-audit 与 Dioxus CLI 的锁另把 `rustix` 1.1.x 升级到 1.1.6，消除健全性警告 [RUSTSEC-2026-0486](https://rustsec.org/advisories/RUSTSEC-2026-0486.html)。这些升级都在原版本的 semver 兼容范围内；Dioxus CLI 锁中 `hyper-util`、`quinn`、`quinn-udp` 随之改用锁内已有的 `socket2 0.6.3`，不新增包版本。

剩余漏洞涉及 Dioxus CLI 的 `gix-date`、两个版本的 `quick-xml`、`rkyv`、`ruzstd` 和两个版本的 `zip`，以及 sccache 的 `rsa` 与 `zip`；`zip` 的匹配含新公布的 [RUSTSEC-2026-0429](https://rustsec.org/advisories/RUSTSEC-2026-0429.html)。已公布修复版都跨越当前依赖的兼容范围（如 `zip` 9.0.0、`rsa` 0.10.0）；不为消除匹配而强行跨版本替换或忽略公告。警告另含未维护（sccache 锁新增 `pin-utils`，[RUSTSEC-2026-0441](https://rustsec.org/advisories/RUSTSEC-2026-0441.html)）、健全性问题，以及 sccache 锁中的已撤回 `scc 2.1.0`。工具安装回执通过只证明来源与配方一致，不表示上述安全审计通过。

应用的直接依赖由 CI 的 `dependency-unused`（`cargo machete`）检查。上游分别要求不同版本的传递依赖时，不为减少锁文件条目而跨不兼容版本替换。

## rustup.sh

```bash
bash ci/tools/rustup.sh
```

已安装的 rustup 版本不低于 1.29.3 时直接退出；否则在排除已失信 CA 的临时信任集合下下载固定版本的 `rustup-init`，校验 SHA-256 后安装。不改变仓库固定的工具链（由 `rust-toolchain.toml` 决定）。

## blaze.sh

```bash
just blaze status
just blaze build -p auction-cli
just blaze test -p auction-domain --lib
just blaze stop
```

调用机器上已安装的 `blaze` 做本地原生编译试验：强制 `--locked`，拒绝 `--release`、`--profile`、`--target`、`--target-dir` 与 `--config` 覆盖；产物、缓存与 daemon 运行目录都在 `target/blaze/` 下。它不属于 `just verify`、Web 打包或发布流程，结果仍须以固定工具链的 Cargo 门禁为准。

# CI 与发布

[文档导航](../docs/README.md) · [开发指南](../docs/development.md) · [构建工具](tools/README.md) · [运维手册](../ops/README.md)

| 目录 | 内容 |
| --- | --- |
| [`ci/checks/`](checks) | 静态门禁：[`ci/checks/check.sh`](checks/check.sh)（策略、格式、编译、lint、审计）与 [`ci/checks/equipment-contract.sh`](checks/equipment-contract.sh) |
| [`ci/release/`](release) | 发布入口 [`ci/release/release.sh`](release/release.sh)、Web payload 构建 [`ci/release/web-bundle.sh`](release/web-bundle.sh)、共用身份函数 [`ci/release/build-identity.sh`](release/build-identity.sh) |
| [`ci/tools/`](tools) | 受控工具链与构建工具的安装、校验与本地 Blaze 试验入口 |

发布工具面向 Linux x86_64；装包还需 Perl（重定位文档链接）。CI 的 Cargo 编译、Clippy 与依赖图解析命令使用 `--locked`。格式检查和独立审计工具使用各自的参数。

## 静态门禁：`ci/checks/check.sh`

```bash
bash ci/checks/check.sh <检查项>...
```

| 检查项 | 内容 |
| --- | --- |
| `repository` | `AGENTS.md`、`.gitignore`、`.editorconfig`、`README.md`、`Cargo.lock` 必须纳入版本控制且未被忽略 |
| `layout` | `src/` 根只能有 `lib.rs`/`main.rs`；每个 crate 的 `src/` 必须有职责子目录；同目录不得有 ≥3 个同前缀源码文件；文件与目录名 snake_case |
| `config` | `ops/config/examples/**/*.toml` 每个赋值字段（含注释掉的示例字段）前必须紧邻含“可选”或“必填”的注释 |
| `i18n` | [`crates/auction-web-public/i18n/`](../crates/auction-web-public/i18n) 与 [`crates/auction-web-admin/i18n/`](../crates/auction-web-admin/i18n) 恰好包含 en、ja、ko、zh-CN、zh-TW 五个 `.ftl`；各语言键集合与 `en.ftl` 一致；代码中 `t!("…")` 引用的键必须存在；键名不得含点号、不得重复 |
| `equipment-contract` | 装备解析诊断原因在 Rust、SQL 约束与 Admin 文案之间的闭集一致 |
| `all` | 以上五项 |
| `fmt` | `cargo fmt --all -- --check` |
| `check` | `cargo check --locked --workspace --all-targets` |
| `feature` | `auction-storage`（仅 `postgres`）、`auction-cache`（仅 `redis`），以及 `auction-setup` 的空默认图与 `postgres` 完整图 |
| `lint` | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` |
| `wasm` | 两个前端编译到 `wasm32-unknown-unknown` |
| `rust` | `fmt check feature lint wasm` |
| `dependency-audit` | `cargo audit --deny warnings`（需要受控 cargo-audit，联网读取 RustSec） |
| `dependency-unused` | `cargo machete`（0.9.2） |

`just verify` = `all fmt check feature lint wasm`。默认门禁不运行测试。

## GitHub Actions：`.github/workflows/ci.yml`

| 作业 | 触发 | 内容 |
| --- | --- | --- |
| `docs` | 所有 push / PR / 手动 | 策略门禁与格式（`ci/checks/check.sh all fmt`）；同一提交已有 PR 时跳过重复的分支 push |
| `verify` | `docs` 通过后 | `ci/checks/check.sh check feature lint wasm`（工具链 1.99.0） |
| `security-audit` | `docs` 通过后 | 受控 cargo-audit 的 `dependency-audit` 与 `dependency-unused` |
| `msrv` | `docs` 通过后 | `cargo +1.94.1 check --locked --workspace --all-targets --all-features` |
| `artifacts` | 默认分支 push 或手动 | 安装受控发布工具，`ci/release/release.sh release-check` 构建并校验完整发布包，上传候选包（保留 1 天） |
| `publish` | 以上全部通过 | 复核候选包 SHA-256，上传正式制品（保留 14 天） |

缓存：Rust 编译缓存按作业与工具链隔离，PR 不写缓存；发布产物与 `target/dx` 以源码身份指纹为键单独缓存；受控工具以配方（bootstrap 脚本、锁文件、补丁、工具链）为键缓存，命中后仍以安装回执逐字节校验。

发布构建开始前，使用与发布入口相同的受控 sccache 和 daemon 地址执行 `sccache rustc -vV`。缓存服务启动失败（例如 GitHub 缓存存储返回 `503 ServerBusy`）时，步骤发出 warning 并清空 `RUSTC_WRAPPER`，由 rustc 直接完成后续构建；实际编译错误与发布门禁失败仍使作业失败。受控工具回执校验在探测前执行，校验失败直接终止。sccache 0.18.0 的 `SCCACHE_IGNORE_SERVER_IO_ERROR` 不覆盖启动失败，因此不能替代此探测。

## 发布：`ci/release/release.sh`

```bash
bash ci/release/release.sh release-check [--single-binary] [--allow-dirty]   # 完整构建 + 门禁 + 装包
bash ci/release/release.sh package [--single-binary] [--allow-dirty]         # 只装配已构建制品
bash ci/release/release.sh build-api | build-embedded | verify-artifacts
bash ci/release/release.sh build-single | verify-single
bash ci/release/release.sh source-fingerprint
```

对应 Justfile：`just build-api`、`just build-release`、`just build-single`、`just verify-artifacts`、`just release-check`、`just package`。

### 制品变体

| 变体 | feature | 程序 | 用途 |
| --- | --- | --- | --- |
| API-only | `postgres` | Public、Admin | 不发布；证明未启用 `web-embed` 时二进制不含任何页面（资源目录固定为空，页面路由 503） |
| embedded | `postgres,cache-redis` + 两端 `web-embed` | Public、Admin、Worker、auction-cli | 分体部署发布包 |
| single | `postgres,cache-redis` + `auction-server/web-embed` | auction-server、auction-cli | 合并部署发布包（`-single` 后缀） |

### 门禁

- **页面 payload**：先用 `ci/release/web-bundle.sh` 构建两端 payload，校验入口页标题（“Auction 公开站点” / “Auction 管理站点”）、跨端隔离、只含普通文件与唯一一组 JS/WASM、payload 清单与哈希。目录遍历或摘要生成失败即拒绝复用，不把失败留下的部分输出当作有效清单。
- **二进制标记**：Public 只含 Public payload 的标记与哈希，Admin 只含 Admin 的，Worker 与 auction-cli 不含任何页面标记；API-only 二进制含“资源缺失”提示而不含 payload。
- **依赖边界**（`cargo tree`）：Worker 不依赖任何前端 crate；Public 不依赖 Admin 前端；Admin 不依赖 Public 前端；`auction-server` 不依赖 `auction-cli`。
- **BUILD-INFO 与校验和**：每个制品目录记录源码身份、构建输入与环境指纹、feature 和文件摘要；工具链经受控工具身份绑定。复用判定比较这些指纹、锁文件、feature 和制品摘要；仅仓库提交或不在构建输入范围内的文档变化，不必重新编译。
- **装包**：文件白名单与权限固定（程序 755、文档 644、配置示例 640、敏感示例 600），禁止符号链接、日志与真实敏感文件名；归档确定性生成（固定排序、mtime、owner），解包复核后输出 `SHA256SUMS` 与 `DEPENDENCIES.md`。正式发布要求干净工作树（`--allow-dirty` 只用于本地证据，包名带 `-dirty`）。

发布包名为 `auction-rust-<版本>-<提交前 12 位>[-single][-dirty].tar.gz`，内容见[运维手册](../ops/README.md#部署形态)。仓库 Markdown 链接相对各文档目录；装包时按同一文件安装清单重定位全部已打包文档（含 `config/README.md`），保留标题锚点，并映射敏感模板的 `.toml.example` 文件名。已打包的目标使用包内相对链接；未打包的源码、其他变体模板和文档链接到本次源码提交的 GitHub 页面，需要联网浏览。

### 构建 profile

| profile | 设置 |
| --- | --- |
| `release`（后端） | `opt-level = 3`、`lto = "fat"`、`codegen-units = 1`、`panic = "unwind"`、不启用增量；不使用 `target-cpu=native` |
| `wasm-release`（前端） | 继承 release，`opt-level = "z"`、`panic = "abort"` |
| `dev` | `debug = "line-tables-only"`（需要完整调试信息时设 `CARGO_PROFILE_DEV_DEBUG=2`） |

### Web payload：`ci/release/web-bundle.sh`

```bash
bash ci/release/web-bundle.sh public
bash ci/release/web-bundle.sh admin
```

以 `wasm-release` profile 运行受控 `dx bundle`，复制到稳定目录 `target/dx/build/<包名>/web/public`（后端 `web-embed` 从这里内嵌），写入 payload 清单与哈希。复用要求本 surface、共享层、契约、工具配方及构建环境指纹一致，且现有 payload 再次通过完整门禁；仅比较 `dx` 与 `rustc` 版本不足以证明可复用。

### 构建身份：`ci/release/build-identity.sh`

`ci/release/release.sh` 与 `ci/release/web-bundle.sh` 共用：选择受控工具（`target/build-tools/bin` 优先、`NO_DOWNLOADS=1`）、计算源码身份指纹（已跟踪与未忽略的新文件、删除与 mode 变化）、读取 BUILD-INFO、校验 Web payload。未通过环境变量指定时，显式清空 Cargo 配置中的两类 rustc 包装器；环境变量指定的 `sccache` 替换为受控版本，其他显式包装器保留并计入构建环境指纹。

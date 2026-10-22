# 开发与静态验证

[文档导航](docs/README.md) · [协作规范](AGENTS.md) · [部署](ops/README.md) · [CI](ci/README.md)

## 工具与依赖来源

准备 Linux x86_64 GNU/glibc 主机、just、Bash、ripgrep（`rg`）、Perl、rustfmt、Clippy 与
`wasm32-unknown-unknown` target；布局、配置及五语言检查需要 `rg`/Perl，CI 的 docs 前置门禁会显式安装，
页面与发布入口另需[受控构建工具](ci/tools/README.md)。首次源码安装这些工具还需 C 编译器、CMake、pkg-config、
OpenSSL 开发文件、patch 和 curl/tar/coreutils。版本、MSRV 和 profile 见[技术栈](docs/design/tech-stack.md)，缓存边界见
[CI 说明](ci/README.md#runner-依赖与缓存)。检查/发布使用 `--locked`，不手工清理条件平台或未启用可选依赖的
锁记录。不设全局 `build.target`，保留 `target/release` 路径；浏览器显式选 WASM target，通用制品禁用
`target-cpu=native`。

## 默认检查

```bash
just verify
```

分项顺序见 [Justfile](Justfile)，实现及 CI/MSRV 范围见[CI 说明](ci/README.md)。`cargo check --all-targets` 只编译
已有目标；`equipment-contract` 只读核对装备诊断原因的 Rust/SQL/Admin/译文闭集，纳入默认静态门禁。
文档链接、锚点、导航和包 README 由人工核对。未经明确要求，不新增或运行单元、集成、功能、端到端、
浏览器测试及手工冒烟，保留已有测试。依赖审计独立联网获取 RustSec，不启动产品或修改锁图：

```bash
bash ci/tools/rustup.sh
bash ci/tools/bootstrap.sh install audit
export PATH="$PWD/target/audit-tools/bin:$PATH"
cargo install cargo-machete --locked --version 0.9.2
bash ci/checks/check.sh dependency-audit dependency-unused
```

漏洞及全部警告均阻断，不设置公告忽略项；cargo-machete 检查未用直接依赖，不代替 feature、可达性或许可评审。
历史结果见[状态页](docs/status.md)。

## 选择制品 feature

| 目标 | feature | 条件 |
| --- | --- | --- |
| PostgreSQL | postgres | 唯一运行后端；先准备配置与迁移 |
| Redis | cache-redis | 配置选择 Redis 并提供敏感 URL |
| 内嵌页面 | Public/Admin 的 web-embed | 先构建对应 payload |
| API-only | 不启用 web-embed | 页面路由 503，API 仍可使用 |
| 合并 supervisor | server 的 postgres；首次向导另需 web-embed | Linux 单主机，auctionctl 独立 |

apps/* 和 auction-storage 默认 feature 为空；Justfile 的 `run-*`、`db-migrate` 显式选择 `postgres`，
Public/Admin 运行配方另启用 `web-embed`。可部署发布配方固定 `postgres,cache-redis` 与双端 `web-embed`，
API-only 对照制品只启用 `postgres`；Redis 仍是可丢弃缓存，连接失败按回源处理。例如
`cargo check --locked -p auction-public-api --features postgres` 只编译所选路径。

## 配置准备与显式运行

按[配置手册](ops/config/README.md)准备公共、角色和私有敏感 TOML 并核对相对路径与权限；以下检查只输出安全投影、不
连接数据库，示例占位值必须先替换为有效私有配置：

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auctionctl -- check-config public
```

建库、停机迁移及分体/合并启动按[部署手册](ops/README.md)操作，本页不授予启动、迁移或功能验证授权；普通启动不隐式
迁移，例外仅限网页确认后的专用空库初始化与严格停机门禁后的部署迁移。

## 浏览器与发布构建

```bash
bash ci/tools/rustup.sh
bash ci/tools/bootstrap.sh install release
just web-build

# 干净工作树上的完整分体归档。
just release-check

# 可选合并服务，auctionctl 仍独立。
just build-single
just release-check --single-binary
```

`web-build` 与 `ci/release/release.sh` 的所有入口（包括 `build-api`、`verify-artifacts` 和 `source-fingerprint`）
先验证 release 工具回执，不在发布期间安装工具；默认 `just verify` 仍只需 Rust 静态检查工具。入口固定受控 PATH，
dx 禁止自动下载配套工具，历史 dx 下载缓存也不参与选择。

Web 使用 `wasm-release`；dx 的 esbuild 仅供构建，不增加 Node/npm 运行链。`build-*` 变体按 `BUILD-INFO` 判定复用，
构建输入（apps、crates、迁移、配置示例、保留策略、清单与发布工具）、`Cargo.lock`、feature、工具二进制身份与构建环境完全一致且制品完整时直接复用，
仅修改这些范围外的文档不会重跑原生 Fat LTO；Web payload 按 surface、共享层、契约、工具链版本及同一构建环境摘要独立复用，
修改 `CARGO_PROFILE_WASM_RELEASE_*` 或 rustflags 会重新构建页面。本机可选用
`RUSTC_WRAPPER=sccache SCCACHE_DIR=target/sccache`（sccache 0.18.0）与
`RUSTFLAGS="-C link-arg=-fuse-ld=lld"`（lld 链接）前缀。发布入口将 sccache 解析为受控二进制，并按用户、工作区和工具身份
选择独立 Unix socket，避免连接旧 daemon；不停止其他构建的进程。这些前缀不改变 feature、资源隔离或归档契约。复用判定、指纹实现、
资源隔离与 `--allow-dirty` 限制归[CI 说明](ci/README.md#发布边界)及[发布手册](ops/README.md#发布制品)；
[AMD KVM 候选](ops/build/README.md)仅叠加目标白名单/并发，本机微基准不能替代目标机或 PostgreSQL 并发验证。

## 常见边界

| 现象 | 优先核对 |
| --- | --- |
| 配置数据库但驱动不可用 | 二进制是否启用 postgres feature |
| API 可用、页面 503 | 对应 payload 与 web-embed |
| runtime 保存但未生效 | restart_required 与受影响角色重启 |
| 配置示例被拒绝 | 占位凭据、权限、相对路径；不得放松 loader |

## 文档维护

文档按唯一 owner 维护，不为同一事实另建说明页、笔记目录或技能模板；职责划分与导航见
[docs/README](docs/README.md)，模块说明见各自 README，系统与技术取舍见[架构](docs/design/architecture.md)与
[技术栈](docs/design/tech-stack.md)。实现事实核对源码、Cargo.lock、迁移和脚本；用户与 AGENTS.md 的规范不得因实现
缺口被静默放宽。版本描述仓库固定值，不写成永久有效的“最新 stable”；命令的 feature、配置、权限与破坏性确认须匹配
真实入口。先明确 owner、能力档位及不变量，再修改领域/端口/DTO、适配器与 HTTP/Web/Worker；不兼容 wire 变更新增版本，
迁移只追加。文档没有自动检查器：链接一律写成仓库根相对路径，移动、重命名或新增页面时人工核对链接、标题锚点和根
README 的可达入口，并确认 Cargo 包仍带 README；敏感运行证据留在受控目录，历史日期与证据边界不得改写，
边界见[CI 说明](ci/README.md#文档边界)。

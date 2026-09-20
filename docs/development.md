# 开发与静态验证

[文档导航](README.md) · [协作规范](../AGENTS.md) · [部署手册](../ops/deploy/README.md)

## 本页导航

- [工具与依赖来源](#工具与依赖来源)
- [默认检查](#默认检查)
- [选择制品 feature](#选择制品-feature)
- [配置准备与显式运行](#配置准备与显式运行)
- [浏览器与发布构建](#浏览器与发布构建)
- [常见边界](#常见边界)

## 工具与依赖来源

| 工具或输入 | 本仓库要求 | 核对入口 |
| --- | --- | --- |
| Rust | 工具链固定 `1.98.0`，edition 2024，MSRV `1.94.1` | [工具链](../rust-toolchain.toml)、[workspace](../Cargo.toml) |
| Rust 组件 | rustfmt、Clippy、`wasm32-unknown-unknown` | [工具链](../rust-toolchain.toml) |
| Rust 依赖 | 检查与发布使用 `--locked` | [Cargo.lock](../Cargo.lock) |
| 命令编排 | `just`；CI/构建脚本使用 Bash 与各脚本声明的系统工具 | [Justfile](../Justfile)、[CI 说明](../ci/README.md) |
| 浏览器构建 | Dioxus `dx`，只在构建/开发页面时需要 | [Web 构建脚本](../ci/web-bundle.sh) |

这里的版本描述仓库配置，不声称是上游此刻最新版本。依赖选择和升级边界见[技术栈](tech-stack.md)。

## 默认检查

```bash
just fmt
just docs-check
just repository-policy-check
just layout-check
just config-example-check
just i18n-check
just check
just feature-check
just lint
just wasm-check

# 串行执行以上静态门禁。
just verify
```

`cargo check --all-targets` 编译已有目标，不运行测试。`docs-check` 检查文档链接、导航覆盖和笔记结构，
不证明业务描述正确。静态命令的实现和 CI/MSRV 差异见 [ci/README.md](../ci/README.md)。
单元、集成、功能、端到端、浏览器测试与手工冒烟默认暂缓；已有测试不删除。

## 选择制品 feature

| 目标 | Cargo feature | 附加条件 |
| --- | --- | --- |
| PostgreSQL 完整档 | `postgres` | 使用 PostgreSQL 配置和迁移树 |
| SQLite Lite | `sqlite` | 单主机、每角色一个实例、Worker 并发 1 |
| Redis | 在目标进程增加 `cache-redis` | 配置选择 Redis，敏感文件提供 URL |
| 内嵌页面 | Public/Admin 增加 `web-embed` | 先构建对应前端产物 |
| API-only | 不启用 `web-embed` | 静态页面缺失明确返回 503 |

`apps/*` 与 `auction-storage` 的默认 feature 均为空。仓库的 `run-public`/`run-admin`
配方只增加 `web-embed`，`run-worker` 不选择数据库驱动；连接数据库的本地运行应使用下面的显式命令。
发布流程另由 `just build-api`/`just build-release` 固定选择 PostgreSQL。

```bash
cargo check --locked -p auction-public-api --features sqlite
cargo check --locked -p auction-admin-api --features sqlite
cargo check --locked -p auction-worker --features sqlite
cargo check --locked -p auctionctl --features sqlite
```

## 配置准备与显式运行

先按[配置说明](../ops/config/README.md)准备角色、公共和真实敏感 TOML，确保路径及权限满足 loader。
下面的示例路径仍指向仓库角色模板；执行前须让模板引用有效的私有敏感文件。
命令只作为操作说明，本节不授权自动启动或功能验证。

```bash
# 配置诊断只输出安全投影，不连接数据库。
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auctionctl -- check-config public

# API-only 的 SQLite Lite 进程；数据库应已通过独立迁移流程准备。
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auction-public-api --features sqlite
AUCTION_CONFIG_FILE=ops/config/examples/admin.toml cargo run --locked -p auction-admin-api --features sqlite
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml cargo run --locked -p auction-worker --features sqlite
```

普通启动不隐式迁移。合并入口只有首次网页明确确认后才初始化专用空库，详情见
[auction-server](../apps/server/README.md)；`auctionctl migrate` 使用 systemd 总控参数、状态目录锁和三角色停机门禁；
不应把它描述为无需环境准备的便捷 seed 命令。初始 PostgreSQL 建库与迁移分离，详见
[auctionctl 命令手册](../apps/auctionctl/README.md)。

## 浏览器与发布构建

```bash
# 安装与仓库 Dioxus 版本配套的 CLI。
cargo install dioxus-cli --version 0.7.10 --locked
just web-build

# 已构建页面后的单端 Lite 本地启动示例。
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auction-public-api --features sqlite,web-embed

# 完整档制品、隔离门禁、干净工作树上的发布归档。
just build-api
just build-release
just verify-artifacts
just release-check

# 可选单文件部署，auctionctl 仍独立。
just build-single
just release-check --single-binary
```

Web 构建使用 `wasm-release` profile，清理对应 surface 中间输出后复制到稳定嵌入目录并生成 payload 清单。
dx 可下载 esbuild 作为构建工具；项目不增加 Node 运行时、npm 依赖或第二套前端构建链。
正式归档要求干净工作树，源码身份在构建前冻结并在各阶段重查。
`--allow-dirty` 只用于显式本地取证，不能据此发布正式制品；细节见[发布与部署](../ops/deploy/README.md)。

## 常见边界

| 现象 | 优先核对 |
| --- | --- |
| 数据库已配置但驱动不可用 | 制品是否显式启用与 URL scheme 对应的 feature |
| API 可用而页面 503 | 对应 Web payload 是否已构建，二进制是否启用 `web-embed` |
| Lite 接口返回 501 | [能力矩阵](architecture.md#部署档位与能力)是否将该操作列为完整档 |
| runtime 保存成功但参数未生效 | 按 `restart_required` 重启受影响的 Admin/Worker |
| 配置校验拒绝示例 | 替换占位凭据、核对文件权限与相对路径，不能放松 loader |

# 开发与静态验证

[文档导航](README.md) · [协作规范](../AGENTS.md) · [部署](../ops/deploy/README.md) · [CI](../ci/README.md)

## 工具与依赖来源

| 输入 | 仓库要求 | 核对入口 |
| --- | --- | --- |
| Rust | 固定 1.98.0，edition 2024；MSRV 1.94.1 | [工具链](../rust-toolchain.toml)、[workspace](../Cargo.toml) |
| 组件 | rustfmt、Clippy、wasm32-unknown-unknown | 工具链文件 |
| 依赖 | 检查/发布使用 --locked | [Cargo.lock](../Cargo.lock) |
| 编排 | just，脚本使用 Bash 和其声明的系统工具 | [Justfile](../Justfile)、[CI 说明](../ci/README.md) |
| 页面构建 | 配套 Dioxus dx CLI，仅构建页面时需要 | [web-bundle.sh](../ci/web-bundle.sh) |

版本描述仓库配置，不声称上游最新；取舍和升级约束见[技术栈](tech-stack.md)。

## 默认检查

```bash
just verify
```

等价分项为 `just fmt`、`docs-check`、`repository-policy-check`、`layout-check`、`config-example-check`、
`i18n-check`、`check`、`feature-check`、`lint`、`wasm-check`，verify 按配方顺序执行。
按变更选择必要分项；完整交付按协作规范运行门禁。cargo check --all-targets 只编译已有目标。
文档检查核验链接/锚点/导航/包 README，不证明业务描述正确；实现与 CI/MSRV 差异见 CI 说明。

未经明确要求，不新增或运行单元、集成、功能、端到端、浏览器测试与手工冒烟；已有测试保留。
依赖审计独立联网获取 RustSec，不启动产品或修改锁图：

```bash
cargo install cargo-audit --locked --version 0.22.2
just dependency-audit
```

CI security-audit 执行同一检查；漏洞、撤回版本、unsound 阻断，停维提示保留。
[依赖升级记录](tech-stack.md#版本与升级)登记 fxhash 停维债的移除；审计退出码不能混同于无风险证明。

## 选择制品 feature

| 目标 | feature | 条件 |
| --- | --- | --- |
| PostgreSQL 完整档 | postgres | PostgreSQL 配置和迁移树 |
| SQLite Lite | sqlite | 本地单主机、每角色一个实例、Worker 并发 1 |
| Redis | cache-redis | 配置选择 Redis 并提供敏感 URL |
| 内嵌页面 | Public/Admin 的 web-embed | 先构建对应 payload |
| API-only | 不启用 web-embed | 页面路由 503，API 仍可使用 |
| 合并 supervisor | server 的 postgres；首次向导另需 web-embed | 仅 Linux/PostgreSQL 单主机，auctionctl 独立 |

apps/* 和 auction-storage 的默认 feature 为空。run-public/run-admin 只增加 web-embed，run-worker
不选驱动；连接数据库须显式选 feature。发布配方 build-api/build-release 固定 PostgreSQL。
例如 `cargo check --locked -p auction-public-api --features sqlite` 只编译所选路径。

## 配置准备与显式运行

按[配置手册](../ops/config/README.md)准备公共、角色和真实敏感 TOML，替换示例占位值并核对相对路径/权限。
以下命令指向示例角色文件，须先使其引用有效私有配置；运行说明不授予自动启动或功能验证授权。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auctionctl -- check-config public

AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auction-public-api --features sqlite
AUCTION_CONFIG_FILE=ops/config/examples/admin.toml cargo run --locked -p auction-admin-api --features sqlite
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml cargo run --locked -p auction-worker --features sqlite
```

check-config 只输出安全投影、不连接数据库；角色运行前数据库须已迁移。
迁移与建库按[CLI 手册](../apps/auctionctl/README.md#停机迁移与初始建库)准备，不是无前提 seed：

- 分体 migrate 使用 systemd 状态锁与三角色严格停机门禁；deployment start/restart 在同一门禁后迁移再启动。
- 合并部署停止整个 server 后，以同一账号 migrate-managed 并取得既有 supervisor/子进程共享锁。
- 角色及合并服务普通启动不隐式迁移；合并入口只有首次网页明确确认后才初始化专用空库。

server 使用 --config-dir，若 AUCTION_CONFIG_FILE 已设置即拒绝；完整流程见[server 手册](../apps/server/README.md)。

## 浏览器与发布构建

```bash
cargo install dioxus-cli --version 0.7.10 --locked
just web-build

# 已构建对应页面后的 Lite 本地启动。
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auction-public-api --features sqlite,web-embed

# 干净工作树上的完整分体归档；含构建、资源隔离和哈希核验。
just release-check

# 可选合并服务，auctionctl 仍独立。
just build-single
just release-check --single-binary
```

Web 使用 wasm-release，清理对应 surface 中间输出后复制至稳定内嵌目录并生成 payload 清单。
dx 可下载 esbuild 作为允许的构建工具，不增加 Node 运行时、npm 依赖或第二套前端链。
分步构建、正式干净树要求和 --allow-dirty 的取证限制见[发布手册](../ops/deploy/README.md#发布制品)。
[AMD KVM 候选](../ops/build/README.md)仅用于显式目标白名单，先独立构建 Web，不改默认 release/WASM；
本机微基准不能替代目标机、PostgreSQL 并发或代表性 PGO 负载。

## 常见边界

| 现象 | 优先核对 |
| --- | --- |
| 配置数据库但驱动不可用 | 二进制是否选择对应 postgres/sqlite feature |
| API 可用、页面 503 | 对应 payload 与 web-embed |
| Lite 接口 501 | [能力矩阵](architecture.md#部署档位与能力)是否支持 |
| runtime 保存但未生效 | restart_required 与受影响角色重启 |
| 配置示例被拒绝 | 占位凭据、权限、相对路径；不得放松 loader |

# Auction Rust

这是 Auction-BF 的全 Rust 重构起点。当前目录是从零建立的 Cargo 工作区；相邻的
`Auction-BF` 仓库只作为旧系统的业务、HTTP、部署和数据行为参考，不作为 Rust
crate 依赖，也不在本仓库的默认修改范围内。

## 设计目标

- 保留 Public、Admin、Worker 三个可独立部署的运行边界。
- 让竞拍规则、状态迁移、幂等和结算不依赖 HTTP、数据库或前端框架。
- 公开端和管理端的 Web UI 也使用 Rust：Dioxus 客户端渲染，产物内嵌进后端二进制。
- 界面原生支持中（简/繁）、英、日、韩五种语言；公开端默认英文，管理端默认简体中文。
- 将论坛、MoogleMail、数据库、日志和配置视为可替换适配器。
- Redis 缓存是可选适配器，默认不编译、不启用；关闭或故障时回退到权威数据源。
- PostgreSQL 提供完整生产能力；SQLite 作为单主机、低并发 Lite 替代品，只实现明确列入
  Lite 范围的功能。两个后端已支持的竞价写入都必须保证事务、幂等和审计一致性。
- 保留旧版本拍卖页面的视觉语言，并在 Rust Web 层逐步恢复完整交互和业务数据。

## 当前状态

当前是可编译的启动骨架，已经具备工作区、进程装配、健康检查、初始领域生命周期、
TOML 配置、PostgreSQL 完整/SQLite Lite 存储边界，以及沿用旧版视觉结构的拍卖页面静态
示例。Auction-BF 的完整功能、数据表、接口契约和外部服务行为仍在迁移中，不能仅凭编译
通过宣称功能等价。

“全 Rust”只约束本仓库实现及其程序库依赖。PostgreSQL、SQLite、可选 Redis、反向代理
等外部组件可以使用各自的官方实现，不要求它们由 Rust 编写。

除非用户明确要求，本项目暂缓新增或运行单元测试、集成测试、功能测试用例、端到端测试
和浏览器测试。已有测试文件保留，不因本规则删除。

## 目录约定

```text
apps/                         # 薄进程入口，负责装配依赖和管理生命周期
├── public-api/               # 公开端 API 与公开 Web
├── admin-api/                # 管理端 API 与管理 Web
├── worker/                   # 拍卖生命周期、论坛和通知任务
└── auctionctl/               # 数据库、配置和运维命令行工具
crates/                       # 可复用能力，按模块所有权拆分
├── auction-domain/           # 纯领域模型与不变量
├── auction-application/      # 用例、端口和事务边界
├── auction-contracts/        # HTTP/WebSocket DTO 与错误契约
├── auction-http/             # Axum 路由与 HTTP 表面适配器
├── auction-web-shared/       # 浏览器端共享设计变量、基础组件与语言选择
├── auction-web-public/       # 公开端 Dioxus 界面（14 条路由，默认英文）
├── auction-web-admin/        # 管理端 Dioxus 界面（21 条路由，默认简体中文）
├── auction-storage/          # PostgreSQL 完整适配器与 SQLite Lite 适配器
├── auction-cache/            # 默认关闭的缓存边界与可选 Redis 适配器
├── auction-integrations/     # E-Hentai、MoogleMail 等外部网关
├── auction-jobs/             # Worker 任务编排
├── auction-config/           # TOML 解析与运行配置
└── auction-observability/    # tracing、metrics 和健康检查约定
docs/                         # 架构、技术选型、兼容性和迁移记录
migrations/                   # PostgreSQL 完整 schema 与 SQLite Lite 子集的追加式迁移
ops/                          # 配置样例、部署和恢复文档
```

这种结构借鉴 Honk 的 Cargo 工作区、`crates/*` 能力拆分、根目录工程约定和独立文档/运
维目录，但不会照搬 Honk 的代理业务模块。

## 本地开发

需要固定的 Rust 1.98.0 当前稳定工具链和 `wasm32-unknown-unknown` 编译目标；workspace
MSRV 为 1.94.1。Rust 官方没有 LTS 且只支持最新 stable；Tokio 使用官方 `1.51.x` LTS，
其他无 LTS 的 crate 由 `Cargo.lock` 固定并受控升级。运行时配置统一使用 TOML；
`AUCTION_CONFIG_FILE` 只用于选择主配置文件，不用于覆盖单个配置项。主配置通过
`secrets_file` 引用独立敏感 TOML；数据库连接串、Redis 连接串、Cookie 和会话签名密钥
只放在该敏感文件中。

构建浏览器端产物还需要 dx CLI（`cargo install dioxus-cli --locked`）。dx 会按需从 npm
注册表下载 esbuild 作为构建期工具，它不是运行时依赖，也不引入 Node 运行时或第二套前端
构建链。后端使用 `allow_missing` 内嵌产物，因此未构建前端时后端仍可编译，只是静态资源
路由返回 503。

```bash
# 使用 SQLite Lite 示例配置（本地开发/单节点低并发）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api

# 构建两个 surface 的前端产物（需要 dx CLI）
just web-build

# 完整生产档在不提交的敏感文件中使用 postgres://... 连接串
just verify
```

受版本控制的 `secrets.toml` 只含安全占位值。实际部署应把它复制为被 Git 忽略的
`*.secret.toml`，限制文件权限，并让主配置的 `secrets_file` 指向该文件。Redis 还需要以
`cache-redis` feature 构建对应进程；仅填写 `redis_url` 不会把 Redis 依赖带入默认制品。

`just verify` 默认只执行格式化、锁定依赖的编译检查、Clippy 和 WASM 静态检查，不执行
测试。
Auction-BF 的行为迁移顺序记录在 [`docs/migration-plan.md`](docs/migration-plan.md)。

# Auction Rust

这是 Auction-BF 的全 Rust 重构起点。当前目录是从零建立的 Cargo 工作区；相邻的
`Auction-BF` 仓库只作为旧系统的业务、HTTP、部署和数据行为参考，不作为 Rust
crate 依赖，也不在本仓库的默认修改范围内。

## 设计目标

- 保留 Public、Admin、Worker 三个可独立部署的运行边界。
- 让竞拍规则、状态迁移、幂等和结算不依赖 HTTP、数据库或前端框架。
- 公开端和管理端的 Web UI 也使用 Rust：Leptos 服务端渲染与 WASM 水合。
- 将论坛、MoogleMail、数据库、日志和配置视为可替换适配器。
- 同时支持 PostgreSQL 和 SQLite；通过后端对应的事务、锁、唯一约束和审计事件保证
  竞价事实的一致性。
- 保留旧版本拍卖页面的视觉语言，并在 Rust Web 层逐步恢复完整交互和业务数据。

## 当前状态

当前是可编译的启动骨架，已经具备工作区、进程装配、健康检查、初始领域生命周期、
TOML 配置、SQLite/PostgreSQL 存储边界，以及沿用旧版视觉结构的拍卖页面静态示例。
Auction-BF 的完整功能、数据表、接口契约和外部服务行为仍在迁移中，不能仅凭编译通过
宣称功能等价。

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
├── auction-web/              # Leptos 公开端/管理端界面
├── auction-storage/          # PostgreSQL/SQLite/SQLx 适配器
├── auction-integrations/     # E-Hentai、MoogleMail 等外部网关
├── auction-jobs/             # Worker 任务编排
├── auction-config/           # TOML 解析与运行配置
└── auction-observability/    # tracing、metrics 和健康检查约定
docs/                         # 架构、技术选型、兼容性和迁移记录
migrations/                   # PostgreSQL/SQLite 分数据库方言的追加式迁移
ops/                          # 配置样例、部署和恢复文档
```

这种结构借鉴 Honk 的 Cargo 工作区、`crates/*` 能力拆分、根目录工程约定和独立文档/运
维目录，但不会照搬 Honk 的代理业务模块。

## 本地开发

需要 Rust 1.98、`wasm32-unknown-unknown` 编译目标和 Cargo Leptos。运行时配置统一使用
TOML；`AUCTION_CONFIG_FILE` 只用于选择配置文件，不用于覆盖单个配置项。

```bash
# 使用 SQLite 示例配置（本地开发/单节点）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api

# 使用 PostgreSQL 时，将示例 TOML 中的 database_url 改为 postgres://...
just verify
```

`just verify` 默认只执行格式化、编译检查和 Clippy 静态检查，不执行测试。
Auction-BF 的行为迁移顺序记录在 [`docs/migration-plan.md`](docs/migration-plan.md)。

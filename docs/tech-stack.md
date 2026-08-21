# 技术栈决策

本文把“可以选什么”和“本项目先选什么”分开。版本号以 workspace 根部
[`Cargo.toml`](../Cargo.toml) 为准，不在各 crate 中重复声明。

## 运行时与后端 HTTP

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| Tokio + Axum | Tower 生态、组合式 middleware、WebSocket/SSE 和 Leptos 官方集成 | 需要自行组织应用层和依赖注入 |
| Tokio + Actix Web | 性能成熟、功能完整、Leptos 也有官方集成 | 与 Tower 生态分离，组合边界不如 Axum 统一 |
| Poem/Rocket | API 体验直接，适合中小型服务 | 本项目需要的生态和团队经验较少 |

**选择：Tokio + Axum 0.8。** 现有系统有 public/admin 两个 HTTP surface、实时更新和
大量横切 middleware；Axum 的 `Router`/Tower 组合适合把这些能力放在适配器层，领域层
不会被框架类型污染。

## 浏览器端

| 方案 | 特点 | 适用情况 |
| --- | --- | --- |
| Leptos | Rust 组件、细粒度响应式、SSR 与 WASM hydration、Axum 集成 | 本项目要同时覆盖公开端和管理后台 |
| Dioxus | Rust UI，可扩展桌面/移动端 | 未来要做原生客户端时更有吸引力 |
| Yew | 成熟的 Rust/WASM SPA | 只需要 CSR，不强调 SSR 时可选 |
| Askama + HTMX | 服务端模板简单、浏览器端负担低 | 交互很少的后台；不适合复杂实时竞价 UI |

**选择：Leptos 0.8 + `leptos_axum`。** Public/Admin 使用同一套 Rust UI 基础设施，
通过 surface 模块隔离路由与权限；首屏采用 SSR，交互通过 hydration，竞价状态通过
REST 命令和 WebSocket/SSE 更新。项目不引入 React、TypeScript 或 Node 构建链；CSS
作为静态资源保留，不属于业务运行时。

## 数据库与查询

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| SQLx + PostgreSQL | 显式 SQL、查询可编译期校验、事务/行锁清晰 | 需要维护 SQL 与 migration |
| SQLx + SQLite | 内嵌文件数据库、零服务依赖、适合本地与快速测试 | 单写者/文件锁语义不同于 PostgreSQL |
| SeaORM + PostgreSQL/SQLite | Entity/ActiveModel 生产力高、关系查询方便 | 复杂竞价事务仍会落到 raw SQL；抽象层较重 |
| Diesel + PostgreSQL/SQLite | 类型安全和同步事务模型成熟 | 异步服务集成与动态查询较繁琐 |

**选择：SQLx 0.9 + PostgreSQL/SQLite 双后端。** TOML 的 `database_url` scheme
选择实际数据库：`postgres://`、`postgresql://` 或 `sqlite:`。Auction 的关键写入是
“锁定 lot → 验证资格 → 计算竞价 → 写入事实/审计/事件”的原子事务，显式 SQL 比 ORM
隐藏事务边界更容易审查。

两种数据库共用 application ports 和 repository 契约，但不强行共用所有 SQL：

- PostgreSQL 是多进程生产部署的推荐后端，使用行锁和完整隔离语义。
- SQLite 支持本地开发、快速测试和单节点部署；写入采用明确的串行事务策略。
- `migrations/postgres/` 与 `migrations/sqlite/` 分开维护同一逻辑 schema 的 dialect 版本。
- 不使用 `sqlx::Any` 隐藏后端差异；锁、事务、查询和 migration 必须在 storage adapter
  中可审查。

SQLx migration 与 `query!`/`query_as!` 用于把各后端的 schema、参数和返回列纳入构建检查。

## API、认证与外部服务

- **API：** 先保持 REST + JSON，与 Auction-BF 的 route/status/field 兼容；实时状态
  使用 WebSocket，无法保持长连接的客户端使用 SSE。GraphQL 不作为第一版依赖。
- **Session：** HttpOnly、Secure、SameSite Cookie；session store 跟随选定数据库，
  多进程生产默认 PostgreSQL，单节点开发可使用 SQLite。后续有明确吞吐需求再把 store
  迁移到 Redis。密码使用 Argon2id，密钥用系统环境或 secret manager 注入，不写入配置文件。
- **HTTP client：** `reqwest` + rustls，用于 E-Hentai 和 MoogleMail 适配器；所有外部
  调用都有 timeout、重试上限、幂等 key 和审计日志。
- **配置：** `serde` + TOML 配置文件；`AUCTION_CONFIG_FILE` 只用于选择配置文件，
  不通过环境变量覆盖单个配置字段，也不在 domain/application 中读取环境变量。
- **错误：** domain/application 使用 `thiserror`；只在 binary composition root 使用
  `anyhow` 汇总启动错误。
- **日志/指标：** `tracing` + `tracing-subscriber`，指标接口预留 Prometheus；日志中
  禁止 cookie、密码、论坛 session 和完整邮件正文。
- **契约文档：** REST 稳定后用 `utoipa` 生成 OpenAPI；它不反向决定领域模型。

## 测试与交付

- 单元测试：按当前要求暂缓，不新增或扩展单测覆盖。
- 静态门禁：`cargo fmt`、`cargo check` 和 `cargo clippy -D warnings`。
- 后续测试恢复后：repository 契约测试同时跑 SQLite 与 PostgreSQL；SQLite 覆盖文件/内存
  数据库路径，PostgreSQL 容器/临时实例额外覆盖行锁、隔离和并发竞价语义。
- 浏览器：Leptos SSR 快照 + Playwright 端到端；生产构建由 `cargo-leptos` 负责。
- 发布：通过静态门禁后生成 Public/Admin/Worker/CLI 制品和 migration checksum。

# 技术栈决策

本文把“可以选什么”和“本项目先选什么”分开。版本号以 workspace 根部
[`Cargo.toml`](../Cargo.toml) 为准，不在各 crate 中重复声明。

## 版本支持策略

“使用 LTS”按各上游实际提供的支持通道执行，不能把较旧版本自行命名为 LTS：

- Tokio 固定在官方 LTS 的 `1.51.x` 分支，只接收该分支的补丁版本。
- Rust 官方没有 LTS 通道，而且旧 stable 会在下一版发布后 EOL。构建与发布固定使用当前
  受支持的 stable `1.98.0`；`1.94.1` 只作为 SQLx 0.9 对应的 workspace MSRV，并由独立
  CI job 校验，不能把 MSRV 当作 LTS。Rust 新 stable 发布后应及时评审升级，安全公告不等
  常规窗口。
- Axum、Leptos、SQLx、reqwest、redis 等 crate 也没有官方 LTS 通道。项目选择仍受维护的稳定
  版本，由 [`Cargo.lock`](../Cargo.lock) 固定完整解析结果；升级按单个组件执行，并同时检查
  Rust MSRV、Leptos SSR/WASM 和数据库适配器。
- PostgreSQL 不使用 LTS 标签，而是每个 major 官方维护 5 年。完整生产档固定 major 18，
  始终使用该 major 的当前修复版本（截至 2026-08-21 为 18.4），major 升级单独评审。
  SQLite 是长期维护项目；Lite 档使用 SQLx 锁图捆绑的引擎版本，当前为 3.51.3，不依赖
  可能陈旧的系统动态库。
- 本地与 CI 的编译、Clippy 和 WASM 检查都使用 `--locked`。禁止在发布构建中隐式刷新
  依赖图，也不以一次性批量 `cargo update` 代替升级评审。

上游依据：Tokio 的
[bug patching policy](https://github.com/tokio-rs/tokio#bug-patching-policy)、Rust 的
[release channel 与维护周期](https://doc.rust-lang.org/book/appendix-07-nightly-rust.html)、
SQLx 0.9 的 [MSRV 说明](https://docs.rs/crate/sqlx/latest/source/CHANGELOG.md)。
数据库依据：PostgreSQL 的 [5 年版本支持策略](https://www.postgresql.org/support/versioning/)
与 SQLite 的 [长期支持说明](https://www.sqlite.org/lts.html)。

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
| Askama + HTMX + SSE | 服务端 HTML、无 hydration、无需 Node 构建链 | 公开竞价页和普通 Admin 很合适；复杂客户端状态较弱，且会引入 JavaScript |

**选择：Leptos 0.8 + `leptos_axum`。** Public/Admin 使用同一套 Rust UI 基础设施，
通过 surface 模块隔离路由与权限；首屏采用 SSR，交互通过 hydration，竞价状态通过
REST 命令和 WebSocket/SSE 更新。项目不引入 React、TypeScript 或 Node 构建链；CSS
作为静态资源保留，不属于业务运行时。

这个选择来自“浏览器端也必须使用 Rust”的现有项目契约，不代表 Leptos 是生产复杂度
最低的方案。若以后放宽“禁止 JavaScript”，本项目优先验证的替代方案是
**Axum + Askama + HTMX + SSE**：出价仍走显式 REST 命令，SSE 只通知状态变化，浏览器再
获取权威快照或 HTML fragment。它可以移除 native/WASM 双目标和 hydration mismatch，
但会改变当前全 Rust 契约，未经用户明确决定不得直接切换。

保留 Leptos 时必须收敛其影响范围：

- 核心竞价、认证和管理命令保持显式、版本化的 REST/JSON 契约，不依赖 Leptos server
  functions 的隐式协议。
- 公开页面默认使用 SSE 接收变化通知，事件带 sequence/version；跳号或重连后重新读取
  权威快照。只有确实需要双向协议时才启用 WebSocket。
- 优先评估 Islands 或等价的局部 hydration，只让倒计时、出价和实时面板进入 WASM；
  静态内容不必整页水合。
- `auction-web` 只能依赖 contracts/application 表面，不能让 Leptos 类型进入领域、存储
  或外部 API 契约，确保以后可以替换前端。

## 数据库与查询

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| SQLx + PostgreSQL | 显式 SQL、查询可编译期校验、事务/行锁清晰 | 需要维护 SQL 与 migration |
| SQLx + SQLite | 内嵌文件数据库、零服务依赖、适合 Lite 部署 | 单写者/文件锁语义不同，不适合高并发或高可用 |
| SeaORM + PostgreSQL/SQLite | Entity/ActiveModel 生产力高、关系查询方便 | 复杂竞价事务仍会落到 raw SQL；抽象层较重 |
| Diesel + PostgreSQL/SQLite | 类型安全和同步事务模型成熟 | 异步服务集成与动态查询较繁琐 |

**选择：SQLx 0.9 + PostgreSQL 完整后端 + SQLite Lite 后端。** 敏感 TOML 中的
`database_url` scheme 选择实际数据库：`postgres://`、`postgresql://` 或 `sqlite:`。
Auction 的关键写入是“锁定 lot → 验证资格 → 计算竞价 → 写入事实/审计/事件”的原子
事务，显式 SQL 比 ORM 隐藏事务边界更容易审查。

PostgreSQL 是标准生产档，负责完整功能、多进程部署、并发写入和后续高可用能力。SQLite
是轻量替代档，面向本地开发、演示和明确的单主机、单实例、低并发部署；不承诺与
PostgreSQL 在吞吐、锁粒度、多副本、故障转移、在线迁移或全部系统功能上对等。

两种数据库只共享稳定的领域语义和 SQLite Lite 已实现能力所需的 application ports：

- PostgreSQL 18.x 使用行锁和完整的生产隔离策略，是生产 schema 与持久化能力的主基线。
- SQLite 写入采用明确的串行事务策略，不支持多副本或网络文件系统上的共享数据库；
  Worker 并发必须保持为一，并在部署说明中记录文件锁和备份假设。
- Lite 已提供的竞价写入仍必须保证 `command_id` 幂等、事务原子性、审计追加和凭据安全；
  “轻量”只缩减系统能力，不能放宽已声明支持功能的数据正确性。
- `migrations/postgres/` 维护完整生产 schema；`migrations/sqlite/` 只维护 Lite 能力需要的
  子集。两个目录不要求迁移文件或数据库对象一一对应，共有对象必须保持领域含义一致。
- 不使用 `sqlx::Any` 隐藏后端差异；锁、事务、查询、能力缺失和 migration 都必须在
  storage adapter 或部署文档中显式可审查。
- `auction-storage` 默认不带数据库驱动；`postgres` 与 `sqlite` Cargo feature 分别启用
  对应 SQLx 驱动。发布制品只打开实际后端，CI 分别检查两条路径并以 `--all-features`
  检查组合图。

SQLx migration 与 `query!`/`query_as!` 用于把每个已实现后端路径的 schema、参数和返回列
纳入构建检查。SQLite 不作为 PostgreSQL 行锁、隔离或并发行为的替代验证环境。

## 可选缓存

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| 不启用缓存 | 依赖最少，状态路径最直接 | 每次读取权威数据源 |
| Redis | 跨进程共享、TTL 和失效机制成熟 | 增加外部服务与失效一致性成本 |
| 进程内缓存 | 部署简单、读取快 | 多进程不一致、每进程重复占用内存 |

**选择：默认关闭，按需启用 Redis。** `auction-cache` 在无 feature 时只提供 disabled
实现；目标进程启用 `cache-redis` 后才编译 `redis` crate 1.6.0。运行时还必须把
`cache.backend` 设为 `redis`，并从敏感 TOML 读取 `redis_url`。客户端自身提供异步复用
连接管理，不再叠加通用连接池；当前也不引入 Moka 等进程内缓存库。

缓存只用于带有限 TTL、可从数据库或外部权威服务重建的查询结果。缓存 miss、连接失败或
失效失败必须回退；竞价事务、资格、命令账本、锁、会话真值和审计均不得依赖缓存。Redis
是否启用因此只影响性能，不改变可用功能的正确性。

## API、认证与外部服务

- **API：** 先保持 REST + JSON，与 Auction-BF 的 route/status/field 兼容；实时状态
  使用 WebSocket，无法保持长连接的客户端使用 SSE。GraphQL 不作为第一版依赖。
- **Session：** HttpOnly、Secure、SameSite Cookie；完整生产档的 session store 使用
  PostgreSQL，Lite 档可在对应能力实现后使用 SQLite。Redis 缓存不能成为会话权威存储。
  密码使用 Argon2id；会话签名密钥只写入权限受限且不提交的敏感 TOML。
- **HTTP client：** `reqwest` + rustls，用于 E-Hentai 和 MoogleMail 适配器；所有外部
  调用都有 timeout、重试上限、幂等 key 和审计日志。
- **配置：** `serde` + TOML。主配置保存普通字段，并用 `secrets_file` 引用敏感 TOML；
  `database_url`、`redis_url`、外部 Cookie 和会话签名密钥只允许出现在敏感文件。旧的
  单文件敏感字段不兼容。`AUCTION_CONFIG_FILE` 只选择主配置，不通过环境变量覆盖单个
  配置字段，也不在 domain/application 中读取环境变量。
- **错误：** domain/application 使用 `thiserror`；只在 binary composition root 使用
  `anyhow` 汇总启动错误。
- **日志/指标：** `tracing` + `tracing-subscriber`，指标接口预留 Prometheus；日志中
  禁止 cookie、密码、论坛 session 和完整邮件正文。
- **契约文档：** REST 稳定后用 `utoipa` 生成 OpenAPI；它不反向决定领域模型。

## 依赖精简原则

- 全 Rust 只约束本仓库实现及其程序库依赖；PostgreSQL、SQLite、Redis、反向代理等外部
  组件无需由 Rust 实现，避免为语言统一重复建设基础设施。
- 应用端口使用 Rust 原生 async trait 返回类型，不再直接依赖 `async-trait` 宏；Leptos
  上游仍会传递引入该宏。HTTP 客户端与 SQLx 统一使用 rustls/ring TLS provider，避免
  同时链接多个 TLS provider。
- SQLx 数据库驱动、Redis 和 WASM 调试 panic hook 都由窄 feature 控制。默认构建不携带
  备用数据库、缓存客户端或仅调试依赖。
- 不额外直接依赖框架已经传递提供且项目未使用的 `tower-http`、通用 Redis 连接池、ORM、
  GraphQL、进程内缓存或第二套前端工具链。确有用例后再按 owner 边界引入。
- 保留 Axum、Leptos、SQLx、reqwest、serde、tracing 等承担明确边界价值的基础库；不为
  单纯减少 crate 数量手写 HTTP、TLS、数据库协议、序列化或加密实现。

## 测试与交付

- 单元测试：按当前要求暂缓，不新增或扩展单测覆盖。
- 静态门禁：`cargo fmt`、带 `--locked` 的 `cargo check`、WASM 编译检查和
  `cargo clippy -D warnings`。
- 后续测试恢复后：PostgreSQL 验证完整 repository 契约、行锁、隔离和并发竞价语义；
  SQLite 只验证 Lite 能力契约及文件数据库路径，不要求运行 PostgreSQL 专属用例。
- 浏览器：Leptos SSR 快照 + Playwright 端到端；生产构建由 `cargo-leptos` 负责。
- 发布：通过静态门禁后生成 Public/Admin/Worker/CLI 制品和 migration checksum。

## 已知未消除风险

- Rust 1.98.0 会为 Leptos 0.8 依赖链中的 `proc-macro-error2 2.0.1` 报告
  `future-incompatibility`：它当前仍能编译，但某个未来 Rust 版本会把相关重导出变成硬
  错误（Rust [#127909](https://github.com/rust-lang/rust/issues/127909)）。截至 2026-08-21，
  crates.io 没有更新版本可直接替换。继续锁定当前版本并跟踪 Leptos/上游修复；在升级到
  会拒绝该代码的 Rust 前，必须升级依赖或评审最小补丁，不能静默忽略。
- SQLite Lite 的能力表目前是架构约束，不代表 repository、migration、单 Worker 并发和
  部署防护都已实现。每项功能迁移时仍需显式标注档位并完成对应适配器。
- PostgreSQL、SQLite 与 Redis feature 目前可以组合编译，方便静态检查，但正式制品仍需
  在发布配置中明确只启用实际使用的数据库驱动及缓存能力；否则依赖裁剪不会反映到二进制。
- 按当前项目规则，行为测试仍暂缓。因此现阶段验证只能证明格式、编译、Clippy、MSRV 和
  WASM 构建成立，不能证明拍卖行为等价或 PostgreSQL/SQLite 事务语义正确。

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
- Axum、Dioxus、SQLx、reqwest、redis 等 crate 也没有官方 LTS 通道。项目选择仍受维护的稳定
  版本，由 [`Cargo.lock`](../Cargo.lock) 固定完整解析结果；升级按单个组件执行，并同时检查
  Rust MSRV、浏览器端 WASM 构建和数据库适配器。
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
| Tokio + Axum | Tower 生态、组合式 middleware、WebSocket/SSE 支持 | 需要自行组织应用层和依赖注入 |
| Tokio + Actix Web | 性能成熟、功能完整 | 与 Tower 生态分离，组合边界不如 Axum 统一 |
| Poem/Rocket | API 体验直接，适合中小型服务 | 本项目需要的生态和团队经验较少 |

**选择：Tokio + Axum 0.8。** 现有系统有 public/admin 两个 HTTP surface、实时更新和
大量横切 middleware；Axum 的 `Router`/Tower 组合适合把这些能力放在适配器层，领域层
不会被框架类型污染。连接装配直接使用 Axum 同版本依赖图中的 Hyper/`hyper-util`：仅用于配置
5 秒 HTTP/1 请求头读取超时、HTTP/2 WebSocket CONNECT 和连接级优雅排空，不建立第二套路由器。

Public/Admin 的监听配置属于装配根，不进入领域或应用层：主配置的 `[listener]` 支持 TCP 和
Linux Unix socket，Admin 的 runtime listener 可覆盖主配置 fallback。两种模式都经过
`auction-http` 的共享绑定器执行目录、单实例锁、stale socket、owner/group 和权限校验；Worker
control socket 是独立传输面，不能与 HTTP listener 复用。Unix socket 只适用于明确的单主机
反向代理部署，跨主机或非 Linux 部署使用 TCP。

## 浏览器端

| 方案 | 特点 | 适用情况 |
| --- | --- | --- |
| Dioxus | Rust 组件、RSX、CSR/SSR 可选，可扩展桌面/移动端 | 本项目要同时覆盖公开端和管理后台 |
| Leptos | Rust 组件、细粒度响应式、SSR 与 WASM hydration | 技术架构本身很契合高频价格表 |
| Yew | 成熟的 Rust/WASM SPA | 生态配套滞后，`yew-router` 落后核心版本 |
| Askama + HTMX + SSE | 服务端 HTML、无 hydration、无需 Node 构建链 | 服务端 CRUD 很合适；复杂客户端状态较弱，且会引入 JavaScript |

**选择：Dioxus 0.7.10 + 纯 CSR SPA。** Public/Admin 使用同一套 Rust UI 基础设施，通过
独立 crate 隔离组件树与路由树；产物是静态 WASM/JS/HTML，由 `auction-http` 内嵌后提供。
项目不引入 React、TypeScript 或 Node 运行时；CSS 作为静态资源保留，不属于业务运行时。

不使用 SSR，理由是旧系统本身没有 SSR：`../Auction-BF/apps/web/vite.public.config.ts` 与
`vite.admin.config.ts` 均无 `ssr`/`prerender` 配置，`manifests/*.routes.json` 是 SPA
fallback 路由表。引入 SSR 会新增 hydration mismatch 和双目标构建，却不替换任何现有能力。
同理不启用 `dioxus-fullstack`/`dioxus-server`：后者自带 axum 服务器，会与 `apps/*` 装配根
和 `auction-http` 的路由所有权冲突。

从 Leptos 切换到 Dioxus 的直接原因是 Leptos 0.8 线存在未修复的 XSS sink，详见
「已知未消除风险」。此外 Leptos SSR 为后端贴现了约 175 个依赖包，而当前浏览器端以
CSR 加载真实 API 投影，不需要 SSR 引入的额外依赖。

保留以下约束不变：

- 核心竞价、认证和管理命令保持显式、版本化的 REST/JSON 契约。
- 公开页面实时更新使用 WebSocket，与旧系统 `internal/features/realtime/hub.go` 的既有
  契约一致；事件带 sequence/version，跳号或重连后重新读取权威快照。
- 浏览器 crate 只能依赖 contracts 表面，不得让 UI 类型进入领域、存储或外部 API 契约，
  确保以后可以替换前端。
- Public 与 Admin 的组件树、路由树和静态产物必须完全分离，公开端二进制不得包含管理端资源；
  两种 `RustEmbed` 类型分别由 Public/Admin 装配根拥有，`auction-http` 仅实现泛型静态响应，
  因此部署制品不依赖链接器裁剪即可形成资源隔离。

## 国际化

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| dioxus-i18n + Fluent | 与 Dioxus 信号集成、支持复数与性别等语法、译文编译期内嵌 | 生态较小，`set_language` 在键缺失时 panic |
| rust-i18n | 下载量高、宏驱动、JSON/YAML 译文 | 与 Dioxus 响应式集成需自行搭建 |
| 自建 `HashMap` 查表 | 无额外依赖 | 复数、插值、回退链全部要自己实现 |

**选择：`dioxus-i18n` 0.5.1 + `unic-langid` 0.9.6。** 它要求 `dioxus ^0.7`，与当前版本
匹配，且语言状态本身是 Dioxus 信号，切换语言即触发重渲染，不需要额外的状态桥接。

受支持语言沿用旧系统的 5 种：`en`、`zh-CN`、`zh-TW`、`ja`、`ko`。公开端默认 `en`；管理端
默认 `zh-CN`，与旧 `manifests/admin.locales.json` 的 `default_locale` 一致。中文按脚本区分
简繁，两者不互相回退。

工程约定：

- 译文以 `include_str!` 在编译期内嵌，首屏不额外请求语言资源。
- 两个 surface 的 `i18n/*.ftl` 彼此隔离；共享的只有 `auction-web-shared` 提供的语言集合、
  `hv.lang` 持久化和 `<html lang>` 同步。存储键与旧系统一致，用户跨系统切换不丢偏好。
- Fluent 消息 ID 不允许包含点号，旧系统的 `nav.home` 一类键改为 `nav-home`。
- `t!` 在键缺失时 panic，因此新增文案必须同步更新全部 5 个 `*.ftl`。可在评审中用键集比对
  确认一致性；`te!` 返回 `Result`、`tid!` 回退为键名，可用于容错要求更高的位置。

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
- SQLite 文件库启用 WAL，读事务不阻塞单个写事务；写入仍由 SQLite 写锁串行化，
  `busy_timeout` 决定等锁而非立即失败。不支持多副本或网络文件系统上的共享数据库；
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

缓存只用于带有限 TTL、可从数据库或外部权威服务重建的查询结果。当前首个接入的是 Public
规则文档：以语言为 key 保存经过应用校验的 JSON 快照，默认 TTL 为 30 秒，Admin 发布成功后
按语言失效。缓存 miss、损坏、连接失败或失效失败必须回退；竞价事务、资格、命令账本、锁、
会话真值和审计均不得依赖缓存。Redis 是否启用因此只影响性能，不改变可用功能的正确性。

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
  配置字段，也不在 domain/application 中读取环境变量。拍卖策略使用普通主配置的
  `[auction]` 模板；创建时冻结为不可变快照，竞价和生命周期事务从快照恢复经济、代理和
  final phase 规则，配置更新不会改变已创建拍卖。
- **错误：** domain/application 使用 `thiserror`；只在 binary composition root 使用
  `anyhow` 汇总启动错误。
- **日志/指标：** `tracing` + `tracing-subscriber`，Public/Admin 可选启用进程隔离的
  Prometheus `/metrics` text scrape，Worker control 可选提供同一 peer-authenticated UDS surface；
  Public/Worker 主配置和 Admin runtime 均可启用 OTLP/HTTP tracing exporter，使用 Tokio 异步批处理、
  有界队列、角色专属敏感 headers 和限定关闭等待。Worker loop 指标、tracing health 和进程资源
  指标已接入；SQLx 存储池边界已补齐旧版 pgx 对应的十项连接池指标，业务事件结果/耗时指标也已
  覆盖 Public 竞价代理命令和 Worker 业务循环。业务指标只使用固定事件与结果枚举，日志和指标
  标签中禁止记录密钥、Cookie、密码、完整连接串、高基数业务主键和完整邮件正文。
- **契约文档：** REST 稳定后用 `utoipa` 生成 OpenAPI；它不反向决定领域模型。

## 依赖精简原则

- 全 Rust 只约束本仓库实现及其程序库依赖；PostgreSQL、SQLite、Redis、反向代理等外部
  组件无需由 Rust 实现，避免为语言统一重复建设基础设施。
- 应用端口使用 Rust 原生 async trait 返回类型，不再直接依赖 `async-trait` 宏。HTTP 客户端
  与 SQLx 统一使用 rustls/ring TLS provider，避免同时链接多个 TLS provider。
- SQLx 数据库驱动和 Redis 都由窄 feature 控制。默认构建不携带备用数据库或缓存客户端。
  浏览器端 crate 与后端 crate 分离，后端二进制不含 Dioxus、`wasm-bindgen` 或 `web-sys`。
- 不额外直接依赖项目未使用的 `tower-http`、通用 Redis 连接池、ORM、
  GraphQL、进程内缓存或第二套前端工具链。确有用例后再按 owner 边界引入。
- 前端静态产物使用 `rust-embed` 内嵌进二进制，得到单文件制品；`allow_missing` 保证未
  构建前端时后端仍可编译，运行时以明确的 503 报告而不是 panic。
- 保留 Axum、Dioxus、SQLx、reqwest、serde、tracing 等承担明确边界价值的基础库；不为
  单纯减少 crate 数量手写 HTTP、TLS、数据库协议、序列化或加密实现。

## 测试与交付

- 单元测试：按当前要求暂缓，不新增或扩展单测覆盖。
- 静态门禁：`cargo fmt`、带 `--locked` 的 `cargo check`、WASM 编译检查和
  `cargo clippy -D warnings`。
- 后续测试恢复后：PostgreSQL 验证完整 repository 契约、行锁、隔离和并发竞价语义；
  SQLite 只验证 Lite 能力契约及文件数据库路径，不要求运行 PostgreSQL 专属用例。
- 浏览器：行为验证恢复后再定端到端方案；生产产物由 `dx build --platform web` 生成，并由
  `auction-http` 内嵌。
- 发布：通过静态门禁后生成 Public/Admin/Worker/CLI 制品和 migration checksum。

## 已知未消除风险

- **Leptos 0.8 的 XSS sink（本项目已通过切换到 Dioxus 规避，记录以备回溯）：**
  `tachys 0.2.18` 把 `textarea` 的 `ESCAPE_CHILDREN` 设为 `false`，与 `script`/`style`/
  `noscript` 同类。按 HTML 规范 `<textarea>` 是 escapable raw text，SSR 时流入的不可信
  字符串会原样输出，`</textarea><script>…</script>` 可突破元素注入活跃标记。上游
  [PR #4730](https://github.com/leptos-rs/leptos/pull/4730) 已于 2026-05-25 合并，但只进
  `leptos_0.9` 分支；0.8 线未 backport，0.2.18 已是 0.2.x 最新版，无修复版本可升。
  **若将来重新评估 Leptos，必须先确认该修复已进入所选版本。**
- **`dx` CLI 依赖构建期 esbuild：** `dx` 会按需从 npm 注册表下载 esbuild
  （`packages/cli/src/esbuild.rs`），`verify_web_tooling` 无条件要求它可用；
  `CliSettings::prefer_no_downloads()` 可改用 PATH 上已有的 esbuild。它是构建期独立
  二进制，不是运行时依赖、Node 运行时或第二套前端构建链，但确实触及“不引入 Node”的
  边界，因此在此显式记录。CI 不安装 dx，也不受该下载影响。
- **`dioxus-web` 0.7.10 漏声明 `web-sys` 的 `Location` feature：** 它在
  `src/history.rs` 无条件调用 `Window::location()`，却没有在自己的 `web-sys` 依赖中
  声明该 feature，只能靠依赖图中其他 crate 顺带启用。本仓库在根 `Cargo.toml` 显式启用
  `web-sys` 的 `Location` 补齐；升级 Dioxus 时应复查上游是否修复，届时可移除该补丁。
- SQLite Lite 的能力表目前是架构约束，不代表 repository、migration、单 Worker 并发和
  部署防护都已实现。每项功能迁移时仍需显式标注档位并完成对应适配器。
- PostgreSQL、SQLite 与 Redis feature 目前可以组合编译，方便静态检查，但正式制品仍需
  在发布配置中明确只启用实际使用的数据库驱动及缓存能力；否则依赖裁剪不会反映到二进制。
- 按当前项目规则，行为测试仍暂缓。因此现阶段验证只能证明格式、编译、Clippy、MSRV 和
  WASM 构建成立，不能证明拍卖行为等价或 PostgreSQL/SQLite 事务语义正确。

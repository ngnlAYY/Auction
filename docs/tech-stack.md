# 技术栈与取舍

[文档导航](./README.md) · [架构](architecture.md) · [开发指南](development.md)

本页按本地 [Cargo.toml](../Cargo.toml)、[Cargo.lock](../Cargo.lock)和[工具链](../rust-toolchain.toml)说明项目选择，
不将仓库锁定版本描述为上游最新或当前安全性保证。

## 版本与升级

| 边界 | 仓库选择 | 约束 |
| --- | --- | --- |
| Rust | 1.98.0，edition 2024；MSRV 1.94.1 | 固定工具链，独立 MSRV 编译 |
| Tokio | 声明 ~1.53.3，锁定 1.53.3 | 官方 1.53.x LTS，后续只接收该分支补丁 |
| HTTP | Axum 0.8.9、Hyper、hyper-util、Tower | 共用连接栈，Public/Admin 路由分离 |
| 浏览器 | Dioxus 0.7.10 | 纯 CSR，禁 fullstack/server |
| 国际化 | dioxus-i18n 0.5.1、unic-langid 0.9.6 | Fluent 编译期内嵌，五语言键一致 |
| 数据库 | SQLx 0.9.0 | 驱动 feature、SQL/锁和迁移树显式分开 |
| 缓存 | redis 1.6.0 | 默认关闭，只缓存可重建值 |
| 外部 HTTP | 直接 reqwest 0.13.5、rustls 0.23.45/ring | 限定来源、预算、凭据与结果证明 |
| 数据/配置 | serde、serde_json、TOML、UUID、chrono | DTO/领域/SQL 行分离，UTC 时间与独立敏感配置 |
| 运维 | clap 4.6.7、tracing、Prometheus、OpenTelemetry | 安全错误、低基数指标、有界 exporter |

PostgreSQL 以 major 18 为部署边界，patch 更新和 major 迁移分开评审。
SQLite 使用 SQLx/libsqlite3-sys 锁图的捆绑引擎，系统动态库版本不能替代制品版本。
升级逐项核对 MSRV、WASM、后端和发布图；检查使用 --locked，不在发布时隐式刷新依赖。
只有上游正式声明才能称 LTS，MSRV 不是长期支持承诺；支持期限/安全状态须在升级评审时重新核查。

2026-10-03 历史审计记录将 rustls/h2/chacha20 锁定到 0.23.45/0.4.19/0.10.2，
对应修复和公告记录见[状态页](status.md#2026-10-03-历史记录)。同日登记的 fxhash 0.2.1 /
RUSTSEC-2025-0057 停维维护债于 2026-10-14 通过 scraper 0.27.0 / selectors 0.38.0 改用 rustc-hash
并移出锁图，提前完成原定 2026-11-03 前的兼容迁移复查；历史审计警告保留，不改写旧结论。
独立 dependency-audit 门禁联网核查公告；退出码 0 不表示没有警告，命中版本范围也不证明生产可利用性。
2026-10-14 将锁图中的 `quinn-proto 0.11.17` 定向更新为 `0.11.18`，修复三条公告范围；未开启 HTTP/3，
当时默认及全 feature/全 target 活跃依赖树均未包含该组件，不把锁图命中描述为已证实运行时漏洞。

## HTTP 与进程

Axum/Tower 拥有协议适配；Hyper/hyper-util 补足请求头超时、HTTP/2 CONNECT 与连接排空，
Tungstenite 用于同一 WebSocket 栈的类型化容量错误。TCP/Linux Unix 共用服务安全边界，Worker control 独立。
auction-process 只提供信号/任务原语，具体停止顺序归装配根。
可选 auction-server 用 Tokio process 与 Linux parent-death signal 管理三角色子进程，复用 lib 装配，
不引入第二运行时；默认分体依赖和发布链保留。

## 浏览器与国际化

Dioxus Rust 组件和路由满足浏览器全 Rust 约束，纯 CSR 消费版本化 REST 与实时投影。
SSR/hydration 增加双目标同步，fullstack/server 增加 HTTP owner，因此不启用。
Askama/HTMX 等替代方案须先明确放宽浏览器语言契约，不能直接变更架构。

两端分别内嵌，显式 web-embed 与 API-only 保证资源隔离，缺页面返回 503。
dx 的 esbuild 仅作构建工具；不引入 Node/npm 运行链。显式 web-sys/Location 补齐 Dioxus feature，升级时复查。
Fluent 文案各 surface 独立，五语言必须同步；共享层只管理语言集合、hv.lang 与 html lang。
静态与视觉边界见[架构](architecture.md)及各 Web 文档。

直接浏览器传输使用 gloo-net 0.7.0、gloo-timers 0.4.0，间接 gloo-utils 为 0.3.0。
共享 GET 仍直接构造浏览器请求，保留精确 URL、同一头/正文超时预算和取消语义。
锁图保留 Dioxus 可选 fullstack 路径上的 gloo-net 0.6.0 / gloo-utils 0.2.0；当前两端 CSR
及全工作区 all-features/all-target-platforms 活跃依赖树未启用该路径，不为清空锁图开启 fullstack。

## SQL、事务与 Lite

SQLx 保留可审阅的显式 SQL、事务和后端锁，不用 ORM/AnyPool 抹平差异。
PostgreSQL 关键写入采用 Serializable/行锁，队列按专属策略使用 Read Committed 加锁；
SQLite Lite 使用 WAL、本地文件、锁等待和 BEGIN IMMEDIATE 单写者边界，Worker 并发 1。
领域不变量相同，系统能力不对等，见[能力矩阵](architecture.md#部署档位与能力)。

当前主要使用运行时查询构造/解码，cargo check 不校验 SQL 字符串的列、约束或运行结果。
若引入 query!/query_as!，须补相应后端的可复现 schema/离线元数据流程。
迁移只追加，历史格式经受限映射兼容，见[迁移说明](../migrations/README.md)。

## 缓存、认证与外部系统

可选 Redis 缓存按语言的规则读模型，故障回源、失效失败靠 TTL 收敛，不影响权威命令。
Public 登录 key、Admin Basic 交换后使用数据库会话，Admin 变更另校验 CSRF。
Admin 启动撤销旧会话，并用 HMAC-SHA256 绑定 actor/secret 与随机 token 摘要；hmac 0.12 配合 sha2 0.10。
没有 Argon2id 密码账户实现，实际 Cookie/轮换/撤销见[HTTP 契约](contracts/routes.md)。

实时使用 WebSocket v1 和 HTTP Catch-up；SSE、OpenAPI/utoipa、外部广播 sink 尚未实现。
integrations 拥有 rustls 传输、限流、解析和凭据；各 owner 按可证明结果决定重试。
未知论坛写入/邮件发送进入对账或人工边界，不能自动重发或抹掉本地已接受事实。

## 指标与追踪

指标由 [auction-observability](../crates/auction-observability/README.md) 集中适配。Prometheus 0.14.0
关闭默认 `protobuf`，显式保留 `process`；内部使用上游 `plain_model`，仍经同一个 `TextEncoder` 输出
Prometheus 0.0.4 文本，不更换 registry、进程/池/tracing collector 或业务 recorder 窄接口。
指标名、label、bucket 和 `/metrics` 的媒体类型契约不变；静态编译不等于实际 scrape 或逐字节兼容验证。
这只移除指标库的二进制编码依赖，不关闭 OTLP/HTTP-proto tracing，也不移除其独立的 prost 编码栈。

2026-10-14 经单独兼容评审，Tokio 切换至 1.53.x；[上游 LTS 政策](https://github.com/tokio-rs/tokio/)
将该分支支持期列至 2027 年 9 月。OpenTelemetry API/SDK/OTLP 配套锁定 0.33.1，
tracing-opentelemetry 为 0.34.0；不启用新的实验能力或更换自有有界批处理器。
OTLP 显式关闭 0.33 新增的默认重试，保留一次导出、失败计数和原预算；上游请求体保护及已知配置边界
见 [observability owner](../crates/auction-observability/README.md#实现布局与-tracing)。

框架评审优先选择已有框架维护升级与 feature 精简，不为追新重写 Axum、SQLx、Dioxus 或持久 Worker。
prometheus-client 需要另审 OpenMetrics 与进程指标兼容，Leptos CSR 需要另审组件/路由/i18n 和唯一构建链；
在缺少明确能力需求或性能证据时，不引入替代框架。

## 依赖与交付纪律

领域只依赖领域安全库，应用只依赖内层/端口，后端不依赖 Web crate。库错误用 thiserror，
anyhow 限装配根；只为实际能力引入依赖。驱动/Redis 用窄 feature，发布图和静态 all-features 图分别审阅。
默认[静态验证](development.md)不执行行为测试；运行证据及未验收范围统一见[状态页](status.md)。

### 2026-10-14 精简审计候选

审计覆盖 20 个工作区包的清单、实际调用和独立/合并 feature 图。scraper/Gloo 升级之后，
五项低风险精简已获授权并实施；SQLite 项仍待评审。未测量体积或构建耗时收益，声明少一条不等于包少一个。

| 状态 / owner | 范围与源码依据 | 收益与边界 |
| --- | --- | --- |
| 已实施 · integrations | 根 scraper 关闭默认 `main`，显式保留 `errors`；[论坛解析](../crates/auction-integrations/src/forum/parser/html.rs)只用库 API | `getopts`、`unicode-width` 已退出活跃图和锁图 |
| 已实施 · HTTP / 装配根 | Axum 关闭 `macros/form/query`，显式保留其余现有默认 feature 和 `ws`；[查询映射](../crates/auction-http/src/http/routes/mapping/query.rs)自有严格解析，无 Axum 宏或 Form/Query 提取器调用 | `axum-macros`、`serde_urlencoded` 已退出活跃图；后者仍在未启用 fullstack 等路径的锁图中；JSON、MatchedPath、日志、HTTP/2/WS 边界保留 |
| 已实施 · observability | 删除[清单](../crates/auction-observability/Cargo.toml)中无源码调用的直接 `serde`；健康详情只用 `serde_json::Value` | 仅删冗余声明，工作区 serde 仍被其他包和传递依赖使用 |
| 已实施 · Admin Web | [可见性轮询](../crates/auction-web-admin/src/app/poll.rs)改用 `std::future::poll_fn`，删除 Admin 直接 futures-util 声明 | 只减耦合，不移除 Public/Shared/Dioxus 的 futures-util，不改唤醒或轮询生命周期 |
| 已实施 · 三个 Web owner | 根 gloo-net 仅留 `http`；Public 增 `json/websocket`，Admin 增 `json`，Shared 只继承 | 独立 Admin/Shared 编译面已收窄；三个 WASM 包分别 check 通过，不靠 feature 统一掩盖缺项 |
| 需存储评审 · SQLite Lite | [storage 清单](../crates/auction-storage/Cargo.toml)的 `sqlx/sqlite` 可评估换成 `sqlite-bundled` + `sqlite-unlock-notify` | 收窄未使用的 deserialize/load-extension Rust 能力，不减包；保留捆绑引擎和锁等待，不宣称原生扩展能力全部禁用，须核对有效编译选项 |

本次全工作区/all-features/all-target-platforms 活跃图去重后 491 → 487 个包节点；
锁图 539 → 536 个包（外部包 519 → 516），只移除三个记录，未新增或升级任何包。
锁图保留 `serde_urlencoded` 不能推导为当前制品启用了它，也不能将节点减少等同于 WASM 体积下降。
各 owner/目标独立 check、完整静态门禁与公告审计的实际范围见[状态页](status.md#2026-10-14-低风险依赖精简)。
不为纸面包数重写 HTML/CSS、Fluent、URL、TLS、密码学、RESP、WebSocket 或迁移装载器；
不删除 `sqlx::migrate!` 所需 macros/migrate，也不改变系统 CA、双后端隔离或纯 CSR 契约。

# 技术栈与取舍

[文档导航](README.md) · [架构](architecture.md) · [开发指南](development.md)

本页按本地 [Cargo.toml](../Cargo.toml)、[Cargo.lock](../Cargo.lock)和[工具链](../rust-toolchain.toml)说明项目选择，
不将仓库锁定版本描述为上游最新或当前安全性保证。

## 版本与升级

| 边界 | 仓库选择 | 约束 |
| --- | --- | --- |
| Rust | 1.98.0，edition 2024；MSRV 1.94.1 | 固定工具链，独立 MSRV 编译 |
| Tokio | 声明 ~1.51.4，锁定 1.51.4 | 沿项目约定 1.51.x 支持分支升级 |
| HTTP | Axum 0.8.9、Hyper、hyper-util、Tower | 共用连接栈，Public/Admin 路由分离 |
| 浏览器 | Dioxus 0.7.10 | 纯 CSR，禁 fullstack/server |
| 国际化 | dioxus-i18n 0.5.1、unic-langid 0.9.6 | Fluent 编译期内嵌，五语言键一致 |
| 数据库 | SQLx 0.9.0 | 驱动 feature、SQL/锁和迁移树显式分开 |
| 缓存 | redis 1.6.0 | 默认关闭，只缓存可重建值 |
| 外部 HTTP | 直接 reqwest 0.13.4、rustls 0.23.45/ring | 限定来源、预算、凭据与结果证明 |
| 数据/配置 | serde、serde_json、TOML、UUID、chrono | DTO/领域/SQL 行分离，UTC 时间与独立敏感配置 |
| 运维 | clap 4.6.6、tracing、Prometheus、OpenTelemetry | 安全错误、低基数指标、有界 exporter |

PostgreSQL 以 major 18 为部署边界，patch 更新和 major 迁移分开评审。
SQLite 使用 SQLx/libsqlite3-sys 锁图的捆绑引擎，系统动态库版本不能替代制品版本。
升级逐项核对 MSRV、WASM、后端和发布图；检查使用 --locked，不在发布时隐式刷新依赖。
只有上游正式声明才能称 LTS，MSRV 不是长期支持承诺；支持期限/安全状态须在升级评审时重新核查。

2026-10-03 历史审计记录将 rustls/h2/chacha20 锁定到 0.23.45/0.4.19/0.10.2，
对应修复和公告记录见[状态页](status.md#2026-10-03-历史记录)。同日登记的 fxhash 0.2.1 /
RUSTSEC-2025-0057 为停维维护债，暂保留间接依赖，2026-11-03 前复查兼容迁移，CI 不隐藏提示。
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

## 依赖与交付纪律

领域只依赖领域安全库，应用只依赖内层/端口，后端不依赖 Web crate。库错误用 thiserror，
anyhow 限装配根；只为实际能力引入依赖。驱动/Redis 用窄 feature，发布图和静态 all-features 图分别审阅。
默认[静态验证](development.md)不执行行为测试；运行证据及未验收范围统一见[状态页](status.md)。

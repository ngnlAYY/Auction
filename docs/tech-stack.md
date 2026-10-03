# 技术栈与取舍

[文档导航](README.md) · [架构](architecture.md) · [开发指南](development.md)

本页描述仓库实际选择及其约束。版本声明由 [Cargo.toml](../Cargo.toml)集中管理，
实际解析以 [Cargo.lock](../Cargo.lock)为准；不以文档中的“最新”替代锁图核对。

## 本页导航

- [版本与升级](#版本与升级)
- [HTTP 与进程](#http-与进程)
- [浏览器与国际化](#浏览器与国际化)
- [SQL、事务与 Lite](#sql事务与-lite)
- [缓存、认证与外部系统](#缓存认证与外部系统)
- [依赖与交付纪律](#依赖与交付纪律)

## 版本与升级

| 边界 | 仓库选择 | 约束 |
| --- | --- | --- |
| Rust | `1.98.0`，edition 2024；MSRV `1.94.1` | [工具链](../rust-toolchain.toml)固定构建版本，CI 单独编译 MSRV |
| Tokio | `~1.51.4` | 沿项目约定的 1.51.x 支持分支升级，不把 MSRV 称为 LTS |
| HTTP | Axum `0.8.9`、Hyper、hyper-util、Tower | 同一服务栈；连接超时/排空不建立第二套路由 |
| 浏览器 | Dioxus `0.7.10` | 纯 CSR，禁用 fullstack/server |
| 国际化 | dioxus-i18n `0.5.1`、unic-langid `0.9.6` | Fluent 编译期内嵌，五语言键集合一致 |
| 数据库 | SQLx `0.9.0` | PostgreSQL / SQLite 独立 feature 与迁移树 |
| 缓存 | redis `1.6.0`，默认关闭 | 只缓存可重建值 |
| 外部 HTTP | reqwest `0.13.4`、rustls `0.23.45`/ring | 限定来源、预算、凭据及结果证明 |
| 数据与配置 | serde、serde_json、TOML、UUID、chrono | DTO/领域/SQL 记录分离；敏感信息独立 |
| 运维 | clap、tracing、Prometheus、OpenTelemetry | 安全错误、低基数指标、有界 exporter |

完整档部署以 PostgreSQL major 18 为边界，patch 更新与 major 迁移分开评审。
SQLite 使用锁定 SQLx/libsqlite3-sys 图对应的捆绑引擎，不把系统动态库版本当作制品版本。
升级逐组件核对 MSRV、WASM、后端适配与发布依赖；本地和 CI 使用 `--locked`，禁止发布时隐式刷新依赖图。
上游支持期限或安全状态应在升级评审时重新核查。2026-10-03 按 RustSec 核对并将 rustls 下限/锁图
升级至 `0.23.45`、h2 锁图升级至 `0.4.19`，覆盖 RUSTSEC-2026-0285 与 RUSTSEC-2026-0258；
撤回的 chacha20 `0.10.1` 升至未撤回的 `0.10.2`。
依赖公告由独立静态门禁持续检查；`fxhash 0.2.1` 停维维护债及复查期限见
[修复决定](../.agents/notes/implemented/bug-fix/2026-10-03-session-setup-audit-remediation.md)，不把版本命中当作生产可利用性证明。

## HTTP 与进程

Axum/Tower 让 Public/Admin 共享协议边界并保持独立路由。Hyper/hyper-util 只补充请求头超时、
HTTP/2 CONNECT 和连接排空。Tungstenite 用于识别同一 WebSocket 栈的类型化容量错误。
选择现有组合的收益是跨层所有权清楚；成本是应用端口和装配由项目显式维护。
Actix、Poem、Rocket 不进入当前依赖图，迁换框架不能替代领域分层。

TCP 与 Linux Unix listener 共用安全绑定器，业务监听配置由装配根选择；Worker control 是独立 Unix 面。
`auction-process` 共用信号与任务原语，具体停止顺序仍由三个应用决定。
可选 `auction-server` 使用 Tokio process 和 Linux parent-death signal 自管三角色子进程，
复用 app 的 lib 装配而不拼接 ELF 或增加第二种运行时；默认分体依赖及发布链不变。

## 浏览器与国际化

Dioxus 的 Rust 组件与路由满足浏览器全 Rust 约束。纯 CSR 使用版本化 REST/实时投影，
SSR/hydration 会增加双目标和同步边界；fullstack/server 会增加另一 HTTP owner，因此不启用。
Askama + HTMX 涉及不同浏览器契约，不能在未放宽语言约束时替换现有实现。

历史文档将 Leptos SSR 依赖成本与 textarea 转义问题列为未选择它的原因；该历史理由不等于当前上游漏洞结论。
重评替代框架时应核对所选版本与安全公告，不能沿用旧版本判断。

两端产物由各装配根内嵌，`web-embed` 与 API-only 显式区分，缺少产物返回 503。
dx 使用 esbuild 作为允许的构建期工具；不引入 Node 运行时或第二套前端构建链。
根 Cargo.toml 显式开启 `web-sys/Location` 以补齐 Dioxus 调用所需 feature，升级时复查是否仍必要。

Fluent 文案由各 surface 独立维护，共享层只管理语言集合、`hv.lang` 与 `<html lang>`。
新增键必须同时进入全部五种语言；缺键可能触发 panic，不能只验证默认语言。
当前选择减少自建复数、插值与响应式桥接的工作，也要求持续维护译文门禁。

## SQL、事务与 Lite

SQLx 保留显式 SQL、事务与后端锁，便于审阅竞价原子性。ORM 可以减少普通 CRUD，但关键竞价仍需显式锁和事实写入，
因此当前不引入 SeaORM/Diesel 或通用 `AnyPool`。

PostgreSQL 完整档提供 Serializable 与行锁；队列领取按专属策略使用 Read Committed 加锁。
SQLite Lite 提供 WAL、本地文件、等待锁和 `BEGIN IMMEDIATE` 单写者边界，Worker 并发为 1。
两者共享领域不变量，系统能力不对等，见[能力矩阵](architecture.md#部署档位与能力)。

目前存储主要使用运行时 SQLx 查询构造与解码。`cargo check` 不会自动验证 SQL 字符串的列、约束或运行行为；
不能因为 SQLx 支持查询宏，就声称本仓库已有离线 schema 校验。
若引入 `query!`/`query_as!`，必须为相应后端补齐可复现 schema/离线元数据流程。
迁移只追加，兼容已发布格式通过受限映射处理，详见[迁移](../migrations/README.md)。

## 缓存、认证与外部系统

缓存选择默认关闭和可选 Redis。跨进程 TTL 缓存可以降低规则读取成本，但增加失效管理；
故障必须回源，不能影响已提交命令。当前无需进程内第二级缓存或通用 Redis 连接池。

Public 登录 key 与 Admin Basic 凭据交换后由数据库会话边界管理，Admin mutation 另校验 CSRF。
Admin 启动先永久撤销旧会话，并使用 HMAC-SHA256 绑定 actor/secret 与随机 token 摘要；
选用兼容现有 sha2 0.10 的 hmac 0.12，不自制 MAC，也不向数据库新增口令校验值。
当前没有 Argon2id 密码账户实现，不能把该算法写成现有登录能力。
Cookie、安全属性、轮换与撤销的实际协议见[HTTP 契约](contracts/routes.md)。

实时使用 WebSocket v1 和 HTTP Catch-up；SSE 是可另行评审的降级方向，当前未注册 endpoint。
OpenAPI/utoipa 与外部广播 sink 同样未实现，当前契约由 DTO、显式路由和文档共同维护。

外部调用由 integrations 处理 rustls 传输、限流、解析和凭据；重试由结果是否可证明及各 owner 决定。
不能把“所有写入都可按幂等 key 自动重试”作为外部服务保证。
未知论坛写入或邮件发送进入对账/人工边界，不能抹掉本地已接受事实。

## 依赖与交付纪律

领域只依赖领域安全库；应用只依赖内层及稳定端口类型；后端不依赖 Web crate。
库错误使用 `thiserror`，`anyhow` 只用于二进制装配；不为未使用的能力引入 tower-http、GraphQL、ORM 或额外缓存库。
SQLx 驱动和 Redis 通过窄 feature 选择，发布图与静态 all-features 图分别审阅。

默认验证只包含格式、文档、布局、配置、i18n、编译、Clippy、MSRV 与 WASM 检查。
行为测试与生产验收需要独立授权和运行证据；当前缺口统一记录在[验证状态](status.md)。

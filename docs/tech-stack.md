# 技术栈与取舍

[文档导航](./README.md) · [架构](architecture.md) · [开发指南](development.md)

本页按本地 [Cargo.toml](../Cargo.toml)、[Cargo.lock](../Cargo.lock)和[工具链](../rust-toolchain.toml)说明项目选择，
不将仓库锁定版本描述为上游最新或当前安全性保证。

## 版本与升级

| 边界 | 仓库选择 | 约束 |
| --- | --- | --- |
| 原生平台 | 仅 Linux x86_64，正式发布 GNU/glibc | apps 编译期守卫；浏览器 wasm32-unknown-unknown 保留 |
| Rust | 1.99.0，edition 2024；MSRV 1.94.1 | resolver 3 按 MSRV 回退解析，固定工具链、独立 MSRV 编译 |
| Tokio | 声明 ~1.53.3，锁定 1.53.3 | 官方 1.53.x LTS，后续只接收该分支补丁 |
| HTTP | Axum 0.8.9、Hyper、hyper-util、Tower | 共用连接栈，Public/Admin 路由分离 |
| 浏览器 | Dioxus 0.7.10 | 纯 CSR，禁 fullstack/server |
| 国际化 | dioxus-i18n 0.6.0、unic-langid 0.9.6 | Fluent 编译期内嵌，五语言键一致 |
| 数据库 | SQLx 0.9.0、PostgreSQL | 显式 postgres feature、SQL、事务与行锁 |
| 缓存 | redis 1.7.1 | 默认关闭，只缓存可重建值 |
| 外部 HTTP | 直接 reqwest 0.13.5、rustls 0.23.46/ring | 限定来源、预算、凭据与结果证明 |
| 数据/配置 | serde、serde_json、TOML、UUID、chrono | DTO/领域/SQL 行分离，UTC 时间与独立敏感配置 |
| 运维 | clap 4.6.7、tracing、Prometheus、OpenTelemetry | 安全错误、低基数指标、有界 exporter |

数据库仅支持 PostgreSQL，以 major 18 为部署边界，patch 更新和 major 迁移分开评审。
SQLx 未启用的可选依赖即使保留在 Cargo.lock，也不代表活动驱动或运行支持。
升级逐项核对 MSRV、WASM、PostgreSQL 和发布图；检查使用 --locked，不在发布时隐式刷新依赖。
只有上游正式声明才能称 LTS，MSRV 不是长期支持承诺；支持期限/安全状态须在升级评审时重新核查。

2026-10-03 历史审计记录将 rustls/h2/chacha20 锁定到 0.23.45/0.4.19/0.10.2，
对应修复和公告记录见[状态页](status.md#2026-10-03-历史记录)。同日登记的 fxhash 0.2.1 /
RUSTSEC-2025-0057 停维维护债于 2026-10-14 通过 scraper 0.27.0 / selectors 0.38.0 改用 rustc-hash
并移出锁图，提前完成原定 2026-11-03 前的兼容迁移复查；历史审计警告保留，不改写旧结论。
独立 dependency-audit 门禁联网核查公告；自 2026-10-15 起将警告也视为失败，不设 ignore。
旧审计退出码 0 不表示当时没有警告；命中版本范围不证明生产可利用性。
2026-10-14 将锁图中的 `quinn-proto 0.11.17` 定向更新为 `0.11.18`，修复三条公告范围；未开启 HTTP/3，
当时默认及全 feature/全 target 活跃依赖树均未包含该组件，不把锁图命中描述为已证实运行时漏洞。

## 编译优化边界

[根清单](../Cargo.toml)统一拥有生产 profile，定向构建不复制优化参数：

| 档位 | 参数 | 取舍 |
| --- | --- | --- |
| 后端 release | `opt-level=3`、`lto="fat"`、`codegen-units=1`、`incremental=false` | 优先跨 crate 优化，接受更长编译时间和更高链接峰值内存 |
| 后端 panic | `unwind` | 保留 Tokio 任务隔离及 drop/drain 语义，不以全进程 abort 换体积 |
| WASM release | `opt-level="z"`、Fat LTO、单 CGU、`panic="abort"` | 保留浏览器下载体积优先策略，不把后端 CPU 参数传给 WASM |
| 通用 CPU | Linux x86_64 目标默认基线 | 不启用 `native`、架构专属指令、全局 fast-math 或 `panic=abort` |

[AMD 定向候选](../ops/build/README.md)仅叠加已审阅的目标 CPUID 与构建并发。
没有代表性业务负载时，不启用 PGO/BOLT、不宣称“最佳性能”；历史领域微基准不能证明新版服务吞吐/p99。
实际验证范围见[状态页](status.md)。
Rust 1.99 将 fetch_update 更名为 try_update，但新名在 MSRV 1.94.1 尚未稳定；heartbeat 与 audit 计数
仅在原调用语句局部允许 deprecated，注明版本原因，不全局关闭 lint，也不重写已审阅的原子更新语义。

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

## SQL 与事务

SQLx 保留显式 PostgreSQL 查询与事务，不引入 ORM/AnyPool。隔离、锁序及重试边界由
[不变量契约](contracts/invariants.md)拥有，连接池与迁移实现见[storage](../crates/auction-storage/README.md)。

当前主要使用运行时查询构造/解码，cargo check 不校验 SQL 字符串的列、约束或运行结果。
若引入 query!/query_as!，须补可复现 PostgreSQL schema/离线元数据流程。
迁移只追加；SQLite 已发布 SQL 作为历史文件保留，不提供运行入口，见[迁移说明](../migrations/README.md)。

## 缓存、认证与外部系统

可选 Redis 缓存按语言的规则读模型，故障回源、失效失败靠 TTL 收敛，不影响权威命令。
Public 登录 key、Admin Basic 交换后使用数据库会话，Admin 变更另校验 CSRF。
Admin 启动撤销旧会话，并用 HMAC-SHA256 绑定 actor/secret 与随机 token 摘要；hmac 0.13 配合 sha2 0.11。
升级只适配 KeyInit/摘要格式化 API；算法、密钥构造、摘要字节和固定两位小写十六进制格式不变。
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

### 2026-10-15 全依赖审计

以下两节保留当时决策，包括 SQLite 退出运行支持前的精简与缓解，不是当前构建指南。
覆盖 20 个工作区包、50 项直接依赖、完整锁图、全 feature/全 target 活跃图及工具链、dx、CI Actions。
版本依据为 crates.io 稳定版与 Rust 官方分发清单；1.99.1 manifest 当时返回 404，故选可下载的 1.99.0。
MSRV 保持 1.94.1，resolver 回退不能代替实际编译；检查结果见[状态页](status.md#2026-10-15-依赖审计与编译优化)。

| 决定 | 依赖范围 | 理由与兼容边界 |
| --- | --- | --- |
| 安全升级 | rustls 0.23.46；锁图同步 h2、rustls-webpki、hyper-rustls、tokio-rustls、quinn 系列补丁 | 修复下列 RustSec 命中；保留 ring/TLS 1.2/系统 CA，不启用 HTTP/3 |
| 协议与基础库升级 | hyper 1.12.0、hyper-util 0.1.21、ipnet 2.12.2、libc 0.2.190、redis 1.7.1、rust-embed 8.13.0 | 不改 HTTP、缓存故障回源、Web 资源隔离或部署档位 |
| 数据与密码学升级 | base64 0.23.1、hmac 0.13.0、sha2 0.11.1、serde_json 1.0.154、thiserror 2.0.21、toml 1.1.8、uuid 1.28.0 | base64 仅开 std、不直接开启新 SIMD；保留严格引擎、编码与摘要格式；淘汰旧 hmac 0.12 |
| 排期与浏览器升级 | croner 4.0.1、dioxus-i18n 0.6.0、wasm-bindgen 0.2.129、js-sys/web-sys 0.3.106 | cron 先经原严格六字段校验，不开放昵称/宽松语法；项目未用被删除的 I18n::init；CSR/五语言保持 |
| 当时稳定版保持 | [根清单](../Cargo.toml)中其余直接依赖及框架/观测配套 | 不为追新改 API；Tokio 保持 LTS 1.53.x，dx 匹配 Dioxus 0.7.10；exporter 预算/重试与指标 wire 不变 |
| 兼容性保留 | 直接 tungstenite 0.29.0，不升 0.30.0 | Axum 经 tokio-tungstenite 0.29.0 暴露具体错误类型；单独升级破坏 downcast，且增加重复版本 |
| 编译面精简 | UUID v5 仅实际生成它的 owner 启用；SQLite 改为 bundled + unlock-notify | Public CSR 不继承 v5；移除 SQLite 未用 deserialize/load-extension Rust 能力；当时以 Cargo C 编译宏硬化 |
| 工具与供应链 | cargo-audit 0.22.2、cargo-machete 0.9.2；checkout 7.0.1、rust-cache 2.9.2 | 审计告警失败、补未用依赖门禁；Actions 固定已核对的完整提交 SHA，保留只读权限和不持久化凭据 |

RustSec 基线命中 rustls 的 [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html)、
[0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html)、[0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html)、
[0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html)，更新后同一公告库快照为零漏洞/零警告；
这些是版本匹配，不把 ECH/Brotli 等未启用路径写成已证实利用。
SQLite 的 [CVE-2026-11822](https://nvd.nist.gov/vuln/detail/CVE-2026-11822) 不由 RustSec 零结果覆盖：
当时 SQLx 上限阻止直接升级引擎，以禁编 FTS 缓解，并要求跟进上游绑定升级、禁止导入不可信数据库。
当前 SQLite 运行支持已移除；历史缓解措施不能解释为引擎漏洞已修复。
依赖升级当时未消除 OTel HTTP builder 的隐式环境头覆盖；后续已由配置 owner 增加启用前拒绝门禁，
见[配置边界](../ops/config/README.md#文件职责)。不通过修改进程全局环境解决，也不把零公告当作配置安全证明。

版本与节点增减见[状态页](status.md#2026-10-15-依赖审计与编译优化)，不代表运行能力或性能收益。
当时全部外部包来自 crates.io 且有 license 元数据，无本地/Git fork、无公告忽略项。
MPL-2.0 包 cssparser、cssparser-macros、dtoa-short、selectors 的分发仍须履行许可/源码义务，元数据不替代法律审查。
锁图中的可选旧 reqwest/Gloo、SHA-2 0.10、Base64 0.22 和 Dioxus 预发布子组件，不通过开启 fullstack、
跨不兼容范围 patch 或重写协议清理；新增平台/优化传递依赖也不等于新增运行能力。

### 2026-10-14 精简审计候选

覆盖 20 个工作区包的清单、实际调用及独立/合并 feature 图。scraper/Gloo 升级后，前五项获授权实施；
SQLite 项当时待评审，后续见上节。未测量体积或构建耗时，声明、活跃节点和锁图条目不能互换计数。

| 状态 / owner | 决策与依据 | 边界 |
| --- | --- | --- |
| 已实施 · integrations | scraper 关闭 `main`、保留 `errors`；[论坛解析](../crates/auction-integrations/src/forum/parser/html.rs)只用库 API | `getopts`、`unicode-width` 退出活跃图和锁图 |
| 已实施 · HTTP / 装配根 | Axum 关闭未用 `macros/form/query`；[查询映射](../crates/auction-http/src/http/routes/mapping/query.rs)自有严格解析 | 保留其余默认 feature 与 `ws`，包括 JSON、MatchedPath、日志、HTTP/2；`axum-macros`、`serde_urlencoded` 退出活跃图，后者仍锁在未启用 fullstack 等路径 |
| 已实施 · observability | [清单](../crates/auction-observability/Cargo.toml)删除无调用的直接 `serde`，健康详情只用 `serde_json::Value` | 其他 owner 和传递依赖仍使用 serde |
| 已实施 · Admin Web | [轮询](../crates/auction-web-admin/src/app/poll.rs)改用 `std::future::poll_fn`，删直接 futures-util | 不改唤醒/生命周期；Public/Shared/Dioxus 仍需 futures-util |
| 已实施 · Web | gloo-net 根仅留 `http`，Public 加 `json/websocket`，Admin 加 `json`，Shared 只继承 | 独立编译核对，不靠 feature 合并补项 |
| 当时待评审 · SQLite Lite | 评估将 storage 的 `sqlx/sqlite` 换为 `sqlite-bundled` + `sqlite-unlock-notify` | 仅收窄 deserialize/load-extension Rust 能力，不减包；保留捆绑引擎/锁等待，原生扩展是否禁用须核对编译选项 |

节点统计、三个 WASM 包独立 check 与其余检查结果见[状态页](status.md#2026-10-14-低风险依赖精简)。
锁图残留不代表制品启用，节点减少不证明 WASM 体积下降。不为包数重写 HTML/CSS、Fluent、URL、TLS、密码学、
RESP、WebSocket 或迁移装载器；当时保留 `sqlx::migrate!` 的 macros/migrate、系统 CA、双后端隔离及纯 CSR。

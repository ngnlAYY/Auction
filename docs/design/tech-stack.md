# 技术栈与取舍

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [开发指南](docs/guide/development.md)

本页按本地 [Cargo.toml](Cargo.toml)、[Cargo.lock](Cargo.lock)和[工具链](rust-toolchain.toml)说明项目选择，
不将仓库锁定版本描述为上游最新或当前安全性保证。工作区成员由 `apps/*`、`crates/*` 收集，直接依赖集中在根清单，
锁文件保留完整解析图；MSRV 声明在 `workspace.package.rust-version` 且各包继承，Clippy 读取包元数据。WASM profile
继承 release 的 LTO/代码生成单元，只覆盖体积优化和 panic 策略。

## 版本与升级

| 边界 | 仓库选择 | 约束 |
| --- | --- | --- |
| 原生平台 | 仅 Linux x86_64，正式发布 GNU/glibc | apps 编译期守卫；浏览器 wasm32-unknown-unknown 保留 |
| 系统调用 | libc 锁定 0.2.191 | PR #3 仅更新应用 Cargo.lock；不扩大原生平台支持 |
| Rust | 1.99.0，edition 2024；MSRV 1.94.1 | resolver 3 按 MSRV 回退解析，固定工具链、独立 MSRV 编译 |
| Tokio | 声明 ~1.53.3，锁定 1.53.3 | 官方 1.53.x LTS，后续只接收该分支补丁 |
| HTTP | Axum 0.8.9、Hyper、hyper-util、Tower | 共用连接栈，Public/Admin 路由分离 |
| 浏览器 | Dioxus 0.7.10 | 纯 CSR，禁 fullstack/server |
| Web 构建工具 | dx CLI 0.7.10，独立 TLS／Zstandard 修复锁 | 固定源码包重建、禁自动下载；配套工具及 SHA-256 由[工具 owner](ci/tools/README.md)维护 |
| 编译缓存工具 | sccache 0.18.0，独立 TLS 修复锁 | 固定源码包重建，只启用本地/GHA；安装回执和二进制身份进入 `BUILD-INFO` |
| CI Actions | checkout 7.0.1、rust-cache 2.9.2、cache 6.1.0、upload-artifact 7.0.2、github-script 9.0.0 | 固定完整提交 SHA；权限、缓存与下载保留边界由 [CI 说明](ci/README.md)拥有 |
| 国际化 | dioxus-i18n 0.6.0、unic-langid 0.9.6 | Fluent 编译期内嵌，五语言键一致 |
| 数据库 | SQLx 0.9.0、PostgreSQL | 显式 postgres feature、SQL、事务与行锁 |
| 缓存 | redis 1.7.1 | 默认关闭，只缓存可重建值 |
| 外部 HTTP | 直接 reqwest 0.13.5、rustls 0.23.46/ring | 限定来源、预算、凭据与结果证明 |
| 数据/配置 | serde、serde_json、TOML、UUID、chrono | DTO/领域/SQL 行分离，UTC 时间与独立敏感配置 |
| 运维 | clap 4.6.7、tracing、Prometheus、OpenTelemetry | 安全错误、低基数指标、有界 exporter |

数据库仅支持 PostgreSQL，以 major 18 为部署边界，patch 更新和 major 迁移分开评审；SQLx 未启用的可选依赖即使保留在
Cargo.lock，也不代表活动驱动或运行支持。升级逐项核对 MSRV、WASM、PostgreSQL 和发布图，检查使用 `--locked`，不在
发布时隐式刷新依赖。只有上游正式声明才能称 LTS，MSRV 不是长期支持承诺。

密码学传递依赖 `ctutils` 在应用锁图和 sccache 独立锁图中均锁定为 0.4.4，修复
[RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html) 的非恒定时间掩码生成及部分
32 位目标 `u128` 比较。此更新不改变 HMAC/SHA-256 算法、摘要或会话身份；依赖命中不等于已证明本程序存在可利用侧信道。
旧应用与构建工具二进制不会随锁图自动更新，必须分别重新构建；实际检查范围见[状态页](docs/status.md)。

构建工具独立锁的 Tokio 已统一到应用所用的 1.53.3，处理
[RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html)；另更新可兼容的 yanked 依赖。
上述工具更新不改变应用 Tokio 或启用新的运行能力。应用与工具的公告结果必须分开表述，剩余风险及重建前提见
[构建工具审计](ci/tools/README.md#上游公告与实际使用范围)。

2026-10-26 dx 独立锁升级至 `zstd-safe 8.2.0` 等修复版本；应用锁和 sccache 锁不因此改变。
完整工具锁仍命中其他公告，feature 图与受控调用路径的限制见[工具审计](ci/tools/README.md#2026-10-26-zstandard-公告)。

## 编译优化边界

[根清单](Cargo.toml)统一拥有生产 profile，定向构建不复制优化参数：

| 档位 | 参数 | 取舍 |
| --- | --- | --- |
| 本地 dev | `debug="line-tables-only"`，保留默认增量编译 | 保留文件/行号回溯，减少调试数据生成与链接；完整调试可用 `CARGO_PROFILE_DEV_DEBUG=2` |
| CI 静态检查 | verify/MSRV 的 `CARGO_PROFILE_DEV_DEBUG=0` | 不运行程序，不生成调试数据；不覆盖 release/WASM profile |
| 后端 release | `opt-level=3`、`lto="fat"`、`codegen-units=1`、`incremental=false` | 优先跨 crate 优化，接受更长编译时间和更高链接峰值内存 |
| 后端 panic | `unwind` | 保留 Tokio 任务隔离及 drop/drain 语义，不以全进程 abort 换体积 |
| WASM release | `opt-level="z"`、Fat LTO、单 CGU、`panic="abort"` | 保留浏览器下载体积优先策略，不把后端 CPU 参数传给 WASM |
| 通用 CPU | Linux x86_64 目标默认基线 | 不启用 `native`、架构专属指令、全局 fast-math 或 `panic=abort` |

本地与 Actions 使用的 Rust 1.99.0、MSRV 1.94.1 均已包含 GNU x86_64 的默认 LLD，
无需再安装系统 linker 或添加全局 `-fuse-ld=lld`；依据见
[Rust 1.90 发布说明](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/)。
调试信息取舍沿用 [Cargo 构建性能指南](https://doc.rust-lang.org/cargo/guide/build-performance.html)，
不改变优化级别、断言、溢出检查或 panic 语义。没有代表性业务负载，不用 Thin LTO 替换现有生产档来换取未经量化的运行时取舍。

[AMD 定向候选](ops/build/README.md)仅叠加已审阅的目标 CPUID 与构建并发；没有代表性业务负载时，不启用 PGO/BOLT、
不宣称“最佳性能”，历史领域微基准不能证明新版服务吞吐/p99（验证范围见[状态页](docs/status.md)）。Rust 1.99 将
fetch_update 更名为 try_update，但新名在 MSRV 1.94.1 尚未稳定，heartbeat 与 audit 计数仅在原调用语句局部允许
deprecated，不全局关闭 lint，也不重写已审阅的原子更新语义。

## HTTP 与进程

Axum/Tower 拥有协议适配，Hyper/hyper-util 补足请求头超时、HTTP/2 CONNECT 与连接排空，Tungstenite 用于同一 WebSocket
栈的类型化容量错误；TCP/Linux Unix 共用服务安全边界，Worker control 独立。auction-process 只提供信号/任务原语，
停止顺序归装配根；可选 auction-server 用 Tokio process 与 Linux parent-death signal 管理三角色子进程并复用 lib
装配，不引入第二运行时。

Axum 未启用无人消费的 `original-uri` feature：HTTP owner 直接使用请求 URI，不为每次路由额外克隆 URI
并插入 `OriginalUri` 扩展。此裁剪只减少启用能力，不减少锁图包数；`futures-util` 默认宏仍被 OTel/Dioxus
实际使用，保留原声明，避免无收益的 feature 改写。

`reqwest` 共用 `rustls-no-provider`，只有 Worker control 显式启用 JSON；integrations 不使用该 feature。
OTLP 的 `reqwest-client` 本身不启用 TLS，`auction-observability` 因此显式声明同一工作区 `reqwest`，
使独立图也启用 rustls；调用仍由 OTLP 上游执行，`cargo-machete` 只对该 feature-only 依赖作精确忽略。
继续安装 ring provider，不启用上游 `reqwest-rustls` 会引入的 aws-lc，也不改变 exporter 的超时和重试策略。

直接 `tungstenite` 使用 0.29.0，与 Axum 0.8.9 经 `tokio-tungstenite` 引入的实际类型一致。
不能单独升至另一个 minor 后仍以其 `Error` downcast：Rust 会将它们视为不同类型，导致超限帧丢失约定的 1009 关闭响应。
本次移除未被其他路径使用的 0.30.0 锁条目；未启用的 Dioxus 可选旧分支不因此获得运行支持。
全工作区原生 feature 图另含 Dioxus devtools 引入的 Tungstenite 0.28；独立 Public WASM 图不包含该版本，
它不参与 Axum 错误类型匹配。两套 Base64 与 SHA-2 来自直接依赖及 Axum/SQLx 传递约束，不能仅为消除重复强行合并。

## 浏览器与国际化

Dioxus Rust 组件和路由满足浏览器全 Rust 约束，纯 CSR 消费版本化 REST 与实时投影；SSR/hydration 增加双目标同步、
fullstack/server 增加 HTTP owner，因此不启用。直接浏览器传输使用 gloo-net 0.7.0、gloo-timers 0.4.0，间接
gloo-utils 为 0.3.0；共享 GET 仍直接构造浏览器请求，保留精确 URL、同一头/正文超时预算和取消语义。锁图保留可选
fullstack 路径上的 gloo-net 0.6.0 / gloo-utils 0.2.0，当前活跃依赖树未启用该路径，不为清空锁图开启 fullstack。
`chrono` 的 `clock` feature 只由 `auction-config` 为旧标准 cron 的 `Local` 时区启用；共享图保留 `now/serde/std`。
独立 Public/Admin WASM 图不再启用 `iana-time-zone`，UTC 时钟与既有浏览器时间格式不变；原生 cron 的本地时区仍保留。
这属于能力裁剪，不移除根锁中后端仍需要的包，也不声称已测得 WASM 体积或执行速度收益。
共享层显式声明自身使用的 `web-sys` 事件、节点和 MutationRecord feature；`MouseEvent` 不再依赖 Dioxus 顺带启用。
这些 feature 属于编译边界，不表示引入新浏览器 API 或测得体积变化。
独立 Public/Admin WASM 依赖图保留 Dioxus/Manganis 的两代 `const-serialize`、Dioxus 相关路径的两代 `rustc-hash`
以及过程宏使用的不同 `syn` 主版本；这些传递版本遵循上游约束，不强制跨版本替换，也不据此推断最终载荷大小。
资源隔离、`web-embed`/API-only 与缺页 503 见[架构](docs/design/architecture.md#浏览器实时与制品)。

## SQL 与事务

SQLx 保留显式 PostgreSQL 查询与事务，不引入 ORM/AnyPool；隔离、锁序及重试边界由
[不变量契约](docs/contracts/invariants.md)拥有，连接池与迁移实现见[storage](crates/auction-storage/README.md)。
当前主要使用运行时查询构造/解码，cargo check 不校验 SQL 字符串的列、约束或运行结果；若引入 query!/query_as!，
须补可复现 PostgreSQL schema/离线元数据流程。迁移只追加，执行前提与 checksum 兼容见[迁移说明](migrations/postgres/README.md)。

## 缓存、认证与外部系统

可选 Redis 缓存按语言的规则读模型，故障回源、失效失败靠 TTL 收敛，不影响权威命令。Public 登录 key、Admin Basic
交换后使用数据库会话，Admin 变更另校验 CSRF；Admin 启动撤销旧会话，并用 HMAC-SHA256 绑定 actor/secret 与随机
token 摘要，hmac 0.13 配合 sha2 0.11，升级只适配 KeyInit/摘要格式化 API，算法、密钥构造、摘要字节和固定两位小写
十六进制格式不变。没有 Argon2id 密码账户实现，实际 Cookie/轮换/撤销见[HTTP 契约](docs/contracts/routes.md)。
实时使用 WebSocket v1 和 HTTP Catch-up；SSE、OpenAPI/utoipa、外部广播 sink 尚未实现。integrations 拥有 rustls
传输、限流、解析和凭据，各 owner 按可证明结果决定重试；未知论坛写入/邮件发送进入对账或人工边界，不能自动重发或
抹掉本地已接受事实。

## 指标与追踪

指标由 [auction-observability](crates/auction-observability/README.md) 集中适配。Prometheus 0.14.0 关闭默认
`protobuf` 并显式保留 `process`，内部使用上游 `plain_model`，仍经同一个 `TextEncoder` 输出 Prometheus 0.0.4 文本，
不更换 registry、进程/池/tracing collector 或业务 recorder 窄接口；指标名、label、bucket 和 `/metrics` 媒体类型契约
不变。这只移除指标库的二进制编码依赖，不关闭 OTLP/HTTP-proto tracing，也不移除其独立的 prost 编码栈。Tokio 1.53.x
的[上游 LTS 政策](https://github.com/tokio-rs/tokio/)将支持期列至 2027 年 9 月；OpenTelemetry API/SDK/OTLP 配套锁定
0.33.1，tracing-opentelemetry 为 0.34.0。自有有界批处理器直接使用 Tokio，SDK 仅显式启用 `trace`，
不启用未使用的实验性异步批处理器和 SDK Tokio runtime。SQLx 仍可引入 `tokio-stream`，不据此宣称整个锁图消除该包。
OTLP 显式关闭 0.33 新增的默认重试，保留一次导出、失败计数和原预算，请求体保护及配置边界见
[observability owner](crates/auction-observability/README.md#实现布局与-tracing)。

## 依赖与交付纪律

框架评审优先选择已有框架维护升级与 feature 精简，不为追新重写 Axum、SQLx、Dioxus 或持久 Worker；
prometheus-client 需要另审 OpenMetrics 与进程指标兼容，Leptos CSR 需要另审组件/路由/i18n 和唯一构建链。领域只依赖
领域安全库，应用只依赖内层/端口，后端不依赖 Web crate；库错误用 thiserror，anyhow 限装配根，只为实际能力引入依赖，
驱动/Redis 用窄 feature，发布图和静态 all-features 图分别审阅。默认[静态验证](docs/guide/development.md)不执行行为
测试，运行证据见[状态页](docs/status.md)。

### 2026-10-27 最新源码复审中的 feature 精简

核对上游锁定源码与实际装配后，`tracing-opentelemetry` 关闭默认 feature，仅保留 `tracing-log`；本项目只装配 trace
layer，未使用 `MetricsLayer`。锁文件仅删除该包到 `smallvec` 的依赖边，包版本、checksum 与包数不变；其他依赖仍使用
smallvec。`opentelemetry-otlp/http-proto` 自身会启用 metrics，因此没有宣称移除整套 OTel metrics feature。Prometheus
采集、日志归一化、trace exporter 与 TLS 保持原配置。直接依赖用途和工具独立锁的最新检查见[状态页](docs/status.md)。

### 2026-10-27 全仓性能与依赖复核

再次核对工作区直接依赖、重复版本、独立 Public WASM 图及三角色/合并服务的真实发布 feature 图；`cargo machete`
未发现未使用直接依赖。本轮优化使用标准库和现有依赖，不增加、删除或升级依赖，Cargo 清单与锁图保持不变。
已有跨版本传递约束与 feature-only 的 OTLP TLS 依赖仍有用途，不以重写协议或上游内部 API 换取包数下降。
发布隔离门禁已改为检查实际的 PostgreSQL、Redis 与 Web feature，避免默认图漏掉条件依赖。

受控 cargo-audit 刷新 1,380 条 RustSec 公告后，应用锁图为零漏洞、零警告；构建工具仍分别保留完整锁审计结果，
见[工具公告复核](ci/tools/README.md#2026-10-27-公告复核)。这些结论来自本次锁图审计，不替代运行验证。

### 2026-10-27 冗余与死代码复审中的依赖核对

`cargo machete` 仍未发现未使用直接依赖，本轮没有删除或新增直接依赖，也未改变版本与锁图。随无人调用的 `use_media_query`
删除，`auction-web-shared` 不再启用 `web-sys` 的 `MediaQueryList`/`MediaQueryListEvent` feature；断点切换继续只由 CSS 承担。
`auction-integrations` 删除无人调用的 `build_http_client`，rustls/ring provider 安装仍由实际发起请求的适配器负责，
TLS 与 reqwest feature 不变。锁图中 Base64/SHA-2（SQLx）、Tungstenite 0.28（Dioxus devtools）、darling 0.20（croner 的
derive_builder）等重复版本仍来自上游约束，沿用上文不强制合并的决定；`async-stream` 理由同下节，保留。
本轮未刷新 cargo-audit 公告库，不新增公告结论。

### 2026-10-26 性能复审中的依赖核对

`cargo machete` 仍未发现未使用直接依赖；逐项核对低频直接依赖后只收窄一处 feature：`tracing-subscriber` 关闭默认
`ansi`，显式保留 `fmt/std/smallvec/tracing-log` 并继续启用 `env-filter/json`。三个角色日志均为 JSON 且显式关闭 ANSI，
`nu-ansi-term` 因此退出活跃图与锁图，日志格式、过滤器与 OTLP 不变。`async-stream` 只用于 `TrackedPool::fetch_many`
的借用流，替代方案须依赖 sqlx-core 面向驱动的内部接口或手写自引用生成器，因此保留；Public/Admin WASM 中的
`url`/`idna` 也由 dioxus-router 直接依赖，移除本项目调用不会减少载荷，保留。Public 可见性等待改用标准库 `poll_fn`，
不改变 futures-util 在实时流中的使用。本轮未刷新 cargo-audit 公告库，不新增公告结论。

### 2026-10-15 全依赖审计

以下两节保留当时决策与缓解措施，不是当前构建指南；当前版本见[版本与升级](#版本与升级)。当时覆盖 20 个工作区包、50 项直接依赖、完整锁图、全 feature/全 target
活跃图及工具链、dx、CI Actions，MSRV 保持 1.94.1（1.99.1 manifest 当时返回 404，故取 1.99.0）。节点统计与命令结果见
[状态页](docs/status.md#2026-10-15-依赖审计与编译优化)。

| 决定 | 依赖范围 | 理由与兼容边界 |
| --- | --- | --- |
| 安全升级 | rustls 0.23.46；锁图同步 h2、rustls-webpki、hyper-rustls、tokio-rustls、quinn 系列补丁 | 修复下列 RustSec 命中；保留 ring/TLS 1.2/系统 CA，不启用 HTTP/3 |
| 协议与基础库升级 | hyper 1.12.0、hyper-util 0.1.21、ipnet 2.12.2、libc 0.2.190（当时版本）、redis 1.7.1、rust-embed 8.13.0 | 不改 HTTP、缓存故障回源、Web 资源隔离或部署档位 |
| 数据与密码学升级 | base64 0.23.1、hmac 0.13.0、sha2 0.11.1、serde_json 1.0.154、thiserror 2.0.21、toml 1.1.8、uuid 1.28.0 | base64 仅开 std、不直接开启新 SIMD；保留严格引擎、编码与摘要格式；淘汰旧 hmac 0.12 |
| 排期与浏览器升级 | croner 4.0.1、dioxus-i18n 0.6.0、wasm-bindgen 0.2.129、js-sys/web-sys 0.3.106 | cron 先经原严格六字段校验，不开放昵称/宽松语法；项目未用被删除的 I18n::init；CSR/五语言保持 |
| 兼容性保留 | 直接 tungstenite 0.29.0，不升 0.30.0 | Axum 经 tokio-tungstenite 0.29.0 暴露具体错误类型；单独升级破坏 downcast，且增加重复版本 |
| 工具与供应链 | cargo-audit 0.22.2、cargo-machete 0.9.2；checkout 7.0.1、rust-cache 2.9.2 | 审计告警失败、补未用依赖门禁；Actions 固定已核对的完整提交 SHA，保留只读权限和不持久化凭据 |

RustSec 基线命中 rustls 的 [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html)、
[0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html)、
[0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html)、
[0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html)，更新后同一公告库快照为零漏洞/零警告；这些是版本匹配，
不把 ECH/Brotli 等未启用路径写成已证实利用。2026-10-03
历史审计将 rustls/h2/chacha20 锁定到 0.23.45/0.4.19/0.10.2（见[状态页](docs/status.md#2026-10-03-历史记录)），
fxhash 0.2.1 / RUSTSEC-2025-0057 于 2026-10-14 经 scraper 0.27.0 / selectors 0.38.0 改用 rustc-hash 并移出锁图，
`quinn-proto 0.11.17` 定向更新为 `0.11.18`（未开启 HTTP/3，锁图命中不描述为已证实运行时漏洞）。OTel HTTP builder 的
隐式环境头覆盖由配置 owner 增加启用前拒绝门禁，见[配置边界](ops/config/README.md#文件职责)。MPL-2.0 包 cssparser、
cssparser-macros、dtoa-short、selectors 的分发仍须履行许可/源码义务；锁图中的可选旧 reqwest/Gloo、SHA-2 0.10、
Base64 0.22 和 Dioxus 预发布子组件，不通过开启 fullstack、跨不兼容范围 patch 或重写协议清理。

### 2026-10-14 精简审计候选

覆盖 20 个工作区包的清单、实际调用及独立/合并 feature 图；前五项获授权实施，未测量体积或
构建耗时，声明、活跃节点和锁图条目不能互换计数。

| 状态 / owner | 决策与依据 | 边界 |
| --- | --- | --- |
| 已实施 · integrations | scraper 关闭 `main`、保留 `errors`；[论坛解析](crates/auction-integrations/src/forum/parser/html.rs)只用库 API | `getopts`、`unicode-width` 退出活跃图和锁图 |
| 已实施 · HTTP / 装配根 | Axum 关闭未用 `macros/form/query`；[查询映射](crates/auction-http/src/http/routes/mapping/query.rs)自有严格解析 | 保留其余默认 feature 与 `ws`，包括 JSON、MatchedPath、日志、HTTP/2；`axum-macros`、`serde_urlencoded` 退出活跃图，后者仍锁在未启用 fullstack 等路径 |
| 已实施 · observability | [清单](crates/auction-observability/Cargo.toml)删除无调用的直接 `serde`，健康详情只用 `serde_json::Value` | 其他 owner 和传递依赖仍使用 serde |
| 已实施 · Admin Web | [轮询](crates/auction-web-admin/src/app/poll.rs)改用 `std::future::poll_fn`，删直接 futures-util | 不改唤醒/生命周期；Public/Shared/Dioxus 仍需 futures-util |
| 已实施 · Web | gloo-net 根仅留 `http`，Public 加 `json/websocket`，Admin 加 `json`，Shared 只继承 | 独立编译核对，不靠 feature 合并补项 |

三个 WASM 包独立 check 与其他结果见[状态页](docs/status.md#2026-10-14-低风险依赖精简)。锁图残留不代表制品启用，节点
减少不证明 WASM 体积下降；不为包数重写 HTML/CSS、Fluent、URL、TLS、密码学、RESP、WebSocket 或迁移装载器，保留
`sqlx::migrate!` 的 macros/migrate、系统 CA、PostgreSQL/Redis feature 隔离及纯 CSR。

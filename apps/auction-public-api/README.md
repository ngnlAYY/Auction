# auction-public-api

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [docs/contracts/realtime.md](../../docs/contracts/realtime.md) · [ops/config/README.md](../../ops/config/README.md)

公开站点的 HTTP 进程：提供玩家读接口、竞价/代理/标记命令、登录会话、WebSocket 实时推送，并在启用内嵌 feature 的构建中同源提供 Public 浏览器页面（`web-embed` 为 Dioxus，`web-solid` 为 Solid）。本 crate 只做装配——读取配置、连接数据库、把存储适配器注入 `auction-http` 的 `PublicApiState`，业务规则在应用层与领域层。

## feature

| feature | 作用 |
| --- | --- |
| （都不启用） | 只提供 API；两套产物目录都不读取，页面路由返回 503 |
| `web-embed` | 内嵌 Dioxus 版 `target/dx/build/auction-web-public/web/public` 的页面产物；未构建时页面路由返回 503 |
| `web-solid` | 内嵌 Solid 版 `target/web/public/public` 的页面产物（蕴含 `web-embed`）；未构建时页面路由返回 503 |

本地生成 Solid 产物：先运行 `mise run web-prepare` 安装依赖，再运行 `AUCTION_WEB_STACK=solid mise run web-build`；Public 产物写入 `target/web/public/public`，由 `web-solid` 内嵌。两种内嵌 feature 在产物缺失时页面路由都返回 503。

## 启动

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml auction-public-api
cargo run --locked -p auction-public-api --features web-embed   # 需先准备默认运行配置 ops/config/public.toml
mise run run-public                                             # 等价的 mise 入口，按 AUCTION_WEB_STACK 选择 web-embed 或 web-solid；Solid 会注入页面构建号
```

1. 按 `AUCTION_CONFIG_FILE` 或 `ops/config/public.toml` 加载并校验配置（见 [crates/auction-config/README.md](../../crates/auction-config/README.md)）；后者是相对工作目录的默认运行配置位置，仓库模板在 [ops/config/examples](../../ops/config/examples)，不会自动生成该文件。`auction-server` 以显式路径调用 `run_from_path`。两条入口都在监听前校验寄售制品能力。
2. 初始化 JSON 日志、可选 OTLP tracing 与 Prometheus 注册表，订阅 SIGINT/SIGTERM。
3. 经 `auction-http` 的 `HttpListener` 绑定 listener：TCP（默认 `127.0.0.1:8090`）或 Linux Unix socket（受信运行目录 + 实例锁 `.public-api-listener.lock`）；连接服务、请求头超时与排空由 `serve_http` 统一处理。
4. 有数据库时：连接池、心跳发布（10 秒一次，TTL 30 秒）、按 `realtime.consumer_key` 取得 realtime consumer 所有权（PostgreSQL session advisory lock，重复 key 的第二个实例启动失败），装配读端口、竞价命令、会话、标记与 realtime Hub；装备真实链接解析仅在配置 `equipment_data_key` 时装配，缺少该能力时涉及装备链接的非空目录/详情返回 503。没有数据库时数据库业务端口为空；健康、内嵌页面及配置注入的注册 Bot 名单仍可提供，readiness 为未就绪。
5. 有数据库时启动 realtime relay：每 `poll_interval_millis`（默认 250ms）扫描 `auction:` 前缀的事件流，转发给本进程的 WebSocket 连接并持久化消费游标。

有数据库时，Public 用共享存储装配寄售目录、本人历史、登记与管理命令，停用后仍可读历史和重放原命令。登记使用本地 URL 校验和装备数据密钥环，后台解析由 Worker 执行。无数据库的状态接口仍报告配置的有效开关和费率，登记未就绪。预览仅供同时启用 `consignment-dev-preview` 与调试断言的构建使用。 [本地一口价挂单预览](../../docs/development.md#本地一口价挂单预览)

## 就绪与退出

- `/health/ready` 要求完整迁移历史匹配、审计模式门禁通过，且 realtime relay 在最近 `max(4 × poll_interval_millis, 5 秒)` 内扫描成功。默认 shadow 保持全部审计失败阻断并记录影子差异；显式 classified 才只由 permanent + unknown 审计失败阻断。最终 HTTP 判定缓存一秒、并发合并；时限见[docs/contracts/routes.md#健康与指标publicadmin](../../docs/contracts/routes.md#健康与指标publicadmin)。心跳不复用缓存，开关见[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)。
- 收到第一次终止信号后：停止接受新连接、关闭已升级的 WebSocket、发布 stopping 心跳，按 `[shutdown].drain_timeout_seconds`（默认 10 秒）排空；超时或第二次信号时强制中止并以非零状态退出。

## 多实例

指标附加地址也必须按实例区分；一个地址只能绑定一个进程。

可以运行多个 Public 实例（例如在负载均衡之后），但每个实例必须使用不同的 `realtime.consumer_key`；WebSocket Hub 是进程内的，每个实例独立消费事件流。登录与写请求的速率限制同样是进程内计数。

## 部署

附加指标出口由 `common.toml [metrics] public_loopback_listen_address` 选择（示例 `127.0.0.1:19100`），业务 listener 为 Unix socket 时可供 Prometheus 直接抓取。行为见[docs/contracts/routes.md#三角色附加指标出口loopback-tcp](../../docs/contracts/routes.md#三角色附加指标出口loopback-tcp)，字段校验见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)，采集与安装见[ops/README.md#指标](../../ops/README.md#指标)。

分体部署的 unit 示例见 [ops/deploy/systemd/auction-public-api.service.example](../../ops/deploy/systemd/auction-public-api.service.example)，启动顺序与运维流程见[ops/README.md](../../ops/README.md)。

本角色关键段预算只接受 0。二次信号强制非零退出并跳过可选 tracing 刷新；示例 unit 保持 45 秒，配置与外层公式见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。


## 数据库运行时开关

启动装配注入 TOML 就绪基线与指标转发拒绝基线并启动开关刷新任务，任务随进程生命周期收口；同一 `RuntimeSwitches` 供 HTTP 就绪缓存、业务 `/metrics` 与附加出口在请求边界读取。开关闭集、刷新与失败语义见[ops/config/README.md#数据库运行时开关](../../ops/config/README.md#数据库运行时开关)，核对与回退见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

## 数据库连接档位

profiled 模式使用 `auction-public`；`[database_pool]` 默认物理上限 16，本角色长期预留 1（realtime consumer 所有权），扣除后至少剩 1 个普通工作连接。服务超时的启用条件、修改方式与 legacy 回退见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。

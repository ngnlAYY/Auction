# auction-public-api

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [docs/contracts/realtime.md](../../docs/contracts/realtime.md) · [ops/config/README.md](../../ops/config/README.md)

公开站点的 HTTP 进程：提供玩家读接口、竞价/代理/标记命令、登录会话、WebSocket 实时推送，并在 `web-embed` 构建中同源提供 Public 浏览器页面。本 crate 只做装配——读取配置、连接数据库、把存储适配器注入 `auction-http` 的 `PublicApiState`，业务规则在应用层与领域层。

## feature

| feature | 作用 |
| --- | --- |
| `web-embed` | 内嵌 `target/dx/build/auction-web-public/web/public` 的页面产物；未启用时使用固定空资源目录（页面路由返回 503） |

## 启动

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml auction-public-api
cargo run --locked -p auction-public-api --features web-embed   # 需先准备默认运行配置 ops/config/public.toml
```

1. 按 `AUCTION_CONFIG_FILE` 或 `ops/config/public.toml` 加载并校验配置（见 [crates/auction-config/README.md](../../crates/auction-config/README.md)）；后者是相对工作目录的默认运行配置位置，仓库模板在 [ops/config/examples](../../ops/config/examples)，不会自动生成该文件。`auction-server` 以显式路径调用 `run_from_path`。
2. 初始化 JSON 日志、可选 OTLP tracing 与 Prometheus 注册表，订阅 SIGINT/SIGTERM。
3. 经 `auction-http` 的 `HttpListener` 绑定 listener：TCP（默认 `127.0.0.1:8090`）或 Linux Unix socket（受信运行目录 + 实例锁 `.public-api-listener.lock`）；连接服务、请求头超时与排空由 `serve_http` 统一处理。
4. 有数据库时：连接池、心跳发布（10 秒一次，TTL 30 秒）、按 `realtime.consumer_key` 取得 realtime consumer 所有权（PostgreSQL session advisory lock，重复 key 的第二个实例启动失败），装配读端口、竞价命令、会话、标记与 realtime Hub；装备真实链接解析仅在配置 `equipment_data_key` 时装配，缺少该能力时涉及装备链接的非空目录/详情返回 503。没有数据库时数据库业务端口为空；健康、内嵌页面及配置注入的注册 Bot 名单仍可提供，readiness 为未就绪。
5. 有数据库时启动 realtime relay：每 `poll_interval_millis`（默认 250ms）扫描 `auction:` 前缀的事件流，转发给本进程的 WebSocket 连接并持久化消费游标。

## 就绪与退出

- `/health/ready` 要求完整迁移历史匹配、审计模式门禁通过，且 realtime relay 在最近 `max(4 × poll_interval_millis, 5 秒)` 内扫描成功。默认 shadow 保持全部审计失败阻断并记录影子差异；显式 classified 才只由 permanent + unknown 审计失败阻断。最终 HTTP 判定缓存一秒、并发合并；时限见[docs/contracts/routes.md#健康与指标publicadmin](../../docs/contracts/routes.md#健康与指标publicadmin)。心跳不复用缓存，开关见[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)。
- 收到第一次终止信号后：停止接受新连接、关闭已升级的 WebSocket、发布 stopping 心跳，按 `[shutdown].drain_timeout_seconds`（默认 10 秒）排空；超时或第二次信号时强制中止并以非零状态退出。

## 多实例

指标附加地址也必须按实例区分；一个地址只能绑定一个进程。

可以运行多个 Public 实例（例如在负载均衡之后），但每个实例必须使用不同的 `realtime.consumer_key`；WebSocket Hub 是进程内的，每个实例独立消费事件流。登录与写请求的速率限制同样是进程内计数。

## 部署

`common.toml [metrics] public_loopback_listen_address` 省略即关闭；
只有有效 `metrics.enabled = true` 且填写地址才开启附加 `/metrics` 出口。
示例为 `127.0.0.1:19100`，可供 Unix socket 部署直接被 Prometheus 抓取。
出口只持有同一注册表，复用业务指标契约；绑定/服务失败仅记录固定类别 ERROR，
不影响业务 readiness。任务随进程退出取消；删除地址并重启可回退。
校验规则见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)，
采集与安装步骤见[ops/README.md#指标](../../ops/README.md#指标)。

分体部署的 unit 示例见 [ops/deploy/systemd/auction-public-api.service.example](../../ops/deploy/systemd/auction-public-api.service.example)，启动顺序与运维流程见[ops/README.md](../../ops/README.md)。

本角色关键段预算只接受 0。二次信号强制非零退出并跳过可选 tracing 刷新；示例 unit 保持 45 秒，配置与外层公式见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。


## 数据库运行时开关

启动装配注入 TOML 就绪基线与指标转发拒绝基线，并立即读取数据库开关，
随后每五秒刷新、单次一秒预算，任务随进程生命周期收口。审计模式使用三角色
共享规则；Public/Admin 另接入 HTTP 就绪缓存，Worker 负责到期收口。
同一 `RuntimeSwitches` 同时供业务 `/metrics` 与 loopback 出口在请求边界读取；
`metrics_reject_forwarded_requests` 只允许在线收紧。失败保留原快照及期限，
不清零审计累计。核对与回退见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

## 数据库连接档位

profiled 模式使用 `auction-public`，本角色 `[database_pool]` 默认物理上限 16。
长期预留为 1，扣除后至少剩 1 个普通工作连接；服务超时默认关闭。
超时必须显式填写阈值，并经审计分类影子观察、由用户切换三角色有效 classified 后才启用。
池与长期会话参数修改需经停机门禁重启，本批不接入在线开关。
配置与 legacy 回退见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。

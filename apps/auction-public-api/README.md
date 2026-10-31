# auction-public-api

[文档导航](../../docs/README.md) · [HTTP 路由](../../docs/contracts/routes.md) · [实时事件](../../docs/contracts/realtime.md) · [配置参考](../../ops/config/README.md)

公开站点的 HTTP 进程：提供玩家读接口、竞价/代理/标记命令、登录会话、WebSocket 实时推送，并在 `web-embed` 构建中同源提供 Public 浏览器页面。本 crate 只做装配——读取配置、连接数据库、把存储适配器注入 `auction-http` 的 `PublicApiState`，业务规则在应用层与领域层。

## feature

| feature | 作用 |
| --- | --- |
| `postgres` | 连接 PostgreSQL 并装配全部端口；未启用时配置了数据库会拒绝启动 |
| `web-embed` | 内嵌 `target/dx/build/auction-web-public/web/public` 的页面产物；未启用时使用固定空资源目录（页面路由返回 503） |
| `cache-redis` | 允许 `cache.backend = "redis"`（公开规则读模型缓存） |

## 启动

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml auction-public-api
cargo run --locked -p auction-public-api --features postgres,web-embed   # 需先准备默认运行配置 ops/config/public.toml
```

1. 按 `AUCTION_CONFIG_FILE` 或 `ops/config/public.toml` 加载并校验配置（见 [auction-config](../../crates/auction-config/README.md)）；后者是相对工作目录的默认运行配置位置，仓库模板在 [`ops/config/examples/`](../../ops/config/examples)，不会自动生成该文件。`auction-server` 以显式路径调用 `run_from_path`。
2. 初始化 JSON 日志、可选 OTLP tracing 与 Prometheus 注册表，订阅 SIGINT/SIGTERM。
3. 绑定 listener：TCP（默认 `127.0.0.1:8090`）或 Linux Unix socket（受信运行目录 + 实例锁 `.public-api-listener.lock`）。
4. 有数据库时：连接池、心跳发布（10 秒一次，TTL 30 秒）、按 `realtime.consumer_key` 取得 realtime consumer 所有权（PostgreSQL session advisory lock，重复 key 的第二个实例启动失败），装配读端口、竞价命令、会话、标记与 realtime Hub；装备真实链接解析仅在配置 `equipment_data_key` 时装配，缺少该能力时涉及装备链接的非空目录/详情返回 503。没有数据库时数据库业务端口为空；健康、内嵌页面及配置注入的注册 Bot 名单仍可提供，readiness 为未就绪。
5. 有数据库时启动 realtime relay：每 `poll_interval_millis`（默认 250ms）扫描 `auction:` 前缀的事件流，转发给本进程的 WebSocket 连接并持久化消费游标。

## 就绪与退出

- `/health/ready` 要求数据库就绪（迁移版本等于本二进制期望版本、无审计写入失败）且 realtime relay 在最近 `max(4 × poll_interval_millis, 5 秒)` 内扫描成功。
- 收到第一次终止信号后：停止接受新连接、关闭已升级的 WebSocket、发布 stopping 心跳，在 10 秒内排空；超时或第二次信号时强制中止并以非零状态退出。

## 多实例

可以运行多个 Public 实例（例如在负载均衡之后），但每个实例必须使用不同的 `realtime.consumer_key`；WebSocket Hub 是进程内的，每个实例独立消费事件流。登录与写请求的速率限制同样是进程内计数。

## 部署

分体部署的 unit 示例见 [`ops/deploy/systemd/auction-public-api.service.example`](../../ops/deploy/systemd/auction-public-api.service.example)，启动顺序与运维流程见[运维手册](../../ops/README.md)。

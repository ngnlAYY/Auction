# auction-public-api

公开端进程装配根。

源码布局：src/main.rs 只负责 Tokio 入口，src/bootstrap/mod.rs 负责配置、缓存、监听器和
路由装配。

职责仅限于：加载配置、初始化 tracing、装配缓存和数据库安全心跳、装配 Public realtime
relay/Hub、绑定监听器、挂载 `auction_http::public_router()` 和返回启动错误。Public 的
`[tracing]` 使用主配置，OTLP headers 使用敏感 TOML 的 Public 专属字段；exporter 采用有界
异步队列。周期心跳每轮读取数据库/schema readiness、进程内审计写入失败计数和 Public tracing
安全状态。进程通过 `auction-process` 统一监听 SIGINT/SIGTERM；首次信号触发 Axum graceful
shutdown，停止 heartbeat/realtime relay 并发布停止心跳，整个 HTTP/后台任务 drain 最多 10 秒，
排空期间第二次信号会强制收口，最后按配置限时刷新 exporter。稳定退出码仍待统一。
**业务逻辑不属于这里。** relay 只负责当前 Public 实例的
持久化事件分发，外部广播 sink 仍由独立协议负责。

默认监听 `127.0.0.1:8090`。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api
```

## 前端产物

公开端 SPA 由 `auction-http` 内嵌，构建命令：

```bash
just web-build-public
```

未构建产物时进程仍可正常启动，`/health` 与 `/api/v1/health` 照常响应，只有静态资源路由返回
503。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。仅在配置里填 `redis_url` 不会引入该依赖。
- `postgres` / `sqlite`：分别编译对应数据库适配器；数据库 URL 的 scheme 必须与制品 feature
  一致。未编译数据库 feature 时，含 `database_url` 的配置会在启动装配阶段明确失败。

## 边界约束

公开端**不得**依赖管理端 UI 模块或管理专属命令。Public `RustEmbed` 类型由本装配根独占，
`auction-http` 只接收该泛型资源类型；依赖图不引用 Admin 产物，不依赖链接器裁剪保证制品隔离。

# auction-public-api

公开端进程装配根。

源码布局：`src/main.rs` 只负责 Tokio 入口；`src/bootstrap/mod.rs` 负责模块装配和稳定启动
入口；`bootstrap/service.rs` 负责 tracing、HTTP 生命周期与退出 drain；`bootstrap/state.rs`
负责数据库、端口、公开真实装备 URL、heartbeat 和 realtime 状态；`bootstrap/config.rs` 负责安全策略与缓存配置；
`bootstrap/listener.rs` 负责监听器绑定。

职责仅限于：加载配置、初始化 tracing、装配缓存和数据库安全心跳、装配 Public realtime
relay/Hub、绑定监听器、挂载 `auction_http::public_router()` 和返回启动错误。Public 的
`[tracing]` 使用主配置，OTLP headers 使用敏感 TOML 的 Public 专属字段；exporter 采用有界
异步队列。周期心跳每轮读取数据库/schema readiness、进程内审计写入失败计数和 Public tracing
安全状态。进程通过 `auction-process` 统一监听 SIGINT/SIGTERM；首次信号触发 Axum graceful
shutdown，停止 heartbeat/realtime relay 并发布停止心跳，整个 HTTP/后台任务 drain 最多 10 秒，
排空期间第二次信号会强制收口，最后按配置限时刷新 exporter。稳定退出码仍待统一。
**业务逻辑不属于这里。** relay 只负责当前 Public 实例的
持久化事件分发，外部广播 sink 仍由独立协议负责。

relay 的目录读取和消费确认共用持锁数据库连接；PostgreSQL 持锁连接失效时停止推进游标并
撤销 readiness 证明，不以新池连接继续消费。数据库恢复后需要重启该 Public 实例重新取得
owner；SQLite 文件库在实际数据库同目录保持 sidecar 文件锁，不依赖 `/tmp`。运行账号需
能在可信数据库目录创建 0600 锁文件；锁文件退出时保留，数据库不得使用硬链接别名。
文件库 owner 还要求 Linux `statx` mount 身份，拒绝单文件 bind mount；可挂载整个数据库目录。

默认监听 `127.0.0.1:8090`。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api
```

## 前端产物

公开端 SPA 由 `auction-http` 内嵌，构建命令：

```bash
just web-build-public
just build-release
```

`web-embed` 只在内嵌发布构建中显式启用。API-only 制品始终使用固定空资源集合，即使构建机上
存在旧的 `target/dx` 也不会误嵌入；进程仍可正常启动，`/health` 与 `/api/v1/health` 照常响应，
只有静态资源路由返回 503。

PostgreSQL 完整档会使用敏感 TOML 中与装备密文信封匹配的 `equipment_data_key_id`/
`equipment_data_key` 装配真实装备 URL 解析器；两项都缺失时 P09/P11 lot 读取返回 503，成对配置但
无效时启动失败关闭。PostgreSQL 只在最终 `no-store` DTO 映射时短暂生成完整 URL；SQLite Lite
继续返回 lot 投影，但链接字段留空，不伪造可用 URL。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。仅在配置里填 `redis_url` 不会引入该依赖。
- `postgres` / `sqlite`：分别编译对应数据库适配器；数据库 URL 的 scheme 必须与制品 feature
  一致。未编译数据库 feature 时，含 `database_url` 的配置会在启动装配阶段明确失败。
- `web-embed`：只嵌入 Public 的优化 Web payload；默认关闭，API-only 发布不得启用。

## 边界约束

公开端**不得**依赖管理端 UI 模块或管理专属命令。Public `RustEmbed` 类型由本装配根独占，
`auction-http` 只接收该泛型资源类型；依赖图不引用 Admin 产物，不依赖链接器裁剪保证制品隔离。

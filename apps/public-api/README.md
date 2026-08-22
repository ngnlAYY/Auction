# auction-public-api

公开端进程装配根。

源码布局：src/main.rs 只负责 Tokio 入口，src/bootstrap/mod.rs 负责配置、缓存、监听器和
路由装配。

职责仅限于：加载配置、初始化 tracing、装配缓存和数据库安全心跳、绑定监听器、挂载
`auction_http::public_router()`、优雅关闭和退出码。**业务逻辑不属于这里。**

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

公开端**不得**依赖管理端 UI 模块或管理专属命令。本进程不包含管理端静态资源。

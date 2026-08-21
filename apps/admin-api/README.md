# auction-admin-api

管理端进程装配根。

职责仅限于：加载配置、初始化 tracing、装配缓存、绑定监听器、挂载
`auction_http::admin_router()`、优雅关闭和退出码。

默认监听 `127.0.0.1:8091`，与公开端不同端口。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/admin.toml \
  cargo run -p auction-admin-api
```

## 前端产物

管理端 SPA 由 `auction-http` 内嵌，构建命令：

```bash
just web-build-admin
```

产物与公开端完全隔离：本进程不包含公开端资源，公开端进程也不包含管理端资源。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。

## 边界约束

Public 与 Admin 是不同表面，可以分别绑定端口、Unix socket 或反向代理。管理命令必须走版本化
REST 契约并留下审计记录；凭据与密钥材料只在服务端处理，不进入浏览器。

# auction-admin-api

管理端进程装配根。

源码布局：src/main.rs 只负责 Tokio 入口，src/bootstrap/mod.rs 负责配置、缓存、监听器和
路由装配。

职责仅限于：加载配置、初始化 tracing、装配缓存、Admin session、拍卖生命周期/ recovery 确认命令
端口和 PostgreSQL 结算/recovery 报告读取端口、绑定监听器、挂载
`auction_http::admin_router()`、优雅关闭和退出码。Admin Basic Auth 凭据从敏感 TOML 读取，
session 与 CSRF 只以摘要形式持久化；生命周期命令在数据库事务内追加状态版本、审计和 realtime 事件。
结算报告和 recovery 报告只读取已提交事实；recovery 确认还会在 PostgreSQL Serializable 事务中
重校验证据、激活退款、guarded relist 并追加审计；SQLite Lite 不提供这些完整档能力并返回明确的 501。
Admin 论坛预览由本地 `ForumPreviewAdapter` 解析 HTML/文本，限制 2 MiB 输入并返回安全预览；
它不执行远程论坛发布，也不产生数据库写入。
Admin 审计读取由数据库适配器提供两种有界分页形状和安全 metadata 投影；该接口只读，不修改
审计历史。
Admin dashboard 读取由 `auction-observability::HealthService` 聚合 service heartbeat，
三个进程装配根负责发布启动/就绪/周期/停止心跳；返回 health/process 安全投影。其他尚未迁移的
owner section 显式标记为 `unknown`，不伪造 Worker control 或邮件状态。
Admin runtime 配置由主配置的 `runtime_config_file` 选择；PostgreSQL 完整档装配
`auction-application` runtime 服务、TOML 原子文件适配器和 durable 命令账本，GET/validate/PUT
均保留 fingerprint fence 与 `restart_required`。文件更新不会热改当前进程，SQLite Lite 明确
返回 501。

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
- `postgres`：编译 PostgreSQL 完整存储适配器。
- `sqlite`：编译 SQLite Lite 存储适配器。

## 边界约束

Public 与 Admin 是不同表面，可以分别绑定端口、Unix socket 或反向代理。管理命令必须走版本化
REST 契约并留下审计记录；凭据与密钥材料只在服务端处理，不进入浏览器。

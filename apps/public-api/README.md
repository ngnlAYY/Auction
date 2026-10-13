# auction-public-api

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [HTTP 契约](../../docs/contracts/routes.md) · [部署](../../ops/deploy/README.md)

公开 HTTP、会话、实时 relay 与 Public 静态页面的进程装配根。
`src/lib.rs` 导出 `run`、`run_from_path`；独立 `main.rs` 和[合并服务](../server/README.md)的 Public 子进程复用同一入口。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](src/bootstrap) | 配置、依赖、监听、心跳与关闭顺序 |
| [src/web_assets](src/web_assets) | Public 专属内嵌资源类型 |

## 职责与实现边界

只加载配置、装配应用端口与适配器、挂载 `auction_http::public_router()` 并管理进程生命周期。
竞价规则、事务和 DTO 映射分别由 domain/application、storage、http 拥有。

## 装配与依赖

`bootstrap/service.rs` 拥有 tracing 与 HTTP 生命周期，`state.rs` 装配数据库、端口、装备 URL、心跳和 realtime，
`config.rs` 映射安全/缓存策略，`listener.rs` 绑定 TCP 或 Linux Unix socket。

PostgreSQL 的真实装备 URL 需要成对有效的 `equipment_data_key_id`/`equipment_data_key`；缺失时 P09/P11 返回 503，
无效时启动失败。URL 只在最终 `no-store` DTO 映射时生成；SQLite Lite 保留 lot 投影但链接为空。
OTLP headers 只从敏感 TOML 读取。周期心跳报告数据库/schema、审计写入失败计数和 tracing 安全状态。

## 持锁 relay 与故障边界

relay 的目录读取和消费确认共用持锁连接。PostgreSQL owner 连接失效后停止推进游标并撤销 readiness；
数据库恢复后须重启该实例重新取得 owner，不能换池连接继续消费。
SQLite 文件库在数据库同目录保留 0600 sidecar 锁；运行账号须能创建它，数据库不能使用硬链接别名。
文件库 owner 要求 Linux `statx` mount 身份，拒绝单文件 bind mount，可挂载整个数据库目录。
consumer 标识与缓冲配置见[配置说明](../../ops/config/README.md#public-实时-relay)。

## 监听与运行配置

默认 `127.0.0.1:8090`；配置准备和显式启动见[开发指南](../../docs/development.md#配置准备与显式运行)。
普通启动不迁移数据库。SIGINT/SIGTERM 触发 HTTP 停止接收、heartbeat/relay 停止和停止心跳，
整个 drain 最多 10 秒；第二次信号强制收口，随后按配置限时刷新 exporter。稳定退出码仍待统一。

## 前端产物

Public SPA 由本 app 的 `RustEmbed` 类型内嵌，再交给 `auction-http` 提供。
`just web-build-public` 生成页面，`just build-release` 构建内嵌发布制品。
API-only 使用固定空资源集合；即使构建机残留 `target/dx`，也不会误嵌入。
缺少页面只使静态路由返回 503，不阻止 API 和健康接口工作。

## Cargo feature

默认 feature 为空；`postgres` / `sqlite` 选择数据库适配器，`cache-redis` 编译可选 Redis，
`web-embed` 只包含 Public payload。数据库配置必须匹配已编译驱动，详见[制品选择](../../docs/development.md#选择制品-feature)。

## 边界约束

不依赖 Admin UI、管理命令或 Admin 资源类型；隔离由依赖图和显式 feature 保证，不依赖链接器裁剪。
relay 只分发本实例的持久化事件，未实现的外部广播 sink 不属于其能力声明。

## 维护与验证

本模块可用 `cargo check --locked -p auction-public-api` 静态编译；数据库路径须另外选择对应 feature。
完整门禁见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。

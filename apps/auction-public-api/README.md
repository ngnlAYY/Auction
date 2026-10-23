# auction-public-api

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [HTTP 契约](docs/contracts/routes.md) · [部署](ops/README.md)

公开 HTTP、会话、实时 relay 与 Public 静态页面的进程装配根。`src/lib.rs` 导出 `run`、`run_from_path`；
独立 `main.rs` 与[合并服务](apps/auction-server/README.md)的 Public 子进程复用同一入口。仅支持 Linux x86_64 原生目标，
装配入口在编译期拒绝其他 OS/架构；浏览器 WASM 不受此限，默认发布 ABI 见[开发指南](docs/guide/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](apps/auction-public-api/src/bootstrap) | 配置、依赖、监听、心跳与关闭顺序 |
| [src/web_assets](apps/auction-public-api/src/web_assets) | Public 专属内嵌资源类型 |

## 装配与边界

只加载配置、装配应用端口与适配器、挂载 `auction_http::public_router()` 并管理进程生命周期；
竞价规则、事务与 DTO 映射分属 domain/application、storage、http。`bootstrap/` 中 `service.rs` 拥有 tracing 与 HTTP 生命周期，
`state.rs` 装配数据库、端口、装备 URL、心跳和 realtime，`config.rs` 映射安全/缓存策略，`listener.rs` 绑定 TCP 或 Linux Unix socket。
与 Admin 保持独立路由、监听、静态产物和资源类型；隔离由依赖图和显式 feature 保证，不依赖链接器裁剪。
装备 URL 需成对有效的 `equipment_data_key_id`/`equipment_data_key`：缺失时 P09/P11 返回 503，无效时启动失败，
URL 只在最终 `no-store` DTO 映射时生成；OTLP headers 只从敏感 TOML 读取。
部署预检同时比较 Public/Admin/Worker 的装备密钥 ID/材料；全部省略仍表示能力缺失，不自动生成或放宽解密要求，
见[部署预检](apps/auctionctl/README.md#部署预检)。
目录读取端口由 `DatabaseStore.public_lot_reader` 装配，使用共享 runtime 的装备规则启动快照判定稀有/融合，
公开名称保持原样；规则同源、旧配置升级与重启范围见[配置手册](ops/config/README.md#runtime-共享与重启)。
`PublicApiState::with_lot_watches`、`with_lot_watch_preference` 注入同一 PostgreSQL store 支撑标记与 P25/P26，
未编译或未装配数据库时这些接口明确 503，不增加进程、配置或外部依赖。

## 持锁 relay 与故障边界

relay 的目录读取和消费确认共用持锁连接；PostgreSQL owner 连接失效后停止推进游标并撤销 readiness，
恢复后须重启该实例重新取得 owner，不能换池连接继续消费。consumer 标识与缓冲见[配置说明](ops/config/README.md#public-实时-relay)；
relay 只分发本实例的持久化事件，未实现的外部广播 sink 不属于其能力声明。

## 运行、产物与 Cargo feature

默认 `127.0.0.1:8090`；配置准备与显式启动见[开发指南](docs/guide/development.md#配置准备与显式运行)。
普通启动不迁移数据库。SIGINT/SIGTERM 停止接收 HTTP、heartbeat/relay 与停止心跳，drain 最多 10 秒，
第二次信号强制收口，随后按配置限时刷新 exporter；稳定退出码仍待统一。
周期心跳跳过错过的 tick，依赖恢复后只发布当前状态，不集中补发过期观测。
SPA 由本 app 的 `RustEmbed` 类型内嵌、再交 `auction-http` 提供；`bash ci/release/web-bundle.sh public` 生成页面，
`just build-release` 构建内嵌发布制品。API-only 使用固定空资源集合，构建机残留 `target/dx` 不会误嵌入；
缺页面只让静态路由返回 503，不阻止 API 和健康接口。

默认 feature 为空；`postgres` 启用数据库适配器，`cache-redis` 编译可选 Redis，`web-embed` 只包含 Public payload。
数据库配置必须匹配已编译驱动，见[制品选择](docs/guide/development.md#选择制品-feature)。

## 维护与验证

`cargo check --locked -p auction-public-api` 只做默认 feature 静态编译，数据库路径须另选对应 feature。
完整门禁见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。

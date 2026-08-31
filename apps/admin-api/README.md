# auction-admin-api

管理端进程装配根。

源码布局：`src/main.rs` 只负责 Tokio 入口，`src/bootstrap/mod.rs` 只负责启动装配入口；
`src/bootstrap/service.rs` 负责 tracing、HTTP 生命周期和有界 drain，`state.rs` 负责数据库/端口/
heartbeat 状态装配，`config.rs` 负责缓存、Worker control、安全策略和 dashboard 范围投影，
`listener.rs` 负责监听器绑定与服务。这样装配根不把状态构造、配置映射和退出流程混在一个文件中。

职责仅限于：加载配置、初始化 tracing、装配缓存、Admin session、拍卖生命周期/ recovery 确认命令
端口和 PostgreSQL 结算/recovery 报告读取端口、绑定监听器、挂载
`auction_http::admin_router()` 和返回启动错误。Admin Basic Auth 凭据从敏感 TOML 读取，
session 与 CSRF 只以摘要形式持久化；生命周期命令在数据库事务内追加状态版本、审计和 realtime 事件。
结算报告和 recovery 报告只读取已提交事实；recovery 确认还会在 PostgreSQL Serializable 事务中
重校验证据、激活退款、guarded relist 并追加审计；SQLite Lite 不提供这些完整档能力并返回明确的 501。
Admin 论坛预览由本地 `ForumPreviewAdapter` 解析 HTML/文本，限制 2 MiB 输入并返回安全预览；
它不执行远程论坛发布，也不产生数据库写入。
Admin 审计读取由数据库适配器提供两种有界分页形状和安全 metadata 投影；该接口只读，不修改
审计历史。
Admin dashboard 读取由 `auction-observability::HealthService` 聚合 service heartbeat，
三个进程装配根已有启动、就绪、周期和 SIGINT/SIGTERM 停止心跳路径；返回 health/process 安全
投影，周期值每轮读取真实数据库/schema readiness、进程内审计写入失败计数和对应 tracing 安全
状态。Admin 同时
通过 runtime 文档指定的 Worker control Unix socket 读取 live/ready/status，socket 只允许
命中 Worker 显式 UID/GID allowlist 的内核认证 peer，失败时显示有限 condition；mail、realtime、credential、delivery、fulfillment、
recovery 和 alerting section 分别由对应 owner 提供安全摘要，owner 未装配时才显式标记为 `unknown`。
Admin runtime 配置由主配置的 `runtime_config_file` 选择；PostgreSQL 完整档装配
`auction-application` runtime 服务、TOML 原子文件适配器和 durable 命令账本，GET/validate/PUT
均保留 fingerprint fence 与 `restart_required`；Admin 对外就绪前会先恢复未收口的 pending/unknown
命令，若结果为 `unknown` 或需要人工核对则保留错误级告警并继续启动，其他恢复错误会阻断启动。
文件更新不会热改当前进程，SQLite Lite 明确返回 501。runtime 可选的 `dashboard_mailboxes`
是非敏感 world/mailbox 投影，Admin 用它核对 MoogleMail poller 数量、邮箱标识和凭据范围；旧版
`worlds`、发件人 allowlist、装备过滤和 14 个通知模板也由该文档管理，并在 Worker 重启时投影为
MoogleMail 业务快照。因此业务区块更新要求重启受影响的 Admin 与 Worker。
重启后，`allow_manual_early_open` 与 `allow_manual_early_settlement` 会作为不可变策略进入生命周期
服务，且默认均关闭：计划时间已到的开放不受影响，人工提前开放与提前结算则在锁内按同一策略失败
关闭，HTTP capability 使用同一来源。人工进入 Final 会把较晚的 live lot 截止时间收紧到冻结的最终
窗口（默认 15 分钟），保留竞价与软截止延期；提前结算则要求理由并仅在 PostgreSQL 完整档把截止时间
收紧到当前事务时间，实际结算仍由 Worker 领取。`[equipment_source]` 的 host/path allowlist、响应上限、共享请求间隔、总超时和
`require_tradeable` 也会映射到装备来源适配器及手工录入策略；该开关默认关闭，关闭时允许 Admin
录入明确标记为 Soulbound 的装备，开启时恢复只接受可交易装备的旧限制。MoogleMail 邮件装备仍由
独立过滤策略拒绝不可交易来源，不受该 Admin 开关影响。适配器在启动时读取 A32 中目标 world 或 shared scope 的 active `mail_cookie`，
并在远端明确返回登录页时轮换候选、回写失效状态。没有数据库候选时才兼容敏感配置的单 Cookie
入口；凭据变更与其他启动快照一样需要重启 Admin。固定 HTTPS origin、禁用重定向和精确 world
路径校验始终保留。

Admin 已通过 `auction-process` 统一监听 SIGINT/SIGTERM；首次信号触发 TCP/Unix Axum graceful
shutdown、停止 heartbeat 并发布停止状态，整个 HTTP/后台任务 drain 最多 10 秒，排空期间第二次
信号会强制收口，最后按配置限时刷新 tracing exporter。稳定退出码仍待统一。

默认监听 `127.0.0.1:8091`，与公开端不同端口。Admin runtime 的 `listener.mode` 支持 TCP；
Linux 部署还支持 Unix socket。Unix 模式要求 `runtime_directory` 预先存在并由运行账号拥有，
`socket_path` 必须直接位于该目录中，启动时会取得目录级单实例锁，并将 socket 固定为配置的
group 与 `0660` 权限。检测到仍在使用的 socket、非安全路径或不满足 owner/group 的目录时，
启动会失败；失效的 stale socket 只会在身份校验通过后清理。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/admin.toml \
  cargo run -p auction-admin-api
```

## 前端产物

管理端 SPA 由本装配根的 `RustEmbed` 类型内嵌，再交给 `auction-http` 提供，构建命令：

```bash
just web-build-admin
just build-release
```

路由只提供管理端产物；本装配根不引用 Public `RustEmbed` 类型，因此制品级资源隔离不依赖
链接器裁剪。`web-embed` 默认关闭，API-only 制品使用固定空资源集合并以 503 报告静态资源未包含；
内嵌发布构建才显式选择 Admin 的优化 payload。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。
- `postgres`：编译 PostgreSQL 完整存储适配器。
- `sqlite`：编译 SQLite Lite 存储适配器。
- `web-embed`：只嵌入 Admin 的优化 Web payload；默认关闭，API-only 发布不得启用。

## 边界约束

Public 与 Admin 是不同表面，可以分别绑定端口、Unix socket 或反向代理。管理命令必须走版本化
REST 契约并留下审计记录；凭据与密钥材料只在服务端处理，不进入浏览器。

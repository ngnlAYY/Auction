# auction-admin-api

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [HTTP 契约](../../docs/contracts/routes.md) · [部署](../../ops/deploy/README.md)

管理 HTTP、会话和 runtime 服务的进程装配根。`src/lib.rs` 导出 `run`、`run_from_path` 与 Admin 资源类型；
独立 `main.rs` 和[合并服务](../server/README.md)的 Admin 子进程复用装配。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](src/bootstrap) | 管理端口、首次模式选择、runtime 恢复与生命周期 |
| [src/web_assets](src/web_assets) | Admin 专属内嵌资源类型 |

## 职责与实现边界

只加载配置、装配管理应用端口与适配器、挂载 `auction_http::admin_router()` 并管理进程生命周期。
管理命令、事务和外部服务调用由各自 owner 实现，不能放入装配根。

## 装配与依赖

`bootstrap/service.rs` 拥有 tracing 与 HTTP 生命周期，`state.rs` 装配数据库/端口/心跳，
`config.rs` 映射缓存、Worker control、安全和 dashboard 范围，`listener.rs` 绑定监听器。
`first_run.rs` 在正式 loader 前选择模式：内嵌 PostgreSQL Web 制品只有在无默认配置且未显式设置
`AUCTION_CONFIG_FILE` 时自动开放 loopback 设置页；也可显式执行 `setup --output-dir <绝对路径>`。
损坏、缺失的显式配置、已有残缺目录或符号链接都不会降级为设置页。分体设置保存后退出，
不自动迁移或启动其他角色；完整操作见[首次部署](../../ops/deploy/README.md#首次部署配置)。

Admin 凭据只从敏感 TOML 读取。每次启动在 session 服务就绪前永久撤销旧 Admin 会话，失败阻断启动；
token 摘要绑定当前 actor/secret，Public 会话不受影响。多实例轮换前须停止全部旧凭据实例。

## 管理查询和命令能力

装配生命周期、业务 world、规则、审计、凭据、结算/恢复报告等窄端口；精确路由与错误码见 HTTP 契约。
论坛预览只在本地解析不超过 2 MiB 的输入，不远程发布或写库。业务 world 状态来自数据库。
PostgreSQL 提供完整写入与报告；SQLite Lite 对未支持的 world 写入、settlement/recovery 等能力返回 501。
生命周期和 recovery 确认沿用应用/存储 owner 的锁、幂等、追加审计与持久化事件边界。

## Dashboard 与可观测性

`HealthService` 聚合三角色心跳；Admin 另通过内核 peer 认证的 Worker control Unix socket 读取 live/ready/status。
mail、realtime、credential、delivery、fulfillment、recovery 与 alerting 各自提供安全摘要，缺失 owner 才显示 `unknown`。
心跳每轮读取数据库/schema readiness、审计失败计数和 tracing 状态；socket 不可用只产生有限 condition。

## Runtime 恢复与启动快照

PostgreSQL runtime 服务组合应用端口、TOML 原子替换和 durable 命令账本，保留 fingerprint fence 与
`restart_required`。就绪前恢复 pending/unknown 命令：未知结果或需人工核对时保留错误告警，其他恢复错误阻断启动。
恢复后文件指纹若不同于已加载配置，必须重新部署重启，不能用旧监听/策略快照报告新配置生效。
SQLite Lite 的 HTTP 更新返回 501；文件保存不会热改当前进程。

共享业务区块、六组 `operations`、重启范围及默认值统一见[配置说明](../../ops/config/README.md#runtime-共享与重启)。
新经济模板只影响后续拍卖，人工提前开放/结算由启动快照和事务内策略共同约束。
Admin 装备来源的 `require_tradeable` 默认关闭，仅影响手工录入；MoogleMail 过滤独立执行。

装备来源适配器使用启动时的唯一 active `mail_cookie`，无数据库记录时才兼容敏感文件单 Cookie；
远端明确登录失效后标记失效，不自动切换。A44 手动只读校验则每次读取指定凭据 ID 的当前信封，
不复用该快照或回退配置；缺密钥和 Lite 档保持 501，不改变 Worker owner 或重启要求。

## 生命周期与监听

默认 `127.0.0.1:8091`。主配置 TCP/Unix listener 是 fallback，加载 runtime 后使用其有效 listener；
Linux Unix 模式的目录、owner/group、锁和 `0660` socket 门禁见[配置说明](../../ops/config/README.md#http-listener-与安全)。
SIGINT/SIGTERM 停止接收 HTTP、停止 heartbeat 并发布停止状态，drain 最多 10 秒；
第二次信号强制收口，随后按配置限时刷新 exporter。稳定退出码仍待统一。

## 前端产物

`just web-build-admin` 生成 Admin SPA，`just build-release` 构建内嵌发布制品。
本 app 不引用 Public 资源类型；API-only 使用固定空集合，静态路由以 503 报告页面未包含。

## Cargo feature

默认 feature 为空；`postgres` / `sqlite` 选择存储，`cache-redis` 选择可选 Redis，
`web-embed` 只包含 Admin payload。首次设置需要 `postgres,web-embed`。
配置和显式运行见[开发指南](../../docs/development.md)。

## 边界约束

Admin 与 Public 保持独立路由、监听和静态产物。管理变更走版本化 REST、认证/CSRF 与审计边界；
凭据、密钥和数据库内部模型不得进入浏览器 DTO。

## 维护与验证

`cargo check --locked -p auction-admin-api` 只做默认 feature 静态编译。
完整门禁见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。

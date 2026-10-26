# auction-admin-api

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [HTTP 契约](docs/contracts/routes.md) · [部署](ops/README.md)

管理 HTTP、会话和 runtime 服务的进程装配根。`src/lib.rs` 导出 `run`、`run_from_path` 与 Admin 资源类型；
独立 `main.rs` 和[合并服务](apps/auction-server/README.md)的 Admin 子进程复用装配。仅支持 Linux x86_64 原生目标，
装配入口在编译期拒绝其他 OS/架构；浏览器 WASM 不受此限，默认发布 ABI 见[开发指南](docs/guide/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](apps/auction-admin-api/src/bootstrap) | 管理端口、首次模式选择、runtime 恢复与生命周期 |
| [src/web_assets](apps/auction-admin-api/src/web_assets) | Admin 专属内嵌资源类型 |

## 职责与边界

只加载配置、装配管理应用端口与适配器、挂载 `auction_http::admin_router()` 并管理进程生命周期；
管理命令、事务和外部服务调用由各自 owner 实现。与 Public 保持独立路由、监听和静态产物，
管理变更走版本化 REST、认证/CSRF 与审计边界，凭据、密钥和数据库内部模型不得进入浏览器 DTO。
`bootstrap/service.rs` 拥有 tracing 与 HTTP 生命周期，`state.rs` 装配数据库/端口/心跳，
`config.rs` 映射缓存、Worker control、安全和 dashboard 范围，`listener.rs` 绑定监听器。
`first_run.rs` 在正式 loader 前选择模式：内嵌 PostgreSQL Web 制品只在无默认配置且未显式设置
`AUCTION_CONFIG_FILE` 时自动开放 loopback 设置页，也可显式执行 `setup --output-dir <绝对路径>`；
损坏、缺失的显式配置、已有残缺目录或符号链接都不降级为设置页，分体设置保存后退出，
不自动迁移或启动其他角色，见[首次部署](ops/README.md#首次部署配置)。
Admin 凭据只从敏感 TOML 读取。每次启动在 session 就绪前永久撤销旧 Admin 会话，失败阻断启动；
token 摘要绑定当前 actor/secret，Public 会话不受影响；多实例轮换前须停止全部旧凭据实例。

## 查询、命令与可观测性

装配生命周期、业务 world、规则、审计、凭据、结算/恢复报告等窄端口；精确路由与错误码见 HTTP 契约。
创建拍卖与更新排期共用同一无可变状态的存储适配器和启动策略快照，不重复建立策略副本。
拍卖创建与生命周期适配器直接使用事务内数据库时间，不再注入闲置时钟；规则、runtime、录入等实际使用时钟的端口不变。
业务 world 状态来自数据库，装配注入论坛本地安全投影读取端口以承接手动状态刷新，不注入论坛写入授权；
论坛预览只在本地解析不超过 2 MiB 的输入，不远程发布或写库。生命周期和 recovery 确认沿用应用/存储 owner 的锁、
幂等、追加审计与持久化事件边界。`HealthService` 聚合三角色心跳，Admin 另经内核 peer 认证的 Worker control Unix socket
读取 live/ready/status；mail、realtime、credential、delivery、fulfillment、recovery 与 alerting 各提供安全摘要，
缺失 owner 才显示 `unknown`，socket 不可用只产生有限 condition，心跳每轮读数据库/schema readiness、审计失败计数与 tracing 状态。
周期心跳跳过错过的 tick，依赖恢复后只发布当前状态，不集中补发过期观测。

## Runtime 恢复与启动快照

PostgreSQL runtime 服务组合应用端口、TOML 原子替换和 durable 命令账本，保留 fingerprint fence 与 `restart_required`。
就绪前恢复 pending/unknown 命令：任何恢复错误（含未知结果和需人工核对）都阻断启动，不得带旧安全快照监听；
恢复后文件指纹若不同于已加载配置，必须重新部署重启，不能用旧监听/策略快照报告新配置生效，保存不热改当前进程。
装配 TOML 存储时传入 OTLP headers 是否配置及可选 Worker 非敏感预算快照；不传头内容或 Worker secrets，
仍有 `admin_otlp_headers` 时拒绝保存关闭 tracing。先移除敏感字段并重启 Admin，再保存关闭配置。
`worker_config_file` 由配置 owner 读取并校验角色/runtime 同源；跨组预算只校验新候选，
既有 durable 恢复不受新门禁改写，Worker 基值变化后须重启 Admin 更新该快照。
共享业务区块、六组 `operations`、重启范围及默认值见[配置说明](ops/config/README.md#runtime-共享与重启)；
新经济模板只影响后续拍卖，人工提前开放/结算由启动快照和事务内策略共同约束。
装备来源适配器使用启动时唯一 active `mail_cookie`，无数据库记录时才兼容敏感文件单 Cookie，远端登录失效后标记失效、不自动切换。
`require_tradeable` 默认关闭，仅影响 Admin 手工录入。A44 手动只读校验每次读取指定凭据 ID 的当前信封，
不复用该快照或回退配置；缺密钥返回 501，不改变 Worker owner 或重启要求。
同一凭据数据密钥另装配 A32 `CredentialAcquirer`，只在显式导入账号来源时发起固定论坛登录；
新鲜表单、共享 Cookie 提取与远端正向证明归 integrations，装配根不保存密码、不接受动态外部目标。

## 生命周期与监听

默认 `127.0.0.1:8091`。主配置 TCP/Unix listener 是 fallback，加载 runtime 后使用其有效 listener；
Linux Unix 模式的目录、owner/group、锁和 `0660` socket 门禁见[配置说明](ops/config/README.md#http-listener-与安全)。
SIGINT/SIGTERM 停止接收 HTTP、停止 heartbeat 并发布停止状态，drain 最多 10 秒，第二次信号强制收口，
随后按配置限时刷新 exporter；稳定退出码仍待统一。
启动期的 listener 绑定与异步依赖装配同时等待 SIGINT/SIGTERM；收到信号即取消未完成装配并释放 listener，
不会等到数据库恢复后才退出，也不会启动 heartbeat。已写入的 runtime 恢复事实仍按 durable/unknown 契约核对，取消不代表回滚。

## 产物、feature 与验证

`bash ci/release/web-bundle.sh admin` 生成 Admin SPA，`just build-release` 构建内嵌发布制品；本 app 不引用 Public 资源类型，
API-only 使用固定空集合，静态路由以 503 报告页面未包含。默认 feature 为空；`postgres` 启用存储，
`cache-redis` 选择可选 Redis，`web-embed` 只包含 Admin payload；首次设置需要 `postgres,web-embed`。

`cargo check --locked -p auction-admin-api` 只做默认 feature 静态编译；完整门禁见[开发指南](docs/guide/development.md)，
运行证据见[验证状态](docs/status.md)。

# auction-admin-api

[文档导航](docs/README.md) · [HTTP 路由](docs/contracts/routes.md) · [Admin 前端](crates/auction-web-admin/README.md) · [配置参考](ops/config/README.md)

管理后台的 HTTP 进程：Admin 会话、拍卖生命周期与排期、lot 修改与手工录入、凭据与用户管理、投递与恢复人工处置、规则发布、runtime 配置编辑、审计与仪表盘，并在 `web-embed` 构建中同源提供 Admin 页面。另外承担分体部署的首次设置入口。

## feature

| feature | 作用 |
| --- | --- |
| `postgres` | 连接 PostgreSQL 并装配全部端口；同时启用首次设置（`auction-setup/postgres`） |
| `web-embed` | 内嵌 Admin 页面产物；首次设置页面也依赖它 |
| `cache-redis` | 允许 Redis 缓存后端 |

## 启动模式

`auction-admin-api` 在加载配置前选择模式：

| 情况 | 行为 |
| --- | --- |
| 带参数 `setup [--output-dir <绝对路径>] [--control-group <组>] [--control-uid <UID>]` | 进入首次设置，写完配置后退出 |
| 无参数，且设置了 `AUCTION_CONFIG_FILE` 或存在 `ops/config/admin.toml` | 正常启动 |
| 无参数，存在 `auction-config/admin.toml` | 以该文件正常启动 |
| 无参数，以上都不存在 | 进入首次设置，输出到当前工作目录下的 `auction-config`，control 组默认 `auction-worker-control`、UID 为当前用户 |

首次设置需要 `postgres` + `web-embed` 构建，流程见 [auction-setup](crates/auction-setup/README.md)。其他参数一律拒绝。

## 正常启动

1. 加载 Admin 配置。Admin 的角色文件还通过 `worker_config_file` 读取 Worker 配置（用于 control 客户端与仪表盘的世界/邮箱投影）。
2. 绑定 listener：TCP（默认 `127.0.0.1:8091`）或 Unix socket。
3. 有数据库时：
   - 必须配置 `admin_actor` 与 `admin_secret`。
   - 配置了 `runtime_config_file` 时先执行 runtime 配置恢复：检查是否有未完成的配置更新命令并收口；恢复后的文件 fingerprint 必须与本次启动加载的配置一致，否则拒绝启动（说明文件已被更新但进程尚未用新配置重启）。
   - 按数据密钥是否配置启用可选能力：`equipment_data_key` → 手工录入装备；`user_login_data_key` → 用户与登录 key 管理；`credential_data_key` → 凭据录入与校验。缺少密钥时对应接口返回能力不可用。
   - 装配会话（空闲与绝对过期时间来自 `[session]`）、心跳、结算/恢复报告、投递与 realtime 运维等端口。
4. 没有数据库时只提供启动骨架（健康与仪表盘的配置投影），配置了数据库但未编译 `postgres` 则拒绝启动。

## Worker control

启用 `[worker.control]` 时，Admin 通过 `WorkerControlClient` 访问 Worker 的 Unix socket（读取状态与邮件轮询进度、手动触发轮询）。Admin 的运行账号必须能访问 socket 所在目录，且 UID/GID 在 Worker 的 `allowed_peer_uids`/`allowed_peer_gids` 中——systemd 示例通过 `SupplementaryGroups=auction-worker-control` 满足这一点。

## runtime 配置文件

Admin 是 runtime 文件（默认 `runtime/admin.toml`）的唯一写者：后台“参数配置”页提交的更新先经服务端校验，再以命令账本 + 原子替换写入。Admin 的 listener、HTTP 安全、会话、metrics、tracing 等设置以 runtime 文件为准；Worker 读取其中由 `managed = true` 接管的分组。文件写入后需要按[运维手册](ops/README.md)重启相关角色才会生效。

## 退出

与 Public 相同：第一次信号开始 10 秒有界排空（停止接受新请求、发布 stopping 心跳），超时或第二次信号时强制退出。

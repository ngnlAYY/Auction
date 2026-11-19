# auction-admin-api

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [crates/auction-web-admin/README.md](../../crates/auction-web-admin/README.md) · [ops/config/README.md](../../ops/config/README.md)

管理后台的 HTTP 进程：Admin 会话、拍卖生命周期与排期、lot 修改与手工录入、凭据与用户管理、投递与恢复人工处置、规则发布、runtime 配置编辑、审计与仪表盘，并在启用内嵌 feature 的构建中同源提供 Admin 页面（`web-embed` 为 Dioxus，`web-solid` 为 Solid）；另外承担分体部署的首次设置入口。

## feature

| feature | 作用 |
| --- | --- |
| （都不启用） | 只提供 API；不内嵌 Admin 页面，首次设置入口不可用 |
| `web-embed` | 内嵌 Dioxus 版 Admin 页面产物，首次设置也依赖它 |
| `web-solid` | 内嵌 Solid 版 `target/web/admin/public` 的产物（蕴含 `web-embed`），首次设置页随 Admin 一起切换；目前尚无 Admin 的 Solid 产物，未构建时页面路由返回 503 |

## 启动模式

| 情况 | 行为 |
| --- | --- |
| 带参数 `setup [--output-dir …] [--control-group …] [--control-uid …]` | 进入首次设置，写完配置后退出 |
| 无参数，且设置了 `AUCTION_CONFIG_FILE` 或存在 `ops/config/admin.toml` | 正常启动 |
| 无参数，存在 `auction-config/admin.toml` | 以该文件正常启动 |
| 无参数，以上都不存在 | 进入首次设置，输出到工作目录下 `auction-config`，control 组默认 `auction-worker-control`、UID 为当前用户 |

首次设置需要 `web-embed` 构建，流程见[crates/auction-setup/README.md](../../crates/auction-setup/README.md)；其他参数一律拒绝。`ops/config/admin.toml` 与 `auction-config/admin.toml` 是相对工作目录的运行配置位置，仓库模板位于 [ops/config/examples](../../ops/config/examples)。

## 正常启动

1. 加载 Admin 配置；角色文件还通过 `worker_config_file` 读取 Worker 配置（control 客户端与仪表盘的世界/邮箱投影）。
2. 经 `auction-http` 的 `HttpListener` 绑定 listener（TCP 默认 `127.0.0.1:8091` 或 Unix socket），连接服务与排空由 `serve_http` 统一处理。
3. 有数据库时：必须配置 `admin_actor` 与 `admin_secret`；配置了 `runtime_config_file` 时先执行 runtime 恢复（未完成命令收口，恢复后 fingerprint 必须与启动加载一致，否则拒绝启动）。可选能力按数据密钥启用：`equipment_data_key` → 手工录入；`user_login_data_key` → 用户与登录 key；`credential_data_key` → 凭据导入与在线校验（只返回瞬时观测；论坛 Cookie 在本进程检查，邮件 Cookie 经 Worker control 交给 Worker 检查，Worker 不可用时只报暂不可用）。装配会话、心跳与运维端口；初始化会话服务会永久撤销既有 Admin 会话，重启后须重新登录。
4. 无数据库时仍提供健康与内嵌页面，但 readiness 未就绪、会话端口缺失，需认证的 API 返回会话服务不可用。

## Worker control 与 runtime 配置

Admin 从 common 注入[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)并注册分类指标；默认 shadow 保持旧计数并记录差异，显式 classified 才只由 permanent + unknown 阻断；HTTP 结果缓存一秒并合并并发，心跳独立检查。启用 `[worker.control]` 时经 `auction-integrations` 的 `WorkerControlClient`（状态、轮询进度、外部账号快照、手动触发）与 `WorkerRemoteReadClient`（手工录入时由 Worker 代读装备页、邮件 Cookie 在线检查由 Worker 执行）访问 Worker socket，路径见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)；运行账号的 UID/GID 须在 Worker 的 `allowed_peer_uids`/`allowed_peer_gids` 中（systemd 示例用 `SupplementaryGroups=auction-worker-control`）。

配置 `runtime_config_file` 后 Admin 是该文件的唯一写者（首次设置使用 `runtime/admin.toml`）；参数更新经校验、命令账本与原子替换写入。listener、HTTP 安全、会话、`metrics.enabled` 与 tracing 以 runtime 为准，Worker 只读取 `managed = true` 接管的分组；写入后须重启对应角色。附加指标出口由 `metrics.admin_loopback_listen_address` 选择（示例 `127.0.0.1:19101`）；runtime 只覆盖 `metrics.enabled`，地址与 `reject_forwarded_requests` 基线始终来自 common。出口行为见[docs/contracts/routes.md#三角色附加指标出口loopback-tcp](../../docs/contracts/routes.md#三角色附加指标出口loopback-tcp)，采集见[ops/README.md#指标](../../ops/README.md#指标)。

## 退出与数据库

第一次信号开始由 `[shutdown].drain_timeout_seconds`（默认 10 秒）配置的有界排空（停止接受新请求、发布 stopping 心跳），超时或第二次信号强制非零退出并跳过可选 tracing 刷新；本角色关键段预算只接受 0，示例 unit 保持 45 秒。启动装配注入 TOML 就绪与指标转发拒绝基线并启动开关刷新任务，语义见[ops/config/README.md#数据库运行时开关](../../ops/config/README.md#数据库运行时开关)。profiled 模式使用 `auction-admin`，`[database_pool]` 默认上限 16、长期预留 0。见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)与[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。

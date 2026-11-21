# auction-worker

[docs/README.md](../../docs/README.md) · [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) · [docs/guide/settlement.md](../../docs/guide/settlement.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

后台任务进程：拍卖生命周期、慢速代理、排期、结算与履约、邮件投递、付款恢复、MoogleMail 收件与维护、论坛同步与回帖、装备私信、运维告警与本机 Worker control。本 crate 只做装配（读取配置、连接数据库与外部服务、取得运行时所有权），把端口交给 [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) 的调度运行时。未配置数据库时以“骨架”运行（仅 control 与指标），不领取任务、不伪造就绪。

## 启动

`AUCTION_CONFIG_FILE=/etc/auction/worker.toml auction-worker`（本地开发 `cargo run --locked -p auction-worker`，优先读 `ops/config/worker.toml`；仓库模板在 [ops/config/examples/worker.toml](../../ops/config/examples/worker.toml)）。

加载角色配置后先检查一口价挂单的制品能力和所需的数据库配置。通过后初始化日志、指标和信号，再连接数据库。启用了 MoogleMail、论坛、告警邮件或告警 Webhook 时也须配置数据库，否则按配置运行骨架。持久装配先同步一口价挂单的准入代次，再装配生命周期、慢速代理、论坛与邮件轮询。实际装载外部凭据时，以 PostgreSQL session advisory lock 取得该种凭据的唯一 owner（每秒检查，失效即排空并非零退出）。完成后才绑定已启用的 Worker control、发布进程心跳并启动持久任务。

MoogleMail 默认 `poll_isolation_enabled = true`：每个世界独立运行收件与维护，维护按自己的日程先 `sweep` 再按最小间隔观察完整 Read Mail；省略维护日程时跟随 Admin 接管后的收件日程，两世界用启动时冻结的相位平移。该 TOML 值是在线开关 `mooglemail_poll_isolation_enabled` 的基线（`auction-cli switch set` 切换，clear 回落）；切换在下一轮或下一次维护门复读后生效，不重写 `mail_poll_state.next_poll_at`。见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

## 外部能力的启用条件

| 能力 | 配置 | 依赖 |
| --- | --- | --- |
| MoogleMail 收件、投递、履约 | `[worker.mooglemail] enabled = true`（默认关闭；默认仅 main，isekai 加入 `additional_worlds`） | 凭据、`equipment_data_key` 与 `user_login_data_key`；数据库凭据另需 `credential_data_key` |
| 论坛同步、回帖与装备 PM | `[worker.forum] enabled = true`（默认关闭） | `equipment_data_key` 与论坛凭据；装备 PM 不依赖 MoogleMail 开关 |
| 运维告警 | `[worker.operations_alert]`：`scan_enabled`（默认 true，只写告警台）、`notify_enabled`（默认 false，MoogleMail 告警邮件）、`webhook_enabled`（默认 false，独立 Webhook）；见[ops/config/README.md#worker](../../ops/config/README.md#worker) | 扫描只需数据库；邮件通知需收件人，告警世界必须在已启用的 MoogleMail 世界内；Webhook 需 `operations_alert_webhook_url` |
| 邮件诊断归档 | `[worker.mooglemail.diagnostics]`（默认启用） | `credential_data_key`，否则用 `equipment_data_key` 作加密父密钥 |
| 一口价挂单解析 | 共用 `[consignment] enabled = true`（默认关闭） | PostgreSQL、装备域当前数据密钥及按需保留的旧钥、固定装备来源。本阶段仅带 `consignment-dev-preview` 的调试制品可开启 |

诊断归档目录默认在 Worker 配置文件旁的 `mail-diagnostics/`，按保留天数（默认 7）与总字节上限（默认 256 MiB）清理；用 `auction-cli mail export-evidence` 离线导出。周排期与早鸟期分别由 `[auction.schedule] enabled`、`[auction.schedule.early_bird] enabled` 控制，默认关闭。关闭邮件或论坛不会停用生命周期、慢速代理与结算；外部履约缺证据时保持等待，不能当作已完成。凭据不是逐项回退：配置完整的 `credential_data_key` 时只读数据库 active 凭据，未配置时邮件用 `keyenv_file` 或敏感配置 `mooglemail_cookie`、论坛用 `ehentai_cookie`。需要联合账号身份证据的流程还依赖论坛目标资料读取端口（仅论坛启用时装配）。

## Worker control 与指标

`[worker.control]` 默认关闭；启用后 Worker 在 Unix socket（默认 `/run/hv-auction/worker/control.sock`，组 `auction-worker-control`）上提供 HTTP/JSON 控制面，连接按 `SO_PEERCRED` 检查 `allowed_peer_uids`/`allowed_peer_gids`。路径、方法与就绪判定见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)，socket 与授权实现见[crates/auction-jobs/README.md#worker-control](../../crates/auction-jobs/README.md#worker-control)。Admin、`auction-cli` 与 `auction-server` 都经该 socket 判断 Worker 状态；Admin 与 CLI 需要的外部只读读取（含 Admin 邮件 Cookie 在线检查）也经它交给 Worker 执行，不在本地直连；邮件凭据检查需要 Worker 启用 MoogleMail 并配置 `credential_data_key`，否则该路由报告能力不可用。

附加指标出口由 `metrics.worker_loopback_listen_address` 选择（示例 `127.0.0.1:19102`），独立于 control socket 上的 `/metrics`；行为见[docs/contracts/routes.md#三角色附加指标出口loopback-tcp](../../docs/contracts/routes.md#三角色附加指标出口loopback-tcp)，字段校验见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)。

## 一口价挂单

启用时，解析循环按数据库全局许可领单，空闲时至多等 5 秒，复用 Worker 的外部访问暂停与共享排空。固定来源读取的字节上限和本次领取的硬截止由 `[consignment]` 提供。维护循环每 30 秒补扫活动挂单，逐单核实碰撞、资格、停用或到期事实，并在开启时回收过期的解析租约。准入同步失败即启动失败。关闭时只在发现待清退存量后启动维护。

登记准入心跳每 10 秒重新核对数据库及 schema，并依赖已装配的开封器、装备密钥环与来源端口。核对通过后续期至数据库时刻后 30 秒。指标固定循环名为 `consignment-resolve`、`consignment-maintenance`、`consignment-readiness`。各循环共用持久任务的监督与停止预算。见[一口价挂单的停用与恢复](../../ops/README.md#一口价挂单的停用与恢复)。

## 退出

第一次终止信号：关闭入口，立即取消未登记交接的工作且不释放 claim；已登记交接或关键段共享一次默认 60 秒截止，之后普通收口默认 10 秒，PM 与主 runner 一起等待。超时或第二次信号强制非零退出，跳过可选 tracing 刷新。独立邮箱维护任务同样受监督：停止后不开始新 `sweep` 或 Read Mail 观察。运行时故障（control 停止、PM 循环停止、持久化循环退出、凭据所有权失效）同样触发排空并**总是非零退出**，由 systemd（`Restart=on-failure`）或 `auction-server` 重启；已领取任务靠持久租约恢复，发送中的外部写入只读对账，不因退出重复发送。

delivery/PM/discard 在持久围栏调用前登记交接，围栏开始即受保护；登记不授权外写，只有围栏确认提交才执行外部操作。论坛在持久 claim 下的第一个实际 POST 前登记。未收口 drop 只减少活动计数并记录未知边界，不补写持久状态、不授权重发。配置、unit 与完整回退见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。

## 数据库与 SLI

启动装配注入 TOML 就绪/SLI/邮件隔离/指标转发拒绝基线并启动开关刷新任务，Worker 另负责为已到期的覆盖追加 expire 事实；语义见[ops/config/README.md#数据库运行时开关](../../ops/config/README.md#数据库运行时开关)，操作见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。profiled 模式使用 `auction-worker`，`[database_pool]` 默认物理上限 16、长期预留最坏 3（邮件与论坛 Cookie 所有权、投递 LISTEN）；服务超时见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。

有数据库且 `metrics.enabled = true` 时建立独立 SLI 任务：`metrics.sli_snapshot_enabled` 默认 false，经 Worker 专用开关 `metrics_sli_snapshot_enabled` 在线覆盖；每 60 秒至多借一个空闲连接、两秒预算只读轮次，失败只降级指标、保留旧快照。见[ops/README.md#sli-基线与容量观察](../../ops/README.md#sli-基线与容量观察)。

## 部署

unit 示例见[ops/deploy/systemd/auction-worker.service.example](../../ops/deploy/systemd/auction-worker.service.example)（`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录）。每个部署只运行一个 Worker 实例。

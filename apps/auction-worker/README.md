# auction-worker

[docs/README.md](../../docs/README.md) · [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) · [docs/guide/settlement.md](../../docs/guide/settlement.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

后台任务进程：拍卖生命周期、慢速代理、排期、结算与履约、邮件投递、付款恢复、MoogleMail 收件与维护、论坛同步与回帖、装备私信、运维告警与本机 Worker control。本 crate 只做装配（读取配置、连接数据库与外部服务、取得运行时所有权），把端口交给 [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) 的调度运行时。未配置数据库时以“骨架”运行（仅 control 与指标），不领取任务、不伪造就绪。

## 启动

`AUCTION_CONFIG_FILE=/etc/auction/worker.toml auction-worker`（本地开发 `cargo run --locked -p auction-worker`，优先读 `ops/config/worker.toml`；仓库模板在 [ops/config/examples/worker.toml](../../ops/config/examples/worker.toml)）。

装配顺序：加载配置与日志/tracing/指标并订阅信号 → 未配置数据库时若启用了 MoogleMail、论坛或告警则拒绝启动，否则骨架运行 → 连接数据库并按 `[worker.mooglemail.send_intervals]` 配置发送间隔 → 装配生命周期、慢速代理、论坛与邮件轮询 → 实际装载外部凭据时以 PostgreSQL session advisory lock 取得该凭据种类的唯一 owner（每秒检查，失效即排空并非零退出）→ 启用 `[worker.control]` 时绑定 socket、发布心跳并启动全部循环。

MoogleMail 默认 `poll_isolation_enabled = true`：每个世界独立运行收件与维护，维护按自己的日程先 `sweep` 再按最小间隔观察完整 Read Mail；省略维护日程时跟随 Admin 接管后的收件日程，两世界用启动时冻结的相位平移。该 TOML 值是在线开关 `mooglemail_poll_isolation_enabled` 的基线（`auction-cli switch set` 切换，clear 回落）；切换在下一轮或下一次维护门复读后生效，不重写 `mail_poll_state.next_poll_at`。见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

## 外部能力的启用条件

| 能力 | 配置 | 依赖 |
| --- | --- | --- |
| MoogleMail 收件、投递、履约 | `[worker.mooglemail] enabled = true`（默认关闭；默认仅 main，isekai 加入 `additional_worlds`） | 凭据、`equipment_data_key` 与 `user_login_data_key`；数据库凭据另需 `credential_data_key` |
| 论坛同步、回帖与装备 PM | `[worker.forum] enabled = true`（默认关闭） | `equipment_data_key` 与论坛凭据；装备 PM 不依赖 MoogleMail 开关 |
| 运维告警邮件 | `[worker.operations_alert] enabled = true`（默认关闭） | 收件人；告警世界必须在已启用的 MoogleMail 世界内 |
| 邮件诊断归档 | `[worker.mooglemail.diagnostics]`（默认启用） | `credential_data_key`，否则用 `equipment_data_key` 作加密父密钥 |

诊断归档目录默认在 Worker 配置文件旁的 `mail-diagnostics/`，按保留天数（默认 7）与总字节上限（默认 256 MiB）清理；用 `auction-cli mail export-evidence` 离线导出。周排期与早鸟期分别由 `[auction.schedule] enabled`、`[auction.schedule.early_bird] enabled` 控制，默认关闭。关闭邮件或论坛不会停用生命周期、慢速代理与结算；外部履约缺证据时保持等待，不能当作已完成。凭据不是逐项回退：配置完整的 `credential_data_key` 时只读数据库 active 凭据，未配置时邮件用 `keyenv_file` 或敏感配置 `mooglemail_cookie`、论坛用 `ehentai_cookie`。需要联合账号身份证据的流程还依赖论坛目标资料读取端口（仅论坛启用时装配）。

## Worker control 与指标

`[worker.control]` 默认关闭；启用后 Worker 在 Unix socket（默认 `/run/hv-auction/worker/control.sock`，组 `auction-worker-control`）上提供 `live`/`ready`/`status`、邮件轮询进度、手动触发与 `/metrics`，连接按 `SO_PEERCRED` 检查 `allowed_peer_uids`/`allowed_peer_gids`。`/v1/health/ready` 在 serving、未排空且数据库就绪探针通过时返回 200，与 Public/Admin `/health/ready` 和心跳同一判定；无数据库骨架只看进程状态。Admin、`auction-cli service` 与 `auction-server` 都经该 socket 判断状态，见[crates/auction-jobs/README.md#worker-control](../../crates/auction-jobs/README.md#worker-control)。

独立指标出口由 `metrics.worker_loopback_listen_address` 选择（省略关闭，示例 `127.0.0.1:19102`），还需 `metrics.enabled = true`；它独立于 control socket，只挂 `/metrics` 并复用 `auction-http` 的公共指标契约，绑定/服务失败只记固定类别 ERROR，不影响 readiness。`reject_forwarded_requests` 对新出口生效，可在线收紧、clear 回落 TOML。配置见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)。

## 退出

第一次终止信号：关闭入口，立即取消未登记交接的工作且不释放 claim；已登记交接或关键段共享一次默认 60 秒截止，之后普通收口默认 10 秒，PM 与主 runner 一起等待。超时或第二次信号强制非零退出，跳过可选 tracing 刷新。独立邮箱维护任务同样受监督：停止后不开始新 `sweep` 或 Read Mail 观察。运行时故障（control 停止、PM 循环停止、持久化循环退出、凭据所有权失效）同样触发排空并**总是非零退出**，由 systemd（`Restart=on-failure`）或 `auction-server` 重启；已领取任务靠持久租约恢复，发送中的外部写入只读对账，不因退出重复发送。

delivery/PM/discard 在持久围栏调用前登记交接，围栏开始即受保护；登记不授权外写，只有围栏确认提交才执行外部操作。论坛在持久 claim 下的第一个实际 POST 前登记。未收口 drop 只减少活动计数并记录未知边界，不补写持久状态、不授权重发。配置、unit 与完整回退见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。

## 数据库与 SLI

启动装配注入 TOML 就绪/SLI/邮件隔离/指标转发拒绝基线，立即读取数据库开关并按五秒周期、单次一秒预算刷新；失败保留原快照及期限，不清零审计累计。profiled 模式使用 `auction-worker`，`[database_pool]` 默认物理上限 16、长期预留 3；服务超时默认关闭，启用须显式填写阈值并确认三角色有效 classified 经影子观察后切换。见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)与[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

有数据库且 `metrics.enabled = true` 时建立独立 SLI 任务：`metrics.sli_snapshot_enabled` 默认 false，经 Worker 专用开关 `metrics_sli_snapshot_enabled` 在线覆盖；每 60 秒至多借一个空闲连接、两秒预算只读轮次，失败只降级指标、保留旧快照。见[ops/README.md#sli-基线与容量观察](../../ops/README.md#sli-基线与容量观察)。

## 部署

unit 示例见[ops/deploy/systemd/auction-worker.service.example](../../ops/deploy/systemd/auction-worker.service.example)（`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录）。每个部署只运行一个 Worker 实例。

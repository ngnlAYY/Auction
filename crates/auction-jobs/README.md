# auction-jobs

[docs/README.md](../../docs/README.md) · [apps/auction-worker/README.md](../../apps/auction-worker/README.md) · [docs/architecture.md](../../docs/architecture.md)

Worker 的调度运行时与本地控制面：把应用层的持久化端口编排成相互独立的循环（领取 → 执行 → 落库），负责轮询间隔、抖动、租约续期、单项超时、有界并发、优雅排空与指标，以及 Worker control 的 Unix socket 服务端。业务状态迁移全部由应用端口决定，本 crate 不复制业务规则。

## 循环

`run_persistent_with_settlement_until_shutdown` 按下表组织循环。排期 bootstrap、生命周期、慢速代理与结算共享拍卖主循环；邮件按世界独立运行收件与维护；论坛发布与回帖导入共享论坛循环；每个循环保留自己的持久化结果与错误边界。

| 循环 | 职责 |
| --- | --- |
| 拍卖主循环 | 预览/开拍/Final/结算到期的生命周期推进与结算认领；在下一个最早到期时刻提前唤醒 |
| 慢速代理 / 排期 bootstrap | 到期 Slow 代理的幂等反超；按周排期与赛季早鸟建立后续草稿场次，早鸟期暂停/恢复对应世界收件 |
| 结算履约 | 领取义务，执行 prepare → activate → reconcile，生成投递 |
| 投递 outbox | 在 sending fence 内调用 MoogleMail transport，续租并写回终态；结果未知转人工对账 |
| 恢复 | 付款超时的提醒、退回、重新上架与退款，以及余额/证据健康刷新 |
| 邮件轮询 / 邮箱维护 | 每个世界一个收件循环与一个维护任务；维护先运行 Sent/Take 恢复与 compose 清理，再按最小间隔观察已读列表。两者用同一互斥串行化，日程按世界相位平移 |
| 论坛同步 / 回帖 | 发布与刷新拍卖主题、导入回帖命令；两阶段错误互相隔离 |
| 装备 PM / 玩家通知 | 超价提醒私信队列；赛季提醒与卖家小结扫描，经各自 JoinSet 监督并排空收口 |
| 资产对账 / 运维告警 | 固定间隔的只读对账；扫描、分发、对账三个告警循环 |

每个循环使用 `batch_limit`、`concurrency`、`poll_interval`、`jitter`、`lease_duration`、`item_timeout` 等边界（来自 `[worker.*]`，可由 runtime 接管）。停止信号后不再领取新任务，在途任务有界排空；常驻任务意外退出会把 control 标记为 draining 并触发非零退出，由进程管理器重启。维护日程与相位规则见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

## 外部账号观测

Worker 经应用层 `ExternalMetricsObserver` 记录邮件与论坛的阶段预算（同步、无 I/O、无 `.await`），关闭指标时为 no-op；记录不改变领取、租约、重试、排空、取消或写入 fence。回帖导入延迟只观测“首次新命令账本提交确认”，`pending` 重试与重放不采样，缺样只记录原因、不填零。完整契约见[crates/auction-observability/README.md#外部账号客户端指标](../auction-observability/README.md#外部账号客户端指标)。

## Worker control

`[worker.control]` 默认关闭。显式启用后 Worker 在 Unix socket 上提供 HTTP/JSON 控制面（协议 `v1`）：

| 路径 | 用途 |
| --- | --- |
| `GET /v1/health/live`、`/v1/health/ready` | 进程存活；control 已 serving 且数据库就绪探针通过（与 Public/Admin 同一判定；无数据库骨架只看进程状态） |
| `GET /v1/status`、`/v1/mail/progress` | 版本、轮询器数量等安全状态；各邮箱轮询进度 |
| `GET /v1/external-accounts` | 进程内外部账号快照；未装配或指标关闭时 `503 capability_unavailable` |
| `POST /v1/mail/polls:trigger` | 触发一次邮件轮询（Admin 后台使用） |
| `GET /metrics` | Worker 的 Prometheus 指标 |

socket 由 Worker 在受信运行目录创建并设为指定组；连接时用 `SO_PEERCRED` 检查 UID/GID 是否在 `allowed_peer_uids`/`allowed_peer_gids` 中，不在名单内直接断开。Admin 经 `auction-integrations` 的 `WorkerControlClient`（5 秒超时）读取状态与触发轮询（协议常量与线上 DTO 由 `auction-application` 拥有）；`auction-cli service` 与 `auction-server` 直接向该 socket 发送 `GET /v1/health/ready` 判定就绪，其运行账号同样要在 allowlist 中。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-jobs/src/control/` | Worker control 协议、服务端（listener/security）、客户端与邮件进度快照 |
| `crates/auction-jobs/src/worker/` | 循环装配与各 owner：`auction`、`settlement`、`forum`、`mail`、`recovery`、`runtime`（投递、恢复动作、通知、资产对账）、`lot_watch_pm`、`operations`、`player_notifications`、`diagnostics` |
| `crates/auction-jobs/src/runtime_switches/`、`mail_templates/`、`sli/` | 五秒开关刷新、五秒通知模板刷新与六十秒只读 SLI 汇总任务 |

## 维护约定

- 新循环必须持久化领取与结果，不能用内存队列或“空端口成功”代替；超时与租约必须满足“单项超时 < 租约”。
- 论坛公开链接只接受无凭据、无路径和无查询的 HTTPS origin，或回环 HTTP origin；运行时与配置预检使用相同边界。
- 错误只以安全分类进入日志、指标与 control，不透传数据库、凭据或外部响应正文。
- 停机新路径由装配根注入 `ExternalWriteLifecycle`：外写前登记并在确认提交后继续；交接与关键段阻止排空结束，父 JoinSet 不按活动快照 abort。详见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。
- 固定 `item_panic` 阶段用 `SafeErrorKind::TASK_PANIC` 并计数 `auction_worker_task_panics_total{loop}`；取消不增加。邮件、论坛、PM 与告警直接执行路径的 panic 仍使常驻任务退出并触发排空，见[ops/README.md#panic-排障](../../ops/README.md#panic-排障)。

## 通知模板刷新

`start_mail_template_refresh` 由三个角色的装配根启动：启动即限时读取一次数据库里的模板修订，此后每五秒调用应用层的刷新用例、每次一秒预算，把生效模板整体替换进进程内快照。定时器只负责唤醒；读取失败或超时保留上一次成功的快照（从未成功读取时是内置默认正文），失败与恢复各记录一次，不逐轮刷日志。正文不满足现行占位符规则的模板退回默认正文并告警，日志只含模板键。

## 运行时开关刷新与 SLI

`start_runtime_switch_refresh` 启动即限时读取，此后五秒周期调用应用刷新服务、每次一秒预算；邮件循环每轮读取 `mooglemail_poll_isolation_enabled`，指标边界在每次请求入口读取 `metrics_reject_forwarded_requests`。`start_sli_snapshot` 是独立六十秒唤醒任务，跳过漏掉的 tick、不追赶不重试；每轮检查 `metrics_sli_snapshot_enabled`，禁用不查询，总预算两秒且只借一个空闲连接，失败保留旧快照。口径见[crates/auction-observability/README.md#sli-与容量基线](../auction-observability/README.md#sli-与容量基线)。

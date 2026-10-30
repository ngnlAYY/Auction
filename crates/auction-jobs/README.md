# auction-jobs

[文档导航](docs/README.md) · [Worker 应用](apps/auction-worker/README.md) · [架构](docs/architecture.md)

Worker 的调度运行时与本地控制面：把应用层的持久化端口编排成相互独立的循环（领取 → 执行 → 落库），负责轮询间隔、抖动、租约续期、单项超时、有界并发、优雅排空与指标；以及 Worker control 的 Unix socket 服务端与 Admin 使用的客户端。任何业务状态迁移都由应用端口决定，本 crate 不复制业务规则。

## 循环

`run_persistent_with_settlement_until_shutdown` 启动以下循环，各自独立失败、独立重试：

| 循环 | 职责 |
| --- | --- |
| 拍卖主循环 | 预览/开拍/Final/结算到期的生命周期推进与结算认领；在下一个最早到期时刻提前唤醒，`poll_interval_seconds` 只是最长等待 |
| 慢速代理 | 扫描到期的 Slow 代理并幂等执行反超 |
| 排期 bootstrap | 按周排期与赛季早鸟日历建立后续草稿场次；早鸟期按配置暂停/恢复对应世界的收件轮询 |
| 结算履约 | 领取结算义务，执行 prepare → activate → reconcile，生成投递 |
| 投递 outbox | 领取待发邮件，在 sending fence 内调用 MoogleMail transport，续租并写回终态；结果未知转人工对账 |
| 恢复 | 付款超时恢复的多阶段动作（提醒、退回、重新上架、退款等），以及余额/证据健康刷新 |
| 邮件轮询 | 每个世界一个，按 cron 计划读取收件箱、逐封交给业务处理器，推进持久游标；可由 Admin 经 control 手动触发 |
| 邮箱维护 | 观察 compose 草稿与 Sent，受控清理可证明的残留 |
| 论坛同步 / 回帖 | 发布与刷新拍卖主题、导入回帖中的竞价命令；两阶段错误互相隔离 |
| 装备 PM | 装备超价提醒的论坛私信队列（独立于主循环，停止时立即不再领取） |
| 运维告警 | 扫描、分发、对账三个独立循环，经 MoogleMail 投递告警邮件 |

每个循环使用 `batch_limit`、`concurrency`、`poll_interval`、`jitter`、`lease_duration`、`item_timeout` 等边界（来自 `[worker.*]` 配置，可由 runtime 接管）。收到停止信号后不再领取新任务，在途任务在有界时间内排空；常驻任务意外退出会把 control 标记为 draining 并触发进程以非零状态退出，由进程管理器重启。

## Worker control

启用 `[worker.control]` 后，Worker 在 Unix socket 上提供 HTTP/JSON 控制面（协议版本 `v1`）：

| 路径 | 用途 |
| --- | --- |
| `GET /v1/health/live` | 进程存活 |
| `GET /v1/health/ready` | 调度运行时已开始服务且未进入排空（draining） |
| `GET /v1/status` | 版本、轮询器数量等安全状态 |
| `GET /v1/mail/progress` | 各邮箱轮询进度的安全观测 |
| `POST /v1/mail/polls:trigger` | 触发一次邮件轮询（Admin 后台使用） |
| `GET /metrics` | Worker 的 Prometheus 指标 |

socket 由 Worker 在受信运行目录中创建并设为指定组；连接时用内核 peer 凭据（`SO_PEERCRED`）检查调用者 UID/GID 是否在 `allowed_peer_uids`/`allowed_peer_gids` 中，不在名单内直接断开。Admin 通过 `WorkerControlClient`（5 秒超时）读取状态与触发轮询；`auction-cli service` 与 `auction-server` 直接向该 socket 发送 `GET /v1/health/ready` 判定 Worker 就绪，因此它们的运行账号同样要在 allowlist 中。

## 维护约定

- 新循环必须持久化领取与结果，不能用内存队列或“空端口成功”代替；超时与租约必须满足“单项超时 < 租约”。
- 错误只以安全分类进入日志、指标与 control，不透传数据库、凭据或外部响应正文。

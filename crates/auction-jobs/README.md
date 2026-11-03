# auction-jobs

[文档导航](../../docs/README.md) · [Worker 应用](../../apps/auction-worker/README.md) · [架构](../../docs/architecture.md)

Worker 的调度运行时与本地控制面：把应用层的持久化端口编排成相互独立的循环（领取 → 执行 → 落库），负责轮询间隔、抖动、租约续期、单项超时、有界并发、优雅排空与指标；以及 Worker control 的 Unix socket 服务端与 Admin 使用的客户端。任何业务状态迁移都由应用端口决定，本 crate 不复制业务规则。

## 循环

`run_persistent_with_settlement_until_shutdown` 启动持久化运行时，按下表组织循环与阶段。排期 bootstrap、生命周期、慢速代理和结算共享拍卖主循环；邮件默认按世界独立运行收件与维护，论坛发布与回帖导入共享论坛循环。各阶段保留自己的持久化结果和错误边界。

| 循环 | 职责 |
| --- | --- |
| 拍卖主循环 | 预览/开拍/Final/结算到期的生命周期推进与结算认领；在下一个最早到期时刻提前唤醒，`poll_interval_seconds` 只是最长等待 |
| 慢速代理 | 扫描到期的 Slow 代理并幂等执行反超 |
| 排期 bootstrap | 按周排期与赛季早鸟日历建立后续草稿场次；早鸟期按配置暂停/恢复对应世界的收件轮询 |
| 结算履约 | 领取结算义务，执行 prepare → activate → reconcile，生成投递 |
| 投递 outbox | 领取待发邮件，在 sending fence 内调用 MoogleMail transport，续租并写回终态；结果未知转人工对账 |
| 恢复 | 付款超时恢复的多阶段动作（提醒、退回、重新上架、退款等），以及余额/证据健康刷新 |
| 邮件轮询 | 每个世界一个，按 cron 计划读取收件箱、逐封交给业务处理器，推进持久游标；可由 Admin 经 control 手动触发 |
| 邮箱维护 / Read Mail 观察 | 默认每个世界一个独立维护任务，先运行 Sent/Take 恢复与 compose 清理，再按最小间隔观察完整已读列表数量；恢复定位仍由原 reader 按需执行，不重新创建历史入站命令 |
| 论坛同步 / 回帖 | 发布与刷新拍卖主题、导入回帖中的竞价命令；两阶段错误互相隔离 |
| 装备 PM | 装备超价提醒的论坛私信队列；由 Worker 装配根独立运行，启用论坛时装配，不依赖邮件开关，停止时立即不再领取 |
| 运维告警 | 扫描、分发、对账三个独立循环，经 MoogleMail 投递告警邮件 |

每个循环使用 `batch_limit`、`concurrency`、`poll_interval`、`jitter`、`lease_duration`、`item_timeout` 等边界（来自 `[worker.*]` 配置，可由 runtime 接管）。收到停止信号后不再领取新任务，在途任务在有界时间内排空；常驻任务意外退出会把 control 标记为 draining 并触发进程以非零状态退出，由进程管理器重启。

邮件的独立维护任务由 `crates/auction-jobs/src/worker/runtime/runner.rs` 的独立 `JoinSet` 持有，共用 shutdown watch、意外退出监督和总排空截止。每次 `sweep` 与随后到期的纯观察各有一个原 `fetch_timeout` 截止点，阶段内不重置；进入 drain 后不再启动新阶段，在途 `sweep` 保留原 discard 围栏。纯观察首次延后一个间隔，只在 `sweep` 结束后检查，失败也消耗间隔；内存时刻仅减少观察读取，不决定持久任务状态。

收件与维护的日程按同一世界相位平移，再叠加原 jitter；手动 `wakeup` 只唤醒收件。`poll_isolation_enabled = false` 时不启动独立维护任务，恢复原 cron、每轮三分支 join 和原观察频率。维护日程、观察间隔与相位规则见[配置参考](../../ops/config/README.md#mooglemail-收件与维护)，指标口径见[可观测性](../auction-observability/README.md#邮件循环口径)。

世界相位计算与日程平移策略由 [`crates/auction-jobs/src/worker/mail/schedule/mod.rs`](src/worker/mail/schedule/mod.rs) 持有，经 crate 根导出 `mail_world_phase` 与 `PhasedMailSchedule`；Worker 装配根只判断是否启用相位、处理启动错误并注入日程。

拍卖到期提示按读取时的时钟转换为单调截止，查询与网络等待消耗原有等待预算；最后休眠不会把这段耗时再次加回。提示失败仍按常规轮询恢复，实际领取继续依赖数据库事实、租约与命令幂等。

## Worker control

`[worker.control]` 默认关闭。显式启用后，Worker 在 Unix socket 上提供 HTTP/JSON 控制面（协议版本 `v1`）：

| 路径 | 用途 |
| --- | --- |
| `GET /v1/health/live` | 进程存活 |
| `GET /v1/health/ready` | control 已标记 serving 且未进入 draining；无数据库骨架也可返回 200，不代表业务调度与外部能力已可用 |
| `GET /v1/status` | 版本、轮询器数量等安全状态 |
| `GET /v1/mail/progress` | 各邮箱轮询进度的安全观测 |
| `POST /v1/mail/polls:trigger` | 触发一次邮件轮询（Admin 后台使用） |
| `GET /metrics` | Worker 的 Prometheus 指标 |

socket 由 Worker 在受信运行目录中创建并设为指定组；连接时用内核 peer 凭据（`SO_PEERCRED`）检查调用者 UID/GID 是否在 `allowed_peer_uids`/`allowed_peer_gids` 中，不在名单内直接断开。Admin 通过 `WorkerControlClient`（5 秒超时）读取状态与触发轮询；`auction-cli service` 与 `auction-server` 直接向该 socket 发送 `GET /v1/health/ready` 判定 Worker 就绪，因此它们的运行账号同样要在 allowlist 中。

## 维护约定

- 新循环必须持久化领取与结果，不能用内存队列或“空端口成功”代替；超时与租约必须满足“单项超时 < 租约”。
- 论坛公开链接只接受无凭据、无路径和无查询的 HTTPS origin，或 `localhost`、`127.0.0.1`、`[::1]` 的回环 HTTP origin；运行时与配置预检使用相同边界。
- 错误只以安全分类进入日志、指标与 control，不透传数据库、凭据或外部响应正文。

停机新路径由装配根注入 `ExternalWriteLifecycle`：非外写 owner 和尚未登记交接的叶子准备期立即取消，不生成释放或失败命令。delivery、discard 和 PM 在围栏调用前登记，围栏调用开始即受保护；登记不授权外写，确认提交后才继续外部操作。论坛在持久 claim 下的首个实际 POST 前登记。交接与关键段均由原 future 驱动并阻止排空结束，结果提交后才 complete，其余 drop 只记未收口。父 JoinSet 不按活动快照 abort，避免连带取消关键子项。所有组共用装配根截止，致命故障不再另建 10 秒预算；PM 由装配根与 runner 一起等待。维护的原 fetch 截止和各任务原业务截止仍然有效。零关键段预算不注入活动工厂，完整选择旧排空路径。详见[排空与停止预算](../../ops/config/README.md#排空与停止预算)。

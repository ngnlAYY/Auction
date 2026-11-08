# auction-jobs

[docs/README.md](../../docs/README.md) · [apps/auction-worker/README.md](../../apps/auction-worker/README.md) · [docs/architecture.md](../../docs/architecture.md)

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
| 装备 PM | 装备超价提醒的论坛私信队列；由 Worker 装配根独立运行，启用论坛时装配，不依赖邮件开关，停止时立即不再领取；W1.9 主动暂停期间发送与只读核对都不领取，解除后在下一个 5 秒重读切片内恢复 |
| 运维告警 | 扫描、分发、对账三个独立循环，经 MoogleMail 投递告警邮件 |

每个循环使用 `batch_limit`、`concurrency`、`poll_interval`、`jitter`、`lease_duration`、`item_timeout` 等边界（来自 `[worker.*]` 配置，可由 runtime 接管）。收到停止信号后不再领取新任务，在途任务在有界时间内排空；常驻任务意外退出会把 control 标记为 draining 并触发进程以非零状态退出，由进程管理器重启。

邮件的独立维护任务由 `crates/auction-jobs/src/worker/runtime/runner.rs` 的独立 `JoinSet` 持有，共用 shutdown watch、意外退出监督和总排空截止。每次 `sweep` 与随后到期的纯观察各有一个原 `fetch_timeout` 截止点，阶段内不重置；进入 drain 后不再启动新阶段，在途 `sweep` 保留原 discard 围栏。纯观察首次延后一个间隔，只在 `sweep` 结束后检查，失败也消耗间隔；内存时刻仅减少观察读取，不决定持久任务状态。

收件与维护的日程按同一世界相位平移，再叠加原 jitter；手动 `wakeup` 只唤醒收件。
每个 world 始终有一个维护任务和一个收件循环，通过同一
`Arc<tokio::sync::Mutex<()>>` 串行化维护与观察：隔离路径由维护任务执行，
旧路径由收件循环在 claim 前取得并持有到三分支 join 结束。切换按下一轮或
下一次维护门复读生效，在途轮次、claim、租约与预算不重置；旧路径下维护任务只
idle，不退出。维护日程、观察间隔与相位规则见
[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)，指标口径见
[crates/auction-observability/README.md#邮件循环口径](../auction-observability/README.md#邮件循环口径)。

世界相位计算与日程平移策略由 [crates/auction-jobs/src/worker/mail/schedule/mod.rs](src/worker/mail/schedule/mod.rs) 持有，经 crate 根导出 `mail_world_phase` 与 `PhasedMailSchedule`；Worker 装配根只判断是否启用相位、处理启动错误并注入日程。

拍卖到期提示按读取时的时钟转换为单调截止，查询与网络等待消耗原有等待预算；最后休眠不会把这段耗时再次加回。提示失败仍按常规轮询恢复，实际领取继续依赖数据库事实、租约与命令幂等。

## 外部账号观测与预算

Worker 通过应用层 `ExternalMetricsObserver` 记录邮件与论坛的阶段预算，关闭指标时为
no-op。记录同步、无 I/O、无 `.await`，不改变领取、租约、重试、排空、取消或写入 fence。
邮件维护/观察、fetch 尝试与批次由各自的截止点 owner 记录 `time` 耗尽；
Sent/Read 扫描和共享读取的 `time/pages` 由客户端预算边界记录。
正常排空取消不冒充预算耗尽；批次提交成功前缀仍保留已发生的耗尽。

论坛 `item_timeout` 记录 `topic_item`，PM 总预算记录 `pm_item`，
资料读取总预算记录 `profile`。回帖导入延迟估计只观测“首次新命令账本提交确认”，
`pending` 重试与已完成命令重放不采样；估计器有界保存内部帖子 ID 与单调时刻，
首次扫描、覆盖不完整、重启或淘汰时只记录缺样原因，不填零，也不影响扫描与命令。
导入确认不是竞价最终接受，跨阶段样本不能当作严格 Final SLA。
完整契约见[crates/auction-observability/README.md#外部账号客户端指标w04w05](../auction-observability/README.md#外部账号客户端指标w04w05)。

## Worker control

`[worker.control]` 默认关闭。显式启用后，Worker 在 Unix socket 上提供 HTTP/JSON 控制面（协议版本 `v1`）：

| 路径 | 用途 |
| --- | --- |
| `GET /v1/health/live` | 进程存活 |
| `GET /v1/health/ready` | control 已标记 serving、未进入 draining，且装配了数据库就绪探针时探针通过（与 Public/Admin `/health/ready`、Worker 心跳同一判定）；无数据库骨架只看进程状态，200 不代表外部能力已可用 |
| `GET /v1/status` | 版本、轮询器数量等安全状态 |
| `GET /v1/mail/progress` | 各邮箱轮询进度的安全观测 |
| `GET /v1/external-accounts` | W5.3：进程内外部账号快照（忙碌时长、请求门等待、维护轮次与论坛请求时间）；未装配或指标关闭时 `503 capability_unavailable`，响应不含凭据、Cookie、URL 或页面正文 |
| `POST /v1/mail/polls:trigger` | 触发一次邮件轮询（Admin 后台使用） |
| `GET /metrics` | Worker 的 Prometheus 指标 |

socket 由 Worker 在受信运行目录中创建并设为指定组；连接时用内核 peer 凭据（`SO_PEERCRED`）检查调用者 UID/GID 是否在 `allowed_peer_uids`/`allowed_peer_gids` 中，不在名单内直接断开。Admin 通过 `WorkerControlClient`（5 秒超时）读取状态与触发轮询；`auction-cli service` 与 `auction-server` 直接向该 socket 发送 `GET /v1/health/ready` 判定 Worker 就绪，因此它们的运行账号同样要在 allowlist 中。

## 维护约定

W13.3a 只在已有 JoinSet 叶子收口增加 panic 观测：
生命周期、结算、慢速代理、履约、恢复 action 和 delivery。
固定 `item_panic` 阶段使用 `SafeErrorKind::TASK_PANIC`，
并增加 `auction_worker_task_panics_total{loop}`；取消不增加。
原 join 诊断及 W0.3 的 `task_join` 未持久化计数不变，
仍按原方式合并 `Canceled` 等结果，不写入失败、退避或人工状态。
没有新增 catch_unwind，也未改动诊断 scope/inherit 或循环入口。

工作项 panic 计次封顶尚未实现（W13.3b 待定）：
生命周期领取已计次；结算、履约和恢复 action 重领不计次；
delivery 的 claimed 可重领，sending 到期加 grace 后转人工，不自动重发；
告警批次已有三次上限。邮件、论坛、PM、告警直接执行路径 panic
仍使常驻任务退出并触发 Worker 排空，详见
[ops/README.md#panic-排障](../../ops/README.md#panic-排障)。

- 新循环必须持久化领取与结果，不能用内存队列或“空端口成功”代替；超时与租约必须满足“单项超时 < 租约”。
- 论坛公开链接只接受无凭据、无路径和无查询的 HTTPS origin，或 `localhost`、`127.0.0.1`、`[::1]` 的回环 HTTP origin；运行时与配置预检使用相同边界。
- 错误只以安全分类进入日志、指标与 control，不透传数据库、凭据或外部响应正文。

停机新路径由装配根注入 `ExternalWriteLifecycle`：非外写 owner 和尚未登记交接的叶子准备期立即取消，不生成释放或失败命令。delivery、discard 和 PM 在围栏调用前登记，围栏调用开始即受保护；登记不授权外写，确认提交后才继续外部操作。论坛在持久 claim 下的首个实际 POST 前登记。交接与关键段均由原 future 驱动并阻止排空结束，结果提交后才 complete，其余 drop 只记未收口。父 JoinSet 不按活动快照 abort，避免连带取消关键子项。所有组共用装配根截止，致命故障不再另建 10 秒预算；PM 由装配根与 runner 一起等待。维护的原 fetch 截止和各任务原业务截止仍然有效。零关键段预算不注入活动工厂，完整选择旧排空路径。详见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。


## 运行时开关刷新

`start_runtime_switch_refresh` 启动即限时读取，此后五秒周期调用应用刷新服务，
每次一秒预算；Worker 另限时追加到期收口。调用方用 TaskGuard 管理生命周期，
刷新不依赖业务循环、主动暂停或就绪状态。邮件循环每轮读取
`mooglemail_poll_isolation_enabled`；指标边界在每次请求入口读取
`metrics_reject_forwarded_requests`。首次失败保留 TOML，后续保留成功快照和
原期限，见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。


## SLI 汇总任务

`start_sli_snapshot` 是独立六十秒唤醒任务，跳过漏掉的 tick，不追赶、不重试。
装配根只在 Worker 已有数据库且启用 metrics 时创建，并用 TaskGuard 持有。
每轮检查同源的 `metrics_sli_snapshot_enabled`，禁用不查询。
应用只读端口由 storage 实现，总预算两秒，只借一个现有空闲池连接。
成功发布完整数值快照，失败只记固定 kind/phase 和刷新指标，保留旧快照及原年龄。
任务不接业务退出通道、不改变 readiness，也不阻塞生命周期或外写循环。
关闭开关不强制取消已开始的短只读轮次；退出时由装配根回收。
口径见[crates/auction-observability/README.md#sli-与容量基线](../auction-observability/README.md#sli-与容量基线)。

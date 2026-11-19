# auction-jobs

[docs/README.md](../../docs/README.md) · [apps/auction-worker/README.md](../../apps/auction-worker/README.md) · [docs/architecture.md](../../docs/architecture.md)

Worker 的调度运行时与本地控制面：把应用层的持久化端口编排成相互独立的循环（领取 → 执行 → 落库），负责轮询间隔、抖动、租约续期、单项超时、有界并发、优雅排空与指标，以及 Worker control 的 Unix socket 服务端。业务状态迁移全部由应用端口决定，本 crate 不复制业务规则。

## 循环

`run_persistent_with_settlement_until_shutdown` 按下表组织循环（装备 PM 除外，由装配根启动）。排期 bootstrap、生命周期、慢速代理与标记/被超提醒共享拍卖主循环，结算是独立任务；邮件按世界独立运行收件与维护；论坛发布与回帖导入共享论坛循环；每个循环保留自己的持久化结果与错误边界。

| 循环 | 职责 |
| --- | --- |
| 拍卖主循环 | 每轮依次执行：排期 bootstrap（按周排期与赛季早鸟建立后续草稿场次，早鸟期暂停/恢复对应世界收件）、预览/开拍/Final/结算到期的生命周期推进、到期 Slow 代理的幂等反超、出价后的标记/被超提醒（按 lot 版本顺序处理，失败退避、事件不删除）；按生命周期与慢速代理的下一个最早到期时刻提前唤醒，常规轮询为上限 |
| 结算 | 独立任务，只领取并执行结算 claim；按最早结算到期提前唤醒，节奏不受拍卖主循环影响，两者只共享数据库租约与锁 |
| 结算履约 | 领取义务，执行 prepare → activate → reconcile，生成投递 |
| 投递 outbox | 先做只读准备，再在 sending fence 内调用 MoogleMail transport，续租并写回终态；结果未知转人工对账。只读准备与单次外部发送各用一份 `request_timeout_seconds` 预算：准备期间按续租间隔续租，交给 transport 的是准备请求的原截止而不是租约截止，续租失败立即取消准备；准备未完成（含超时，失败码 `preflight_timeout`）只释放 claim，不记发送失败。sending 围栏的确定结论由存储层按原因产出：业务门未满足（竞价警告失效、发送节奏未到期、资金门冻结，以及邮件证据 owner 判定凭据被删除、停用、改版本或运行时 generation 已被新 owner 推进这类观察之后的正常失效等）为 Deferred，释放 claim 并按正常延期结束；内部故障确定回滚（SQL 错误、冲突重试耗尽、提交预算耗尽，以及证据结构或摘要错误、generation 非正/超前/缺行（`evidence_request_invalid`）、证据与 claim 或冻结凭据绑定不符、凭据种类/预期玩家名/作用域不符（`dependency_mismatch`）、按数据库时钟已过期或含未来观测（`evidence_expired`）等）为 Faulted，同样释放 claim，但记 Worker 错误并按其安全分类计未持久化错误；提交结果未知保持隔离并撤销原会话 |
| 恢复 | 余额/证据健康刷新、资产对账，以及付款超时的提醒、退回、重新上架与退款；同一循环在消费退件之后推进结算退件案件：激活已提交的持久 Take（只激活本进程能以持久数据库凭据身份执行 Take 的世界，Worker 心跳以 `settlement_return_take_worlds` 声明，每次发布心跳时按当前凭据重新计算；投递领取同样排除其他世界的结算退件 Take，只配置 Cookie 时案件停在待领回）、收口 Take 终态、收口补寄寄出证明，并对待核验的未绑定退件只读读取原邮件（未装配 MoogleMail 时核验保持待办，Worker 心跳以 `settlement_return_recheck_worlds` 声明可核验的世界，Admin 详情据此显示“核验能力不可用”，最近一次未停止的声明过期后一个 TTL 内仍算可用）；结果未知的结算退件 Take 由邮箱维护在原 Take 会话里观察后收口或重新排队；资产对账是本循环的一个阶段，每 6 小时一次、单轮预算 300 秒，只读外部观察并追加报告，占用该轮的批量许可 |
| 邮件轮询 / 邮箱维护 | 每个世界一个收件循环与一个维护任务；维护先运行 Sent/Take 恢复与 compose 清理，再按最小间隔观察已读列表。默认隔离模式（`poll_isolation_enabled = true`，运行期开关 `mooglemail_poll_isolation_enabled`）下两者互不等待：收件循环每轮只取批量许可、不取维护锁；维护任务按维护日程每轮先取批量许可、再取该世界维护锁，sweep 结束即归还许可，只读观察期间仍持锁；两者的物理请求只由共享 MoogleMail 客户端的请求门与在途锁错开。兼容非隔离模式下收件循环每轮先取批量许可、再取维护锁，同一轮内并行执行收件、维护与只读观察并等待最慢分支，维护任务只周期重读开关。两种模式共用“先批量许可、后维护锁”的固定顺序，维护任务取锁后复核开关，切换窗口内既不互相等待，也不会在同一世界重叠维护。维护日程与隔离模式的收件日程由 Worker 装配根使用 `auction-config` 的世界相位平移，再注入循环 |
| 论坛同步 / 回帖 | 发布与刷新拍卖主题、导入回帖命令；两阶段错误互相隔离。Final 阶段按最早有效截止收紧导入节奏（剩余时间 / 6，夹在 5 秒与常规轮询之间），成功扫描、失败重试与同轮重复领取都沿用该节奏；论坛循环跨轮记住各 Final 主题的最新排期并按最早者提前唤醒，单个主题失败（含失败处置写入失败）或离开 Final 不影响其他主题；只有领取成功但为空才丢弃已不可领取主题的排期，领取调用失败时保留排期、至少间隔 5 秒重试；未知写入（`write_unknown`）维持常规轮询 |
| 玩家通知 | 赛季提醒与卖家小结扫描并幂等入队，单轮预算 60 秒；在本 crate 的任务组内监督并排空 |
| 装备 PM | 超价提醒私信队列；由装配根监督，主停止信号一到即停止新领取，不等其他循环排空 |
| 运维告警 | 扫描、分发、对账三个告警循环；未启用主动通知时不运行分发 |

每个循环使用 `batch_limit`、`concurrency`、`poll_interval`、`jitter`、`lease_duration`、`item_timeout` 等边界（来自 `[worker.*]`，可由 runtime 接管）。这些预算以及投递发送的 `request_timeout`、租约失效到期时，对 MoogleMail 读取（含装备页）与写入都只是放弃等待：已发出的由客户端交给独立任务走完，在途锁持有到远端回应或该次请求的 HTTP 预算结束，紧接的同账号请求不与之重叠；被放弃的写入结果按未知落库，绝不重发；尚在排队的撤票退出，不再发出，见[crates/auction-integrations/README.md#关键行为](../auction-integrations/README.md#关键行为)。停止信号后不再领取新任务，在途任务有界排空；常驻任务意外退出会把 control 标记为 draining 并触发非零退出，由进程管理器重启。维护日程与相位规则见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

## 外部账号观测

Worker 经应用层 `ExternalMetricsObserver` 记录邮件与论坛的阶段预算（同步、无 I/O、无 `.await`），关闭指标时只更新进程内外部账号快照、不注册 Prometheus 指标族；记录不改变领取、租约、重试、排空、取消或写入 fence。回帖导入延迟只观测“首次新命令账本提交确认”，`pending` 重试与重放不采样，缺样只记录原因、不填零。完整契约见[crates/auction-observability/README.md#外部账号客户端指标](../auction-observability/README.md#外部账号客户端指标)。

## Worker control

`[worker.control]` 默认关闭。显式启用后 Worker 在 Unix socket 上提供 HTTP/JSON 控制面（协议 `v1`），路径与方法见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)。

socket 由 Worker 装配根经 `auction-http` 的 `UnixHttpListener` 在受信运行目录绑定：目录链校验、单实例锁 `<runtime_directory>/.worker-control.lock`、stale socket 清理、owner/group/0660 权限与退出时按身份删除都与 Public/Admin 的 Unix listener 共用同一实现，`group` 可写组名或数字 GID。本 crate 只在其上叠加 peer 过滤与路由：连接时用 `SO_PEERCRED` 检查 UID/GID 是否在 `allowed_peer_uids`/`allowed_peer_gids` 中，不在名单内直接断开。旧版本的实例锁是 `<socket_path>.lock`，新旧版本不锁同一文件；绑定前的连通探测仍会拒绝活动 socket，升级时由部署流程先停旧 Worker 再启动新版本，保证两者不会同时持有 control socket。Admin 经 `auction-integrations` 的 `WorkerControlClient`（5 秒超时）读取状态与触发轮询、经 `WorkerRemoteReadClient` 代读外部只读页面并转交邮件 Cookie 在线检查（协议常量与线上 DTO 由 `auction-application` 拥有）。邮件 Cookie 在线检查只提交凭据 ID 与期望状态版本，Worker 按 ID 打开数据库信封、前后复核版本，用与邮件循环共用请求门、在途锁与账号忙熔断的客户端读取余额页：账号忙窗口内不发网络、成功不清除忙碌；主动暂停在入口、每个世界开始前、排队等待期间与取得在途锁后的发出前复核，生效后不再发出探测；检查被取消或单个世界 5 秒预算到期时，排队中的探测撤票退出，已发出的探测在独立任务里走完才释放在途锁，Worker 侧紧接的请求不与之重叠（客户端超时不能证明远端已停止，持锁上限见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)）；同时最多一个检查，重叠或上一次检查已发出的探测尚未走完时返回 `busy`，总预算 12 秒；未装配执行器（未启用邮件或没有 `credential_data_key`）时返回 `501 capability_unavailable`，Admin 只报暂不可用、不回退直连。`auction-cli service` 与 `auction-server` 直接向该 socket 发送 `GET /v1/health/ready` 判定就绪，其运行账号同样要在 allowlist 中。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-jobs/src/control/` | Worker control 服务端（peer 过滤 listener 与路由）与邮件进度快照；socket 绑定在 `auction-http`，客户端在 `auction-integrations` |
| `crates/auction-jobs/src/worker/` | 循环装配与各 owner：`auction`、`settlement`、`forum`、`mail`、`recovery`、`runtime`（投递、恢复动作、通知、资产对账）、`lot_watch_pm`、`operations`、`player_notifications`、`diagnostics` |
| `crates/auction-jobs/src/runtime_switches/`、`mail_templates/`、`sli/` | 五秒开关刷新、五秒通知模板刷新与六十秒只读 SLI 汇总任务 |

## 维护约定

- 新循环必须持久化领取与结果，不能用内存队列或“空端口成功”代替；超时与租约必须满足“单项超时 < 租约”。
- 批量与维护循环共享连接配额；同时需要配额许可与其他进程内互斥锁时，一律先取许可、后取锁，持锁期间不得等待许可。
- 论坛公开链接只接受无凭据、无路径和无查询的 HTTPS origin，或回环 HTTP origin；运行时与配置预检使用相同边界。
- 错误只以安全分类进入日志、指标与 control，不透传数据库、凭据或外部响应正文。
- 停机新路径由装配根注入 `ExternalWriteLifecycle`：外写前登记并在确认提交后继续；交接与关键段阻止排空结束，父 JoinSet 不按活动快照 abort。详见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。
- 固定 `item_panic` 阶段用 `SafeErrorKind::TASK_PANIC` 并计数 `auction_worker_task_panics_total{loop}`；取消不增加。邮件、论坛、PM 与告警直接执行路径的 panic 仍使常驻任务退出并触发排空，见[ops/README.md#panic-排障](../../ops/README.md#panic-排障)。

## 通知模板刷新

`start_mail_template_refresh` 由三个角色的装配根启动，按五秒周期把生效模板整体替换进进程内快照；定时器只负责唤醒，刷新与失败语义见[docs/contracts/database.md#出站通知模板](../../docs/contracts/database.md#出站通知模板)。

## 运行时开关刷新与 SLI

`start_runtime_switch_refresh` 启动即限时读取，此后按五秒周期刷新，开关闭集与刷新语义见[ops/config/README.md#数据库运行时开关](../../ops/config/README.md#数据库运行时开关)。`start_sli_snapshot` 是独立六十秒唤醒任务，跳过漏掉的 tick、不追赶不重试；每轮检查 `metrics_sli_snapshot_enabled`，禁用不查询，总预算两秒且只借一个空闲连接，失败保留旧快照。口径见[crates/auction-observability/README.md#sli-与容量基线](../auction-observability/README.md#sli-与容量基线)。

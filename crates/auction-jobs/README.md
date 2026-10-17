# auction-jobs

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

持久化任务的调度、重试、游标推进与本机 Worker control；不复制拍卖状态机。Worker 决定何时调用应用命令、
如何按持久化状态重试，业务决定归领域/应用层，SQL 与锁归存储层。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/worker/runtime](crates/auction-jobs/src/worker/runtime) | 循环组合、端口装配、任务所有权与退出 |
| [src/worker/auction](crates/auction-jobs/src/worker/auction) | 生命周期与慢速代理调度 |
| [src/worker/settlement](crates/auction-jobs/src/worker/settlement) | 结算/履约编排 |
| [src/worker/recovery](crates/auction-jobs/src/worker/recovery) | 恢复阶段与证据收口 |
| [src/worker/mail](crates/auction-jobs/src/worker/mail) | 轮询、续租、重试与完成前缀 |
| [src/worker/forum](crates/auction-jobs/src/worker/forum) | 主题/固定帖同步与命令导入 |
| [src/worker/forum/pm](crates/auction-jobs/src/worker/forum/pm) | 装备 PM 独立有界循环、只读准备、持久 sending fence 后一次发送和原 sender 核对 |
| [src/worker/realtime](crates/auction-jobs/src/worker/realtime)、[operations](crates/auction-jobs/src/worker/operations) | 实时消费与运维告警 |
| [src/control](crates/auction-jobs/src/control) | Unix control 协议、peer 认证与客户端 |

## 源码布局

`lib.rs` 只负责导出，`worker/` 按业务 owner 拆分；`runtime/model.rs` 持有端口/参数，`runtime/runner.rs` 组合循环、
drain 与任务退出，`runtime/delivery/lease.rs` 统一准备、交接与发送的有界续租。control 协议、handler、Unix server
和 Admin client 独立放在 `control/transport/`，调度器通过 `WorkerControlHandle` 接入状态与唤醒。
`worker/forum/reply/rejection.rs` 独占拒绝/警告的分类及英文展示白名单，不复制竞价规则或重试决定；
`worker/forum/sync/layout.rs` 按顺序建立全部目录页及系统日志，并对账精确楼层、拍卖标记和 post ID。

## 生命周期慢速代理与结算

| 循环 | 编排要求 |
| --- | --- |
| Auction | 生命周期、slow due、结算依次执行，各阶段失败不跳过后续阶段 |
| Fulfillment | 按可用并发槽领取，持久化 prepared/enqueued 与 delivery 关联，不提前占满租约 |
| Recovery | 保持 case → return → action → relist → refund 顺序，各阶段独立收口 |
| Delivery | claim 后先查可用性并只读 prepare，持有 prepared guard 后才进入 sending；续租、最终写回和未知结果受 fencing 保护 |
| Mail | main/isekai 独立 mailbox、cron、唤醒器、游标与租约；按旧到新处理 |
| Forum | 独立顺序循环，先发布再读回帖，发布失败不跳过导入；逐主题领取内容租约 |
| Lot-watch PM | 独立 runtime，每轮至多 1 新发送 + 1 只读核对；delivery cadence、forum 总预算/lease，串行共享网关 |
| Operations alert | scan、dispatch、reconcile 各自 cadence、预算与失败边界 |

`run_persistent_until_shutdown` 消费持久化轮次；领取后冻结 owner、精确租约与 state_version。周拍卖只在配置
allowlist、runtime 开关和数据库 enabled world 的交集中新建，已有轮次按冻结事实执行，配置变更不以旧命令 ID 重发
新负载；结算使用持久化 claim 的 due_at，不以实际领取延迟改变业务时间。慢速代理扫描候选后交竞价事务重新校验，
过期 generation 安全跳过。

Fulfillment pending 通过 `prepare_next` 区分 External 与 `InternallySatisfied`：内部成功立即结束 claim，不 activate、
不生成 delivery ID。pending 身份准备与 enqueued 收款核对采用不超过有效 lease/30 秒的单一超时，覆盖 guard 排队、
只读请求及数据库提交，不 detach future；提交阶段取消由 opaque guard 撤销 owner，普通证据/资金等待由应用持久延期。

Delivery 有效并发取配置与 transport 的较小值（MoogleMail 当前上限 1）。发送前无凭据、compose prepare 失败或超时
只释放 claim/在本轮排除该 world 的 Send，下轮重新读取真实 compose；Take/Return 独立运行，`batch_limit=1` 也能继续
取其他合格任务。claimed 准备阶段在原 request_timeout 预算内续租，prepare 完成后先续租再提交 fence，明确成功后再次
确认续租；等待受已知租约单调截止和 finalize_timeout 限制，续租耗时超过间隔时操作仍可在有效截止内先被 poll。
`prepare_authorized` 的现金证据与未 poll 的 future 持同一原 session，同一 commit guard 覆盖完整授权 fence；
通过 `start_sending_authorized_until` 把 prepared 原单调绝对截止与有效租约、finalize 截止的最小值传至存储，
不从交接时刻重建 30 秒预算。只有截止前收到确定回执才确认 guard/fence，过晚回执、提交未知或取消撤销同一 mail owner
且不 poll；确定 Deferred 只延后，legacy 候选也保留撤销钩子。Send、Return、Take 的证明由各 owner 校验，
成功不取决于 HTTP 成功页（见[邮件诊断](ops/mail/diagnostics.md)）。

Mail 仅对可重试错误与超时做有界 inline retry，处理前先查持久化完成证明。批次预算独立于 lease，耗尽只提交已完成
连续前缀及 cursor；处理中途确定失败时同一 fenced 提交保存成功前缀与失败退避，不采用失败页整体 next_cursor。
完成证明和已接受决定允许恢复提交前已完成的步骤，不能按取件后的资产变化重新解释旧邮件。

Forum 目录每帖最多 150 件、从 #1 连续分页，系统日志在最后一个目录页之后、用户回帖再接下一楼。全部固定帖的
exact post ID、拍卖/页码标记与本地持久化身份齐备后才导入普通出价；历史已尝试/发布/未知主题按冻结单页布局继续
处理，升级不挪楼，抢楼、缺页或布局不匹配失败关闭。原始正文校验由部署字段 `worker.forum.verify_post_body` 控制
（默认关闭），启用后用于固定帖刷新及未知结果对账，不是每轮出价导入前的额外条件。系统更新楼使用白名单英文摘要与
具体错误码（如 `already_leading`、`bid_too_low`、`auction_closed`），旧通用码 `command_rejected` 只对已知完整诊断作
只读映射，原始有界详情只留本地命令账本。
可重试身份/存储错误停止后续楼层，真实连续处理楼层遇待重试命令即停；导入前读取持久 pending 边界，
原帖删除或原命令未收口时必须在执行后续竞价前停止。主题同步未知 POST（含 `reconcile_write_unknown`）短退避后只做精确标题只读对账，
无法证明原目标时等待 CLI 持久人工再授权；已发布主题的回帖刷新按持久 post ID 对账，关闭正文校验时保留内容未知门禁。
论坛终态刷新仅纳入升级时未结算及新场次，已 settled 不再导入新竞价，部分进度、租约丢失及 write_unknown 不提前完成。

实时 runner 持久化连续 offsets/failures；当前广播边界是 Public 本地 Hub，未冻结外部 sink 不得伪装为已发送。
PM 的 prepare 只能 GET，持久 sending fence 明确提交才消费 prepared.send，任何 sending 不能安全延期；单项总预算覆盖
claim/prepare/fence/send/finish 且严格小于 lease，drain 停止新领取、主进程 drain 结束后 abort 剩余任务。
各循环默认值、预算及有效并发见[配置手册](ops/config/README.md#worker-循环预算)。

## Worker control

只经 UDS 提供 live、ready、status、`/metrics` 和全局邮件唤醒，不启动第二个指标监听器。socket 位于受信 runtime 目录，
持 `0600` 实例锁，只清理已证明失效且 inode 未变的旧 socket；新 socket 固定配置 group 与 `0660`，连接须通过显式
UID/GID allowlist，并受请求/响应大小与处理预算限制。唤醒使用有界内存 replay window，只表示已接受，让已装配 poller
执行一轮，不能代替 `mail_poll_state` 的持久化游标、租约或业务完成证明。协议与部署参数见
[Worker 入口](apps/auction-worker/README.md)。

## 边界约束

所有业务状态迁移经应用命令，游标、账本与任务进度必须持久化，外部副作用在本地提交后执行；本机 control HTTP 仅用于
经 peer 认证的运维。进入 drain 后门禁不可逆：停止新领取并收口在途 claim，每个循环及并发槽等待后都复核门禁；
某组失败仍继续收口其他组，所有任务共享同一总截止时间，信号、10 秒总上限与二次信号强制退出由装配根和
`auction-process` 拥有。论坛扫描区分未确认 POST 与纯读取超时：前者先只读对账，后者可正常重试；
主题创建返回 `Created`/`Existing`，精确标题命中旧帖不算本轮 POST 成功，关闭校验时既有目录或日志不能充当本轮正文
已发送的证明。

## 维护与验证

```bash
cargo check --locked -p auction-jobs
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。

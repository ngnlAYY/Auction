# auction-jobs

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

持久化任务的调度、重试、游标推进与本机 Worker control；不复制拍卖状态机。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [源码布局](#源码布局)
- [生命周期、慢速代理与结算](#生命周期慢速代理与结算)
- [Worker control](#worker-control)
- [边界约束](#边界约束)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/worker/runtime](src/worker/runtime) | 循环组合、端口装配、任务所有权与退出 |
| [src/worker/auction](src/worker/auction) | 生命周期与慢速代理调度 |
| [src/worker/settlement](src/worker/settlement) | 结算/履约编排 |
| [src/worker/recovery](src/worker/recovery) | 恢复阶段与证据收口 |
| [src/worker/mail](src/worker/mail) | 轮询、续租、重试与完成前缀 |
| [src/worker/forum](src/worker/forum) | 主题/固定帖同步与命令导入 |
| [src/worker/forum/pm](src/worker/forum/pm) | 装备 PM 独立有界循环、只读准备、持久 sending fence 后一次发送和原 sender 核对 |
| [src/worker/realtime](src/worker/realtime)、[operations](src/worker/operations) | 实时消费与运维告警 |
| [src/control](src/control) | Unix control 协议、peer 认证与客户端 |

## 职责与实现边界

Worker 决定何时调用应用命令、如何按持久化状态重试；领域/应用层拥有业务决定，存储层拥有 SQL 与锁。

## 源码布局

`lib.rs` 只负责导出，`worker/` 按业务 owner 拆分；`runtime/model.rs` 持有端口/参数，
`runtime/runner.rs` 组合循环、drain 与任务退出；`runtime/delivery/lease.rs` 统一准备、交接与发送的有界续租。
control 协议、handler、Unix server 和 Admin client
独立放在 `control/transport/`，调度器通过 `WorkerControlHandle` 接入状态与唤醒。
`worker/forum/reply/rejection.rs` 独占拒绝/警告的分类及英文展示白名单，不复制竞价规则或重试决定。
`worker/forum/sync/layout.rs` 按顺序建立全部目录页及系统日志，并对账精确楼层、拍卖标记和 post ID。

## 生命周期、慢速代理与结算

| 循环 | 编排要求 |
| --- | --- |
| Auction | 生命周期、slow due、结算依次执行，各阶段失败不跳过后续阶段 |
| Fulfillment | 按可用并发槽领取，持久化 prepared/enqueued 与 delivery 关联，不提前占满租约 |
| Recovery | 保持 case → return → action → relist → refund 顺序，各阶段独立收口 |
| Delivery | claim 后先查可用性并只读 prepare，持有 prepared guard 后才进入 sending；续租、最终写回和未知结果受 fencing 保护 |
| Mail | main/isekai 独立 mailbox、cron、唤醒器、游标与租约；按旧到新处理 |
| Forum | 独立顺序循环，先发布再读回帖，发布失败不跳过导入；逐主题领取内容租约 |
| Lot-watch PM | 独立 runtime，每轮至多 1 新发送+1 只读核对；delivery cadence、forum 总预算/lease，串行共享网关 |
| Operations alert | scan、dispatch、reconcile 各自 cadence、预算与失败边界 |

`run_persistent_until_shutdown` 消费持久化轮次；领取后冻结 owner、精确租约与 state_version。
周拍卖只在配置 allowlist、runtime 开关和数据库 enabled world 的交集中新建；已有轮次按冻结事实执行，
配置变更不以旧命令 ID 重发新负载。结算使用持久化 claim 的 due_at，不以实际领取延迟改变业务时间。
慢速代理扫描候选后交竞价事务重新校验，过期 generation 安全跳过。

Fulfillment pending通过 `prepare_next` 区分原External与 `InternallySatisfied`；内部成功立即结束claim，
不activate、不生成delivery ID或外部成功记录。pending身份准备与enqueued收款核对采用不超过有效lease/30秒的
单一超时，覆盖guard排队、全部只读请求及数据库提交，不detach future；提交阶段取消由opaque guard撤销owner。
普通证据/资金等待由应用持久延期，不写外部unknown；未知提交保留原稳定命令待重放。

Delivery 有效并发取配置与 transport 的较小值，MoogleMail 当前上限为 1。
发送前无凭据只释放 claim 并在本轮排除该世界；compose prepare 失败/超时仅在本轮排除该世界的 Send，
下轮重新读取真实 compose。Take/Return 仍独立运行，preflight 延后不占发送批次，batch_limit=1 也能继续取其他合格任务。
释放撤销本次领取计数，保留 pending；不把所有排队任务逐个耗成 unknown，也不持久封禁整个 world。
claimed 准备阶段在原 request_timeout 总预算内续租，数据库等待不延长该预算；续租失败/超时丢弃只读准备并尽力释放。
prepare 完成后先续租再提交 fence，fence 明确成功后再次确认续租；等待均受已知租约单调截止和 finalize_timeout 限制。
prepared future 仅在交接确认后 poll；准备首次续租独立于 select 排序，不能被操作分支绕过。
准备/发送均保持 biased 的请求超时、租约过期优先，再给操作一次直接检查双截止的 poll，Pending 后才检查续租。
续租耗时4秒而间隔3秒时，回执后的 renew_at 虽已到期，操作仍能在有效截止内先被 poll，不连续被续租饿死。
下一次续租仍按请求开始时刻计算，不把回执时刻加完整间隔误当剩余 TTL；瞬时回执维持原节奏，截止已到则禁止 poll。
失败先丢弃句柄，release 仅接受 claimed，fence 提交未知保持隔离。
Delivery 使用新增 `prepare_authorized`；现金准备证据与未poll的future持同一原session，
同一 commit guard 覆盖完整授权 fence future 及存储有界重试，不 detach。
现金交接使用 prepared 原单调绝对截止、有效租约截止与 finalize 截止的最小值；前置续租也不越过原证据期限。
通过 `start_sending_authorized_until` 把同一收紧截止传至存储，不从交接时刻重建30秒预算；
只有截止前收到确定回执才确认 guard/fence，过晚回执、提交未知或取消撤销同一 mail owner，不 poll 发送。
确定 Deferred（未写入/回滚/已提交冻结）只延后，legacy 候选也保留撤销钩子。
只有Sending与后续续租均确认才消费证据并首次poll；原双截止、慢续租后操作优先poll的公平性保持不变。
上述未开始写入的延后仍只排除本轮目标 world Send；进入发送后沿用单次发送及未知结果隔离，不盲目重寄。
Send、Return、Take 的证明由各 owner 校验，成功不取决于 HTTP 成功页，见[邮件诊断](../../ops/mail-diagnostics.md)。

Mail 仅对可重试错误与超时做有界 inline retry，处理前先查持久化完成证明。
批次预算独立于 lease；耗尽只提交已完成连续前缀及 cursor，一封未完成则失败退避。
处理中途确定失败时，同一 fenced 提交保存成功前缀与失败退避；不采用失败页的整体 next_cursor，也不跨事务 commit 后 fail。
完成证明和已接受决定允许恢复提交前已完成的步骤，不能按取件后的资产变化重新解释旧邮件。
诊断 resolve 使用独立有界窗口，不改变业务重试分类、完成证明或 cursor。

Forum 目录每帖最多 150 件，从 #1 连续分页；系统日志在最后一个目录页之后，用户回帖再从下一楼开始。
全部固定帖的 exact post ID、拍卖/页码标记与本地持久化身份齐备后才导入普通出价。
历史已尝试/发布/未知主题按冻结单页布局继续处理，升级不挪楼；新主题按 150 件分页。
抢楼、缺页或冻结布局不匹配失败关闭，不覆盖用户帖子，也不自动移动旧日志；续页和日志不作为用户命令解析结果导入。
原始正文校验由部署字段 `worker.forum.verify_post_body` 控制，默认关闭；启用后用于固定帖刷新及未知结果对账，
不是每轮出价导入前的额外条件。系统更新楼对待处理/拒绝命令使用英文摘要；新拒绝按应用错误类型保存具体错误码，
例如 `already_leading`、`bid_too_low`、`auction_closed`，原始有界详情保留在本地命令账本。
展示只使用白名单英文。旧 `command_rejected` 或空码仅对已知完整诊断作只读映射，兼容已有领先者拒绝和规范整数金额诊断；
未知码/详情回退安全通用提示。解析器和运行期警告同样只使用白名单，格式问题提示新增回帖，待重试提示不要重复提交。
通知错误不代表竞价失败。更新楼按 pending、最近警告、忽略记录分类，展示上限与截断说明由 integrations 持有。
升级只改变正常扫描时的系统更新楼镜像，不改历史账本、金额、命令 ID 或拒绝终态，也不重放出价。
可重试身份/存储错误停止后续楼层；真实连续处理楼层遇待重试命令即停，不能从 post ID 或正文版本推算。
导入前读取持久 pending 边界；原帖删除、命令被编辑掉或原命令未收口时，必须在执行后续竞价前停止。
主题同步未知 POST（含 `reconcile_write_unknown`）短退避后仅精确标题只读对账，无法证明原目标时等待 CLI 持久人工再授权；
历史同步未知结果可在有可用凭据的进程首轮水位下重新读取一次，不自动补写、升级旧布局或解除人工授权门禁。
已发布主题的回帖刷新按持久 post ID 对账；关闭正文校验时保留内容未知门禁，现有 CLI 的主题发布授权不适用。代理行不进入论坛命令账本。

论坛终态刷新仅纳入升级时未结算及新场次；已 settled 不再导入新竞价，已有 pending 继续展示并阻止完成，不删除或伪报成功。
每个用户帖前复核接收状态，命令开始由存储锁内再查；扫描期间进入 settled 时，刷新仍从同一权威快照渲染最终结果。
终态必须逐一确认全部冻结目录页及日志，即使本地镜像相同；关闭正文校验时用本轮精确 POST 回执，开启时按原始正文证明规则回读。
全页确认后的快照版本经存储锁/CAS 持久化，之后停止该主题轮询；部分进度、租约丢失及 write_unknown 不提前完成。
无新楼层、无历史结拍回填、无新配置；大场次仍受单主题预算和租约约束。

实时 runner 持久化连续 offsets/failures；当前广播边界是 Public 本地 Hub，未冻结外部 sink 不得伪装为已发送。
各循环默认值、预算及有效并发见[配置手册](../../ops/config/README.md#worker-循环预算)。

PM 的 prepare 只能 GET，持久 sending fence 明确提交才消费 prepared.send；任何 sending 不能安全延期。
单项总预算覆盖 claim/prepare/fence/send/finish，严格小于 lease，超时取消留给持久恢复；drain 停止新领取，
主进程已有 drain 结束后 abort 剩余任务，不追加论坛长等待。原 sender Sent 无证据保持 manual_reconcile，绝不重寄。
论坛 enabled=false 不装配，MM disabled 不影响此 runtime；无行为验收证据。

## Worker control

只经 UDS 提供 live、ready、status、`/metrics` 和全局邮件唤醒，不启动第二个指标监听器。
socket 位于受信 runtime 目录，持 `0600` 实例锁，只清理已证明失效且 inode 未变的旧 socket；
新 socket 固定配置 group 与 `0660`，连接须通过显式 UID/GID allowlist，并受请求/响应大小与处理预算限制。

唤醒使用有界内存 replay window，只表示已接受，让已装配 poller 执行一轮；
不能代替 `mail_poll_state` 的持久化游标、租约或业务完成证明。协议与部署参数见[Worker 入口](../../apps/worker/README.md)。

## 边界约束

所有业务状态迁移经应用命令，游标、账本与任务进度必须持久化；外部副作用在本地提交后执行。
本机 control HTTP 仅用于经 peer 认证的运维，不承载 Public/Admin 业务写入。

进入 drain 后门禁不可逆，停止新领取并收口在途 claim；每个循环及并发槽等待后都复核门禁。
某组失败仍继续收口其他组，所有任务共享同一总截止时间。信号、10 秒总上限与二次信号强制退出
由装配根和 `auction-process` 拥有，不能为后续任务重新计时。

论坛扫描区分未确认 POST 与纯读取超时；前者先只读对账，后者可正常重试。
主题创建返回 `Created`/`Existing`；精确标题命中旧帖不算本轮 POST 成功。关闭校验且无人工授权时，
既有目录或日志不能充当本轮正文已发送的证明；正常新建则必须有本轮创建与每个续页/日志的回复回执。
按帖子记录本轮目标的写入回执，授权补楼或升级后不重复编辑；开启正文校验而回读不匹配时转未知门禁，不在本轮再次 POST。
关闭正文校验时，历史未知结果仍须人工对账；同步转人工授权，回帖刷新保留未知门禁，不发送 POST，
不把恢复期间新生成但未发送的镜像记为已同步。正常成功 POST 只确认身份与回执，不宣称远端正文逐字一致。
开启时，所有固定楼层均须按[正文确认规则](../../docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)
回读匹配才能解除；本地目标镜像的变更检测始终保留，避免关闭校验后每轮重复编辑。

## 维护与验证

```bash
cargo check --locked -p auction-jobs
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。

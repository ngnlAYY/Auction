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

`lib.rs` 只负责导出，`worker/` 按业务 owner 拆分；`runtime/model.rs` 持有端口/参数，`runtime/config.rs` 以共用的
批量/轮询/租约边界谓词二次校验各循环配置（Recovery 复用 Fulfillment 的同形运行边界），`runtime/runner.rs` 组合循环、
drain 与任务退出，`runtime/delivery/lease.rs` 统一准备、交接与发送的有界续租；
`runtime/delivery/wakeup.rs` 经窄端口接收提交后提示、重连后补扫描并合并唤醒，退出跟随 drain。control 协议、handler、Unix server
和 Admin client 独立放在 `control/transport/`，调度器通过 `WorkerControlHandle` 接入状态与唤醒。
`worker/forum/reply/rejection.rs` 独占拒绝/警告的分类及英文展示白名单，不复制竞价规则或重试决定；
`worker/forum/sync/layout.rs` 按顺序建立全部目录页及系统日志，并对账精确楼层、拍卖标记和 post ID。

## 生命周期慢速代理与结算

| 循环 | 编排要求 |
| --- | --- |
| Auction | 生命周期、slow due、结算依次执行，各阶段失败不跳过后续阶段；`worker/auction/wakeup.rs` 以只读提示在最早的未来到期时刻提前唤醒，轮询间隔为最长等待 |
| Fulfillment | 按可用并发槽领取，持久化 prepared/enqueued 与 delivery 关联，不提前占满租约 |
| Recovery | 保持 case → return → action → relist → refund 顺序，各阶段独立收口 |
| Delivery | claim 后先查可用性并只读 prepare，持有 prepared guard 后才进入 sending；续租、最终写回和未知结果受 fencing 保护 |
| Mail | main/isekai 独立 mailbox、cron、唤醒器、游标与租约；按旧到新处理 |
| Forum | 独立顺序循环，先发布再读回帖，发布失败不跳过导入；逐主题领取内容租约 |
| Lot-watch PM | 独立 runtime，每轮至多 1 新发送 + 1 只读核对；delivery cadence、forum 总预算/lease，串行共享网关 |
| Operations alert | scan、dispatch、reconcile 各自 cadence、预算与失败边界 |

主题发布与回帖导入的单项截止在领取前冻结，领取和外呼共用预算，慢领取不能延长处理时间越过 lease。
创建、回复和编辑的实际 POST 共用写入进度；纯读取超时保存普通可重试失败，
本轮已写或历史未知仍保留 reconcile/人工授权，不能把暂时数据库读取失败误记为未知外写。
生命周期、结算、履约与 recovery action 的并发任务也沿用领取前绝对截止，任务排队不重置剩余预算；
首次 poll 已到期时不开始应用命令或外呼，保留原持久 claim 交由租约恢复。
生命周期、结算、履约及 recovery action 在领取回执后和任务首次调度时复核 drain；慢速代理任务首次调度也复核。
论坛主题同步与回帖导入也在领取回执后复核，停止前尚未外呼的 claim 不进入发布或导入流程。
drain 前已开始的处理继续有界收口，尚未执行的 claim 保留原租约和命令身份，恢复时不追加伪造失败。
持久化 runner 在启动任何循环前先轮询停止 future；装配期间已到达的停止请求直接进入 drain，不先派生任务再补停机通知。
邮箱配置在启动时校验一次；各轮只计算下一触发时刻，非严格未来时刻立即停止该邮箱循环，避免零间隔忙等。

`run_persistent_with_settlement_until_shutdown` 消费持久化轮次；领取后冻结 owner、精确租约与 state_version。周拍卖只在配置
allowlist、runtime 开关和数据库 enabled world 的交集中新建，已有轮次按冻结事实执行，配置变更不以旧命令 ID 重发
新负载；结算使用持久化 claim 的 due_at，不以实际领取延迟改变业务时间。慢速代理扫描候选后交竞价事务重新校验，
过期 generation 安全跳过。唤醒提示由调度、慢速代理与结算各自的 storage owner 只读计算，生命周期与慢速代理以数据库时钟、
结算以其领取所用的进程时钟为参照；提示只决定醒来时刻，2 秒内未取得或已过期时沿用 `poll + jitter`，避免持续失败时空转。
结算完成请求摘要由应用层统一计算，保留原 `workflow_id`、`due_at` 字段顺序和稳定命令身份。
锁内身份或命令摘要冲突均记录为 `settlement_mismatch`；失败仍按原 claim 持久退避，不把冲突当作可重试的事务序列化错误。
v2 优惠按奖池同比缩放由领域计划拥有，Worker 不为等优惠预算暂停结算，也不自行发行或补充奖池。
已登记的 Admin 首期/后继按数据库身份执行，不套用自动 bootstrap 的 UUID，也不要求两小时预览间隔。暂停/恢复命令
按周基础身份、邮箱及本次观测版本派生，并把期望版本写入摘要；Admin 改动后不重放本周旧结果。已预览后取消的轮次
恢复邮箱轮询；未预览的 missed 轮次不撤销与本轮无关的暂停状态。
周排期在数据库时间、world、冻结轮次和邮箱状态读取后复核同一 drain 门禁；
读取跨过门禁后，不再创建轮次、进入下一世界或发起邮箱状态命令，已提交结果保留。


Fulfillment pending 通过 `prepare_next` 区分 External 与 `InternallySatisfied`：内部成功立即结束 claim，不 activate、
不生成 delivery ID。pending 身份准备与 enqueued 收款核对采用不超过有效 lease/30 秒的单一超时，覆盖 guard 排队、
只读请求及数据库提交，不 detach future；提交阶段取消由 opaque guard 撤销 owner，普通证据/资金等待由应用持久延期。
每次阶段交接先复核 claim 截止，零剩余预算不 poll 应用 future，避免 Tokio timeout 的首次 poll 在过期后启动只读外呼。
阶段收口只保留日志所需的 obligation ID 与重试次数，不为指标复制整份装备/交付快照。
自动恢复健康与履约复用数据库资金策略：仅记账账户跳过 Balance 请求，现金状态标为不适用，仍更新恢复积压及真实投递状态。

Recovery take 的新 prepare 命令绑定 action 与来源 MID，activate 从该 prepare 派生；同轮领取/重试不改变身份，
真实新退件获得独立身份。旧 prepared/enqueued 保留原 prepare/activate；pending 先只读恢复精确匹配的旧 prepare，
覆盖 prepare 已提交、action 未关联的升级窗口，避免同一 MID 产生第二个 take。退件消费唯一性及计数由 storage
同事务保证，Worker 不清除消费历史。

Delivery 的七类发送间隔由存储在领取和 sending fence 内复核，Worker 不持有第二套时间真值。
超价 MM 提交后优先唤醒，当前轮次运行时合并为下一次即时扫描；不抢占在途任务，不额外唤醒 Auction。
连接失效保留普通 poll/jitter，通知无任务数据、不能授权发送，断线重建后先补一次扫描。
批次收件人排除项仅在 `recipient_not_found` 分支从原 claim 规范化构造，成功及其它失败不复制收件人/world。
运行时重复校验 Delivery 的轮询、租约、请求及收口预算上限，与部署配置边界一致。
领取请求受领取前单调租约截止约束；超时或迟到时不启动发送，未知领取结果保留原数据库租约恢复。
领取回执跨过 drain 时，已确认且尚未开始准备的 claimed 按原 fence 释放；任务首次调度也复核 drain。

Delivery 有效并发取配置与 transport 的较小值（MoogleMail 当前上限 1）。发送前无凭据、compose prepare 失败或超时
只释放 claim/在本轮排除该 world 的 Send，下轮重新读取真实 compose；Take/Return 独立运行，`batch_limit=1` 也能继续
取其他合格任务。claimed 准备阶段在原 request_timeout 预算内续租，prepare 完成后先续租再提交 fence，明确成功后再次
确认续租；等待受已知租约单调截止和 finalize_timeout 限制，续租耗时超过间隔时操作仍可在有效截止内先被 poll。
续租回执还须早于本次 `min(旧租约截止, finalize_timeout)`；即使 Tokio 内层 future 在一次 poll 中
迟到返回成功，也不延长本地 owner 或继续 poll 外部写入。
`prepare_authorized` 的现金证据与未 poll 的 future 持同一原 session，同一 commit guard 覆盖完整授权 fence；
通过 `start_sending_authorized_until` 把 prepared 原单调绝对截止与有效租约、finalize 截止的最小值传至存储，
不从交接时刻重建 30 秒预算。只有截止前收到确定回执才确认 guard/fence，过晚回执、提交未知或取消撤销同一 mail owner
且不 poll；确定 Deferred 只延后，legacy 候选也保留撤销钩子。Send、Return、Take 的证明由各 owner 校验，
成功不取决于 HTTP 成功页（见[邮件诊断](ops/mail/diagnostics.md)）。
实际发送的完整主题、正文摘要及资产矩阵由准备句柄冻结，交接时随原 Sending 事务落库；Worker 不重渲染正文。
Take 的准备句柄冻结原账号与 world/MID，并持有原凭据会话；Worker 通过 Send/Take 判别期待原样传给普通 Sending fence。
现金证据只授权 Send，Take 不进入现金授权入口；回执迟到、续租失败和取消继续沿同一截止与撤销路径收口。
确定的 `invalid_payload` 在有效 claimed 下失败收口并追加审计，不伪造 Sending；其它只读准备失败仍延期。

Mail 仅对可重试错误与超时做有界 inline retry，处理前先查持久化完成证明。批次预算独立于 lease，耗尽只提交已完成
连续前缀及 cursor；处理中途确定失败时同一 fenced 提交保存成功前缀与失败退避，不采用失败页整体 next_cursor。
完成证明和已接受决定允许恢复提交前已完成的步骤，不能按取件后的资产变化重新解释旧邮件。
邮箱租约的单调截止在领取请求前建立，回执只会按剩余 TTL 收紧；续租等待不得跨越已知旧租约，
新截止锚定续租发起时刻。租约失效或回执过晚只取消原处理 future，不额外释放或重排已开始的副作用。
领取等待也受原单调租约截止约束；超时或迟到回执不启动远程读取，可能已提交的 claim 等待原租约恢复。
领取回执跨过 drain 时不开始邮箱处理，也不伪造失败；claim 保留到原租约到期后恢复。
各已配置世界的 Read Mail 观察、Sent/compose 维护与 Inbox 处理三条 future 并行运行，拥有独立的 fetch/处理预算；
一方失败不撤销其他分支已提交的结果。三条 future 归同一轮次持有，取消与排空不遗留 detached 任务；停止后不再领取新的邮箱 claim。
候选以 `000060` 的持久环形游标逐条轮转，20/200 边界不推进未处理后缀；discard 的一次性尝试围栏保留未知结果，
不自动重新删除草稿或发送邮件。普通读取共享观察，但不借用 compose 或资金写授权，精确规则见[维护契约](ops/mail/diagnostics.md#定期三页维护)。
批次 ID 使用借用哈希集合做期望线性去重，不复制邮件或 ID；论坛 512 字符摘要惰性生成，不先建立完整分词/拼接副本。
recovery 摘要只借用字段，旧 prepare 查询仅保留一个必要 owned 副本，JSON、摘要与命令身份不变。
轮询上下文直接按 claim 构造并转移给处理器，不再建立只用于克隆的临时上下文和重复复制邮箱/world 字符串。
论坛楼层索引借用整轮 preview 的帖子 ID，固定布局移动已拥有的目录续页，避免重复复制 ID 和 BBCode；
关闭正文校验且本轮已获得楼层回执时，不再复制正文调用必然成功的重复校验路径；开启校验仍逐字回读。
原顺序、逐页证明、命令身份与未知 POST 门禁不变。
轮询将当前 claim 传给 `fetch_claimed`；生产 MoogleMail gateway 在首个详情 GET 前持久登记整窗列表身份，优先恢复同邮箱、
同 world 的待读前缀。完成 cursor 与队列前缀在同一 fenced 事务提交，读取取消或业务失败保留未完成后缀，不能靠未读列表恢复。

Forum 目录每帖最多 150 件、从 #1 连续分页，系统日志在最后一个目录页之后、用户回帖再接下一楼。全部固定帖的
exact post ID、拍卖/页码标记与本地持久化身份齐备后才导入普通出价；历史已尝试/发布/未知主题按冻结单页布局继续
处理，升级不挪楼，抢楼、缺页或布局不匹配失败关闭。正常发布与刷新的原始正文校验由部署字段
`worker.forum.verify_post_body` 控制（默认关闭）；回帖刷新出现未知写入时，无论开关如何都只读核对全部固定帖，
首帖同时核对服务器保存的标题和描述，不匹配继续保留未知门禁，不自动重发。系统更新楼使用白名单英文摘要与
具体错误码（如 `already_leading`、`bid_too_low`、`auction_closed`），旧通用码 `command_rejected` 只对已知完整诊断作
只读映射，原始有界详情只留本地命令账本。
可重试身份/存储错误停止后续楼层，导入前按稳定 post ID 读取 pending 边界；pending 原帖删除、编辑或未收口仍阻断。
已完成命令按稳定帖子身份和原业务负载重放，不因前序普通帖子删除而重新编号历史；完整扫描的派生楼层可下降，
仍受固定布局、lease 和 claim/revision CAS 保护，不回退事实 sequence。主题同步未知 POST（含 `reconcile_write_unknown`）
短退避后只做当前及历史标题的精确只读对账；最多二十三个标题合并一次版面读取，候选指向不同主题时失败关闭。
无法证明原主题目标时等待 CLI 持久人工再授权；已发布主题的回帖刷新对未知写入只做固定帖逐字只读对账。
状态后缀直接来自同源快照，不按日期推断。Preview、Live、Final、Settling 及需收口的终态可刷新已有主题，
只有 Live/Final 继续导入竞价；Cancelled 只刷新既有主题，不创建新帖或导入新竞价，完整证明后停止轮询。
终态刷新保留升级时历史 settled 的豁免；部分进度、租约丢失及 write_unknown 不提前完成。

实时 runner 持久化连续 offsets/failures；校验后把事件所有权交给 sink，确认与失败收口只保留 event ID/序号，
不深复制事件 payload。当前广播边界是 Public 本地 Hub，未冻结外部 sink 不得伪装为已发送。
PM 的 prepare 只能 GET，持久 sending fence 明确提交才消费 prepared.send，任何 sending 不能安全延期；单项总预算覆盖
claim/prepare/fence/send/finish 且严格小于 lease，drain 停止新领取、主进程 drain 结束后 abort 剩余任务。
单项单调截止在领取前冻结，并在每个阶段回执后复核；迟到的 sending 成功回执不授权首次 poll Send，保留原 sending 对账。
PM 存储接口按整秒领取；总预算与实际截取后的整秒租约比较，不能使用小数秒余量延长外呼。
PM 保留既有毫秒离散抖动：`max` 为 jitter 的整毫秒数（上限 `u64::MAX`），取值 `[0, max)`，不足一毫秒时为零。
只复用 Worker 的 drain 等待门禁，等待前复核停止值，停机通知已到达时立即结束；构造参数边界沿用原约束。
同步、回帖及 PM 共用唯一 gateway 的实际 world 可用性，数据库在 LIMIT 前过滤不可用世界；
PM 的来源世界从通知关联拍卖读取，不改冻结消息或摘要，shared 凭据仍允许两世界。
各循环默认值、预算及有效并发见[配置手册](ops/config/README.md#worker-循环预算)。

## Worker control

只经 UDS 提供 live、ready、status、`/metrics`、只读邮件进度和全局邮件唤醒，不启动第二个指标监听器。socket 位于受信 runtime 目录，
持 `0600` 实例锁，只清理已证明失效且 inode 未变的旧 socket；新 socket 固定配置 group 与 `0660`，连接须通过显式
UID/GID allowlist，并受请求/响应大小与处理预算限制。Admin client 逐块限制 JSON 响应为 64 KiB，
禁用环境代理、自动重定向和协议层重试；缺少 Content-Length 的响应也不能先完整缓冲再验限。
唤醒使用有界内存 replay window，只表示已接受，
让已装配 poller 执行一轮，不能代替 `mail_poll_state` 的持久化游标、租约或业务完成证明。
进程启动、降级原因与 drain 使用同一原子状态；论坛健康恢复只清除自身原因，不能覆盖其他任务故障，
`status.ready` 和 `status.state` 来自同一快照。group 名称使用调用独占缓冲的可重入 NSS 查询，避免并发查询覆盖 GID；
缓冲从 1 KiB 倍增且不超过 1 MiB，查询失败或仍超限时拒绝绑定。
control 目录禁止 group/other 写入；祖先只允许 root/运行 UID 拥有且不可由 group/other 写入，root-owned sticky 祖先例外。
权限写入前后均核对 socket 身份；未进入 run 便丢弃 server 时也先清理 socket、再释放实例锁。
root/运行 UID 仍属于信任边界，路径复核不宣称消除受信账号恶意替换产生的全部 TOCTOU。
[control/mail_progress.rs](crates/auction-jobs/src/control/mail_progress.rs) 持有最多两个世界的进程内观测；
详情 GET、入站处理和批次提交前报告真实阶段/MID，`processing/progress.rs` 的 RAII 守卫在完成、取消或提前返回时收口。
内部按 claim token/版本隔离迟到回调，不向协议导出这些值；失败阶段/MID 保留到本进程后续观测，重启清空。
`GET /v1/mail/progress` 复用原 peer allowlist、协议头、64 KiB 与请求预算，旧 status 闭集不变；
它只观测入站 claim，不推断三页维护或外发状态，也不能代替持久化完成事实。
协议与部署参数见[Worker 入口](apps/auction-worker/README.md)和[邮件诊断契约](docs/contracts/routes.md#邮件审查与轮询)。

## 边界约束

所有业务状态迁移经应用命令，游标、账本与任务进度必须持久化，外部副作用在本地提交后执行；本机 control HTTP 仅用于
经 peer 认证的运维。进入 drain 后门禁不可逆：停止新领取并收口在途 claim，每个循环及并发槽等待后都复核门禁；
常驻循环意外退出或 panic 时先撤销就绪、通知装配根并停止领取，再收口其他在途组，返回原始首错。
正常停机由装配根和 `auction-process` 持有共享总截止及二次信号；致命故障的 10 秒兜底也覆盖无监督的直接调用。
论坛扫描区分未确认 POST 与纯读取超时：前者先只读对账，后者可正常重试；
主题创建返回 `Created`/`Existing`，精确标题命中旧帖不算本轮 POST 成功，关闭校验时既有目录或日志不能充当本轮正文
已发送的证明。

## 维护与验证

```bash
cargo check --locked -p auction-jobs
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。

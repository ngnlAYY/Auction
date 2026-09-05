# auction-jobs

Worker 任务编排：调度、重试、游标推进和生命周期任务。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；Worker 生命周期与调度实现放在 `src/worker/`，
并按 `auction/`、`forum/`、`mail/`、`realtime/`、`recovery/`、`settlement/` 和 `operations/`
分组。Worker control 传输层独立归属 `src/control/`，入口只负责装配；协议 DTO、HTTP handler、
Unix socket server 和 Admin client 分别位于 `src/control/transport/{protocol,handlers,server/{mod,listener,security},client}.rs`，
其中 server 再按生命周期、peer allowlist 和 Unix socket 文件安全划分 owner；调度器只通过
`WorkerControlHandle` 接入状态与唤醒端口。
`src/worker/runtime/` 的 `mod.rs` 只保留跨 owner 装配与稳定导出，`model.rs` 管理 Worker 端口/运行参数，
`runner.rs` 管理启动、任务装配、drain 顺序和任务退出错误；具体业务循环继续由相邻 owner 模块拥有。

`run_persistent_until_shutdown()` 已接入持久化 `scheduled_rounds` claim、租约、尝试次数、
指数退避失败记录和由装配根注入的 shutdown future；claim 冻结领取后的调度版本，完成与失败必须
同时匹配 owner、精确租约期限和 `state_version`，因此 Admin 修改计划后陈旧任务不能继续收口。它通过
生命周期应用端口执行 preview/open，不复制状态机。收到 drain 后，各独立循环完成当前有界批次并停止领取新任务；SIGINT/SIGTERM、
10 秒总上限和二次信号强制收口由 `apps/worker` 与 `auction-process` 拥有。
每周场次 bootstrap 只处理 `[auction.schedule].worlds` allowlist、runtime-enabled worlds 和数据库
`worlds.enabled=true` 的交集；每次达到 due 都通过注入的 `WorldReader` 重读数据库，缺失/损坏 world
失败关闭，已关闭 world 则安全跳过而不制造重复写入错误。该门禁只阻止新场次，不取消既有
settlement、fulfillment、delivery 或 recovery durable 工作。
`runtime/schedule.rs` 先通过 `find_current_round` 读取本地周的冻结轮次，仅在不存在且窗口已到时
调用严格的首次创建端口。经济模板或周时间变更不会以同一旧命令 ID 重发不同负载；既有轮次继续
使用持久化时间，并以这些时间对账邮箱暂停/恢复，新配置只用于后续缺失轮次的创建。
`run_persistent_with_settlement_until_shutdown()` 在装配根注入 slow due 读取/命令端口，并在
PostgreSQL 完整档额外领取结算 workflow。Auction 主循环立即执行首轮，再按一分钟加最多五秒
抖动唤醒，依次运行生命周期、慢速代理和结算三个独立阶段；任一阶段失败仍继续后续阶段，也不会
终止其它 Worker owner。结算命令的 `now` 使用持久化 claim 的 `due_at`，遵守冻结的 task 时钟语义，
不会因 Worker 实际领取延迟改变结算事实；workflow、claim 和 complete 身份继续绑定同一 due 世代。
完成阶段失败会映射到安全错误闭集，单项超时后还会在独立两秒窗口内尽力提交 attempt、固定退避和
retry 审计；若结算其实已经提交，settled fence 会阻止误写失败进度。慢速代理每轮只按数据库时钟扫描一次有界候选，使用稳定命令身份，
再由竞价事务核对 generation/due/authorization、截止时间和 owner 资格；stale candidate 成功跳过，
失格 owner 原子冻结。各阶段最多处理 100 条、按 8 并发波次和 30 秒单项预算执行；SQLite Lite
的有效并发由配置层固定为 1，且不启用结算 workflow。PostgreSQL 完整档还提供结算履约的 claim/fencing、
`pending → prepared → enqueued` 推进和 delivery outbox 的发送中/成功/失败边界；MoogleMail
`DeliveryTransport` 已由 Worker 装配，当前执行版本化静态通知、注册/重绑用户通知、return 和
take payload。动作前会核对 sender 和预期资产；return 以精确 `mid` 从收件箱消失证明结果，
take 以同一邮件的资产归零和 Credits/COD/附件回执证明结果；注册/重绑的用户 key 只在发送前由
独立密钥解密。send 的明确收件人不存在会保留退信 `mid` 和回收标记，并在退信资产取回前进入
`manual_reconcile`；本轮后续 claim 会暂时排除同一 `(world, recipient)`，其他玩家继续发送，
下一轮重新评估。邮资拒绝在已暂存资产时同样进入人工对账，Worker 不会自动重发。
Delivery Worker 在 `start_sending` 前调用 transport availability gate；目标 world 暂无
scoped/shared 凭据时，以完整 lease token/fencing 把 claim 释放回 `pending`，不会误写失败终态。
该循环使用独立的 poll/jitter/batch/concurrency 配置，每轮完成后等待 `poll + [0, jitter]`；有效
并发取配置值与 transport 上限的较小值，只在槽位空闲后领取下一条。MoogleMail transport 因账号级
compose 可变状态把上限固定为 1，避免多个 claim 先进入 `sending` 再等待凭据锁并消耗请求超时。
进入 `sending` 后按配置周期续租，并用完整请求超时限制远端调用；超时、
网络错误或凭据竞态按结果未知写入人工对账。终态写回受独立 finalize timeout 限制，续租失效时
只允许尝试同一 token/fencing 的结果未知写回；若 fence 已失效，则由 lease 加 sending grace
到期后的防线转入人工对账，禁止重新领取并盲目重发。
上述 delivery 状态推进均由 PostgreSQL owner 在状态事务内追加审计，Worker 不另建内存审计或
在事务提交后补写证据。履约应用端口已经识别的依赖、证据和外部结果会以已登记的
`error_code` 写入 `pending`、`failed` 或 `manual_reconcile`；尚未完成状态迁移的 claim/持久化
错误只结束当前项目并保留原 lease/fencing，由下一轮在租约恢复后重试，不由 Worker 猜测新的
阶段错误码。单轮错误不会终止整个 Worker；只有循环任务异常退出才使进程失败关闭。
实时消费 runner 已接入
Worker 循环，并通过 `realtime_consumer_offsets` / `realtime_failures` 提供连续游标、失败停靠
和显式 replay 边界；当前契约只向 Public 进程内 Hub 分发，装配根因此不注入不存在的外部 sink，
未来跨进程广播需要独立协议。结算退信与 MoogleMail
Sent/Balance 回执对账已接入；PostgreSQL 完整档的论坛主题发布、主题读取和回帖导入也已接入，
主题任务只为已经实际开放的 `live` 拍卖初始化一次，已发布记录不会因后续状态版本变化再次发帖；
发布前由装备链接解析端口按快照中的无 key 目标生成真实 HentaVerse URL，再交给 BBCode renderer。
主题固定 #1 为拍卖目录、#2 为系统编辑/错误记录，只有两个 exact post ID 均持久化后才读取 #3+
用户命令；竞价事务完成后重新读取权威 lot 快照并按 post ID 编辑 #1，把编辑、解析、身份与确定拒绝
以及仍待重试的命令稳定投影到 #2，并把完整主题读取已经连续处理到的真实楼层单独持久化；该水位
从 #2 起步，遇到待重试命令停在其前一楼，不能从固定帖内容版本或远端 post ID 推算。两个远端证明都通过后才推进本地内容版本。待重试命令可以更新
#1/#2 的本地正文证明，但不会释放当前 claim 或推进 `content_next_scan_at`；只有整场扫描成功后才
从完成时刻计算下一次 due。每个主题只在即将处理时领取独立内容 lease/fencing，无变化也会释放
claim 并推进公平扫描水位，避免多 Worker 交叉覆盖远端正文。成功扫描同时把持久化下一次 due
推进到完整论坛轮询周期，默认 300 秒；后者通过
`ForumReplyStore` 只为至少含一条可执行命令的帖子按论坛数字账号键定位映射，再按论坛/MM 共用的
可变玩家名称解析 Users 主体；首次同名复用必须由 active key 或跨世界相同数字键证明，数字键只进入
来源映射和命令账本 actor，MM 通知使用玩家名称。
随后再经 world 映射、lot 映射、稳定命令 ID 和命令账本调用现有竞价端口；同一
world 的一批主题只有全部成功才推进扫描游标，账本完成写入失败会保留重试边界，已应用竞价
对应的 MM 通知也会在后续轮次独立重放。接管历史回帖账本时，Worker 以当前算法生成候选 ID，
但后续竞价、通知和完成写入统一使用存储端返回的实际既有 ID，避免同一自然键反复冲突并阻断
整场扫描。主题远端写入结果未知或本地发布证明写入失败时，Worker
进入 reconciliation-only 并按精确标题 GET：命中只补本地 topic 证明，未命中则持久化
`write_authorization_required`，不会自动重试可能已经成功的远端 POST。只有 `auctionctl` 通过逐字
auction UUID 确认写入幂等授权账本和追加审计后，Worker 下一次领取才可再次 POST。回帖导入实现又按主题/映射、竞价命令、MoogleMail
通知和完成/错误辅助拆入 `worker/forum/reply/{topic,command,notification,common}.rs`，
`reply.rs` 只保留运行时与循环入口。Forum 使用默认 `300s + [0,5s]` 的独立顺序循环，先发布后读取
回帖；发布失败仍继续回帖导入，主题逐条领取而不提前占用整批租约，循环异常退出才使进程失败关闭。
论坛新回复只允许普通出价进入命令账本；代理创建/降低属于 Public Web 专属能力，解析入口静默
忽略对应行且不生成 #2 日志，历史已持久化论坛账本仍按既有幂等规则读取。Worker 的唯一论坛 Cookie
可为空并保持 degraded；凭据可用时，每个进程世代只恢复早于首轮水位的 `credential_unavailable`
任务一次，再次失败不会在同一进程形成领取风暴。
Operations alert 的 scan、dispatch 和 reconcile 现在按各自独立 cadence 运行，默认每 60 秒加最多
5 秒随机抖动，分别应用有界 deadline、结果/耗时/调度延迟指标和错误重试；任一 owner 的临时失败不会
阻塞另外两类任务，reconcile 上限与通知批次上限保持一致。收到取消后立即跳过抖动等待，不改变 claim
或 fencing 语义。
Fulfillment 与 Recovery 也已从共享全局 tick 拆为独立循环：两者立即执行首轮，再按一分钟加最多
五秒抖动调度，使用各自 100 条批次、8 并发、60 秒 lease/持久化重试和 30 秒单项总时限。
Fulfillment 按并发波次领取，避免未开始任务提前消耗 lease；Recovery 的
`worker/recovery/loop/{health,cases,returns,actions,resolutions,common}.rs` 保持
`case → return → action → relist → refund` 顺序，各 phase 失败互不跳过，且不会终止其它 owner。
MoogleMail 入站按已配置的 `main`/`isekai` world 分别运行独立 poller，每个 poller 使用独立
`mailbox_key`、游标、claim/fencing 和固定指标 label；同一 shared Cookie 只在 Worker 内复用，不合并
不同世界的持久化状态。轮询实现位于 `src/worker/mail/poll/`，由 `types.rs`、`cycle.rs`、
`processing/{mod,fetch,business}.rs`、`mapping.rs` 和 `failure.rs` 分别负责协议/运行时、调度领取、
租约与批次提交编排、远程读取 retry、单封业务 retry、事实映射和失败退避。
每个 poller 还拥有按标准五字段 cron 驱动的独立定时器和手动唤醒器；
默认 `*/5 * * * *`（每 5 分钟），并兼容描述符、`@every`、本地时区和显式 IANA
时区前缀。Worker 启动会协调持久化调度水位：只把旧配置遗留的过晚时刻向当前 cron 收敛，暂停、
活动 claim 和失败退避仍由数据库状态决定。远程读取按暂时
错误有界重试；单封业务错误由应用层冻结为 `Retryable`/`Permanent`，只有前者和超时按配置次数
重试，确定性输入、配置和业务拒绝立即收口。共享 transport 使用一秒最小请求间隔与 15 秒请求超时；
批次期间周期续租；续租 fence 失效后取消陈旧处理，稳定命令 ID 负责
恢复超时前可能已完成的本地步骤。
论坛 `start/cancel` 和 PM-only 通知在当前论坛命令契约中是 warning-only；MM `notify` 已进入
MoogleMail delivery outbox。若未来要扩展这些命令，需先冻结新的论坛/
拍卖 owner、幂等账本和远程回执契约。
recovery action 现在也使用 `pending → prepared → enqueued` 的两阶段本地 delivery 边界，
prepare/activation identity 可在 Worker 重启后幂等重放，并在 prepare/activation 前复核 Users
owner 的 active 收件人资格；`worker/runtime/recovery.rs` 又按 dispatch、activation、blacklist、
observation 和 retry 拆入 `worker/runtime/recovery/`，只保留 runtime 稳定出口；真实外部回执与
目标部署验证仍需独立运行证据。

Worker control 已提供 UDS-only 的 `live`、`ready`、`status`、`/metrics` 和 MoogleMail poll 唤醒协议；
`/metrics` 与 control 共享同一 peer credential 校验的 Unix socket 和 Worker 进程 registry，
不会启动第二个监听器。Worker recorder 使用固定的 9 类循环、3 类结果和 2 类重试 label，
并记录循环耗时、批次容量、重试和调度延迟。
socket 必须直接位于预先建立的安全 runtime 目录；服务端持有 0600 实例锁，只清理已证实失效且
inode 未变化的旧 socket，并把新 socket 固定为配置 group 和 0660。连接还必须命中显式 UID/GID
allowlist，同时受请求体、响应体和处理超时限制；Admin 只接收有限状态条件。唤醒命令使用有界的内存 replay window，不能
替代 `mail_poll_state` 的持久化游标、租约和失败状态；Admin 的 A20 只提交一个 command ID，
Worker control 让全部已装配 poller 跳过下一次调度时间并立即执行一轮，实际入站事实仍由各自
邮箱的持久化租约事务提交。

## 边界约束

**不得创建第二套拍卖状态机。** 所有状态迁移必须调用 `auction-application` 的命令端口，
Worker 只负责「何时触发」和「失败后如何重试」。

Worker 游标和命令账本必须持久化：内存定时器或内存游标不足以支持重放、重启和故障转移。

外部投递发生在本地事务提交之后；外部失败不能回滚已经接受的竞价事实。

Lite 档下 Worker 实例数和并发都必须保持为一，但游标仍然要持久化。
入口接收配置层已校验的并发数；SQLite Lite 档固定为 1，不能依赖运行时自行降级。

本 crate 不暴露业务写入 HTTP 表面；`src/control/` 的 HTTP 仅是本机 peer-authenticated 运维
传输，不能承载拍卖或管理业务写入，也不与 Public/Admin 的 `auction-http` 路由混用。

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
指数退避失败记录和由装配根注入的 shutdown future；它通过生命周期应用端口执行 preview/open，
不复制状态机。收到 drain 后，各独立循环完成当前有界批次并停止领取新任务；SIGINT/SIGTERM、
10 秒总上限和二次信号强制收口由 `apps/worker` 与 `auction-process` 拥有。
`run_persistent_with_settlement_until_shutdown()` 在装配根注入 slow due 读取/命令端口，并在
PostgreSQL 完整档额外领取结算 workflow。Auction 主循环立即执行首轮，再按一分钟加最多五秒
抖动唤醒，依次运行生命周期、慢速代理和结算三个独立阶段；任一阶段失败仍继续后续阶段，也不会
终止其它 Worker owner。结算命令的 `now` 使用持久化 claim 的 `due_at`，与旧版 task 时钟语义一致，
不会因 Worker 实际领取延迟改变结算事实；workflow、claim 和 complete 身份继续绑定同一 due 世代。
完成阶段失败会映射到旧版安全错误闭集，单项超时后还会在独立两秒窗口内尽力提交 attempt、固定退避和
retry 审计；若结算其实已经提交，settled fence 会阻止误写失败进度。慢速代理每轮只按数据库时钟扫描一次有界候选，使用旧版稳定命令身份，
再由竞价事务核对 generation/due/authorization、截止时间和 owner 资格；stale candidate 成功跳过，
失格 owner 原子冻结。各阶段最多处理 100 条、按 8 并发波次和 30 秒单项预算执行；SQLite Lite
的有效并发由配置层固定为 1，且不启用结算 workflow。PostgreSQL 完整档还提供结算履约的 claim/fencing、
`pending → prepared → enqueued` 推进和 delivery outbox 的发送中/成功/失败边界；MoogleMail
`DeliveryTransport` 已由 Worker 装配，当前执行版本化静态通知、注册/重绑用户通知、return 和
take payload。动作前会核对 sender 和预期资产；return 以精确 `mid` 从收件箱消失证明结果，
take 以同一邮件的资产归零和 Credits/COD/附件回执证明结果；注册/重绑的用户 key 只在发送前由
独立密钥解密。send 的明确收件人不存在会保留退信 `mid` 和回收标记，并在退信资产取回前进入
`manual_reconcile`；邮资拒绝在已暂存资产时同样进入人工对账，Worker 不会自动重发。
Delivery Worker 在 `start_sending` 前调用 transport availability gate；目标 world 暂无
scoped/shared 凭据时，以完整 lease token/fencing 把 claim 释放回 `pending`，不会误写失败终态。
该循环使用独立的 poll/jitter/batch/concurrency 配置，每轮完成后等待 `poll + [0, jitter]`，
只在并发槽空闲后领取下一条，避免等待执行的
claim 提前耗尽 lease。进入 `sending` 后按配置周期续租，并用完整请求超时限制远端调用；超时、
网络错误或凭据竞态按结果未知写入人工对账。终态写回受独立 finalize timeout 限制，续租失效时
只允许尝试同一 token/fencing 的结果未知写回；若 fence 已失效，则由 lease 加 sending grace
到期后的防线转入人工对账，禁止重新领取并盲目重发。
上述 delivery 状态推进均由 PostgreSQL owner 在状态事务内追加审计，Worker 不另建内存审计或
在事务提交后补写证据。履约应用端口已经识别的依赖、证据和外部结果会以旧版对应的
`error_code` 写入 `pending`、`failed` 或 `manual_reconcile`；尚未完成状态迁移的 claim/持久化
错误只结束当前项目并保留原 lease/fencing，由下一轮在租约恢复后重试，不由 Worker 猜测新的
阶段错误码。单轮错误不会终止整个 Worker；只有循环任务异常退出才使进程失败关闭。
实时消费 runner 已接入
Worker 循环，并通过 `realtime_consumer_offsets` / `realtime_failures` 提供连续游标、失败停靠
和显式 replay 边界；旧版只向当前 Public 进程内 Hub 分发，装配根因此不注入不存在的外部 sink，
未来跨进程广播需要独立协议。结算退信与 MoogleMail
Sent/Balance 回执对账已接入；PostgreSQL 完整档的论坛主题发布、主题读取和回帖导入也已接入，
主题任务只为已经实际开放的 `live` 拍卖初始化一次，已发布记录不会因后续状态版本变化再次发帖；
发布前由装备链接解析端口按快照中的无 key 目标生成真实 HentaVerse URL，再交给 BBCode renderer。
主题固定 #1 为拍卖目录、#2 为系统编辑/错误记录，只有两个 exact post ID 均持久化后才读取 #3+
用户命令；竞价事务完成后重新读取权威 lot 快照并按 post ID 编辑 #1，把编辑、解析、身份与确定拒绝
以及仍待重试的命令稳定投影到 #2，两个远端证明都通过后才推进本地内容版本。每个主题只在即将
处理时领取独立内容 lease/fencing，无变化也会释放 claim 并推进公平扫描水位，避免多 Worker 交叉
覆盖远端正文。后者通过 `ForumReplyStore` 按论坛/MM 共用 `game_player_id` 复用或建立 Users 主体，
再经 world 映射、lot 映射、稳定命令 ID 和命令账本调用现有竞价端口；同一
world 的一批主题只有全部成功才推进扫描游标，账本完成写入失败会保留重试边界，已应用竞价
对应的 MM 通知也会在后续轮次独立重放。接管旧版回帖账本时，Worker 以新算法生成候选 ID，
但后续竞价、通知和完成写入统一使用存储端返回的实际既有 ID，避免同一自然键反复冲突并阻断
整场扫描。主题远端写入结果未知或本地发布证明写入失败时，Worker
进入 reconciliation-only 并按精确标题 GET：命中只补本地 topic 证明，未命中则持久化
`write_authorization_required`，不会自动重试可能已经成功的远端 POST。只有 `auctionctl` 通过逐字
auction UUID 确认写入幂等授权账本和追加审计后，Worker 下一次领取才可再次 POST。回帖导入实现又按主题/映射、竞价命令、MoogleMail
通知和完成/错误辅助拆入 `worker/forum/reply/{topic,command,notification,common}.rs`，
`reply.rs` 只保留运行时与循环入口。Forum 使用独立顺序循环，先发布后读取回帖；发布失败
仍继续回帖导入，主题逐条领取而不提前占用整批租约，循环异常退出才使进程失败关闭。
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
`mailbox_key`、游标、claim/fencing 和固定指标 label；共享凭据池只在 Worker 内复用，不合并
不同世界的持久化状态。轮询实现位于 `src/worker/mail/poll/`，由 `types.rs`、`cycle.rs`、
`processing/{mod,fetch,business}.rs`、`mapping.rs` 和 `failure.rs` 分别负责协议/运行时、调度领取、
租约与批次提交编排、远程读取 retry、单封业务 retry、事实映射和失败退避。
每个 poller 还拥有按标准五字段 cron 驱动的独立定时器和手动唤醒器；
默认 `* * * * *`，并兼容旧版描述符、`@every`、本地时区和显式 IANA 时区前缀。远程读取按暂时
错误有界重试；单封业务错误由应用层冻结为 `Retryable`/`Permanent`，只有前者和超时按配置次数
重试，确定性输入、配置和业务拒绝立即收口。共享 transport 由配置层恢复旧版一秒最小请求间隔与 15 秒请求超时；
批次期间周期续租；续租 fence 失效后取消陈旧处理，稳定命令 ID 负责
恢复超时前可能已完成的本地步骤。
论坛 `start/cancel` 和 PM-only 通知在旧版 `forumingest` 中就是 warning-only，当前保持该兼容
边界；MM `notify` 已进入 MoogleMail delivery outbox。若未来要扩展这些命令，需先冻结新的论坛/
拍卖 owner、幂等账本和远程回执契约。
recovery action 现在也使用 `pending → prepared → enqueued` 的两阶段本地 delivery 边界，
prepare/activation identity 可在 Worker 重启后幂等重放，并在 prepare/activation 前复核 Users
owner 的 active 收件人资格；`worker/runtime/recovery.rs` 又按 dispatch、activation、blacklist、
observation 和 retry 拆入 `worker/runtime/recovery/`，只保留 runtime 稳定出口；更广泛的外部回执
对照和真实部署验证仍在后续阶段。

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

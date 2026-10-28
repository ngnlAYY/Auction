# auction-worker

[文档导航](docs/README.md) · [任务编排](crates/auction-jobs/README.md) · [外部适配器](crates/auction-integrations/README.md) · [配置](ops/config/README.md)

持久化生命周期、论坛、邮件、投递和恢复循环的装配根。`src/lib.rs` 导出 `run`、`run_from_path`，
独立 `main.rs` 与[合并服务](apps/auction-server/README.md)的 Worker 子进程复用装配。仅支持 Linux x86_64 原生目标，
装配入口在编译期拒绝其他 OS/架构；浏览器 WASM 不受此限，默认发布 ABI 见[开发指南](docs/guide/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](apps/auction-worker/src/bootstrap) | 配置、持久化任务、论坛、告警、履约与 control 装配 |
| [src/bootstrap/mail](apps/auction-worker/src/bootstrap/mail) | 邮件凭据、业务处理器、transport、轮询与安全映射 |

## 职责与实现边界

Worker 不监听业务写入 HTTP，不创建第二套拍卖状态机。应用规则由 application/domain 拥有，
租约、游标和事实由 storage 持久化，循环/重试由 jobs 编排，远端传输由 integrations 实现。
默认 feature 为空；`postgres` 启用持久存储/任务，`cache-redis` 编译可选缓存。

## 装配与生命周期

`bootstrap/mod.rs` 选择配置/缓存/运行模式，`persistent.rs` 装配持久化 Worker；`forum.rs`、`operations_alert.rs`、
`fulfillment.rs`、`control.rs` 各有独立职责，无数据库时 `skeleton.rs` 只报告进程状态、不伪造数据库就绪。`forum.rs`
与既有 mail/runtime gateway 共享唯一凭据集合、client 和 owner，同一 gateway 同时注入 `with_internal_fulfillment` 与
`with_cash_evidence_gateway`，PM sender 与只读资料 reader 仅 `forum.enabled` 时装配
（见[外部适配器](crates/auction-integrations/README.md)）。SIGINT/SIGTERM 将 control 标记为 draining、停止新 claim，
在途任务沿原租约和结果证明路径收口；10 秒 drain 后停止心跳，第二次信号强制收口，最后按配置限时刷新 tracing exporter。
信号与运行时故障共用 `drain_worker` 的预算和错误映射；故障路径继续保留原故障上下文和非零退出语义。
control 只有一个停机接收者，通知保留到接收者开始等待，覆盖启动后立即停机的窗口，不依赖任务先被调度。
邮件或论坛 owner 永久失效时，监督任务在一秒观察周期内通知装配根，沿用同一 10 秒排空上限并非零退出，由 systemd 等进程管理器
重建 Worker。监督只读取已取得 owner 的已知失效状态；关闭功能或无 active 凭据时不启动对应监督，不增加数据库探测。失效后的新外呼继续
失败关闭，在途发送保留原 sending/未知结果对账语义；新进程取得 advisory lock、提交新 generation 后才恢复邮件操作。
control 或独立 PM 任务提前退出、panic 或取消立即撤销 readiness，并沿用同一 drain 预算后非零退出；
退出守卫在 spawn 前建立，覆盖首次 poll 前取消。固定容量故障队列保留首个原因，正常停机收口不另开超时，
也不覆盖已选定的正常关闭结果；任务所有权仍由 abort-on-drop 守卫持有。
持久化循环 panic 或意外退出时，运行器立即撤销 readiness、停止各组及 PM 的新领取并保留首个错误；
装配根仍以同一 10 秒预算和二次信号收口在途任务，结束后非零退出。直接使用运行器时，致命故障排空也有 10 秒上限。

持久化装配结束后先检查已到达的停止请求，再决定是否启动周期心跳与 PM；停止 future 只消费一次，主循环保持立即 drain。
竞价与生命周期适配器直接使用事务内数据库时间，不再装配未使用的时钟；邮件、履约与调度仍保留各自实际使用的时钟。
周排期和恢复健康只借用已配置 world/mailbox 的窄投影，不复制邮件发件人名单；世界顺序和邮箱标识保持不变。

`persistent.rs` 在装配时创建同一 `MailPollProgressTracker`，经 `mail/runtime.rs` 的 `MailPollRuntimePorts`
同时注入 gateway、轮询任务与 control。它只保存有界进程观测，不能授权或恢复业务；协议和旧端降级见
[邮件诊断契约](docs/contracts/routes.md#邮件审查与轮询)，本次诊断不新增配置或迁移。
邮件留证的完成日志和取消日志共用守卫持有的一份安全 context/outcome，避免为两种互斥结果重复复制字符串；
留证 future 首次 poll 前被丢弃时仍报告未确认完成，日志字段、回执与归档载荷沿用既有契约。

## 部署配置与 runtime

Worker 没有业务 `listen_address`。Admin/Worker 可共用 `runtime_config_file`；Worker 只消费 world、发件人、
装备过滤、14 个邮件模板、bot 内部履约名单及接管后的 `operations`，不继承 Admin listener/security/session；
transport、凭据、mailbox identity 和全局启用仍由 Worker 部署配置拥有。保存后须重启对应进程，
不改既有拍卖快照、due、游标或租约
（分组责任见[配置说明](ops/config/README.md#runtime-共享与重启)）。

`bot_players` 经独立窄投影注入履约服务；收件人命中名单但当前远端身份不匹配时保留内部证据等待，不进入外发。
只有真实身份及相应资产/资金证明齐备后才能内部满足义务。
旧 prepared 退款也必须重新取得当前认证账号、对应世界的 Armory 正向库存证明；装备详情可读不构成 custody。
准备/激活沿原 opaque guard 和单调截止完成数据库提交，缺证明则延期或人工核对，不因 legacy 路径绕过库存。
资金策略由同一数据库窄端口注入履约、投递及恢复健康；`record_only` 跳过自动余额采集与比较，仍保留真实身份、库存、
原收款与发送证据。现金健康明确为不适用，恢复积压及真实投递失败继续刷新；默认 `strict_cash` 保持现金保护。

## 拍卖邮件起拍价

未写 `start` 的装备默认 50,000 Credits，可由 `[worker.mooglemail].start_price` 覆盖，独立于公共经济模板。
一条 `start amount` 应用于全部装备，逐件形式为同一行的 `<equipment-url> start amount`，旧跨行三元组仍兼容；
普通说明、空行、Attachment 块、URL 包裹/句末标点和空白后的 `#` 注释不改变默认价。金额支持正整数、
规范千分位及精确整数 Credits 的 `k`/`m` 小数（如 `1.5k`）；缺金额、误写 `start=10k`/`start:10k`/`start10k`、
零负值、溢出、重复定价、未知 ID 或 key 不匹配均拒绝，`starting`/`starter` 等普通说明不是定价指令，不能退回默认价。

## 撤拍装备寄回

PostgreSQL 撤拍事务创建 `withdrawn_return` 与履约记录，Worker 在原 world 把原装备单独 Send 给冻结卖家，
Credits/COD 为零；同封其他 lot 不受影响，已 Take 的邮件不能整体 Return。旧未知发送结果不自动重寄，
旧冻结 Return 不能直接改为 Send（核对见[历史资产处置](ops/mail/diagnostics.md#历史资产处置核对)）。

## 部署约束

生命周期、慢速代理、结算及 fulfillment/recovery 等 owner 各有预算，不因单个积压阻塞其他循环，默认值与租约见
[Worker 循环预算](ops/config/README.md#worker-循环预算)。自动周排期默认关闭，新轮次取配置 allowlist、runtime 启用集合与
数据库 `worlds.enabled` 的交集；数据库 world 关闭阻止新轮次，但不阻止已持久化工作安全排空。经济模板仅在建立后继时
冻结，旧场次与已接受命令不重算。

## 邮件调度

每个 world 有独立 cron、抖动和手动唤醒器，读取、处理、重试及续租均受预算约束；重启只把过晚的 due 向当前
调度收敛，不恢复暂停邮箱、越过活动 claim 或压缩失败退避。批次只提交完成前缀，确定性错误立即收口，只有
Retryable/超时进入有界重试（参数见[论坛与邮件配置](ops/config/README.md#论坛与邮件)，编排见 [auction-jobs](crates/auction-jobs/README.md)）。

详情 GET 可能把无附件邮件标为已读。轮询先在当前 claim 下持久登记整窗 MID 和列表摘要，再读取详情；重启或取消后优先
按持久队列直接读取 MID，入站事实、命令账本与 cursor 提交时原子删除完成前缀。队列沿用 batch limit，已有 cursor 保留；
allowlist 改动使待读发件人不再获准时失败关闭并保留队列，须核对配置后恢复，不静默丢弃待读项。新队列不补造升级前已丢失的 MID。
每个已配置世界的 Read Mail 观察、Sent/compose 维护与 Inbox 处理三路并行，各自使用有界预算并共用同一 client 的读取协调器。
`000060` 持久化逐条核对的环形位置，长期无匹配项不再饿死后缀，游标不是业务完成证明；Read Mail 只定位已知恢复目标，不重放历史入站。

## 持久化与外部结果证明

任务 due、租约、尝试次数、完成 fence、命令账本和游标均持久化，内存计时器只负责唤醒；claim token/owner/版本或续租
fence 失效后取消陈旧处理，只能从权威 cursor 重放。外部写入只在本地提交后执行，未知结果进入对账，不能抹掉已接受事实
或自动重发（见[外部写入不变量](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)）。
MoogleMail delivery 在 sending fence 前做只读 compose preflight，prepared future 持有同一凭据独占 guard，
不安全时释放并在本轮跳过该 world 的 Send（Take/Return 独立）；`prepare_next` 的内部完成结果直接结束 claim，
不伪造 delivery/Sent，Send 与 Return/Take 各有独立证明。证明与资产核对见[邮件诊断手册](ops/mail/diagnostics.md)，
业务原子边界见[履约端口](crates/auction-application/README.md#结算与恢复端口)，启动前须
[停机同版升级与显式资金核对](ops/README.md#bot-内部履约与资金保护升级)。
`000061` 在 Sending 同事务冻结实际发送内容摘要、资产矩阵和尝试身份；后续核对不使用当前模板或当前登录 key 猜测原正文。
所有发送来源使用同一精确证明与唯一远端 MID 绑定。升级前停止旧 Worker，按[共享读取升级](ops/README.md#邮箱共享读取与发送证明升级)执行。
Take 在 Sending 前持有原账号会话并冻结 world/MID/凭据身份；交接失败或取消不重新选择 Cookie。
其期待通过普通 fence 持久化，不进入 Send 的现金授权入口；未知结果的恢复须同时核验原绑定和本次观察身份。

凭据由 PostgreSQL 独立数据密钥解密，论坛和邮件每种 Cookie 全局最多一条 active，生命周期 advisory lock 保证单 Worker
owner；失效后不切换 Cookie，导入或轮换后须重启交接。未配置凭据数据密钥时保留受限 `.keyenv`/敏感文件兼容入口
（`.keyenv` 仅 `hv-ck-key1` 供 Worker 使用，`hv-ck-key2` 只供显式隔离验证）；告警经同一幂等 outbox 发送。

## Recovery 推进

PostgreSQL recovery due 推进独立于邮件 transport；付款、超时、提醒、退件 Take、黑名单、guarded relist 与退款完成
由各 owner 的稳定命令、资产摘要和证明推进，只有启用邮件 transport 后才关联 delivery outbox，未知结果保持人工复核。

## 论坛同步

仅 PostgreSQL 且启用论坛时装配主题发布和回帖导入，每轮按“发布 → 导入”执行，一阶段失败不跳过另一阶段。
只为实际 `live` 拍卖创建主题；未知 POST 先精确标题 GET 对账，未命中须通过
[`authorize-forum-write-retry`](apps/auctionctl/README.md#论坛与结算核验)显式授权；全部目录页和系统日志的 exact post ID
与内容 lease 持久化后，才处理固定区之后的用户回帖。部署字段 `worker.forum.verify_post_body` 默认关闭，关闭时不保证
远端正文与目标镜像一致（见[配置说明](ops/config/README.md#论坛与邮件)）。论坛数字账号 ID 是稳定身份，用户名只用于显示和
MM 收件；新作者可建立 revoked-key 的 `forum_only` 主体，不自动获 active Key。只导入普通 `bid`，
`proxy`/`proxy lower` 静默忽略，`start/cancel` 与 PM-only `notify` 仅作 warning。
实际数据库 credential scope 贯通主题和 PM 操作，调度在 LIMIT 前过滤不可用世界。纯预读超时可以普通重试，
真实或历史未知 POST 仍保留对账门禁；命令依据稳定 post ID 和原负载，不因前序删帖改变历史身份。

## 心跳与论坛启用状态

数据库 Worker 发布启动、就绪、周期及停止心跳，含 runtime 投影后的 `details.forum_sync_enabled`；Public 据此显示
实际启用状态，不从历史帖子推断，开关变更须重启 Worker。周期心跳每 10 秒唤醒，慢数据库导致漏 tick 时跳过补发，
数据库恢复后不集中补写漏掉的周期。`[worker.control]` 提供受信目录、实例锁、固定 group/0660 和 UID/GID peer allowlist
保护的 live/ready/status，部署见[systemd 手册](ops/README.md#systemd-服务与顺序控制)。

## 维护与验证

`cargo check --locked -p auction-worker` 只做默认 feature 静态编译；完整门禁见[开发指南](docs/guide/development.md)，
历史运行结果与剩余缺口见[验证状态](docs/status.md)，本 README 不授予测试或远端写入授权。

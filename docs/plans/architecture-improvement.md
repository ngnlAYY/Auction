# 架构改进计划

[文档导航](../README.md) · [架构](../architecture.md) · [开发指南](../development.md) · [运维手册](../../ops/README.md)

版本 v12（2026-11-02）。本计划只处理代码与工程体系，不包含线上数据处置；线上问题在各阶段完成并经用户批准部署后自然受益。每完成一个工作项，在[进度](#8-进度)中更新状态。正文中的仓库路径均以仓库根为起点；代码证据以“文件 + 符号名”引用，不写行号。

| 版本 | 变更 |
| --- | --- |
| v1 | W0–W8 初版 |
| v2 | 依最新源码复审：修正请求间隔基线；新增已知缺陷 D1/D2；W0 覆盖论坛与连接池；W1 增加 Read Mail 快速限界、论坛对齐与通知预算；W3 增加时区无关的行快照证明；W7 改用 CI 候选制品；新增 W9 安全加固、W10 数据生命周期与备份、W11 资产与资金对账 |
| v3 | 记录用户决策：提醒保持现状；批准领域测试与 PG18 集成测试并提前到 M2；告警只建通道入口；Admin 不限制暴露、不加第二因素；异机备份与数据清理按建议；main 作为发布基线；现线上即正式环境，不建预发 |
| v4 | 全仓审计后修正：更正连接池指标、请求间隔门、`maximum_pages` 默认值、通知出队顺序与错误丢弃范围等基线；新增 D3（Sent 页数悬崖）与 D4（出队优先级待定）；W0 扩展到存储/应用层错误、HTTP 指标、指标采集路径与待核实事实；W1 改为“三层争用 + 跨进程”调度并增加主动暂停与访问收口；W5 补告警来源与外部存活检查；W6 扩大测试范围与入口；W7 增加时段门禁、回滚边界与制品来源；W8 拆分部署形态决策与代码结构评估；W9/W10 补密钥、备份范围与保留政策前置；新增 W11.4 内部一致性巡检与 W12 竞价热路径；修正依赖倒置并重排里程碑 |
| v5 | 二次全仓审计补遗（用户批准，决策 11）：新增 D5（同场出价序号热点）、D6（论坛回帖时序与页数悬崖）；定时与异机备份（W10.1/W10.2）提前到 M1，一致性巡检（W11.4）提前到 M2 作为改造安全网；W0 增加 HTTP 层错误、`lock_timeout`、连接占用与用户可见 SLO；W1 增加告警最高优先级、令牌桶无突发、论坛增量导入（W1.11）；W5 增加处置手册；W6 增加属性测试、解析器健壮性、配置示例加载、DTO 快照与 SQL 覆盖目标；W7 增加版本偏差、运行时开关、迁移兼容分级与故障域隔离；W9 增加 Admin 登录防护、主机层加固与操作者身份；W10 增加迁移演练并修正异机副本轮换方式；W12 扩为数据库热点与读路径；新增 W13 回归门禁；补齐 W8–W13 验收与回退；待决定事项标注阻塞与最晚决定时间；进度表增加部署列 |
| v6 | 记录用户决策 12–22：本机 Prometheus；`age` 公钥加密备份；D4 告警优先、提醒按截止排序且截止后退役；D6 维持导入时刻；Webhook 告警通道与心跳打卡存活检查；单人运营身份；启用 WAL 归档（新增 W10.7）；W8 移到最后；批准 W6.4 与 W12.3；线上为分体部署。W1.7 与 W6.4、W10.7 进入 M2，W12.3 进入 M4，W8 整体移到 M6 |
| v7 | 记录用户决策 23–28：心跳、实时事件与邮件诊断的保留期；不提供个人数据删除；批准 W7.8 与 W6.3；登录 key 与会话默认永久保持不变；新增待决定的 W9.8（永久会话占满并发上限） |
| v8 | 记录用户决策 29–31：会话记录保留期；W9.8 满额时注销最久未活动的会话且并发上限扩大到 10，移到 M2；RPO/RTO 与演练频率。待决定事项清空 |
| v9 | 三次全仓审计与用户决策 32–46：新增 D7（一次审计写入失败即永久未就绪）与 D8（停机排空短于在途外部写入）；新增 W0.14–W0.16、W6.6、W7.10、W7.11、W9.9、W9.10、W10.8、W13.5，拆出 W7.1a、W7.8a 提前到 M1；基线补充就绪探针、排空预算、Admin 的 `Requires=`、连接分档、数据库权限、登录 key、R2 权限、PostgreSQL 基线、死代码与运行配置生效方式；W1.11 游标改用帖子 ID；W12.1 增补候选方案；W13.1 扩大棘轮范围；重排 M1 内部顺序与 M2–M6；登记用户前置动作；标注已被取代的决策 |
| v10 | 记录用户决策 47–57：W7.5 保护范围收窄到 Final 与临近截止；数据密钥托管与 R2 端到端恢复演练提前到 M1；时间点恢复窗口 14 天（WAL 与基础备份保留 21 天）及各前缀锁定期；外部心跳与告警服务暂不启用（W5.1、W5.6 暂缓，打卡地址留空）；M1–M5 部署改用 `main` 的 CI 制品；旧分支处理；`rekey` 与 `register` 的 24 小时回信限制；M2 拆为 M2a/M2b；W1.5 退避上限 10 分钟；SLO 目标值 M1 结束时审批；换季发布冻结 |
| v11 | 第四次全仓审计，按用户要求专项复核全部通知与提醒：新增 D9（付款期限与逾期提醒不感知付款邮件是否寄出）、D10（论坛 `notify` 无内容且无上限）、D11（PM 渠道未绑定论坛账号时提醒被静默抑制）；新增 W14 通知与提醒，以及 W4.5、W4.6、W5.8、W9.11、W10.9、W12.8、W13.6；基线补充通知内容、提醒覆盖、Web 与运维提示、邮资与告警噪声；修正 W5 需要迁移、W7.8a 对新增闭集取值的兼容判定、W11.2 口径与论坛游标术语；登记新的待决定事项 |
| v12 | 记录用户决策 58–66：线上未启用运维告警；通知只用英文；网站普通出价自动标记，论坛 `notify` 改为订阅被超提醒；账号类回信排在被超提醒之前；D9 只延后提醒；说明信每 24 小时 10 封；新增提醒范围；不新增 Public 显示（取消 W14.7）；告警分级与每日摘要。复核慢速代理：补充执行滞后指标、告警与巡检，提醒时机登记为待决定 |

## 1. 目标与原则

目标：在不削弱现有资金安全与审计保证的前提下，降低外部依赖导致的故障面、减少“例外谓词”式维护、让人工处置以证据为中心，并建立可重复的发布、备份与安全基线。

原则：

1. [协作规范](../../AGENTS.md)的领域不变量全部保持：命令幂等、Serializable 关键事务、追加式事实、结果未知不重发、缓存不决定正确性、冻结策略不改写。
2. 每个工作项一个主题提交，按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”落地，同步契约与运维文档。
3. 默认门禁仍为 `just verify`；行为测试、数据删除与线上部署都需要用户逐项批准（见[决策记录](#7-决策记录)）。
4. 先可观测、再改行为：每个改变运行行为的阶段，都先有能证明改进效果的指标，并且指标有地方存储和查看（W0.10）。
5. 无预发环境（决策 9），因此改变运行行为的工作项必须带运行时开关与旧路径回退（W7.7 落地后开关切换无需重启，不受 W7.5 时段门禁阻挡）；涉及判定或计数口径的改动先影子运行（规范见[交付规范](#6-每个工作项的交付规范)），差异归零后再切换。
6. 每个工作项写明可度量的验收标准与回退方式。W7.8a 随一版部署之后，只含 `compatible` 迁移的版本可以换回旧程序；含 `breaking` 迁移的版本只能按[备份与恢复](../../ops/deploy/RECOVERY.md)回退（见 W7.2、W7.8a）。
7. 每项结构或错误处理改进都配套防回退门禁（W13），避免改进成果被后续提交抵消。

## 2. 现状基线

| 项 | 基线 | 证据 |
| --- | --- | --- |
| 规模 | 约 21.5 万行 Rust；`auction-storage` 5.4 万行、`auction-application` 3.5 万行、`auction-domain` 0.5 万行 | 源码统计 |
| 迁移 | 74 个，三周内累积 | `migrations/postgres` |
| 测试 | 17 个测试函数，集中在论坛解析与少量领域状态；竞价/代理、排期日历、邮件解析器无测试；CI 有静态、审计、MSRV 与发布构建作业，无测试作业，也没有 `just test` 入口；856 条 SQL 均为运行时字符串，没有编译期校验，迁移改列只会在运行时暴露；配置示例门禁只检查注释，不检查示例能被 loader 加载；没有 MoogleMail/论坛的本地服务替身，账号忙、Cookie 失效、页面结构变化等场景只能在线上观察；应用层有 157 个端口 trait、数百个方法需要存储覆盖 | `.github/workflows/ci.yml`、`Justfile`、`ci/checks/check.sh`（`check_config_examples`）、`crates/auction-application/src/ports` |
| 外部账号：请求来源 | 同一游戏账号 Cookie 至少被四个独立限速器使用：Worker MoogleMail 客户端（代码默认 750 ms，线上 1000 ms，main/isekai 共用一个门）、Worker 装备来源读取（自带 250 ms 门）、Admin 进程手工录入的装备来源读取、CLI 的 `mail inspect-sent` 与 `funds` 证据读取 | `crates/auction-integrations/src/clients/mooglemail/client/security.rs`（`IntervalGate`）、`crates/auction-integrations/src/equipment/reader.rs`（`EquipmentSourceAdapter`）、`apps/auction-admin-api/src/bootstrap/state/database.rs`（`configure_state`）、`apps/auction-cli/src/operations/mail.rs`（`inspect_sent`）、`apps/auction-cli/src/operations/funds/evidence.rs`（`build`） |
| 外部账号：进程内争用 | 三层串行点：发送/资产/内部证据的独占操作锁覆盖“Sent 基线扫描 → compose → 发送 → 发送后核对”全程；Read Mail/Inbox 列表扫描持 `incoming` 读锁，期间阻塞改变成员关系的 Inbox 详情读取；最后是 FIFO 请求门。每轮邮件轮询以 `tokio::join!` 并行执行观察、维护与收件，下一轮须等最慢分支结束；两个世界共用同一 cron，同时触发 | `crates/auction-integrations/src/clients/mooglemail/gateway/credentials.rs`（`begin_exclusive`）、`crates/auction-integrations/src/clients/mooglemail/client/reads/lists.rs`（`observe_incoming_list`）、`crates/auction-jobs/src/worker/mail/poll/cycle.rs`（`run_cycle`）、`apps/auction-worker/src/bootstrap/mail/mod.rs`（`configure_mail_poll`） |
| 外部账号：出队与节奏 | 投递出队故意让 `lot_watch`（出价被超提醒）排第一，其余类别（含运维告警）一律按 `created_at` 先进先出，“投递积压”告警本身会被积压拖住；提醒的发送资格不检查 lot 是否已截止，截止后的提醒照发；迁移 059 为出队建索引；已有按七类持久化的发送节奏 `[worker.mooglemail.send_intervals]`；`[worker.delivery] concurrency` 默认 8，但 MoogleMail transport 实际上限为 1 | `crates/auction-storage/src/database/delivery_scheduler/claims.rs`（`claim_postgres`）、`crates/auction-storage/src/database/lot_watch/proof.rs`（`delivery_watch_gate`）、`migrations/postgres/000059_delivery_send_cadence.up.sql`、`ops/config/examples/worker.toml` |
| 全量扫描 | 维护每 5 分钟完整扫描 Sent（约 37 页），Read Mail 观察完整扫描已读（约 34 页），仅 main 两类扫描每 5 分钟约 71 次请求，约占 1 req/s 门的 24%；Read Mail 扫描即使目标已知也不提前停止。`maximum_pages` 代码默认 20、硬上限 100，线上值待核实 | 线上日志：部署 `68c1b8df` 后维护每轮 120 秒超时；`crates/auction-config/src/config/settings/worker/mooglemail.rs`（`MoogleMailWorkerConfig::default`） |
| Sent 基线 | 每次发送前要求把 Sent 完整扫到末页才能建立排除集（有缓存复用，但过期或换凭据即重扫）；客户端没有删除能力。若 HV 不自动清理 Sent，列表超过 `maximum_pages` 后所有发送失败关闭 | `crates/auction-integrations/src/clients/mooglemail/sender/proof.rs`（`prepare_sent_baseline`） |
| 论坛回帖 | 回帖没有发帖时间，出价按导入时的数据库时间判定，导入轮询默认 300 秒：截止前几分钟的回帖可能被判为截止后，软截止也从导入时刻起算；玩家文档未说明。每轮重读主题全部分页；主题超过 `maximum_topic_pages`（默认 10 页、硬上限 20 页，每页 20 楼）后整场导入失败关闭。迁移 005 建立的 `auction_forum_reply_cursors`（按世界记录 `last_scan_at`）没有任何代码读写，[数据库契约](../contracts/database.md)仍把它列为当前运行状态 | `crates/auction-integrations/src/forum/parser/model.rs`（`ParsedPost`）、`crates/auction-integrations/src/clients/ehentai/client/pagination.rs`（`topic_page_targets_through`）、`ops/config/examples/worker.toml` 的 `[worker.forum]`、`migrations/postgres/000005_recovery_observability_forum.up.sql` |
| 账号忙 | 战斗页按请求逐个失败，每次保存诊断；诊断目录硬上限 1024 文件；人也会用该账号战斗或手工发信，没有主动暂停手段 | 2026-11-02 约 2000 次/小时 `account_busy`，旧诊断被全部挤出 |
| 通知量 | 出价被超提醒按 lot、用户与轮次各发一封 MoogleMail，无按收件人合并或限额；失去资格的提醒成批预检退役 | `crates/auction-application/src/services/public/watch.rs`；线上 110 条预检退役 |
| 通知内容 | 模板闭集 11 种（`MailNotificationTemplate`），另有 5 种结算正文；出价被超提醒（MM 与论坛 PM）和论坛 `notify` 回执的英文正文写死在代码中，不能经 `[mail_templates]` 定制；被超提醒只给 lot UUID 与价格，没有 lot 代码、装备名、有效截止或网站链接；论坛回执不含任何 lot、金额或结果；COD 与载体付款邮件不写付款期限（赛季前场次只有 24 小时）；卖方货款、载体与退款邮件只有金额与 UUID；流拍、撤拍与买家未付款后的退回共用同一“装备附上”正文；注册与换绑结果只给结果码；投稿成功没有回执（[邮件流程](../guide/mail-flows.md)称 `intake_seconds` 含“投稿接收回执”，与实现不符）；正文全为英文、标题固定 | `crates/auction-application/src/services/public/watch.rs`（`lot_watch_notification`）、`crates/auction-application/src/services/public/watch_notification.rs`、`crates/auction-integrations/src/clients/mooglemail/transport/outcomes.rs`（`render_notification`）、`crates/auction-integrations/src/clients/mooglemail/transport/payload/settlement.rs`（`render_settlement`）、`crates/auction-application/src/ports/admin/runtime_config/mail_templates.rs`（`placeholder_rule`） |
| 提醒覆盖与时效 | 只有新建代理会自动标记，普通出价（含论坛）不标记，未标记的领先者被超时没有任何提醒；慢速代理持有人被普通出价超过后立即收到提醒，但其代理仍会在约 10 分钟后自动应价，等待期间持有人既不能提高上限，也不能普通出价，这封提醒无法据以行动；论坛专用账号无法标记，`notify` 只在导入时寄一封无内容回执，`notify pm` 不可用；选择 PM 渠道而未绑定论坛账号时，提醒在出价事务内即记为 `suppressed`，偏好接口与页面都不提示，论坛同步关闭时 PM 仍可选；付款期限从结算起算，逾期提醒在期限到达时创建，不检查付款邮件是否已寄出；恢复策略冻结“到期提醒一次”，没有到期前提醒；卖方成交与流拍、被拉黑、重新上架与换季前领取都没有通知（买方的成交通知就是 COD 或载体邮件） | `crates/auction-storage/src/database/lot_watch/facts.rs`（`auto_watch_proxy`、`notify`）、`crates/auction-storage/src/database/lot_watch/pm_fact.rs`、`crates/auction-jobs/src/worker/forum/reply/notification.rs`、`crates/auction-storage/src/database/recovery_scheduler/scheduler/advance.rs`、`crates/auction-domain/src/settlement/policy.rs`（`RecoveryPolicy`） |
| Web 与运维提示 | Public 的“我的”只有出价、代理、标记与两项偏好，没有本人成交、应付金额、付款期限、付款与交付状态、寄售与货款，也没有站内通知或公告；出价被超只能从领先高亮消失察觉；未登录时实时为手动模式，外壳每 60 秒重读；Admin 仪表盘只有告警计数，没有活动告警列表、确认、静默或恢复通知，导航没有待办数，自动轮询默认关闭；投递队列只能按状态过滤，玩家报出 Reference 后无法直接检索 | `crates/auction-web-public/src/app/identity/account.rs`、`crates/auction-web-public/src/app/surface/board/realtime.rs`、`crates/auction-web-admin/src/app/operations/dashboard/content.rs`、`crates/auction-web-admin/src/app/poll.rs`、[路由契约](../contracts/routes.md) |
| 邮资与告警噪声 | 外发邮件需要邮资（金额与计费方式待 W0.12 核实），远端以 `cannot afford the postage` 拒绝时归入通用 `send_rejected` 并直接失败；仅记账模式不读取余额，邮资耗尽没有预警；投递 `failed` 不产生告警（只有履约与恢复失败会）；默认阈值把正常等待计为积压：履约积压包含等待买家付款的 COD（`enqueued`）与等待前置的卖方货款（`blocked`），`fulfillment_backlog_count_threshold`、`delivery_pending_count_threshold` 默认都是 1，付款逾期也按 30 分钟冷却反复通知；告警种类、来源与范围是迁移 005、020 的 CHECK 闭集；运维告警由 Bot 账号寄给 `recipient`，HV 拒绝给发件账号自身寄信（`recipient_is_sender`）；线上未启用运维告警（决策 58），目前没有任何主动告警通道；论坛 `notify` 每行每个 lot 代码寄一封，没有去重或上限 | `crates/auction-integrations/src/clients/mooglemail/writer/response.rs`、`crates/auction-application/src/ports/operations/alert/evaluate.rs`、`crates/auction-storage/src/database/operations_alert/snapshot/reads.rs`、`ops/config/examples/worker.toml` 的 `[worker.operations_alert]`、`migrations/postgres/000020_operations_alert_returned_mail.up.sql` |
| 错误可观测 | 丢弃错误详情：jobs 层 46 处、应用层 25 处、集成层 22 处 `Err(_error)`/`Err(_) =>`；另有 `map_err(\|_\| …)` 应用层 99 处、jobs 层 22 处、集成层 359 处（多为解析闭集，需逐类甄别）；HTTP 层 113 处 `Err(_) =>` 与 121 处 `map_err(\|_\| …)`，5xx 的 `internal_error()` 不记录原因；存储层 1430 处 `map_err(\|_\| …)` 把 SQLSTATE 与约束名压成固定中文串（几乎全部经同一个 `storage_error` helper，共 1432 处调用，写法一致）；`ApplicationError` 没有安全分类；D1 所在的实收资金化把三种不同原因记成同一个 defer reason | `crates/auction-jobs/src/worker`、`crates/auction-http/src/http/routes/session/errors.rs`（`internal_error`）、`crates/auction-application/src/services/settlement/fulfillment/collected.rs`（`reconcile_collected`） |
| 指标 | 已有业务事件、Worker 循环、重试、调度延迟、连接池（占用/空闲/等待/取消）与 tracing 指标；缺 HTTP 按路由的请求量/状态/延迟、WebSocket 连接数、限流拒绝、序列化冲突与外部账号指标；没有面向用户的 SLO（出价延迟、冲突中止率、实时推送延迟、登录 key 邮件送达延迟、提醒送达时距截止的余量）与容量目标。Worker `/metrics` 只在 Unix socket 上提供，Public/Admin 若以 Unix socket 监听也同样无法被 Prometheus 直接抓取；仓库未体现线上指标存储；没有主机（磁盘、内存）与 PostgreSQL（归档、autovacuum、长事务、锁等待）指标；浏览器端 WASM panic 与解码失败只在用户控制台可见，运营方无从得知 | `crates/auction-observability/src/metrics`、`apps/auction-worker/README.md`、`crates/auction-web-shared/src/bootstrap/mod.rs`（`install_panic_hook`） |
| 数据库连接 | 每进程固定 16 个连接，不可配置；Worker 的凭据运行时所有权最多长期占用其中 2 个，Public 的 realtime consumer 所有权占用 1 个；Worker 十余个循环与 Public 的读、写共用同一池，没有按用途的配额；服务连接没有 `statement_timeout`、`lock_timeout`、`idle_in_transaction_session_timeout` 与 `application_name`；获取连接沿用 sqlx 默认 30 秒，长于 HTTP `request_timeout` 的 15 秒；CLI（迁移、回填、核对）与三个服务共用同一个 `DatabaseStore::connect`，连接参数无法按用途区分；竞价 Serializable 事务冲突后立即重试 3 次，无退避抖动、无冲突指标 | `crates/auction-storage/src/database/postgres_store.rs`（`PostgresStore::connect`）、`apps/auction-cli/src/database/mod.rs`、`crates/auction-storage/src/database/credentials/ownership.rs`、`crates/auction-storage/src/database/competition/postgres.rs`（`TransactionPolicy`）、`crates/auction-storage/src/database/backend.rs`（`retry_conflicts`） |
| 数据库权限 | 迁移中没有任何 `GRANT`/`OWNER`；Public、Admin、Worker 与 CLI 使用同一个对象 owner 账号，owner 可 `ALTER TABLE … DISABLE TRIGGER` 或 `TRUNCATE`，只追加触发器挡不住被攻破的进程 | `migrations/postgres`、`apps/auction-cli/src/deployment/check.rs`（只核对数据库目标，不区分账号） |
| 就绪探针 | 审计 SQL 的任何错误（含可重试的 40001/40P01、连接中断、超时）都累加进程级计数，计数大于 0 后 `/health/ready`、心跳与 `auction-cli service` 的就绪判定永久失败，只能重启恢复；W0.6 新增的超时会增加此类瞬时失败。`/health/ready` 每次请求都查询 `_sqlx_migrations`，匿名可达、不限流，浏览器每个可见标签页每 10 秒轮询一次，与出价共用 16 个连接 | `crates/auction-storage/src/database/audit_health/mod.rs`、`crates/auction-storage/src/database/store.rs`（`check_process_readiness`）、`crates/auction-web-public/src/app/surface/router.rs` |
| 竞价并发 | 每笔出价（含代理反击、慢速代理）在 Serializable 事务内递增同一 `realtime_streams` 行（`auction:<id>`）的 `next_sequence`；同场不同 lot 的出价只要事务重叠，必有一方序列化失败（40001），Final 阶段出价密集时可能耗尽 3 次重试；出价还以 `FOR UPDATE` 锁定出价人的 `users` 行 | `crates/auction-storage/src/database/realtime/sequence.rs`（`allocate_stream_sequence`）、`crates/auction-storage/src/database/competition/postgres/bidding/eligibility.rs` |
| 读路径 | 限流只有登录、写请求与实时连接三类，匿名 GET 不限流；论坛主题嵌入的倒计时 SVG 每次访问都查库且 `no-store`；价格历史以 `ILIKE ALL` 与 `$n IS NULL OR …` 万能谓词查询，难以使用索引；存储层 SQL 中 88 处 JSON 路径表达式，包括投递领取热路径上的 `lower(payload ->> 'world')`；浏览器包原始 WASM 为 Public 2.7 MB、Admin 4.0 MB，没有体积预算 | `crates/auction-http/src/http/security/mod.rs`（`RateLimiter`）、`crates/auction-http/src/http/public/countdown/handler.rs`、`crates/auction-storage/src/database/price_history/postgres.rs` |
| 规则位置 | 资格与状态机散落在存储层：JSON `Value` 字段比较、SQL 推进、数据库函数（迁移 073/074）；早鸟期与排期日历的纯规则位于应用层 ports | `crates/auction-storage/src/database/sent_confirmation/snapshot.rs`、`crates/auction-storage/src/database/recovery_scheduler`、`crates/auction-storage/src/database/settlement_funding/receipt.rs`、`crates/auction-application/src/ports/scheduling/early_bird.rs` |
| 时区耦合 | legacy 自寄与装备损失证明用 `to_jsonb(行)` 与快照整行比较（含时间戳），只因 sqlx 连接固定 `TimeZone=UTC` 才成立；其他会话时区下结论翻转 | `crates/auction-storage/src/database/legacy_self_proof/mod.rs`、`crates/auction-storage/src/database/equipment_loss/proof.rs` |
| 处置语义 | legacy 自寄、装备损失、Admin 判失败、预检退役、人工 Sent 证明各有专用表或谓词，仪表盘/告警/列表分别拼排除条件 | `crates/auction-storage/src/database/operations_alert/snapshot/reads.rs` |
| 资产与资金 | 没有定期核对 bot 应持装备与 Armory 实际库存；仅记账模式下没有余额漂移检测；没有投影与事实的内部一致性巡检 | 无对应端口；`FundingPolicy::observes_cash` |
| 告警 | 告警来源只有 Process、Delivery、Fulfillment、Recovery 四类，缺收件停滞、凭据失效、生命周期逾期、论坛、磁盘与备份；告警由 Worker 经同一游戏账号的 MoogleMail 发送，账号忙或 Worker 崩溃循环时静默；无外部存活检查；运维手册只有故障排查表，没有按告警来源组织的处置手册 | `crates/auction-application/src/ports/operations/alert/signals.rs`（`OperationsAlertSource`）、`[worker.operations_alert]` |
| 调度依赖与故障域 | 慢速代理到期与进入 Final 由 Worker 执行；Worker 在截止前停机时代理可能来不及反超。合并部署中任一子进程意外退出，supervisor 即停止全部角色，unit 为 `Restart=no`，而 Worker 在凭据所有权失效（如数据库连接闪断）时总以非零状态退出，结果是 Public 一并下线且不自动恢复；分体部署中 Public 以 `Requires=` 绑定 Worker 与 Admin，Admin 也以 `Requires=` 绑定 Worker，Worker 故障时最需要用来排查的 Admin 会一起停止。线上为分体部署（用户确认） | `crates/auction-jobs/src/worker/auction/slow_proxy.rs`、`apps/auction-server/README.md`、`ops/deploy/systemd/auction-server.service.example`、`ops/deploy/systemd/auction-public-api.service.example`、`ops/deploy/systemd/auction-admin-api.service.example` |
| 停机排空 | 排空预算固定 10 秒，角色 unit `TimeoutStopSec=45s`，而 `[worker.delivery] request_timeout_seconds` 为 180 秒；停机（部署、崩溃重启、主机重启、凭据所有权丢失）一旦落在越过 sending 围栏之后的窗口就产生写入结果未知。W7.2 的在途发送门禁只覆盖 CLI 部署 | `crates/auction-process/src/lifecycle/mod.rs`（`DEFAULT_DRAIN_TIMEOUT`）、`apps/auction-worker/src/bootstrap/mod.rs`（`drain_worker`）、`ops/deploy/systemd/auction-worker.service.example` |
| Worker 主循环 | 排期 bootstrap、生命周期、慢速代理与结算在同一任务内依次执行，结算耗时会推迟另一世界的阶段推进与慢速代理；常驻任务内的 panic 只记为一次 join 失败，同一工作项在租约到期后被反复重领，没有转人工的上限 | `crates/auction-jobs/src/worker/auction/lifecycle.rs`（`run_loop`） |
| 运行配置 | runtime 文件的参数修改须重启相关角色才生效（见[配置参考](../../ops/config/README.md)），重启又受 W7.5 时段门禁阻挡，Final 期间无法调整发送节奏或请求预算；W7.7 的数据库开关只选择新旧路径，二者分工未定义 | `crates/auction-config/src/runtime` |
| 安全暴露 | Caddy 模板把 Admin 作为独立公网站点代理（决策 4 保持），而 `docs/architecture.md` 与运维手册仍写“Admin 不应暴露到公网”；Admin 为单一共享密钥登录，`admin_secret` 由人工输入、最短 16 字节；限流按单个 IP 地址计（IPv6 可轻易轮换地址），表上限 4096 条并淘汰最久未见者；无登录失败指标与告警；应用与 Caddy 都未设 HSTS，CSP 缺 `form-action`；限流依赖 `trusted_proxy_cidrs` 正确配置，`config check` 不提示缺失，若站点前还有 CDN 代理，限流键可能退化为 CDN 节点地址；Public/Admin 的 `/metrics` 与业务共用监听，处理器不核对来源，只靠 Caddy 规则返回 404；CI 只在 push/PR 时运行 cargo-audit，代码不变时新出的公告不会被发现 | `ops/deploy/caddy/Caddyfile`、`crates/auction-http/src/http/security/headers.rs`、`crates/auction-http/src/http/security/mod.rs`、`crates/auction-config/src/setup/mod.rs`、`crates/auction-http/src/http/metrics/mod.rs`、`.github/workflows/ci.yml` |
| 登录 key | key 与会话默认永久（决策 25）；`register` 回信重发同一个 key，玩家无法自助轮换；`register_key` 模板含 `{login_key}`，Bot 的 Sent 保留全部 key 明文，而该账号也供人工使用；key 泄露后，攻击者还可借 W9.8 的满额注销反复挤掉真实用户 | `docs/guide/mail-flows.md`、`ops/config/examples/worker.toml` 的 `[worker.mooglemail.mail_templates]` |
| 操作者身份 | Admin 全体操作共用一个 `admin_actor`；CLI 的 `--actor` 是未经认证的自由文本；审计与人工处置中的“操作者”无法追溯到具体的人 | `crates/auction-application/src/services/sessions/mod.rs`、`apps/auction-cli/src/support/text.rs` |
| 主机 | systemd 模板已有基础沙箱，但无资源上限与 `LimitCORE=0`（进程内存含 Cookie 与数据密钥）；SSH、防火墙、PostgreSQL 监听范围、自动更新重启窗口、时间同步与 PostgreSQL 小版本升级流程均未纳入仓库 | `ops/deploy/systemd` |
| 密钥 | 每类数据密钥只有单个 key ID，无密钥环与重加密流程；邮件诊断归档复用 `credential_data_key`（缺失时回退装备密钥），与“三组密钥不得复用”冲突 | `auction-config` secrets 模型；`apps/auction-worker/src/bootstrap/mail/diagnostics.rs` |
| 备份与保留 | 只有手动 `backup create`，范围仅数据库，不含 runtime 配置、诊断证据与密钥托管；未定义 RPO/RTO；仓库无定时任务模板；没有“用最新备份演练新迁移”的部署前流程；[保留政策](../../ops/deploy/retention-policy.v1.json)对 realtime、会话等数据族为 `retain_forever`，另有 12 项 `required_operator_decisions` 未决，其中 `production_postgresql_topology_autovacuum_and_maintenance_envelope` 在仓库中没有任何 PostgreSQL 服务端配置基线（`max_connections` 预算、`pg_stat_statements`、慢查询与锁等待日志、热行表 autovacuum）。R2 的令牌只有“读写”与“只读”两档，无法授予只写不删，防删除只能依靠 bucket lock | `ops/deploy/systemd`、`ops/deploy/RECOVERY.md`、`apps/auction-cli/README.md` |
| 发布 | CI 只在默认分支推送与手动触发时构建 `release-candidate-<sha>`，正式制品保留 14 天，无 tag 触发与来源证明；部署仍在本地重建并手工多步执行；部署状态写在服务器 README；readiness 要求迁移历史与二进制完全一致，旧程序无法在新 schema 上就绪，迁移也没有“兼容/破坏”分级；仓库无 `main`，远端默认分支为 `refactor/dioxus-frontend`，本地 `master` 已过时 | `.github/workflows/ci.yml`、`crates/auction-storage/src/database/store.rs`（`check_postgres_readiness`） |
| 前后端契约 | 契约中 106 处 `deny_unknown_fields`（含响应 DTO），实时事件负载须恰好匹配注册表字段；前后端没有构建号握手，部署后仍打开的旧页面遇到新增字段会解码失败，只能手动刷新 | `crates/auction-contracts/src/contracts`、`docs/contracts/realtime.md` |
| 模块结构 | 401 处 `#[path]` 让模块树与目录脱钩，333 处 `use super::*`，`auction-application` 平铺导出 838 个符号；存在 947 行的 `crates/auction-storage/src/database/mailbox_sweep/mod.rs` 等大文件；Worker 的 `realtime_consumer` 在装配根恒为 `None`，整条运行路径不可达；`auction_forum_reply_cursors`、`bid_proxy_attempts`、`bid_proxy_outbid_uses`、`settlement_pools` 四张表在 Rust 代码中无引用（`settlement_pools` 按契约只作冻结证据），前三者也没有任何迁移函数写入，迁移 005 的 `bid_proxies` 视图同样无人读取，数据库契约仍把前三者描述为在用 | 源码统计、`apps/auction-worker/src/bootstrap/persistent.rs`、[数据库契约](../contracts/database.md) |
| 回归防护 | 非测试代码当前没有 `unwrap()`/`expect()`，但没有 lint 锁定；release profile 未开 `overflow-checks`；`#[path]`、`use super::*` 与错误丢弃的数量没有门禁，可随时回涨；文档没有链接检查 | `Cargo.toml`（`[workspace.lints]`、`[profile.release]`）、`ci/checks/check.sh` |

## 3. 工作流总览

| 编号 | 工作流 | 优先级 | 依赖 | 迁移 | 估算 |
| --- | --- | --- | --- | --- | --- |
| W0 | 错误分类、指标与采集路径、用户可见 SLO、已知缺陷诊断、待核实事实、就绪探针、影子运行框架、浏览器错误信标 | P0（W0.15、W0.16 为 P1） | W0.14 先于 W0.6 | 否 | 10–13 天 |
| W1 | 外部账号调度：快速限界、熔断、三层争用统一调度、访问收口、主动暂停、增量扫描、证据退避、论坛对齐与增量导入 | P0 | W0；W1.11 依赖 W0.5；W1.1、W1.4、W1.11 先有 W6.3；模拟类验收依赖 W6.6；W1.10 依赖 W1.3 | 是（暂停开关、扫描水位、论坛游标） | 15–19 天 |
| W2 | 统一处置事实模型 | P1 | W0、W3.7、W9.9；操作者字段按 W9.7 | 是 | 6–8 天 |
| W3 | 规则上移：类型化快照、纯函数判定、时区无关证明、排期规则下沉 | P1 | W3.7 无依赖；其余批次依赖 W2 的领域类型与 W6.2 | 视批次（收窄数据库函数需迁移） | 10–14 天，分批 |
| W4 | 证据优先的 Admin 运维台、投递检索与玩家支持视图 | P1 | W2、W1.4 | 视实现 | 9–12 天 |
| W5 | 告警通道入口、告警来源补齐、外部存活检查、仪表盘口径与告警分级、处置手册、告警台 | P1（W5.1、W5.6 暂缓，决策 50） | W5.1、W5.5–W5.7 依赖 W0；W5.2–W5.4、W5.8 依赖 W2 | 是（W5.5 扩展告警闭集，W5.8 确认与静默） | 8–11 天 |
| W6 | 验证体系（W6.1–W6.4、W6.6 已批准） | P1 | W6.1 中各算法测试随对应工作项交付；W6.6 依赖 W6.3 | 否 | 16–22 天 |
| W7 | 发布流水线与环境：时段门禁、运行时开关、版本偏差、故障域隔离、迁移兼容分级、运行参数热读取、停机排空 | P2（W7.1a、W7.5、W7.7、W7.8a、W7.11 为 P0；W7.9、W7.10 为 P1） | W7.2 依赖 W10.6；W7.9 依赖 W0.12；W7.8 其余依赖 W6.2、W6.4 | 是（运行时开关、迁移兼容登记） | 12–16 天 |
| W8 | 规模收敛评估（只出决策记录，放在最后） | P2 | 其余工作流完成后 | 否 | 2–3 天 |
| W9 | 安全加固：密钥环、已知取舍复核、Admin 登录防护、主机加固、操作者身份、数据库最小权限、登录 key 自助轮换、入站与论坛滥用防护 | P2（W9.5、W9.7–W9.11 为 P1） | W9.9 先于 W2；W9.11 与 W9.10 共用计数 | 是（密钥环、运行账号授权、回信预算计数） | 11–15 天 |
| W10 | 数据生命周期与备份：定时与异机备份、迁移演练、WAL 归档、RPO/RTO、恢复演练、按政策清理、PostgreSQL 服务端基线、恢复后外部核对 | P0（W10.1、W10.2、W10.6），其余 P1 | W10.7 依赖 W10.2、W10.8；W10.9 依赖 W1.9、W1.10 | 视政策 | 13–17 天 |
| W11 | 资产对账、偿付能力与内部一致性巡检 | P1 | W11.1–W11.3 依赖 W1.3、W5；W11.2 另依赖 W14.8；W11.4 无依赖 | 是（报告事实） | 5–6 天 |
| W12 | 数据库热点与读路径：同场出价序号热点、重试退避、读路径保护、连接配额、JSON 键列化、出价事务内的唤醒通知 | P1（W12.1、W12.4、W12.5），其余 P2 | W0.6、W0.9、W0.13、W7.7 | 是（W12.1 视方案、W12.6） | 8–11 天 |
| W13 | 回归门禁：计数棘轮、panic 与溢出防护、文档链接检查、定时依赖审计、依赖升级节奏 | P1 | — | 否 | 3–4 天 |
| W14 | 通知与提醒：通知目录与模板、内容修正、新增提醒、论坛 notify 改为订阅、普通出价自动标记与渠道前置、付款提醒联动、通知指标与邮资、收件人异常 | P1 | W14.8 先于 W14.1，W14.1 先于 W14.2–W14.4；W14.3 依赖 W1.3；W14.5 依赖 W7.6 | 是（通知事实与订阅上限、偏好字段） | 11–16 天 |

估算为单人连续工作日，含文档同步与静态门禁，不含线上部署与观察；合计约 139–187 天（v10 为 117–156 天，增量来自 W14 与各工作流补遗；v12 取消 W14.7）。影子运行与部署后观察另占日历时间（见[交付规范](#6-每个工作项的交付规范)）。

### 已知缺陷

| 编号 | 缺陷 | 处理 |
| --- | --- | --- |
| D1 | Admin 失败更正后的 COD 实收资金化每轮返回未持久化错误，付款义务停在 `enqueued`，卖方货款保持阻塞；数据库无报错，属于 Rust 侧拒绝，具体原因被应用层 `Err(_)` 丢弃 | W0.7：先让错误可分类，再定位修复；修复前保持人工证明事实不变 |
| D2 | `68c1b8df` 让 Read Mail 观察真实翻页后，与 Sent 维护共享预算并持有 `incoming` 读锁，维护每轮超时，Sent 找回、草稿清理与目击停摆 | W1.0 快速限界，作为 M1 首项单独交付（决策 46）；W1.3、W1.4 根治 |
| D3 | 潜在：发送前 Sent 完整基线受 `maximum_pages ≤ 100` 限制，Sent 若只增不减，超过上限后所有发送失败关闭 | W0.12 核实增速与 HV 清理行为；W1.4 改为 MID 水位前缀扫描；若核实结果显示临近上限则提前单独交付 |
| D4 | 出队让出价被超提醒优先，与 W1.7“通知最低优先级”的方向相反；提醒发送资格不检查 lot 是否已截止 | 已决定（决策 14，决策 61 修订）：运维告警 > 账号类回信 > 按 lot 有效截止排序的出价被超提醒 > 其他邮件；截止后尚未发送的提醒退役（MM 与论坛 PM 队列都适用，PM 的发送资格同样不检查截止）。在 W1.3、W1.7（M2b）实现；W0.13 的“提醒送达时距截止余量”用于验收 |
| D5 | 同场出价序号热点：每笔出价在 Serializable 事务中递增同一 `realtime_streams` 行，同场重叠出价必有一方序列化失败，立即重试 3 次后可能耗尽，玩家收到暂不可用 | W0.6 先按事务 owner 量化冲突与耗尽；W12.1 根治；若 Final 阶段耗尽次数非零，W12.1 提前单独交付 |
| D6 | 论坛回帖：按导入时刻判定截止（轮询 300 秒），截止前的回帖可能被拒或软截止从导入时起算；主题超过 `maximum_topic_pages` 后整场导入失败关闭 | 已决定（决策 15）：维持按导入时刻判定，玩家可见说明（Admin 发布的五语言规则文档与 `docs/guide`）随 M1 先行；W1.11 增量导入与 Final 加速；若 W0.12 显示已有主题临近页数上限，增量导入提前单独交付 |
| D7 | 一次审计写入失败即永久未就绪：计数不区分可重试冲突、连接中断与超时，进程只能重启恢复；W0.6 的超时会放大该问题 | W0.14（决策 32）：按 SQLSTATE 分类，须先于 W0.6 上线 |
| D8 | 停机排空 10 秒短于在途外部写入：越过 sending 围栏后被中断的发送、发帖与私信成为结果未知 | W7.11（决策 33）：关键段独立排空预算并同步 `TimeoutStopSec` |
| D9 | 付款期限从结算建立恢复记录起算，逾期提醒在期限到达时创建，两者都不检查付款邮件（COD 或载体）是否已寄出：邮件积压、账号忙或人工对账期间，买家可能从未收到付款邮件，却收到“请查看原付款邮件”的逾期提醒；赛季前场次只有 24 小时，影响最大 | 已决定（决策 62）：W14.6（M2b）把逾期提醒延后到付款邮件有发送证明之后，并告警“付款邮件未寄出但已逾期”；不改冻结期限，不新增恢复策略版本 |
| D10 | 论坛 `notify` 每行、每个 lot 代码各寄一封 MoogleMail，没有去重或上限，回执正文不含 lot、金额或结果；一条回帖即可让 Bot 账号消耗大量请求预算与邮资 | 已决定（决策 60）：W14.4（M2b）把 `notify` 改为订阅该 lot 的被超提醒，不再寄回执；W0.12 若发现大量使用或滥用，单独提前交付 |
| D11 | 选择论坛私信渠道而未绑定论坛账号的用户，被超提醒在出价事务内即被抑制（`recipient_missing`），偏好接口与页面都不提示；论坛同步关闭时 PM 渠道仍可选 | W14.5（M2b） |

## 4. 分阶段方案

### W0 错误分类、指标与已知缺陷诊断

| 项 | 内容 | 涉及位置 |
| --- | --- | --- |
| W0.1 | 为 `ApplicationError` 增加 `safe_kind() -> &'static str`，闭集分类（如 `funding_unproven`、`evidence_expired`、`repository_conflict`、`remote_busy`），不含秘密与正文 | `crates/auction-application/src/errors` |
| W0.2 | 规则：凡把错误映射为“暂不可用/内部错误”或丢弃后继续的分支，必须记录 `safe_kind` 与业务标识（obligation/delivery/world/路由），保持原控制流不变；同一函数内不同原因必须映射到不同 defer reason。范围覆盖 jobs、应用、集成层的 `Err(_)` 分支与 `map_err(\|_\| …)`（解析闭集按类甄别），以及 HTTP 层 5xx 路径（`internal_error()` 与各 `*_unavailable()`） | `crates/auction-jobs/src/worker`、`crates/auction-application/src/services`、`crates/auction-integrations/src`、`crates/auction-http/src/http/routes` |
| W0.3 | Worker 指标：`worker_unpersisted_error_total{loop,kind}` | `crates/auction-observability` |
| W0.4 | MoogleMail 指标：按 world/operation/类别统计请求数、请求门等待、独占操作锁与 `incoming` 锁等待、`account_busy`/限流/超时次数、维护与观察耗时及预算耗尽次数 | MoogleMail 客户端、`crates/auction-jobs/src/worker/mail/poll/cycle.rs` |
| W0.5 | 论坛客户端同口径指标（主题扫描页数、写入未知、PM、回帖从发帖到导入的延迟估计） | `crates/auction-integrations/src/clients/ehentai` |
| W0.6 | 数据库：连接池已有指标保留；连接按用途分档：服务档（Public/Admin/Worker）设置连接数（扣除所有权长期占用的连接）、`statement_timeout`/`lock_timeout`/`idle_in_transaction_session_timeout`、按角色的 `application_name`，以及短于 HTTP `request_timeout` 的获取连接超时；维护档（CLI 迁移、回填、备份核对、重建）不受 `statement_timeout` 限制、单独命名；各档连接数之和按 W10.8 的 `max_connections` 预算核对。按事务 owner 统计序列化冲突与重试耗尽（量化 D5）；按 Worker 循环统计连接占用。须在 W0.14 上线后启用超时 | `crates/auction-storage/src/database/postgres_store.rs`、`crates/auction-storage/src/database/backend.rs`、`apps/auction-cli/src/database/mod.rs` |
| W0.7 | 用 W0.1 的分类定位 D1：资金化校验各拒绝分支映射到独立 `safe_kind` | `crates/auction-application/src/services/settlement/fulfillment/collected.rs`、`crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W0.8 | 存储层 SQL 错误安全上下文：保留 SQLSTATE 类别、约束名与表名（schema 标识，非秘密），不保留 `detail`/`message`（可能含值）；新增接收原错误的 helper，以 codemod 把 `storage_error` 的 1432 处调用一次改完，不按 owner 分批 | `crates/auction-storage/src/database/backend.rs`（`storage_error`）、`crates/auction-storage/src/database/**/mapping.rs` |
| W0.9 | HTTP 指标：按路由模板的请求量、状态码与延迟，WebSocket 连接数，限流拒绝次数（按 login/mutation/realtime，W12.4 后加 read）；`/metrics` 改为拒绝经可信代理转发的请求（带转发头即 404），不再只依赖反向代理规则 | `crates/auction-http/src/http/shared/boundary.rs`、`crates/auction-http/src/http/metrics/mod.rs` |
| W0.10 | 指标采集路径：Worker 指标增加只监听 127.0.0.1 的 TCP 出口（默认关闭，独立于 control socket）；Public/Admin 若以 Unix socket 监听，同样提供 loopback 出口；指标存储采用本机 Prometheus（只监听 127.0.0.1，保留 15–30 天，经 SSH 隧道查看，决策 12）；同时采集 node_exporter 与 postgres_exporter（只监听 127.0.0.1，决策 42），覆盖磁盘、内存、`pg_stat_archiver`、`pg_wal` 大小、autovacuum、长事务与锁等待；W1/W12 的验收以此为准 | Worker control、`ops/README.md` |
| W0.11 | 重复错误的日志去重：同类错误在窗口内只记首条与计数，避免账号忙期间日志淹没其他信号 | `crates/auction-observability` |
| W0.12 | 只读核实计划依赖的事实：Sent 实际增速与 HV 是否自动清理、Read Mail 排序键、线上 `maximum_pages`/`fetch_timeout_seconds`、`trusted_proxy_cidrs` 是否配置为反向代理地址、站点在 Caddy 之前是否还有 CDN 代理（决策 43）、Public/Admin 的监听方式（TCP 或 Unix socket）、线上是否有指标存储、线上 systemd 版本对 `Requires=` 依赖在自动重启时的实际传播、已有论坛主题的最大页数、PostgreSQL 现行配置（`max_connections`、autovacuum、已装扩展）、主机内存与磁盘余量（容纳 Prometheus、exporter 与 WAL 暂存）；通知与告警相关（运维告警已确认未启用，决策 58）：每封邮件的邮资与近 30 天发信量，HV 邮件与 COD 的过期退回期限，HV 是否原样保存非 ASCII 正文（启用运维告警前核实中文摘要能否通过 Sent 精确证明），选择 PM 渠道而未绑定论坛账号的用户数与被抑制提醒数，论坛 `notify` 使用量，2026-11-01 换季时 isekai 仍未完成的义务数 | 线上只读查询与远端只读页面 |
| W0.13 | 用户可见 SLI/SLO 与容量目标：出价 p99 延迟、出价冲突中止率与耗尽次数、实时推送延迟（事件写入到推送）、注册/登录 key 邮件送达延迟、出价被超提醒送达时距 lot 有效截止的余量（D4 验收依据）、结算到首封与末封付款邮件寄出的延迟、买家付款到卖方货款寄出的延迟、投稿到收取或拒收回执的延迟、慢速代理从到期到执行的滞后；容量目标（Final 阶段峰值出价/秒、并发 WebSocket 数）供 W12 验收 | `crates/auction-observability`、`ops/README.md` |
| W0.14 | 就绪探针（D7，决策 32）：审计写入失败按 SQLSTATE 分类，可重试或瞬时的错误（40001、40P01、57014、55P03、08 类连接错误、取连接超时）只计入指标与日志，不再翻转 readiness；不可重试的失败（约束或触发器拒绝、42501 权限、42P01/42703 schema 漂移）仍使进程未就绪并进入告警。`/health/ready` 结果在进程内缓存 1–2 秒并合并并发探测，匿名请求不再逐次查库。须先于 W0.6 的超时上线 | `crates/auction-storage/src/database/audit_health/mod.rs`、`crates/auction-storage/src/database/store.rs`、`crates/auction-http/src/http/routes/health.rs` |
| W0.15 | 影子运行框架（M2a）：共享的新旧判定比对 helper、[交付规范](#6-每个工作项的交付规范)规定的 `auction_shadow_*` 指标与结构化差异日志、按分支统计样本；W1.4、W2.5、W3 复用，不各自实现 | `crates/auction-observability`、`crates/auction-application` |
| W0.16 | 浏览器错误信标（M3，决策 45，先于 W7.6）：Public/Admin 前端把 WASM panic、DTO 解码失败与实时协议错误按闭集类别和构建号上报到匿名、限流、无正文的信标接口，服务端只累加计数、不入库；同步[路由契约](../contracts/routes.md) | `crates/auction-web-shared/src/bootstrap`、`crates/auction-http/src/http/routes` |

验收：`just verify` 通过；新指标可被采集并查看；错误日志不含秘密；D1 的拒绝分支、SQL 失败约束名与 HTTP 5xx 的原因可从日志直接读出；D5 的冲突率有基线数据；SLO 有当前值；W0.12 结论写入本计划基线；人为制造一次可重试的审计失败后进程仍就绪，不可重试的失败使进程未就绪；`/health/ready` 突发请求不随请求数线性增加数据库查询。回退：指标与日志均为附加输出，可经配置关闭；W0.14 的分类可回到“任一失败即未就绪”。

### W1 外部账号调度

| 项 | 内容 | 关键约束 |
| --- | --- | --- |
| W1.0 快速限界 | Read Mail 观察改为有界页数或降频；维护从每轮 `tokio::join!` 中拆出为独立循环与独立节奏，不再拖慢下一轮收件；main 与 isekai 错峰触发；先确认共享 reader 对完整已读列表的依赖 | 只减少读取；不改变任何证明或业务结果 |
| W1.1 账号忙熔断 | 识别战斗页/冻结后按账号记录 `busy_until`，退避 1→2→5→10 分钟，期间只放行一次只读探测，其余请求不发网络直接返回 `AccountBusy`；熔断状态留在 Worker 进程内，Admin 与 CLI 的远端读取经 W1.10 走同一熔断 | 熔断只限流，不决定业务状态（不变量 5）；写入类在 sending 围栏前被拒 |
| W1.2 诊断去重 | 同一忙碌期只保存首份战斗页；诊断淘汰按类别优先保留写入未知证据 | 不降低 write_unknown 证据保全 |
| W1.3 统一账号调度 | 把三层串行点作为一个调度问题设计：出队优先级（与已有 `send_intervals` 七类节奏合并为一套；运维告警固定最高，其次是账号类回信（登录 key、注册与换绑结果，决策 61），出价被超提醒按 lot 有效截止先后排在其他类别之前，决策 14）、独占操作锁（长扫描不得在持锁期间进行，或按优先级排队）、`incoming` 锁（长列表扫描分段释放）、请求门（分级令牌桶替换 FIFO `IntervalGate`，桶容量为 1 不允许突发，低优先级保底份额） | 总速率不超过配置间隔；同账号远端写入串行；优先级只影响先后，不影响结果 |
| W1.4 增量扫描 | Sent 与 Read Mail 持久化“已完整扫描到的最高 MID + 时间”水位，常规轮按 MID 前缀扫描新增部分而不是按页号；发送前基线同样改为“发送前最高 MID + 前缀扫描”，消除 100 页悬崖（D3）；Read Mail 水位以 W0.12 确认的排序键为前提，目标已知时找齐即停；每日一次完整校验扫描，超过上限时只告警不阻断发送 | 水位不作为发送或收取证明；缺失或不连续回退完整扫描；影子运行期间新旧结果并行比对 |
| W1.5 证据退避 | 履约证据按年龄指数退避（每分钟逐步放宽到 10 分钟，决策 55），付款截止前后提高频率；晚付款的确认与卖方货款因此最多延后 10 分钟 | 不改变付款期限与恢复策略 |
| W1.6 论坛对齐 | 论坛客户端接入同一调度抽象：优先级、写入未知、熔断与指标 | 论坛写入仍按既有未知结果流程，不自动重发 |
| W1.7 通知优先级 | 出价被超提醒保持不合并、不限额（决策 1）；出队位置按决策 14 在 W1.3 实现；lot 有效截止后尚未发送的提醒在 sending 围栏前退役，不再发送，MoogleMail 投递与论坛 PM 队列同样适用（决策 14） | 截止后不再收到提醒是玩家可见变化，同步 Admin 发布的五语言规则文档与[玩家指南](../guide/player-guide.md)；退役沿用预检退役语义，不改变其他邮件 |
| W1.8 配置收尾 | 各项的预算、退避、暂停与通知参数随所在工作项同一提交进入 `worker.toml`（带中文“可选/必填”注释并同步[配置参考](../../ops/config/README.md)），其中发送节奏与预算类参数纳入 W7.10 的热读取；本项只做汇总核对，并清理无实际效果的配置（如 `[worker.delivery] concurrency` 对 MoogleMail 的有效上限为 1） | 默认值不超过当前上限；删除字段按配置兼容规则处理 |
| W1.9 主动暂停 | 运维可经 Admin 或 CLI 设置带到期时间的“暂停外部访问”标记（复用 W7.7 的持久化开关机制），Worker、Admin 装备读取与 CLI 远端读取都遵守；用于人工使用账号、事故处置与恢复后的安全启动（[备份与恢复](../../ops/deploy/RECOVERY.md#6-外部历史核对与启动)）。与已有的按世界暂停收件并存：后者只停业务收件，本项停全部外部访问，二者同时生效时以更严格者为准，并在运维手册写明 | 暂停只阻止新请求，不中断已进入 sending 围栏的写入；到期自动恢复并留审计 |
| W1.10 访问收口 | Admin 手工录入的装备读取与 CLI `mail inspect-sent` 等远端读取改为经 Worker control 代为执行（决策 40），与 Worker 自身请求共用 W1.3 的统一调度与 W1.1 的熔断；Worker 自身的装备来源读取并入 W1.3 的请求门。`auction-cli funds` 依设计须先停止 Worker 并持有凭据所有权，不在此列 | 不在 Admin/CLI 进程持有第二个限速器；Worker 不可用时明确返回能力不可用 |
| W1.11 论坛增量导入 | 按主题持久化“已处理到的最大帖子 ID 及其所在页”游标，每轮只读游标所在页之后的分页与固定楼层所在首页，消除 `maximum_topic_pages` 整场失败的悬崖（D6）；每轮核对游标所在页仍包含该帖子 ID，删帖导致楼层前移或帖子缺失时回退完整扫描；未被引用的 `auction_forum_reply_cursors` 由本项决定改造复用或交 W8.2 清理，并同步[数据库契约](../contracts/database.md)；场次进入 Final 后按最早 lot 有效截止加快该主题的导入节奏；按决策 15 维持导入时刻判定并写明语义，同步 Admin 发布的五语言规则文档、[论坛](../guide/forum.md)与[玩家指南](../guide/player-guide.md) | 游标只缩小扫描范围，不作为出价证明；帖子 ID 不按楼层偏移推算；缺失或不连续回退完整扫描；编辑过的帖子仍不产生新命令；加速受论坛请求门约束 |

验收（以 W0.10 采集的指标度量）：稳态外部请求不超过请求门容量（配置间隔对应的每秒请求数）的 30%；维护 p95 低于 60 秒且无预算耗尽；收件轮询间隔不受维护耗时影响；在 W6.6 替身上模拟战斗期，每个退避间隔不超过 1 次探测、1 份诊断；Sent 页数超过 `maximum_pages` 时发送仍可完成；论坛主题超过 `maximum_topic_pages` 时导入仍可完成，Final 阶段回帖导入延迟达到 W0.13 的目标；投递积压时运维告警仍先于其他类别发出。回退：各项经 W7.7 运行时开关回到旧路径；水位与游标表只追加，停用后不影响旧路径。

### W2 统一处置事实模型

| 项 | 内容 |
| --- | --- |
| W2.1 领域 | `DeliveryDisposition` 闭集：`auto_sent_proof`、`operator_sent_confirmation`、`admin_failed`、`equipment_loss`、`legacy_self`、`preflight_retired`、`operator_acknowledged`；每类定义准入事实，并明确闭集覆盖“全部终态”还是“仅异常处置”，避免与正常成功路径重复计数 |
| W2.2 迁移 | 只追加表 `delivery_dispositions`（delivery、fence、kind、来源引用、操作者、原因、时间），追加触发器；来源引用按 kind 使用各自的可空外键并以 CHECK 约束“恰好一个非空”；表由单一应用端口拥有，其他用例经该端口写入；“操作者”按 W9.7（M2a 已落地）记录：Admin 为共享身份，CLI 附带系统用户，均如实标注来源；按 W9.9 的运行账号授权；同步保留政策与[数据库契约](../contracts/database.md) |
| W2.3 回填 | 迁移内只回填能由既有事实精确证明的行，其余保持未处置；回填比较使用 W3.7 的时区无关形式 |
| W2.4 写入 | 各写者在原事务内经 W2.2 的端口追加处置：Admin 对账、装备损失、`confirm-sent`、提醒预检退役、legacy 修复 |
| W2.5 读侧 | 仪表盘、告警、Admin 列表改用“未处置”视图，先影子比对再删除 `reads.rs` 中的专用排除；新增“确认已处置”命令处理无业务影响的历史失败，领域层限定只适用于没有资金或资产影响的条目，必须填写原因 |
| W2.6 履约 | 履约终态同理（如装备损失后的线下补偿） |

验收：迁移前后各面板计数与当前排除口径逐项一致（影子期差异为零且各 kind 均有样本）；读侧不再含专用排除谓词。回退：读侧经运行时开关切回旧排除口径；处置表只追加，保留不删。

### W3 规则上移

统一模式：

```text
storage：按锁序加载 → 类型化快照（结构体，而非 JSON Value）
application/domain：evaluate(snapshot) → 允许 / 阻断项闭集
storage：以快照版本做 CAS 写入；数据库函数保留为最后一道约束
```

权威划分：业务判定以 Rust 为准，数据库函数只保留不可绕过的最低约束；两边规则变化须在同一提交中同步，并以同一组样例证明结论一致。收窄或改写数据库函数需要新增迁移。各批次涉及的 SQL 侧 JSON 键谓词一并按 W12.6 提升为类型化列。

| 批次 | 目标 | 现位置 |
| --- | --- | --- |
| W3.1 | 人工 Sent 证明准入（两种准入、时间窗、资金占用） | `crates/auction-storage/src/database/sent_confirmation/snapshot.rs` |
| W3.2 | 装备损失处置准入 | `crates/auction-storage/src/database/equipment_loss/snapshot.rs` |
| W3.3 | 恢复阶段推进收敛为领域状态机 | `crates/auction-storage/src/database/recovery_scheduler/scheduler/advance.rs` |
| W3.4 | Admin 投递动作守卫（目击、退役、迁移表） | `crates/auction-storage/src/database/delivery_admin/commands.rs` |
| W3.5 | COD 实收资金化校验（结合 D1 结论） | `crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W3.6 | 履约 claim 一致性校验 | `crates/auction-storage/src/database/fulfillment_scheduler/claims.rs` |
| W3.7 | 行快照证明改为时区无关：显式列比较或 UTC 规范化时间，不依赖会话 `TimeZone`（提前到 M3，先于 W2） | `crates/auction-storage/src/database/legacy_self_proof/mod.rs`、`crates/auction-storage/src/database/equipment_loss/proof.rs` |
| W3.8 | 排期日历纯规则（早鸟期、赛季前场次、夏令时换算）从应用层 ports 下沉到领域 | `crates/auction-application/src/ports/scheduling/early_bird.rs`、`crates/auction-application/src/ports/scheduling/calendar.rs` |

每批验收：业务规则不再以 JSON 下标表达；Rust 判定与数据库函数在同一组样例上结论一致（以 W6.2 集成测试固化，切换前影子比对）；W3.7 在非 UTC 会话下结论不变；W3.8 的已冻结场次排期结果逐场不变。回退：每批经运行时开关切回存储层旧判定，直到后续版本删除旧路径。

### W4 证据优先的 Admin 运维台

| 项 | 内容 |
| --- | --- |
| W4.1 | 投递详情 DTO：尝试历史、冻结期望摘要、Sent 观察与目击、领取状态、处置、关联义务与恢复 |
| W4.2 | 复用维护已写入的 `mailbox_sweep` 观察与 `sent_reference_sighting`，把 Sent 详情观察升级为可引用证据（摘要与回执，不含登录 key 正文） |
| W4.3 | Admin 发起人工 Sent 证明、Admin 失败更正、确认已处置：选择已采集证据 + 原因 + 稳定命令 ID，复用同一应用端口 |
| W4.4 | 对账对话框展示目击与候选；判失败保持闭集原因 |
| W4.5 | 投递检索：按收件人、参考编号（义务、投递、通知 ID）、模板与发送类别、世界和时间检索；列表显示模板、类别与退役或抑制原因 |
| W4.6 | 玩家支持视图：按用户或参考编号只读汇总出价、成交、义务、投递与通知（含退役与抑制原因），用于处理玩家咨询与核对冒充 Bot 的邮件 |

同步[路由契约](../contracts/routes.md)、五语言译文与前端 `Route`。验收：已领取货款、误判失败、结果未知通知、退件重试四类处置均可在 Admin 内闭环；按玩家报出的 Reference 一次检索即可定位对应义务、投递与通知。回退：新页面与接口为增量，旧对账入口保留到验收完成。

### W5 告警通道、来源、仪表盘口径与处置手册

| 项 | 内容 |
| --- | --- |
| W5.1（暂缓） | 运维告警抽象出通道端口与配置入口，与 MoogleMail 通道并存、可单独启用；外部通道采用通用 Webhook（决策 16），请求体为可配置的 JSON 模板，具体服务与地址作为敏感配置由用户填写。用户暂不启用外部告警（决策 50），待选定服务后排入当时的里程碑 |
| W5.2 | 仪表盘状态复用告警策略的阈值评估，消除“>0 即需处理”与告警阈值两套口径；区分正常等待与超时积压：付款期内等待买家付款的 COD、等待前置的卖方货款不计入积压，超过付款期限或预期时长才计入，并复核各默认阈值（现为数量 1）；告警分级（决策 66）：需要立即处理的信号保留 30 分钟冷却重复通知，付款逾期、待重列等非紧急信号改入每日摘要，不单独发恢复通知，已恢复项列入下一封摘要 |
| W5.3 | 新增“外部账号”分区：忙碌与暂停状态及持续时间、请求门使用率、维护健康、论坛状态 |
| W5.4 | 告警正文附 W5.7 处置手册的锚点；W10、W11 落地时各自注册备份与对账告警来源 |
| W5.5 | 补齐告警来源：收件轮询停滞、凭据失效、生命周期逾期（应进入 Final/结算而未进入）、论坛同步与私信失败、磁盘与备份目录容量、外部账号忙或熔断持续、非结算通知投递失败（含邮资不足）、Sent 与论坛主题页数达到上限的 80%、付款邮件未寄出但已逾期（D9）、换季前 isekai 仍有未完成义务、慢速代理到期后超过阈值仍未执行；告警种类、来源与范围是数据库 CHECK 闭集（迁移 005、020），扩展须新增迁移并按 W7.8a 登记为 `breaking`；Final 期间 Worker、Public 或数据库不可用只告警，不延长 lot 截止（决策 38）。备份、上传与 WAL 归档的失败经本项并入运维告警与仪表盘；线上未启用 MoogleMail 运维告警（决策 58），在它或外部通道（决策 50）启用前，这些来源只在仪表盘与 W5.8 告警台可见，没有主动通知 |
| W5.6（暂缓） | 外部存活检查采用心跳打卡（决策 16；用户暂不启用，决策 50，待选定服务后排入当时的里程碑）：Worker 在自身心跳新鲜且常驻循环正常时，定期向配置的外部地址报到，超时未报到由外部服务告警，覆盖 Worker 崩溃循环与整机宕机；报到地址属于敏感配置。Public 的 `/health/ready` 已由反向代理对外可达，可在同一外部服务配置 HTTP 检查 |
| W5.7 | 处置手册：在[运维手册](../../ops/README.md)按告警来源组织处置步骤，至少覆盖账号忙、Cookie 失效、Worker 崩溃循环、数据库不可用、磁盘满、HV 页面结构变化、备份失败、邮资耗尽、Bot 账号被盗或被封 |
| W5.8 | 告警台：Admin 新增活动与历史告警列表（来源、范围、首次与最近时间、计数、处置手册锚点），支持确认与带到期的静默（只抑制重复通知，不改变信号与阈值评估），以及每日摘要（决策 66）；运维告警未启用期间（决策 58），告警台是唯一的告警入口；导航显示待办数（人工对账、待确认恢复、活动告警）；仪表盘自动轮询默认开启（30 秒）；同步[路由契约](../contracts/routes.md)与五语言译文 |

验收：在 W6.6 替身上模拟账号忙、Cookie 失效，并在本地停止 Worker，三种场景各在阈值时间内经至少一条不依赖该故障的路径产生告警（外部通道启用前以仪表盘为准，Worker 停止与账号忙期间可能没有主动通知）；每个告警来源都有对应的处置手册锚点；正常运营（没有超时积压）的一周内不产生积压类告警；告警台能列出全部活动信号，并完成一次确认与静默到期。回退：新告警来源与通道可分别经配置停用；分级与摘要可配置回逐条冷却通知；告警台为增量页面，确认与静默记录只追加。

### W6 验证体系（W6.1–W6.4、W6.6 已批准）

| 项 | 内容 |
| --- | --- |
| W6.1 | 纯逻辑测试：结算守恒、持有曲线插值、比例分配、冲正；竞价增量 v1/v2、代理竞争与上限出价、软截止；排期日历（夏令时、早鸟期、赛季前场次）；MoogleMail 与论坛解析器（合成样例覆盖收件人 15 字符截断、标题 7 位 hex、分钟精度时间、战斗页、登录页）；前端纯逻辑（实时事件合并与缺口检测、URL 查询状态）；随对应工作项交付的算法：W1.1 熔断状态机、W1.3 调度算法（tokio 暂停时间）、W1.4 水位与 W1.11 游标、W2 处置闭集、W7.5 时段门禁判定。另含：结算守恒、价格单调、金额网格与代理竞争不变量的属性测试；不可信输入解析器（论坛回帖语法、邮件 `start` 指令、HV 页面）以属性测试生成任意输入，保证不 panic 且失败关闭（不引入 nightly fuzz 工具）；`ops/config/examples` 与首次设置生成结果能被 loader 加载；DTO 序列化快照，发现破坏兼容的契约变化（配合 W7.6）；通知渲染快照（每个模板与占位符，含旧载荷）与提醒周期（领先周期、重新领先失效、截止后退役、PM 缺收件人） |
| W6.2 | PostgreSQL 18 集成测试骨架：测试夹具用本机 PG18 二进制建临时集群，每个测试从模板库克隆独立库；以独立 feature 控制，`just verify` 只编译不运行；覆盖空库执行全部迁移、竞价（含并发序列化冲突）、结算、人工 Sent 证明、恢复推进、仪表盘计数、非 UTC 会话下的证明；W9.9 落地后以运行账号执行。覆盖分两步：M2a 先覆盖资金、投递、生命周期与竞价 owner 的存储端口；每个存储端口方法至少执行一次（157 个端口 trait、856 条运行时 SQL 没有编译期校验）作为 M6 前的目标 |
| W6.3（已批准，M2a） | 脱敏的真实页面回放语料（Inbox/Read Mail/Sent/详情/战斗页/截断列表/论坛页）：去除 Cookie、登录 key 与表单 token，替换玩家名称，只保留解析所需的页面结构（决策 28）；先于 W1.1、W1.11、W1.4 交付，供其解析与替身复用 |
| W6.4（已批准） | CI 增加独立测试作业，运行 `just test` 与 `just test-db`（CI 内安装 PostgreSQL 18），不并入默认 `just verify` |
| W6.5 | 测试入口与规范：新增 `just test`（纯逻辑）与 `just test-db`（PG18 集成）；同步更新 `AGENTS.md` 验证范围、`CONTRIBUTING.md` 与[开发指南](../development.md#测试)，写明已批准的测试类别与维护要求；[交付规范](#6-每个工作项的交付规范)第 5 步改为要求运行受影响范围的测试 |
| W6.6（已批准，M2a） | 外部服务替身（决策 44）：以 W6.3 语料驱动的本地 MoogleMail（Inbox/Read Mail/Sent 列表与详情、compose、战斗页、登录页）与论坛（主题分页、回帖、私信）模拟服务，可注入账号忙、Cookie 失效、超时、页面结构变化、写入结果未知、删帖、邮资不足与收件人不存在；只在测试入口启动，绝不连接真实账号；W1 与 W5 的模拟类验收先在替身上完成，再到线上观察 |

验收：W6.1 覆盖上述每类至少一组正反例并全部通过；W6.2 在本机可一条命令运行并清理临时集群，存储端口方法覆盖率可统计；W6.6 能复现 D2、账号忙与 Cookie 失效三类场景；规范文档与 Justfile 入口一致。回退：测试只在独立入口运行，不影响默认门禁与运行时。

### W7 发布流水线与环境

| 项 | 内容 |
| --- | --- |
| W7.1a（M1） | 分支基线：以 `refactor/dioxus-frontend` 的 HEAD 建立 `main` 并设为远端默认分支，开启分支保护（必须通过 CI）；此后各里程碑的部署使用 CI 在 `main` 上产出的候选制品，部署前核对校验和，不再本地构建（决策 51）。旧分支按决策 52 处理：`master`、`audit-deps`、`ci/dependency-version-guard` 与已合入的 dependabot 分支删除；`tokio-1.54.1`（违反 Tokio 1.53 LTS 锁定）与 `tungstenite-0.30.0`（须随 Axum 同步升级）的 PR 关闭不合并；远端设置与删除由用户执行 |
| W7.1 | 版本发布（M6）：只从 `main` 上的 tag 发布（决策 8）：CI 增加 tag 触发，制品附到 GitHub Release 长期保留并生成来源证明；CI 增加“已发布迁移文件未被修改”检查；部署只使用 CI 制品并核对其提交可从发布 tag 到达 |
| W7.2 | `auction-cli deploy <制品目录>`：在现有 `service restart`（严格配置检查、停机、迁移、按序启动、readiness）之上增加在途发送门禁、W7.5 时段门禁、含迁移版本的 W10.6 迁移演练回执核对、备份并新旧 CLI 校验、保留旧程序、静态资源核对与回执；先面向线上的分体部署，合并部署的支持随 W8.1 的决定。配套 `auction-cli rollback` 只用于不含迁移（或只含 W7.8a 登记为 `compatible` 的迁移）的版本；含破坏性迁移的版本回退只能按[备份与恢复](../../ops/deploy/RECOVERY.md)恢复并做外部历史核对 |
| W7.3 | 部署回执写入状态目录并可查询，取代手改服务器 README |
| W7.4（取消） | 现线上即正式环境，不建预发、不更名（决策 9）；行为验证使用本地 PostgreSQL 18、W6.2 与 W10.6 迁移演练 |
| W7.5 | 时段门禁（提前到 M1，决策 47）：`service stop/restart` 只在两种情况下拒绝执行：任一场次处于 Final；或任一在拍 lot 的有效截止落在保护窗口内（默认 60 分钟、可配置，且不短于 W10.6 实测停机时间的 3 倍）。慢速代理只在其 lot 落入保护窗口时计入，Live 期间其余时间允许重启（代理只会顺延执行）；拒绝时需显式 `--force` 并写明原因；同步[运维手册](../../ops/README.md)，并写明系统自动更新重启等绕过 CLI 的停机由 W9.6 约束 |
| W7.6 | 前后端版本偏差：HTTP 响应头与 WebSocket 首帧携带构建号，浏览器发现与自身构建号不一致时提示刷新（不自动刷新正在操作的页面）；在 `crates/auction-contracts/README.md` 写明契约演进规则（响应新增字段的兼容方式）；同步[路由契约](../contracts/routes.md)与[实时事件契约](../contracts/realtime.md) |
| W7.7 | 运行时开关（提前到 M1）：行为开关持久化在数据库（闭集开关名、取值、到期、操作者、原因，追加审计），Worker/Public/Admin 周期读取，切换无需重启，因此不受 W7.5 阻挡；开关只在都正确的新旧代码路径间选择，不决定金额、资格、幂等或锁；W1.9 主动暂停复用同一机制；同步[数据库契约](../contracts/database.md)与保留政策 |
| W7.8a（M1，决策 34） | 迁移兼容登记与 readiness 放宽：每个新迁移在自身事务内向只追加的兼容登记表写入一行 `compatible`（只新增、旧程序可安全运行）或 `breaking`，CI 检查每个新迁移恰好登记一次；已发布的 001–074 视为 `breaking`；readiness 允许数据库比二进制多出的版本全部登记为 `compatible`，其余情况仍拒绝就绪。旧程序靠登记表判断自己不认识的版本，因此放宽须先随一版部署，之后的版本才能换回它；像 069、071 这类要求全停、改冻结策略、收紧约束或改触发器的迁移一律 `breaking`，默认 `breaking`，只对纯新增放宽；放宽 CHECK 以写入新的闭集取值（状态、种类、失败码、告警来源）会让旧程序读到无法解析的值，同样登记为 `breaking`，除非旧程序对未知值已有明确容错；同步[数据库契约](../contracts/database.md)、保留政策、[迁移说明](../../migrations/postgres/README.md)与运维手册的回退章节 |
| W7.8（M6） | 兼容声明验证：用 W6.2 以上一版存储测试验证 `compatible` 登记属实，并接入 W6.4 的 CI 作业（决策 27 的其余部分） |
| W7.9 | 故障域隔离（线上为分体部署）：把 Public 对 Worker/Admin、Admin 对 Worker 的 `Requires=` 都改为 `Wants=`，启动顺序继续由 `auction-cli service` 保证；按 W0.12 核实的 systemd 行为验证 Worker 故障重启不连带 Public 与 Admin；同步运维手册。合并部署的同类问题（任一子进程退出即全停且 `Restart=no`）记入 W8.1 |
| W7.10（M2b，决策 39） | 运行参数热读取：Worker 周期检测 runtime 文件变化，只对发送节奏（`send_intervals`）、邮件与论坛请求预算（页数、超时、批量）以及 W1 新增的预算、退避与通知参数做完整校验后原子替换；校验失败保留旧值并记录告警；其余字段仍需重启。Admin 参数页标明即时生效的字段，同步[配置参考](../../ops/config/README.md)的“生效”列。热读取不受 W7.5 阻挡，参数仍只影响节奏，不决定金额、资格或幂等 |
| W7.11（M1，D8，决策 33） | 停机排空：排空预算改为按角色配置；Worker 停止时未越过 sending 围栏的工作立即放弃（租约到期后重领），已越过围栏的外部写入关键段（compose/发帖/私信请求到结果落库）在独立预算内完成（默认 60 秒）；角色 unit 的 `TimeoutStopSec` 与 `auction-server` 的子进程排空同步调大；关键段预算耗尽仍按结果未知处理，不改变人工对账规则；W7.2 的在途发送门禁继续覆盖部署 |

验收：从 tag 到上线一条命令完成并产生回执；含破坏性迁移版本的 `rollback` 被拒绝并给出恢复指引；W7.8a 部署后，只多出 `compatible` 迁移的数据库上旧程序可就绪；Final 阶段的停机请求被拒绝；运行时开关与热读取参数切换在一个读取周期内生效且有审计；部署后旧页面出现刷新提示而不是解码错误；Worker 崩溃或重启期间 Public 持续可出价、Admin 持续可用；在 W6.6 替身上于发送关键段中途停止 Worker，写入完成并落库后进程才退出。回退：W7.2/W7.3 为新增命令，旧的手工流程保留；W7.7 开关表与兼容登记表只追加；W7.9 的 unit 与 supervisor 变化可换回旧模板或旧程序；W7.10 可关闭热读取回到重启生效；W7.11 的排空预算可配置回 10 秒。

### W8 规模收敛评估

输出决策记录，不直接实施；放在全部工作流之后评估（决策 19）。

| 项 | 内容 |
| --- | --- |
| W8.1 | 部署形态：是否只保留合并部署（须先解决合并部署任一子进程退出即全停且 `Restart=no` 的故障域问题）、是否移除可选 Redis、首次设置服务与停用世界（isekai）代码路径的去留 |
| W8.2 | 代码结构：`#[path]` 与目录对齐、`use super::*` 收敛、`auction-application` 平铺导出改为按上下文分组、大文件（如 `crates/auction-storage/src/database/mailbox_sweep/mod.rs`）拆分、按限界上下文拆 crate 的编译收益；旧兼容层清单（`crates/auction-application/src/services/auction/competition/legacy_ledger.rs`、legacy 自寄、v1 竞价策略、迁移执行视图）及各自的移除条件；不可达代码与无人读写的 schema 对象清单（Worker `realtime_consumer` 运行路径、`bid_proxy_attempts`、`bid_proxy_outbid_uses`、`bid_proxies` 视图，以及 W1.11 未复用时的 `auction_forum_reply_cursors`）、删除条件与数据库契约更正；是否把 W9.9 的共用运行账号再拆为按角色账号；实施时以 W13.1 的计数门禁锁定进度 |

验收：决策记录写入 [§7](#7-决策记录)，并同步 `docs/architecture.md` 与运维手册的对应描述。回退：只改文档，无需回退。

### W9 安全加固

| 项 | 内容 |
| --- | --- |
| W9.1（取消） | Admin 暴露面保持现状，不加来源限制（决策 4） |
| W9.2（取消） | 不加第二因素（决策 5） |
| W9.3 | 数据密钥环：多 key ID 并存解密、新写入用当前 key、离线重加密命令与回执；邮件诊断归档改用独立密钥域；旧密钥保留期不短于最长备份保留期（W10.2 为 12 个月）；凭据 Cookie 轮换流程文档化 |
| W9.4 | 复核 CSP `unsafe-eval` 等已知取舍并记录接受理由；增加 HSTS 与 `form-action 'self'` |
| W9.5 | Admin 补偿控制（不改变决策 4、5；提前到 M2a）：登录失败计数指标与告警；Admin 只有一个账号，登录失败另加不按来源地址的全局限流与退避；限流键对 IPv6 按 /64 聚合；首次设置为 `admin_secret` 生成随机值，`config check --strict` 对短于 32 字节的密钥与生产形态（`cookie_secure = true`、loopback 监听）下未配置可信代理报警；`admin_secret` 轮换流程；若 W0.12 核实站点前有 CDN 代理（决策 43），Caddy 只信任 CDN 地址段并从其头部取得真实客户端地址，应用的 `trusted_proxy_cidrs` 只信任 Caddy，`config check --strict` 提示该链路缺失；在 `docs/architecture.md#安全边界`与运维手册中按决策 4、5 改写为“已接受风险 + 补偿控制” |
| W9.6 | 主机加固：systemd 模板补资源上限（MemoryMax、TasksMax）、`LimitCORE=0` 与可用的沙箱选项，用 `systemd-analyze security` 记录评分；运维手册补主机层基线：SSH 禁止 root 与口令登录、防火墙只开放必要端口、PostgreSQL 只监听本机、自动更新的重启窗口避开 Final（或关闭自动重启改人工）、时间同步、journald 持久化与容量上限（`SystemMaxUse`）、PostgreSQL 小版本升级纳入 W7.5 时段门禁 |
| W9.7 | 操作者身份（单人运营，决策 17；M2a，先于 W2）：Admin 维持共享身份；CLI 写入审计时自动附带系统 UID 与 `SUDO_USER`，`--actor` 仍为必填说明；审计与处置记录如实标注来源（Admin/CLI） |
| W9.8 | Public 会话上限（决策 30，M2b）：登录 key 与会话默认永久保持不变；`max_public_sessions` 默认值由 5 改为 10（配置示例、[配置参考](../../ops/config/README.md)与[业务规则](../business-rules.md)同步）；满额登录时在同一事务、同一用户行锁内把最久未活动的有效会话标记为已注销（不删除记录），再建立新会话；被注销的设备用原 key 重新登录即可，并看到五语言的“已在其他设备登录”提示；同步 Admin 发布的五语言规则文档 |
| W9.9 | 数据库最小权限（决策 35；M3 首项，先于 W2）：迁移 owner 账号只供 CLI 的迁移、重建与恢复使用；Public、Admin、Worker 共用一个无 DDL、无 `TRUNCATE`、对只追加表无 `UPDATE`/`DELETE` 的运行账号，权限由迁移内的 `GRANT` 与默认权限维护（含序列与 `_sqlx_migrations` 只读）；敏感 TOML 区分运行账号与维护账号，`config check --strict` 拒绝服务使用 owner 账号；首次设置与合并部署同步；授权变更在部署时由用户执行；同步[数据库契约](../contracts/database.md)、[配置参考](../../ops/config/README.md)与运维手册；是否再拆为按角色账号交 W8.2 评估 |
| W9.10 | 登录 key 自助轮换（决策 37，M2b，与 W9.8 同批）：新增邮件主题 `rekey`（不带资产）：在同一事务内轮换该账号的登录 key、注销其全部有效会话并回信新 key；`register` 仍重发原 key；`register` 与 `rekey` 每个账号 24 小时内各自只回信一次，超出的照常记录命令与审计但不回信（决策 53）；被拉黑或身份冲突时与 `register` 一样回信说明；新增回信模板进入配置示例；同步[邮件流程](../guide/mail-flows.md)、[业务规则](../business-rules.md)与 Admin 发布的五语言规则文档 |
| W9.11 | 入站与论坛滥用防护（M2b，与 W9.10 共用计数）：按发件人与论坛账号限制每 24 小时最多 10 封说明信（决策 63），覆盖 `rebind` 结果、`auction` 拒收说明、`user_assets_rejected` 与论坛回执；超出只记命令与审计、不寄说明信，附带资产照常退回（资产属于发件人）；超额次数进入 W14.8 指标；阈值进入配置并纳入 W7.10 热读取；同步[邮件流程](../guide/mail-flows.md)与 Admin 发布的五语言规则文档 |

验收：`systemd-analyze security` 评分写入运维手册；响应含 HSTS 与 `form-action`；Admin 登录失败超过阈值时产生告警且全局限流生效；密钥轮换在本地 PG18 演练并出回执；服务进程以运行账号连接时无法执行 DDL、`TRUNCATE` 或改写只追加表；`rekey` 后旧 key 无法登录、原有会话全部失效；同一发件人超出说明信预算后不再收到说明信，附带资产仍被退回。回退：安全头与限流参数可经配置回退；密钥环保留旧 key，可回到单 key 写入；运行账号可临时换回 owner 账号（`--strict` 下报错提示）；`rekey` 主题可经配置停用。

### W10 数据生命周期与备份

| 项 | 内容 |
| --- | --- |
| W10.0 | 恢复目标已定：启用 WAL 归档（决策 18，实施见 W10.7，M2a）；RPO ≤ 5 分钟（`archive_timeout` 300 秒）、RTO ≤ 2 小时、每月一次恢复演练（决策 31）。本项只剩把这些结论写入保留政策 v2 的 `restore_rto_rpo_and_rehearsal_cadence`，随 W10.4 的政策发布在 M5 完成 |
| W10.1 | 定时备份（M1 首批单独交付）：systemd timer 模板 + `auction-cli backup create/verify`，回执；成功、失败与磁盘水位检查留有可选的打卡地址（成功报到、失败立即报失败，超时未报到由外部服务告警，决策 41）；用户暂不启用外部服务（决策 50），地址留空即不打卡，在 W5.5 并入运维告警之前，失败只体现在回执、systemd 失败状态与日志中；本地保留份数与磁盘水位 |
| W10.2 | 异机副本（M1 首批单独交付）：加密后上传 Cloudflare R2，保留 30 份日备份与 12 份月备份，副本校验（决策 6）；日/月备份分前缀，轮换由 bucket 生命周期规则按前缀与年龄淘汰；R2 令牌只有“读写”与“只读”两档，服务器令牌只授予该 bucket 的对象读写，防删除与防覆盖依靠必须启用的 bucket lock，按前缀设锁定期且不长于生命周期淘汰期：日备份 30 天、月备份 365 天、WAL 与基础备份 21 天（决策 41、49）；上传与校验结果同样经可选打卡地址报到；以 `age` 公钥加密，服务器只持有公钥，私钥离线保存于密码管理器与一份离线副本（决策 13）；三组数据密钥与 `age` 私钥一起离线托管，托管与取回步骤写入[备份与恢复](../../ops/deploy/RECOVERY.md)（决策 48）；R2 凭据进入敏感 TOML 并纳入 `config check` |
| W10.3 | 定期恢复演练：在隔离实例执行 `db rebuild --ephemeral`/恢复预检并出回执；演练前确认磁盘余量 |
| W10.4 | 按政策只清理运行态数据族（会话、心跳、realtime 事件、诊断等）；审计与资金事实永不删除；每次删除先只读演练并经用户批准（决策 7）。期限：进程心跳、实时事件与邮件诊断见决策 23–25；会话只删除已注销或已过期满 30 天的记录（决策 29），有效会话（含永久会话）不受影响；不提供个人数据删除（决策 26）。前置：发布保留政策 v2（现行 v1 对 realtime、会话为 `retain_forever`，并有未决的 `required_operator_decisions`）；在[实时事件契约](../contracts/realtime.md)中定义回放保留期限与过期游标行为 |
| W10.5 | 备份范围：除数据库外，纳入 runtime 配置、加密邮件诊断证据与部署回执（数据密钥与备份密钥的离机托管已随 W10.2 在 M1 完成，决策 48） |
| W10.6 | 迁移演练（无预发的替代，提前到 M1）：含迁移的版本部署前，把最新备份恢复到本机或隔离的 PostgreSQL 18 → 用新程序执行迁移 → readiness 检查 → W11.4 巡检（落地后加入）→ 记录迁移耗时与回执；M1 先以手工流程与回执模板交付，W7.2 自动核对回执 |
| W10.7 | WAL 归档与时间点恢复（决策 18、41、49，M2a）：`archive_mode = on`，`archive_command` 只在本机完成 zstd 压缩与 W10.2 的 `age` 公钥加密，写入本地暂存目录并 fsync，任一步失败返回非零由 PostgreSQL 重试；独立的上传器（systemd timer）把暂存文件上传 R2 独立前缀、校验后删除本地副本，对象已存在且校验一致视为成功（bucket lock 下重传返回 409）；每周一次 `pg_basebackup` 物理基础备份同样加密上传；`archive_timeout` 为 300 秒（RPO ≤ 5 分钟，决策 31）；可恢复到过去 14 天内任一时刻：WAL 与基础备份都保留 21 天（同时保有 3 份基础备份），生命周期与锁定期按此对齐（决策 49）；归档、上传与暂存目录大小进入 W0.10 的指标、可选打卡地址与 W5.5 告警，避免积压撑满磁盘；开启 `archive_mode` 需重启 PostgreSQL，受 W7.5 时段门禁约束；W10.3 演练覆盖时间点恢复；同步[备份与恢复](../../ops/deploy/RECOVERY.md) |
| W10.8 | PostgreSQL 服务端基线（M2a，先于 W10.7）：仓库提供 `postgresql.conf` 片段模板与说明：`max_connections` 按三角色服务档、维护档与 exporter 的连接预算；`shared_preload_libraries = 'pg_stat_statements'`（W12.4 的热点分析依据）；`log_min_duration_statement`、`log_lock_waits`、`log_autovacuum_min_duration`；热行表（`realtime_streams`、`lots`、`users`）与大只追加表的 autovacuum 参数；`archive_*`（W10.7）；只监听本机（W9.6）；以此回答保留政策的 `production_postgresql_topology_autovacuum_and_maintenance_envelope`；需重启的参数受 W7.5 约束 |
| W10.9 | 恢复后外部核对（M5）：Worker 在 W1.9 全局暂停下启动，经 W1.10 的 control 只读扫描备份时刻之后的 Sent 与 Read Mail；把恢复库中仍为 pending/claimed、且与 Sent 精确匹配的投递隔离为人工对账，列出恢复库没有记录的已读入站邮件与论坛新帖，输出回执；替代[备份与恢复](../../ops/deploy/RECOVERY.md#6-外部历史核对与启动)中的纯人工核对 |

验收：定时备份连续 7 天成功并有回执；R2 中日/月副本按规则轮换且校验通过，服务器令牌删除锁定期内的对象被拒绝；M1 内完成一次端到端演练：从 R2 下载、用离线私钥解密、恢复到本地 PG18 并核对，同时用托管的数据密钥解开加密列（决策 48）；人为制造一次备份失败，回执与 systemd 状态如实反映（启用打卡后由外部服务在阈值内告警）；WAL 归档与上传无积压，并在隔离实例完成一次恢复到指定时间点的演练；迁移演练与恢复演练各至少完成一次并出回执；W10.3 演练中，W10.9 能找出备份时刻之后已寄出的投递。回退：停用 timer 与上传，不影响服务；关闭 `archive_mode` 需重启 PostgreSQL。

### W11 资产、资金对账与内部一致性巡检

| 项 | 内容 |
| --- | --- |
| W11.1 | 装备持有核对：由数据库推导 bot 应持装备（待退回、草稿、未寄出等），与 Armory 实际库存比对，输出差异报告 |
| W11.2 | 偿付能力报告：按待付卖方货款、待执行退款与邮资估算（W14.8）核对 Bot 的只读余额是否足够，不足时告警；仅记账模式下个人消费与 Bot 共用账号，不做余额漂移归因；只报告不改账 |
| W11.3 | 报告以追加事实保存，差异进入仪表盘与告警（W5） |
| W11.4 | 内部一致性巡检（提前到 M2a，作为 W2/W3 改造的安全网）：只读 SQL 不变量（lot 投影与出价事实、补偿池余额与流水、义务链完整性、投递与义务状态对应、realtime 流序号连续、已截止的 lot 不残留未执行的慢速代理调度），结果同样以追加事实保存；W10.6 迁移演练复用 |

约束：外部观察只读，按 W1 的最低优先级执行；报告不授权任何资金或资产动作。

验收：W11.4 在当前线上数据上运行，差异为零或每项差异有解释并进入处置；W11.1/W11.2 报告追加保存并进入仪表盘。回退：关闭巡检开关（只读，无数据回滚）。

### W12 数据库热点与读路径

| 项 | 内容 |
| --- | --- |
| W12.1 | D5 根治：以 W0.6 量化后，在三种方案间选择：“提交后分配序号（事件仍与出价同事务写入）”——每个 Public 实例各有自己的 `consumer_key`，因此须另设唯一的序号分配者，而不是由各 relay 分配，未分配序号的事件不进入回放；“进入 Serializable 事务前按场次排队”；“按 lot 拆分事件流”——同 lot 出价本就在 lot 行锁上串行，但浏览器须合并订阅多个流。保持 `stream_sequence` 连续与回放契约并同步[实时事件契约](../contracts/realtime.md)；同时为竞价与独立 Serializable 写事务的重试加入带抖动的退避，重试预算不变 |
| W12.2 | 以 W0.9 指标度量 Final 阶段每次出价触发的页面重读量；必要时在前端增加短合并窗口，仍以权威接口为准，不从事件负载推导结果 |
| W12.3（已批准） | 本地 PG18 负载演练：模拟 Final 阶段并发出价与匿名读突发，在 W12.1、W12.4 前后各测一次 |
| W12.4 | 读路径保护：匿名读按来源地址（IPv6 按 /64）限流或设并发上限（`/health/ready` 的缓存已由 W0.14 先行）；读接口与出价使用独立连接池或信号量，读不得耗尽出价连接；倒计时 SVG 改为短期可缓存（数秒）；价格历史等搜索在 W0.6 的超时下运行，并以 EXPLAIN 基线评估索引与查询改写 |
| W12.5 | Worker 循环连接配额：拍卖关键循环（生命周期、慢速代理、结算）保留连接，批量与维护循环受信号量限制；评估把结算阶段从拍卖主循环拆为独立任务，避免结算耗时推迟另一世界的生命周期推进与慢速代理 |
| W12.6 | 热路径 JSON 键列化：投递领取等热路径上的 `payload ->> …` 键（world、operation、template）提升为类型化列（新迁移回填） |
| W12.7 | 浏览器包体积预算：记录基线（原始 WASM Public 2.7 MB、Admin 4.0 MB），超出阈值时发布脚本提示 |
| W12.8 | 出价事务内的唤醒通知：被超提醒在出价 Serializable 事务内插入 `pending` 投递，触发器 `notify_delivery_ready` 随即执行 `pg_notify`；PostgreSQL 提交含 NOTIFY 的事务时持有一把全局锁直到提交完成，以保证通知按提交顺序入队，Final 阶段这些出价的提交因此串行。以 W0.6、W12.3 量化提交等待，必要时把唤醒移到提交之后（提交者另发或由轮询承担） |

验收：以 W0.13 的目标判定：Final 阶段出价无冲突耗尽、出价 p99 延迟达标；匿名读突发时出价路径的连接等待不上升；Worker 维护高峰时拍卖关键循环的调度延迟不上升。回退：经 W7.7 运行时开关回到旧路径；序号分配方式切换前后的事件均可回放。

### W13 回归门禁

| 项 | 内容 |
| --- | --- |
| W13.1 | `ci/checks/check.sh` 新增计数棘轮：`#[path]`、`use super::*`，以及 jobs、应用、HTTP、存储、集成与 apps 各层的 `Err(_)` 与 `map_err(\|_\| …)` 数量只降不升（存储层 1430 处、集成层 359 处同样纳入，W0.8 的下降随之锁定）；基线文件随仓库提交，下降时同步更新 |
| W13.2 | workspace clippy `unwrap_used`、`expect_used` 设为 deny，测试代码豁免 |
| W13.3 | release profile 开启 `overflow-checks`：整数溢出转为任务 panic、失败关闭，不再静默回绕；评估对 `panic = "unwind"` 任务隔离与 Worker 常驻任务退出语义的影响；工作项执行中的 panic 计入该项尝试次数，超过上限转人工，不随租约到期无限重领 |
| W13.4 | 文档静态检查（M1）：Markdown 相对链接目标存在、同页锚点存在（不访问网络），并入 `just verify`；须先于 W5.4 让告警正文引用处置手册锚点 |
| W13.5 | 定时依赖审计（M1）：CI 每周对锁定依赖运行 cargo-audit，W7.1 后同时审计最近一个发布 tag；新公告以失败作业提示，不自动改依赖 |
| W13.6 | 依赖与工具链升级节奏（M6）：按发布冻结日历每季度安排一次升级窗口（Rust 工具链、Tokio LTS、Axum 与 tungstenite、sqlx、Dioxus），dependabot 只作提示；升级版本运行 W6 测试，涉及数据库或 HTTP 栈时加跑 W12.3 负载演练 |

验收：`just verify` 包含新检查；人为增加一处 `#[path]`、`unwrap()` 或断链时门禁失败；定时审计作业按周运行；下一次升级窗口写入运维手册的发布日历。回退：移除对应检查即可，不影响运行时（W13.3 可单独回退 profile）。

### W14 通知与提醒

目标：玩家在 MoogleMail、论坛私信与网站上都能及时、准确地知道发生了什么、需要做什么、截止何时；运营者只在需要处理时被提醒。约束：已冻结的场次策略与投递载荷不改写；结果未知不重发；出价被超提醒保持不合并、不限额（决策 1）；通知正文只用英文（决策 59）；不新增 Public 页面显示（决策 65）；外发邮件消耗邮资与账号请求预算，新增通知一律经 W1.3 调度并定义失效条件。

| 项 | 内容 | 关键约束 |
| --- | --- | --- |
| W14.1 通知目录与模板 | 在[邮件流程](../guide/mail-flows.md)建立通知目录：每种通知的触发事实、收件人、渠道、发送类别与出队位置、失效条件、模板键与冻结字段；出价被超（MM 与 PM）和论坛回执改由模板渲染（仍在创建时冻结正文），模板校验覆盖新键；新增占位符：lot 代码、装备名、世界与期号、有效截止、付款期限、网站链接（复用公开站点 origin 配置）、千分位金额 | 已冻结载荷原样发送，模板变化只影响新通知；正文不含代理上限与秘密（`register_key` 的登录 key 除外） |
| W14.2 内容修正 | 被超提醒写明 lot 代码、装备名、被超价格、当前有效截止与网站链接；COD 与载体付款邮件写明付款期限；卖方货款、载体与退款邮件写明 lot 与装备；寄回邮件区分流拍、撤拍与买家未付款后的退回；注册与换绑结果给出可读原因与下一步；慢速代理等待反击期间的提醒时机见[待决定](#7-决策记录) | 只改新投递；Sent 精确证明继续以冻结正文摘要匹配 |
| W14.3 新增提醒（决策 64） | 启用投稿接收回执（lot 代码、场次、起拍价，按原邮件合并）、卖方成交与流拍汇总（每卖家每场一封）、换季前 isekai 领取与交付提醒；不新增付款期限前提醒邮件（期限已写入付款邮件，W14.2）与标记装备开拍或即将截止提醒。每项定义失效条件（已开拍、已过换季边界等即在 sending 围栏前退役） | 新提醒不改变冻结策略；换季提醒按冻结的赛季边界计算 |
| W14.4 论坛 notify 改为订阅（D10，决策 60） | `notify` 改为订阅该 lot 的被超提醒：在导入事务内为回帖者标记该 lot，按账号提醒渠道发送，不再寄导入回执；标记按用户 × lot 唯一，天然去重；每帖与每账号每场的订阅数设上限，超出写入系统日志楼；同步[论坛](../guide/forum.md) | 稳定命令 ID 不变；重放不重复标记，也不补寄旧回执 |
| W14.5 自动标记与渠道前置（D11，决策 60） | 网站普通出价在出价事务内自动标记（与新建代理一致），写入纳入 W12.3 负载演练；提醒偏好响应增加 PM 可用性（论坛同步已启用且已绑定论坛账号），不可用时拒绝保存并说明原因 | 偏好 DTO 变化按 W7.6 的契约演进规则；同步[路由契约](../contracts/routes.md) |
| W14.6 付款提醒联动（D9，决策 62） | 逾期提醒的领取要求付款投递已有发送证明（自动或人工），否则延后，并触发 W5.5 的“付款邮件未寄出但已逾期” | 不改已冻结的 `payment_deadline_at`，不新增恢复策略版本，不影响退件与恢复流程 |
| W14.7（取消） | 不新增 Public 显示：本人成交与付款、本人寄售、站内通知列表、公告横幅以及页内被超提示均不做（决策 65） | — |
| W14.8 通知指标与邮资 | 按通知种类统计创建、退役、抑制、发送、失败与送达延迟；`send_rejected` 细分出邮资不足；按发送次数与 W0.12 核实的邮资估算消耗，供 W11.2 使用；非结算通知投递失败接入 W5.5 | 标签为闭集，不含收件人或正文 |
| W14.9 收件人异常 | 零资产通知遇收件人不存在直接失败关闭并记审计，不进入人工对账；发送前核对收件人仍是该用户当前名称，改名后退役并记原因 | 带资产的投递维持现有退信人工流程 |
| W14.10 文档 | 玩家指南新增“会收到的邮件与提醒”；纠正[邮件流程](../guide/mail-flows.md)中不存在的投稿接收回执；[业务规则](../business-rules.md)与玩家指南写明出价与新建代理自动标记、论坛 `notify` 为订阅被超提醒、PM 需绑定论坛账号；每项玩家可见变化给出 Admin“规则内容”五语言更新稿 | 随所在工作项交付 |

验收：通知目录覆盖全部出站种类，并与模板注册一致；新投递的被超提醒、付款与货款邮件不登录网站也能识别装备、金额与期限；论坛 `notify` 不再立即寄信，只建立有上限的订阅；网站普通出价后自动标记；选择 PM 而未绑定论坛账号时保存被拒并说明原因；付款邮件未寄出时不发逾期提醒且产生告警；W14.8 能分种类观察送达延迟与失败。回退：新正文可经 runtime 模板改回；新提醒、自动标记与订阅经 W7.7 运行时开关停用；通知事实只追加。

## 5. 推进顺序与里程碑

| 里程碑 | 内容 | 完成标志 |
| --- | --- | --- |
| M1 | W1.0、W10.1、W10.2、W7.1a、W0.1–W0.14、W7.5、W7.7、W7.8a、W7.11、W10.6、W13.1–W13.5 | 维护不再超时；定时备份与异机副本运行，数据密钥已离线托管，端到端恢复演练完成一次；`main` 成为部署基线，部署改用 CI 制品；错误、外部请求、HTTP、SLO、主机与数据库可度量且可采集；D1 原因可读，D5 冲突率有基线；就绪探针不再被瞬时错误永久翻转；待核实事实写入基线；Final 阶段无法误停机，停机不再打断越过围栏的外部写入；运行时开关可用；此后只含兼容迁移的版本可换回旧程序；迁移演练流程可用；回归门禁、链接检查与定时审计生效 |
| M2a | W0.15、W6.3、W6.6、W6.1、W6.2、W6.4、W6.5、W9.5、W9.7、W10.8、W10.7、W11.4 | 测试与安全底座：回放语料、外部服务替身、测试入口、规范与 CI 测试作业可用；影子运行框架就绪；Admin 登录防护补齐；CLI 操作者可追溯；PostgreSQL 有配置基线，WAL 归档可做 14 天内的时间点恢复；一致性巡检可作为后续改造的安全网 |
| M2b | W1.1–W1.3、W1.6、W1.7、W1.9–W1.11、W7.9、W7.10、W9.8、W9.10、W9.11、W14.8、W14.1、W14.2、W14.4–W14.6、W14.9 | 调度与玩家可见变化：熔断、统一调度与主动暂停生效，指标达标；Admin/CLI 不再直连账号；论坛增量导入消除页数悬崖；单角色故障不再拖垮 Public 与 Admin；发送节奏与预算无需重启即可调整；提醒按截止排序且截止后退役；玩家会话满额可自动腾出名额，玩家可自助轮换登录 key；通知正文能识别装备、金额与期限，论坛 notify 与说明信有上限，PM 渠道不再静默失效，逾期提醒不早于付款邮件 |
| M3 | W9.9、W3.7、W1.4、W1.5、W1.8、W2、W0.16、W7.6 | 服务改用无 DDL 的运行账号；时区无关证明先行；增量扫描消除 Sent 悬崖；处置模型落地，仪表盘无专用排除；浏览器错误可计数，部署后旧页面提示刷新 |
| M4 | W5.2–W5.5、W5.7、W5.8、W3.1–W3.3、W3.8、W4、W12.1、W12.3、W12.4、W12.5、W12.8、W14.3 | 告警来源齐全、分级合理、处置手册就绪，告警台可确认与静默；运维台闭环，可按 Reference 检索并查看玩家支持视图；核心准入上移；同场出价热点消除，读路径与关键循环受保护；投稿回执、卖方成交汇总与换季提醒上线 |
| M5 | W3 其余、W10.0、W10.3–W10.5、W10.9、W11.1–W11.3、W9.3、W9.4、W9.6、W12.6 | 恢复演练、清理与对账自动化，恢复后外部核对有工具，偿付能力可报告，密钥环与主机加固完成 |
| M6 | W7.1、W7.2、W7.3、W7.8、W12.2、W12.7、W13.6，最后 W8 | 发布流水线可用，兼容声明有测试验证，热路径按指标调整，依赖升级有固定节奏，最后完成部署形态与结构评估 |

里程碑内按表中顺序推进（决策 46、54）。M1 的前三项先行：W1.0（D2）、W10.1 与 W10.2（含 bucket lock、数据密钥托管与端到端恢复演练）各自完成即单独交付，W7.1a 只需用户完成远端设置；随后 W0.1 → W0.7（D1）、W0.14（D7，须先于 W0.6 的超时）、W7.11（D8），再完成 M1 其余各项。M2a 先交付 W6.3 与 W6.6，M2b 才开始依赖它们的 W1.1、W1.11；W10.8 先于 W10.7；W9.10 与 W9.8 同批，W9.11 与 W9.10 共用计数；M2b 中 W14.8 先于 W14.1，W14.1 先于 W14.2、W14.4。M3 以 W9.9 开头，先于 W2 新建的表。W5.1、W5.6 暂缓（决策 50），用户选定外部服务后排入当时的里程碑。

发布冻结（决策 57）：从赛季前场次开拍起，到首场早鸟场次结算完毕，只部署修复，不上线行为变更或含 `breaking` 迁移的版本；下一次为 2027-05-01 换季前后。W7.5 的时段门禁照常生效。

每个里程碑结束时停下来：汇报变更、门禁结果与部署建议，由用户决定是否部署。以下缺陷一经满足条件即单独交付，不等待里程碑：D2 的 W1.0 完成即交付；D1 修复一经定位；D3 若经 W0.12 核实临近上限，W1.4 的发送基线部分；D5 若 Final 阶段耗尽次数非零，W12.1；D6 若已有主题临近页数上限，W1.11 的增量导入部分；D10 若 W0.12 发现 `notify` 被大量使用或滥用，W14.4 的去重与上限。D6 现行语义的玩家可见说明（Admin 发布的五语言规则文档与 `docs/guide`）随 M1 先行。

## 6. 每个工作项的交付规范

1. 开始前 `git status --short`，确认基线分支，保留与任务无关的改动。
2. 先写清本项保护的不变量与契约变化（路由、JSON、事件、配置、数据库），以及验收标准与回退方式。
3. 改变运行行为的项带运行时开关并保留旧路径（W7.7 落地前用 TOML 开关，并在说明中注明切换需要重启、受 W7.5 约束）。判定或计数口径变化先影子运行（使用 W0.15 的共享框架）：差异以结构化日志与 `auction_shadow_mismatch_total{owner}`、`auction_shadow_evaluations_total{owner,branch}` 记录；切换条件为观察期覆盖至少一个包含 Final 与结算的完整场次周期、每个判定分支都有样本且差异为零（无样本的分支不算通过）；切换后在后续版本删除旧路径。
4. 实现后同步契约文档、模块 README、运维文档与配置示例；文档中的仓库路径以仓库根为起点，代码证据写符号名而非行号。玩家可见的变化除 `docs/guide` 外，还要在交付说明中给出 Admin“规则内容”五语言规则文档的更新稿，部署后由用户发布；界面文案同步五语言 `i18n/*.ftl`；新增或修改出站通知时同步 W14.1 的通知目录。
5. `cargo fmt --all` 与 `just verify` 通过；W6.5 落地后运行受影响范围的 `just test`/`just test-db`，涉及外部账号或论坛的行为先在 W6.6 替身上验证；涉及数据库的判断在本地 PostgreSQL 18 只读预览核对；含迁移的版本在交付说明中附 W10.6 迁移演练回执；已批准的测试（W6）随功能一起更新。
6. 提交标题 `type(scope): summary`，正文写不变量与验证范围；一个主题一个提交；W13 的计数基线随之下降时一并更新。
7. 在[进度](#8-进度)更新状态与提交号。
8. 用户部署后，按验收标准核对线上指标，在进度表记录部署版本与验收结论；未达标时按回退方式处理并记录原因。

## 7. 决策记录

2026-11-02 用户决策：

| # | 事项 | 决定 |
| --- | --- | --- |
| 1 | 出价被超提醒合并与限额 | 保持现状，只调整调度优先级（W1.7） |
| 2 | 行为测试 | 批准领域测试与 PG18 集成测试（W6.1、W6.2）；回放语料与 CI 测试作业待定（后由 #20、#28 批准） |
| 3 | 告警外部通道 | 先保留通道入口，具体通知方式后定（W5.1；后由 #16 确定） |
| 4 | Admin 暴露方式 | 不限制（取消 W9.1） |
| 5 | Admin 第二因素 | 不加（取消 W9.2） |
| 6 | 异机备份 | 加密上传 R2，30 份日备份 + 12 份月备份（W10.2） |
| 7 | 数据清理 | 只清理运行态数据；审计与资金事实永不删除；每次先演练并经批准（W10.4） |
| 8 | 分支与发布基线 | 建立 `main` 作为发布基线，只从 `main` 的 tag 发布（W7.1） |
| 9 | 预发与更名 | 现线上即正式环境，不建预发、不更名（取消 W7.4） |
| 10 | 规模收敛 | 只出决策记录，评估后再定（W8） |
| 11 | 二次审计补遗（v5） | 批准：W10.1/W10.2/W10.6 提前到 M1，W11.4 提前到 M2；新增 D5、D6、W13 及各工作流补遗 |
| 12 | W0.10 指标存储 | 本机 Prometheus，只监听 127.0.0.1，保留 15–30 天，经 SSH 隧道查看 |
| 13 | W10.2 备份加密与密钥托管 | `age` 公钥加密：服务器只放公钥，私钥离线保存（密码管理器 + 离线副本） |
| 14 | D4 出队优先级 | 运维告警 > 按 lot 有效截止排序的出价被超提醒 > 其他邮件；lot 截止后尚未发送的提醒退役（W1.3、W1.7） |
| 15 | D6 论坛出价时刻 | 维持按导入时刻判定并在玩家文档写明；Final 阶段加快导入（W1.11） |
| 16 | W5.1/W5.6 外部通道与存活检查 | 告警走通用 Webhook，具体服务作为配置填写；存活检查采用 Worker 定期向外部服务报到的心跳打卡（启用由 #50 暂缓） |
| 17 | W9.7 操作者身份 | 单人运营：Admin 维持共享身份；CLI 自动记录系统用户 |
| 18 | W10.0 WAL 归档 | 启用（W10.7）；RPO/RTO 与演练频率另行确认（后由 #31 确定） |
| 19 | W8 规模收敛 | 放在全部工作流之后评估 |
| 20 | W6.4 CI 测试作业 | 批准 |
| 21 | W12.3 负载演练 | 批准 |
| 22 | 线上部署形态 | 分体部署（W7.9 只处理分体；合并部署问题记入 W8.1） |
| 23 | W10.4 进程心跳 | 过期满 30 天后删除 |
| 24 | W10.4 实时事件 | 浏览器回补窗口 24 小时，超出则整页重读；事件保留 30 天；已解决的推送失败记录保留 90 天；序号与消费进度表永久保留 |
| 25 | W10.4 邮件诊断文件 | 一般诊断保留 7 天；写入结果未知的证据保留到对应投递处置完毕后 90 天 |
| 26 | 个人数据删除 | 不提供：数据本就公开可见；资金与审计事实永久保留 |
| 27 | W7.8 迁移兼容分级 | 批准：M6、W6.2 与 W6.4 就绪后实施；默认破坏性，只对纯新增迁移放宽（登记与 readiness 放宽由 #34 提前到 M1） |
| 28 | W6.3 回放语料 | 批准：去除 Cookie、登录 key 与表单 token，替换玩家名称，只保留解析所需结构 |
| 29 | W10.4 会话记录保留 | 已注销或已过期满 30 天的会话记录删除；有效会话（含永久会话）与登录 key 不受影响 |
| 30 | W9.8 会话上限 | 满额时注销最久未活动的会话并允许登录；并发上限由 5 扩大到 10 |
| 31 | W10.0 恢复目标 | RPO ≤ 5 分钟、RTO ≤ 2 小时、每月一次恢复演练 |
| 32 | D7 审计失败与就绪 | 只有不可重试的审计失败使进程未就绪；可重试或瞬时错误只进指标与告警；须先于 W0.6 的超时上线（W0.14） |
| 33 | D8 停机排空 | 已越过 sending 围栏的外部写入关键段在独立预算（约 60 秒）内完成，`TimeoutStopSec` 同步调整（W7.11，M1） |
| 34 | 二进制回退 | W7.8 的兼容登记与 readiness 放宽提前到 M1（W7.8a），默认 `breaking`；兼容声明的测试验证仍在 M6 |
| 35 | 数据库最小权限 | 拆分迁移 owner 与运行账号；三个服务先共用一个无 DDL 的运行账号，按角色再拆交 W8.2 评估（W9.9，M3 首项） |
| 36 | 论坛同名自动绑定 | 保留：E-Hentai 论坛名与 HV 角色名是同一命名空间，同名匹配不构成冒名风险 |
| 37 | 登录 key 自助轮换 | 新增邮件主题 `rekey`：轮换 key 并注销全部会话；`register` 仍重发原 key（W9.10，M2） |
| 38 | 故障补时 | 不提供：Final 期间 Worker、Public 或数据库故障只告警，不延长 lot 截止 |
| 39 | 运行参数热读取 | 发送节奏与预算类参数热读取，其余仍需重启（W7.10，M2） |
| 40 | W1.10 访问收口 | 经 Worker control 代为执行，不做 PostgreSQL 跨进程限速 |
| 41 | 备份打卡与 WAL | 备份、上传与归档的成功和失败向心跳服务打卡，提前到 M1；R2 必须启用 bucket lock，锁定期不长于生命周期淘汰期；每周一次基础备份、WAL 保留 14 天；WAL 先落本地暂存再上传（W10.1、W10.2、W10.7；打卡由 #50 暂缓，保留期由 #49 修订） |
| 42 | 主机与数据库指标 | 随 W0.10 采集 node_exporter 与 postgres_exporter，只监听 127.0.0.1 |
| 43 | CDN 与真实客户端地址 | W0.12 核实站点前是否有 CDN 代理；若有，W9.5 按 CDN 地址段取真实客户端地址 |
| 44 | 外部服务替身 | 批准本地 MoogleMail/论坛模拟服务，作为新增测试类别（W6.6，M2） |
| 45 | 浏览器错误与响应 DTO | 增加匿名、限流、只计数的浏览器错误信标（W0.16）；响应 DTO 保持 `deny_unknown_fields`，版本偏差由 W7.6 的刷新提示处理 |
| 46 | 里程碑调整 | W1.0（D2）与定时、异机备份作为 M1 最先单独交付；W13.4 提前到 M1，W6.3 提前到 M2；W7.1 拆出建立 `main` 与分支保护的 W7.1a 提前到 M1；W9.7 提前到 M2，先于 W2 |
| 47 | W7.5 保护范围 | 只在任一场次处于 Final，或任一在拍 lot 的有效截止落在保护窗口内（默认 60 分钟、可配置，且不短于实测停机时间的 3 倍）时拒绝停机；慢速代理只在其 lot 落入窗口时计入 |
| 48 | 数据密钥托管与恢复演练 | 三组数据密钥与 `age` 私钥一起离线托管，提前到 M1；W10.2 验收包含一次从 R2 下载、解密到本地 PG18 恢复的端到端演练 |
| 49 | 时间点恢复窗口 | 可恢复到过去 14 天内任一时刻：WAL 与基础备份保留 21 天（3 份周基础备份）；锁定期：日备份 30 天、月备份 365 天、WAL 与基础备份 21 天（修订 #41 的“WAL 保留 14 天”） |
| 50 | 外部心跳与告警服务 | 暂不启用：W5.1、W5.6 暂缓；备份与归档的打卡地址留空即不打卡，失败在 W5.5（M4）并入 MoogleMail 运维告警前只体现在回执、systemd 状态与日志中 |
| 51 | 部署制品 | `main` 建立后，部署使用 CI 在 `main` 上产出的候选制品并核对校验和，不再本地构建 |
| 52 | 旧分支 | 以 `refactor/dioxus-frontend` 的 HEAD 建立 `main`；已合入的 `master`、`audit-deps`、`ci/dependency-version-guard` 与 dependabot 分支删除；`tokio-1.54.1`、`tungstenite-0.30.0` 的 PR 关闭不合并 |
| 53 | `rekey` 与回信频率 | 主题名定为 `rekey`；`register` 与 `rekey` 每个账号 24 小时内各自只回信一次，超出的只记命令与审计 |
| 54 | M2 拆分 | 拆为 M2a（测试与安全底座）与 M2b（调度与玩家可见变化），中间停一次部署 |
| 55 | W1.5 退避上限 | 10 分钟（原 30 分钟） |
| 56 | SLO 目标值 | M1 结束时按线上基线提出数值，由用户审批后作为 W1、W12 的验收标准 |
| 57 | 换季发布冻结 | 从赛季前场次开拍到首场早鸟场次结算完毕，只部署修复，不上线行为变更或含 `breaking` 迁移的版本 |
| 58 | 运维告警现状 | 线上未启用（用户确认）；收件人问题随决策 50 一并暂缓，启用前告警只在 Admin 仪表盘与 W5.8 告警台可见 |
| 59 | 通知语言 | MoogleMail 与论坛私信通知只用英文，不保存玩家语言偏好 |
| 60 | 自动标记与论坛 `notify` | 网站普通出价自动标记；论坛 `notify` 改为订阅该 lot 的被超提醒并去重，不再寄导入回执（W14.4、W14.5） |
| 61 | 出队顺序 | 运维告警 > 账号类回信（登录 key、注册与换绑结果）> 按 lot 有效截止排序的出价被超提醒 > 其他邮件（修订决策 14，W1.3） |
| 62 | D9 处理深度 | 只把逾期提醒延后到付款邮件有发送证明，并告警；不改冻结期限、不新增恢复策略版本，按执行修正处理（W14.6） |
| 63 | 说明信预算 | 每个发件人或论坛账号每 24 小时最多 10 封说明信，超出只记录，附带资产照常退回（W9.11） |
| 64 | 新增提醒 | 启用投稿接收回执（按原邮件合并）、卖方成交与流拍汇总（每卖家每场一封）、换季前 isekai 领取提醒；不新增付款期限前提醒邮件（期限写入付款邮件，原建议的网站倒计时随决策 65 取消）与标记装备开拍或即将截止提醒（W14.3） |
| 65 | Public 新增显示 | 不需要：取消 W14.7（本人成交与付款、本人寄售、站内通知、公告横幅、页内被超提示） |
| 66 | 告警分级 | 紧急信号保留 30 分钟冷却；付款逾期、待重列等非紧急信号改为每日摘要；不单独发恢复通知，已恢复项列入下一封摘要（W5.2、W5.8） |

仍待决定（新的待决定事项出现时在此登记，并标注阻塞的工作项与最晚决定时间）：

| 事项 | 阻塞 | 最晚决定 |
| --- | --- | --- |
| W12.1 的序号热点方案（提交后由唯一分配者分配 / 事务前按场次排队 / 按 lot 拆分事件流），依 W0.6 的冲突数据选择 | W12.1 | M4 开工前；若 D5 触发提前交付，则在提前交付前 |
| SLO 目标值（出价 p99 延迟、冲突中止率、实时推送延迟、登录 key 邮件送达延迟、提醒距截止余量、Final 阶段回帖导入延迟），由我按 W0.13 基线提出（决策 56） | W1、W12 的验收 | M1 结束时 |
| 外部心跳与告警服务的选型与启用时间（决策 50），以及是否启用 MoogleMail 运维告警（决策 58，启用须先有 Bot 以外的收件角色） | W5.1、W5.6、备份打卡与 W5.5、W5.8 的主动通知；启用前没有任何主动告警，备份失败、Worker 崩溃与整机宕机都不会主动通知 | 无硬性期限 |
| 慢速代理的被超提醒时机：建议改为“持有人能够行动时才提醒”——慢速代理仍会自动应价时不立即提醒，代理耗尽（到期无法反超、进入 Final 时无法反超或等待期间被更高普通出价击败）时才提醒，反超成功则不提醒；实现上在被超时只记录待定提醒，按代理结果入队或失效；备选：维持立即提醒 | W14.2、W14.5 | M2b 开工前 |

### 用户前置动作

以下动作需要用户在对应工作项交付前完成；我负责给出命令或步骤与只读核验。

| 动作 | 服务的工作项 | 时间 |
| --- | --- | --- |
| R2：建立 bucket，按日/月备份、WAL、基础备份前缀配置生命周期规则与 bucket lock（日 30 天、月 365 天、WAL 与基础备份 21 天），签发只限该 bucket 的对象读写令牌并写入敏感 TOML | W10.2、W10.7 | M1（WAL 与基础备份前缀在 M2a） |
| 把三组数据密钥与 `age` 私钥一起离线托管（密码管理器 + 离线副本），并参加一次端到端恢复演练 | W10.2 | M1 |
| 离线生成 `age` 密钥对：私钥存密码管理器与一份离线副本，公钥交给服务器 | W10.2 | M1 |
| 选定心跳/Webhook 服务：建立备份与 WAL 打卡检查、Worker 心跳检查与 Public `/health/ready` 的 HTTP 检查，报到地址写入敏感配置 | W10.1、W10.7、W5.1、W5.6 | 暂缓（决策 50） |
| 主机安装 Prometheus、node_exporter、postgres_exporter（只监听 127.0.0.1），配置 SSH 隧道查看 | W0.10 | M1 |
| GitHub：建立 `main`、设为默认分支并开启分支保护（必须通过 CI）；删除已合入的旧分支，关闭 `tokio-1.54.1` 与 `tungstenite-0.30.0` 两个 dependabot PR | W7.1a | M1 |
| 提供 W0.12 线上只读核实所需的 SSH 与 psql 访问 | W0.12 | M1 |
| 应用 PostgreSQL 配置片段并按 W7.5 选择窗口重启 | W10.8、W10.7 | M2a |
| 建立运行账号并在部署时执行授权变更 | W9.9 | M3 |
| 在 Admin“规则内容”发布五语言规则更新 | D6、W1.7、W1.11、W9.8、W9.10、W9.11、W14 的玩家可见变化 | 随对应部署 |
| 启用 MoogleMail 运维告警时：准备 Bot 以外的 HV 角色作为告警收件人并写入配置 | W5.5、W5.8 | 暂缓（决策 58） |

线上运维事项（不属于本计划）：`fetch_timeout_seconds` 维持现状；xilixilixi 邮件为日常交流、与拍卖无关，不重试，待 W2.5 的“确认已处置”收口。

## 8. 进度

| 编号 | 状态 | 提交 | 部署 | 备注 |
| --- | --- | --- | --- | --- |
| D1 | 待诊断 | — | — | 依赖 W0.7 |
| D2 | 待处理 | — | — | W1.0；线上已部署 `92259475`，未包含修复 |
| D3 | 待核实 | — | — | W0.12 核实后决定是否提前交付 W1.4 发送基线 |
| D4 | 已决定 | — | — | 决策 14；W1.3、W1.7 随 M2b |
| D5 | 待量化 | — | — | W0.6 量化后决定是否提前交付 W12.1 |
| D6 | 已决定 | — | — | 决策 15；玩家可见说明随 M1；W0.12 核实主题页数后决定是否提前交付 W1.11 |
| D7 | 已决定 | — | — | 决策 32；W0.14 随 M1，先于 W0.6 |
| D8 | 已决定 | — | — | 决策 33；W7.11 随 M1 |
| D9 | 已决定 | — | — | 决策 62；W14.6 随 M2b |
| D10 | 已决定 | — | — | 决策 60；W14.4 随 M2b，W0.12 发现滥用则提前单独交付 |
| D11 | 待处理 | — | — | W14.5 随 M2b |
| 计划外 | 已提交 | `7ba074a5`、`f99a9d4e` | 未部署 | 履约证据预算与内部证据快照时效修复；按 D2 备注线上仍为 `92259475`，随下一次部署上线 |
| W0 | 未开始 | — | — | W0.1–W0.14 随 M1；W0.15 随 M2a；W0.16 随 M3 |
| W1 | 未开始 | — | — | W1.0 为 M1 首项（D2）；W1.10 方案已定（决策 40） |
| W2 | 未开始 | — | — | 随 M3，排在 W9.9 之后 |
| W3 | 未开始 | — | — | W3.7 随 M3 先行 |
| W4 | 未开始 | — | — | 含 W4.5、W4.6，随 M4 |
| W5 | 未开始 | — | — | W5.1、W5.6 暂缓（决策 50）；其余（含 W5.8）随 M4 |
| W6 | W6.1–W6.4、W6.6 已批准 | — | — | 全部随 M2a，W6.3、W6.6 先行 |
| W7 | 未开始 | — | — | W7.1a、W7.5、W7.7、W7.8a、W7.11 随 M1；W7.9、W7.10 随 M2b；W7.6 随 M3；W7.1、W7.2、W7.3、W7.8 随 M6 |
| W8 | 未开始 | — | — | 放在 M6 最后 |
| W9 | 未开始 | — | — | W9.5、W9.7 随 M2a；W9.8、W9.10、W9.11 随 M2b；W9.9 为 M3 首项 |
| W10 | 未开始 | — | — | W10.1、W10.2 为 M1 首批，W10.6 随 M1；W10.8、W10.7 随 M2a；W10.9 随 M5 |
| W11 | 未开始 | — | — | W11.4 随 M2a；W11.2 改为偿付能力报告（v11） |
| W12 | 未开始 | — | — | W12.3 已批准，随 M4；W12.8 随 M4 |
| W13 | 未开始 | — | — | W13.1–W13.5 全部随 M1；W13.6 随 M6 |
| W14 | 未开始 | — | — | W14.8、W14.1、W14.2、W14.4–W14.6、W14.9 随 M2b；W14.3 随 M4（决策 64）；W14.7 取消（决策 65）；W14.10 随各项交付；慢速代理提醒时机待决定 |

## 9. 术语

| 术语 | 含义 |
| --- | --- |
| D / W / M 编号 | D 为已知缺陷，W 为工作流（W0.1 为其中的工作项），M 为里程碑 |
| sending 围栏 | 外部写入前持久化的发送状态与 fencing 版本；越过后结果未知只能人工核对，不自动重发 |
| 目击（sighting） | 维护在 Sent 列表中观察到的投递引用，用于找回或核对发送结果 |
| legacy 自寄 | 旧实例遗留的发给 bot 自身的投递，需要专门证明后处置 |
| 预检退役 | 发送前预检发现提醒已失去资格，直接退役不发送 |
| 水位 | 已完整扫描到的最高远端邮件 ID；只用于缩小扫描范围，不作为发送或收取证明 |
| 论坛游标 | 已处理到的最大帖子 ID 及其所在页；只用于缩小扫描范围，不作为出价证明 |
| 通知目录 | 全部出站通知的种类、触发事实、收件人、渠道、出队位置、失效条件与模板键清单（W14.1），新增或修改通知时同步 |
| 邮资 | HV 对外发邮件收取的费用（金额与计费方式待 W0.12 核实）；余额不足时远端以 `cannot afford the postage` 拒绝发送 |
| 告警台 | Admin 中列出活动与历史告警、支持确认与静默的页面（W5.8） |
| 独占操作锁 | 同一凭据上 compose、资产动作与内部证据共用的进程内互斥锁 |
| 影子运行 | 新旧路径并行计算、只记录差异，不改变实际结果 |
| 运行时开关 | 持久化在数据库、无需重启即可切换的新旧路径选择；只选路径，不决定业务结果 |
| 迁移演练 | 用最新备份在隔离的 PostgreSQL 18 上预先执行新迁移并核对，作为无预发环境的替代 |
| 兼容登记 | 每个新迁移在自身事务内写入的 `compatible`/`breaking` 等级；旧程序据此判断自己不认识的版本能否就绪 |
| 心跳打卡 | 定期向外部服务报到，超时未报到即由外部服务告警（死人开关）；用于 Worker 存活、备份与 WAL 归档 |
| 关键段排空 | 停机时只等待已越过 sending 围栏的外部写入完成并落库，其他工作立即放弃、由租约重领 |
| 外部服务替身 | 由脱敏回放语料驱动的本地 MoogleMail/论坛模拟服务，只用于测试，绝不连接真实账号 |
| 棘轮门禁 | 只允许计数下降、不允许回涨的静态检查 |

业务流程详见[结算与履约](../guide/settlement.md)与[邮件流程](../guide/mail-flows.md)。

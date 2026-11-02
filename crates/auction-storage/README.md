# auction-storage

[文档导航](../../docs/README.md) · [数据库契约](../../docs/contracts/database.md) · [迁移](../../migrations/postgres/README.md)

PostgreSQL 适配器：实现应用层的仓储、事务与命令端口，拥有全部 SQL、迁移执行、连接池与运维数据库能力（建库、预检、盘点、重建）。唯一支持的数据库是 PostgreSQL 18。

## feature

| feature | 作用 |
| --- | --- |
| （默认无） | 只编译 `DatabaseBackend`、部署目标身份与错误类型；应用以“无数据库骨架”启动 |
| `postgres` | SQLx PostgreSQL 驱动（rustls + webpki 根证书）与全部实现 |

## 对外入口

| 入口 | 用途 |
| --- | --- |
| `DatabaseStore::connect(url)` | 建立连接池（16 连接，带获取追踪与 Prometheus 指标源）；`DatabaseStore` 为大部分端口实现者 |
| `DatabaseStore::migrate()` | 在 SQLx 迁移锁内执行内嵌迁移；空库或合法历史才允许，未知非空库拒绝 |
| `ProcessReadinessPort` | readiness：迁移历史等于当前二进制期望版本且无审计写入失败 |
| `Database*Commands` | 竞价、生命周期、结算、Admin 拍卖/录入/lot/邮件/规则/runtime/世界等命令组合器 |
| `initialize_empty_postgres` | 合并部署首次启动：确认专用空库且无其他客户端后在迁移锁内建 schema |
| `PostgresDatabaseBootstrap` | 维护库连接上的“缺失才创建”建库（`auction-cli db create`） |
| `PostgresDatabaseRebuildGuard` | 持锁预检与 DROP/CREATE 同定义空库（`auction-cli db rebuild`） |
| `PostgresStore::collect_recovery_preflight` / `collect_retention_inventory` | 只读恢复预检与保留盘点 |
| `strict_deployment_database_identity` | 不连接数据库比较部署目标（主机/socket、端口、库名），拒绝隐式目标与 `options` |
| `postgres_connect_options` | `postgres` feature 下供连接装配与 CLI 目标/libpq 投影共用的 SQLx 选项解析；固定默认端口，保留驱动的 host/hostaddr 查询顺序与 socket 优先级，不连接数据库，错误脱敏 |
| `postgres_expected_migration_version` | 当前二进制内嵌的最高迁移版本 |

## 实现约定

- **事务**：竞价、关键生命周期等命令在 Serializable 事务内完成，序列化失败（40001）与死锁（40P01）由命令 owner 有界重试（`is_serialization_conflict` 是统一判定）。竞价、结算、规则与 Admin 邮件经各自的 `TransactionRunner` 按 `TransactionPolicy` 重试；其余独立 Serializable 写入共用 `retry_conflicts`（默认 `SERIALIZABLE_ATTEMPTS` 次，每次重开事务，只重跑 owner 判定的明确中止，结果未知的提交不重试）。运行期 claim、进度及部分账本写入使用默认隔离事务，并按各自 owner 取行锁或校验租约/fencing；不能把所有写入都视为 Serializable。同一幂等命令 ID 通过事务级 advisory 锁协调（`lock_command_id` 共用键空间）。
- **幂等账本**：各 owner 的命令表保存命令 ID、请求摘要与结果；同 ID 同摘要重放原结果，异摘要冲突。
- **追加式事实**：竞价、代理转换、黑名单历史、审计与投递尝试证明等由触发器拒绝改写；结算账本按 owner 追加。改名保留原名称证据，撤销时更新 `reverted_at`，不能把整张改名表视为不可更新。审计 `INSERT` 失败会让整个事务回滚，进程内另记失败计数，使 readiness 失败。
- **冻结竞价策略**：锁内快照和账户代理降额能力按同一场次策略版本解码，未知版本失败关闭；公开普通最低价继续使用领域计算。新旧策略与追加迁移边界见[数据库契约](../../docs/contracts/database.md#拍卖与竞价)。
- **租约与 fencing**：Worker 的拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、告警批次都以“claim + 租约 + fencing 版本”推进，过期租约可被重新领取，发送中的外部写入以 fencing 防止重复。
- **恢复义务锁**：人工确认 recovery 时，用单条 `ANY` 查询按 UUID 升序锁定该 case 的 3～4 条义务，保持既有 owner 锁序；返回集合必须完整，且逐条核对拍卖、lot、世界和装备身份后才能推进，缺失义务继续拒绝确认。
- **装备损失 owner**：`database/equipment_loss` 实现 `SettlementEquipmentLossPort`；首次唯一库存读取失败才能通过正式命令终结原 COD，损失事实/原版本/人工声明永久追加。复用资金 owner 的 `record_operator_equipment_loss` 与既有补偿分配，释放正常卖家付款队列，不伪造实收或补偿完成。CAS、统一锁序、三次 Serializable 冲突重试与退休投递禁止复活详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递)。
- **人工 Sent owner**：`database/sent_confirmation` 实现 `DeliverySentConfirmationPort`；追加独立 operator 观察及责任声明，与原冻结发送尝试、唯一 MID 和当前 fence 绑定。装备继续经共享强证明谓词和原履约恢复 helper 取得真实 Sent collected/资金化/原金额卖方付款。现金只允许 main 已收款 lot 的原卖方正额货款（已领取须匹配原生金额回执并显式接受），精确绑定原 UID/lot/义务/投递/金额、`unknown` seller 分配及预留命令/分录；同事务复用自动恢复的 `finish_funded_delivery(Sent)` 记为 `spent`，同 ID 重放不重复支出，不创建来源或收款。被 Admin 对账判失败、但 Sent 证明已寄出的投递以追加 confirmation 更正（迁移 `000074` 扩展强谓词），原失败命令与审计不改写。不伪造 `sent_verified`，不重发邮件，原 Admin 手填成功仍拒绝。准入与分钟时窗见[结算指南](../../docs/guide/settlement.md#sent-人工证据确认)，永久保留事实见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递)。
- **运行时独占**：外部 Cookie 的运行时 owner（邮件、论坛各一）和 Public realtime consumer 以 PostgreSQL session advisory lock 实现，连接断开即失去所有权；所有权失效后持有方必须退出。
- **只读快照**：报表与运维读取使用可重复读只读事务，运维快照设置 5 秒语句与 1 秒锁等待上限。
- **Public 会话活动**：普通活动不延长期限，`last_activity_at` 按 60 秒节流写入；需要活动写入时，在语句完成后复用独立只读查询重新认证。行锁等待可能跨过期限或撤销，仅靠等待前的物化快照或 `UPDATE WHERE clock_timestamp()` 不能证明等待后的身份仍有效；无需活动写入的快路保持单条查询且不写会话行。
- **收件目标**：Admin 录入与邮件入库共用 `database/intake_target`：在已持有 world 锁后锁定收件目标与拍卖、校验一致性、按锁定版本 CAS 推进预览修订并写入装备 key；错误分类仍由各自 owner 决定。
- **邮件入库编号**：持有原 world/目标拍卖锁，在同一 Serializable 事务中按分类聚合已有编号，同时归并全局 lot 序号；空场次仍从 1 开始，分类编号各自递增，溢出和损坏分类快照继续失败关闭。
- **赛季收件边界**：新场次冻结的 `intake_opens_at` 同时约束锁后数据库实时钟和邮件原始 `received_at`；任一早于边界都返回 `IntakeClosed`，延迟轮询不能把旧赛季邮件加入新赛季。等于边界允许继续，历史 NULL 不增加限制；已持久接受命令仍先重放原决定。Admin 录入复核锁后当前时刻；恢复重列在既有 world/auction/recovery 等锁和证明复核后取 `clock_timestamp()`，不能使用事务开始时间或 Worker 选候选时的旧时刻替代换季判断。旧赛季识别同时保留原场次开拍和原始投稿收件事实，以二者较早时刻为起点，避免改期开拍绕过换季阻断。
- **错误**：对外只返回固定分类文本，不携带 SQL、连接串或驱动原文。
- **连接目标**：连接前固定显式端口（省略为 5432），拒绝未知查询参数；IPv6 主机名去括号后重新匹配 pgpass。

## 迁移兼容层

迁移文件位于 [`migrations/postgres/`](../../migrations/postgres)，编译期经 `sqlx::migrate!` 内嵌。[`crates/auction-storage/src/database/migrations/`](src/database/migrations) 为少数历史迁移提供“执行视图”：保留文件原 checksum，只在执行时对固定版本做限定替换（`000005`/`000015` 给保留字别名改名，`000044` 先立即执行延迟约束，`000047` 给保留字列名加引号）。仅 `000005`/`000015` 额外接受已部署的固定别名修正版 checksum；`000044`/`000047` 只接受原文件 checksum，其他差异均失败关闭。已发布的迁移文件永远不能修改。

## 源码结构

[`crates/auction-storage/src/database/`](src/database) 按功能 owner 划分子模块（每个模块的 `//!` 注释说明其边界），主要分组：

| 分组 | 模块 |
| --- | --- |
| 基础 | [`crates/auction-storage/src/database/store.rs`](src/database/store.rs)、[`crates/auction-storage/src/database/postgres_store.rs`](src/database/postgres_store.rs)、[`crates/auction-storage/src/database/pool/`](src/database/pool)、[`crates/auction-storage/src/database/backend.rs`](src/database/backend.rs)、[`crates/auction-storage/src/database/migrations/`](src/database/migrations)、[`crates/auction-storage/src/database/audit_health/`](src/database/audit_health) |
| 竞价与拍卖 | [`crates/auction-storage/src/database/competition/`](src/database/competition)、[`crates/auction-storage/src/database/auctions/`](src/database/auctions)、[`crates/auction-storage/src/database/lots/`](src/database/lots)、[`crates/auction-storage/src/database/bid_log/`](src/database/bid_log)、[`crates/auction-storage/src/database/price_history/`](src/database/price_history)、[`crates/auction-storage/src/database/slow_proxy_due.rs`](src/database/slow_proxy_due.rs)、[`crates/auction-storage/src/database/scheduler/`](src/database/scheduler)、[`crates/auction-storage/src/database/commands/`](src/database/commands) |
| 结算与资金 | [`crates/auction-storage/src/database/settlement/`](src/database/settlement)、[`crates/auction-storage/src/database/settlement_commands/`](src/database/settlement_commands)、[`crates/auction-storage/src/database/settlement_scheduler/`](src/database/settlement_scheduler)、[`crates/auction-storage/src/database/settlement_policy/`](src/database/settlement_policy)、[`crates/auction-storage/src/database/settlement_report/`](src/database/settlement_report)、[`crates/auction-storage/src/database/settlement_catalog/`](src/database/settlement_catalog)、[`crates/auction-storage/src/database/settlement_funding/`](src/database/settlement_funding)、[`crates/auction-storage/src/database/legacy_self_proof/`](src/database/legacy_self_proof)、[`crates/auction-storage/src/database/compensation/`](src/database/compensation)、[`crates/auction-storage/src/database/compensation_reversal/`](src/database/compensation_reversal) |
| 履约与投递 | [`crates/auction-storage/src/database/fulfillment_scheduler/`](src/database/fulfillment_scheduler)、[`crates/auction-storage/src/database/delivery_scheduler/`](src/database/delivery_scheduler)、[`crates/auction-storage/src/database/mail_delivery/`](src/database/mail_delivery)、[`crates/auction-storage/src/database/delivery_admin/`](src/database/delivery_admin)、[`crates/auction-storage/src/database/delivery_audit/`](src/database/delivery_audit) |
| 恢复 | [`crates/auction-storage/src/database/recovery_scheduler/`](src/database/recovery_scheduler)、[`crates/auction-storage/src/database/recovery_commands/`](src/database/recovery_commands)、[`crates/auction-storage/src/database/recovery_report/`](src/database/recovery_report)、[`crates/auction-storage/src/database/recovery_health.rs`](src/database/recovery_health.rs) |
| 邮件 | [`crates/auction-storage/src/database/mail_poll/`](src/database/mail_poll)、[`crates/auction-storage/src/database/mail_intake/`](src/database/mail_intake)、[`crates/auction-storage/src/database/intake_target/`](src/database/intake_target)、[`crates/auction-storage/src/database/mailbox_sweep/`](src/database/mailbox_sweep)、[`crates/auction-storage/src/database/admin_mail/`](src/database/admin_mail) |
| 论坛与关注 | [`crates/auction-storage/src/database/forum/`](src/database/forum)、[`crates/auction-storage/src/database/lot_watch/`](src/database/lot_watch)、[`crates/auction-storage/src/database/lot_watch_pm/`](src/database/lot_watch_pm) |
| 用户与会话 | [`crates/auction-storage/src/database/users/`](src/database/users)、[`crates/auction-storage/src/database/sessions/`](src/database/sessions)、[`crates/auction-storage/src/database/credentials/`](src/database/credentials)、[`crates/auction-storage/src/database/account/`](src/database/account)、[`crates/auction-storage/src/database/blacklist/`](src/database/blacklist)、[`crates/auction-storage/src/database/admin_renames/`](src/database/admin_renames) |
| Admin 读取 | [`crates/auction-storage/src/database/admin_auctions/`](src/database/admin_auctions)、[`crates/auction-storage/src/database/admin_lots/`](src/database/admin_lots)、[`crates/auction-storage/src/database/admin_audit/`](src/database/admin_audit)、[`crates/auction-storage/src/database/admin_proxy_events/`](src/database/admin_proxy_events)、[`crates/auction-storage/src/database/admin_intake/`](src/database/admin_intake) |
| 实时与运维 | [`crates/auction-storage/src/database/realtime/`](src/database/realtime)、[`crates/auction-storage/src/database/realtime_consumer/`](src/database/realtime_consumer)、[`crates/auction-storage/src/database/realtime_admin/`](src/database/realtime_admin)、[`crates/auction-storage/src/database/operations_alert/`](src/database/operations_alert)、[`crates/auction-storage/src/database/health/`](src/database/health) |
| 运维数据库 | [`crates/auction-storage/src/database/initialization/`](src/database/initialization)、[`crates/auction-storage/src/database/bootstrap/`](src/database/bootstrap)、[`crates/auction-storage/src/database/rebuild/`](src/database/rebuild)、[`crates/auction-storage/src/database/recovery_preflight/`](src/database/recovery_preflight)、[`crates/auction-storage/src/database/retention_inventory/`](src/database/retention_inventory) |
| 其他 | [`crates/auction-storage/src/database/rules/`](src/database/rules)、[`crates/auction-storage/src/database/worlds/`](src/database/worlds)、[`crates/auction-storage/src/database/equipment_secrets.rs`](src/database/equipment_secrets.rs)、[`crates/auction-storage/src/database/public_equipment_link.rs`](src/database/public_equipment_link.rs) |

[`crates/auction-storage/src/deployment_identity/`](src/deployment_identity) 拥有连接参数规范化与部署目标比较。

## 维护约定

- SQL 只出现在本 crate；新增表或约束必须新增迁移文件，并同步[数据库契约](../../docs/contracts/database.md)与[保留政策](../../ops/deploy/retention-policy.v1.json)（`database_schema_version` 须等于最高迁移版本，否则 `db inventory` 拒绝运行）。
- 动态拼接 SQL 只允许编译期常量片段，业务值一律 `bind`；同一 owner 内多条查询共享的投影或过滤以字面量宏定义，经 `concat!` 组合成静态查询（如 lot 列表/详情、竞价日志、价格历史、账户代理），不在运行时拼接；DDL 标识符经 `quote_identifier`，库名先过 `valid_ddl_database_name`（ASCII 字母数字与 `_-.`）。
- 新增 `pub` 接口前确认存在调用方，编译器不会对跨 crate 未使用的 `pub` 项告警。

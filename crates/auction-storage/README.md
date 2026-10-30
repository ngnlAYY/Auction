# auction-storage

[文档导航](docs/README.md) · [数据库契约](docs/contracts/database.md) · [迁移](migrations/postgres/README.md)

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
| `postgres_expected_migration_version` | 当前二进制内嵌的最高迁移版本 |

## 实现约定

- **事务**：写命令在 Serializable 事务内完成，序列化失败（40001）与死锁（40P01）由命令 owner 有界重试（`is_serialization_conflict` 是唯一判定）。同一命令 ID 的并发入口先取事务级 advisory 锁（`lock_command_id`，全仓共用键空间）。
- **幂等账本**：各 owner 的命令表保存命令 ID、请求摘要与结果；同 ID 同摘要重放原结果，异摘要冲突。
- **追加式事实**：竞价、代理转换、结算账本、改名、黑名单历史、审计等只追加不改写；审计 `INSERT` 失败会让整个事务回滚，进程内另记失败计数，使 readiness 失败。
- **租约与 fencing**：Worker 的拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、告警批次都以“claim + 租约 + fencing 版本”推进，过期租约可被重新领取，发送中的外部写入以 fencing 防止重复。
- **运行时独占**：外部 Cookie 的运行时 owner（邮件、论坛各一）和 Public realtime consumer 以 PostgreSQL session advisory lock 实现，连接断开即失去所有权；所有权失效后持有方必须退出。
- **只读快照**：报表与运维读取使用可重复读只读事务，运维快照设置 5 秒语句与 1 秒锁等待上限。
- **错误**：对外只返回固定分类文本，不携带 SQL、连接串或驱动原文。
- **连接目标**：连接前固定显式端口（省略为 5432），拒绝未知查询参数；IPv6 主机名去括号后重新匹配 pgpass。

## 迁移兼容层

迁移文件位于 [`migrations/postgres/`](migrations/postgres)，编译期经 `sqlx::migrate!` 内嵌。[`crates/auction-storage/src/database/migrations/`](crates/auction-storage/src/database/migrations) 为少数历史迁移提供“执行视图”：保留文件原 checksum，只在执行时对固定版本做限定替换（`000005`/`000015` 给保留字别名改名，`000044` 先立即执行延迟约束，`000047` 给保留字列名加引号）；已用早期修正版执行过的数据库按记录的 checksum 匹配，其他任何差异都按版本不匹配失败。已发布的迁移文件永远不能修改。

## 源码结构

[`crates/auction-storage/src/database/`](crates/auction-storage/src/database) 按功能 owner 划分子模块（每个模块的 `//!` 注释说明其边界），主要分组：

| 分组 | 模块 |
| --- | --- |
| 基础 | [`crates/auction-storage/src/database/store.rs`](crates/auction-storage/src/database/store.rs)、[`crates/auction-storage/src/database/postgres_store.rs`](crates/auction-storage/src/database/postgres_store.rs)、[`crates/auction-storage/src/database/pool/`](crates/auction-storage/src/database/pool)、[`crates/auction-storage/src/database/backend.rs`](crates/auction-storage/src/database/backend.rs)、[`crates/auction-storage/src/database/migrations/`](crates/auction-storage/src/database/migrations)、[`crates/auction-storage/src/database/audit_health/`](crates/auction-storage/src/database/audit_health) |
| 竞价与拍卖 | [`crates/auction-storage/src/database/competition/`](crates/auction-storage/src/database/competition)、[`crates/auction-storage/src/database/auctions/`](crates/auction-storage/src/database/auctions)、[`crates/auction-storage/src/database/lots/`](crates/auction-storage/src/database/lots)、[`crates/auction-storage/src/database/bid_log/`](crates/auction-storage/src/database/bid_log)、[`crates/auction-storage/src/database/price_history/`](crates/auction-storage/src/database/price_history)、[`crates/auction-storage/src/database/slow_proxy_due.rs`](crates/auction-storage/src/database/slow_proxy_due.rs)、[`crates/auction-storage/src/database/scheduler/`](crates/auction-storage/src/database/scheduler)、[`crates/auction-storage/src/database/commands/`](crates/auction-storage/src/database/commands) |
| 结算与资金 | [`crates/auction-storage/src/database/settlement/`](crates/auction-storage/src/database/settlement)、[`crates/auction-storage/src/database/settlement_commands/`](crates/auction-storage/src/database/settlement_commands)、[`crates/auction-storage/src/database/settlement_scheduler/`](crates/auction-storage/src/database/settlement_scheduler)、[`crates/auction-storage/src/database/settlement_policy/`](crates/auction-storage/src/database/settlement_policy)、[`crates/auction-storage/src/database/settlement_report/`](crates/auction-storage/src/database/settlement_report)、[`crates/auction-storage/src/database/settlement_catalog/`](crates/auction-storage/src/database/settlement_catalog)、[`crates/auction-storage/src/database/settlement_funding/`](crates/auction-storage/src/database/settlement_funding)、[`crates/auction-storage/src/database/legacy_self_proof/`](crates/auction-storage/src/database/legacy_self_proof)、[`crates/auction-storage/src/database/compensation/`](crates/auction-storage/src/database/compensation) |
| 履约与投递 | [`crates/auction-storage/src/database/fulfillment_scheduler/`](crates/auction-storage/src/database/fulfillment_scheduler)、[`crates/auction-storage/src/database/delivery_scheduler/`](crates/auction-storage/src/database/delivery_scheduler)、[`crates/auction-storage/src/database/mail_delivery/`](crates/auction-storage/src/database/mail_delivery)、[`crates/auction-storage/src/database/delivery_admin/`](crates/auction-storage/src/database/delivery_admin)、[`crates/auction-storage/src/database/delivery_audit/`](crates/auction-storage/src/database/delivery_audit) |
| 恢复 | [`crates/auction-storage/src/database/recovery_scheduler/`](crates/auction-storage/src/database/recovery_scheduler)、[`crates/auction-storage/src/database/recovery_commands/`](crates/auction-storage/src/database/recovery_commands)、[`crates/auction-storage/src/database/recovery_report/`](crates/auction-storage/src/database/recovery_report)、[`crates/auction-storage/src/database/recovery_health.rs`](crates/auction-storage/src/database/recovery_health.rs) |
| 邮件 | [`crates/auction-storage/src/database/mail_poll/`](crates/auction-storage/src/database/mail_poll)、[`crates/auction-storage/src/database/mail_intake/`](crates/auction-storage/src/database/mail_intake)、[`crates/auction-storage/src/database/mailbox_sweep/`](crates/auction-storage/src/database/mailbox_sweep)、[`crates/auction-storage/src/database/admin_mail/`](crates/auction-storage/src/database/admin_mail) |
| 论坛与关注 | [`crates/auction-storage/src/database/forum/`](crates/auction-storage/src/database/forum)、[`crates/auction-storage/src/database/lot_watch/`](crates/auction-storage/src/database/lot_watch)、[`crates/auction-storage/src/database/lot_watch_pm/`](crates/auction-storage/src/database/lot_watch_pm) |
| 用户与会话 | [`crates/auction-storage/src/database/users/`](crates/auction-storage/src/database/users)、[`crates/auction-storage/src/database/sessions/`](crates/auction-storage/src/database/sessions)、[`crates/auction-storage/src/database/credentials/`](crates/auction-storage/src/database/credentials)、[`crates/auction-storage/src/database/account/`](crates/auction-storage/src/database/account)、[`crates/auction-storage/src/database/blacklist/`](crates/auction-storage/src/database/blacklist)、[`crates/auction-storage/src/database/admin_renames/`](crates/auction-storage/src/database/admin_renames) |
| Admin 读取 | [`crates/auction-storage/src/database/admin_auctions/`](crates/auction-storage/src/database/admin_auctions)、[`crates/auction-storage/src/database/admin_lots/`](crates/auction-storage/src/database/admin_lots)、[`crates/auction-storage/src/database/admin_audit/`](crates/auction-storage/src/database/admin_audit)、[`crates/auction-storage/src/database/admin_proxy_events/`](crates/auction-storage/src/database/admin_proxy_events)、[`crates/auction-storage/src/database/admin_intake/`](crates/auction-storage/src/database/admin_intake) |
| 实时与运维 | [`crates/auction-storage/src/database/realtime/`](crates/auction-storage/src/database/realtime)、[`crates/auction-storage/src/database/realtime_consumer/`](crates/auction-storage/src/database/realtime_consumer)、[`crates/auction-storage/src/database/realtime_admin/`](crates/auction-storage/src/database/realtime_admin)、[`crates/auction-storage/src/database/operations_alert/`](crates/auction-storage/src/database/operations_alert)、[`crates/auction-storage/src/database/health/`](crates/auction-storage/src/database/health) |
| 运维数据库 | [`crates/auction-storage/src/database/initialization/`](crates/auction-storage/src/database/initialization)、[`crates/auction-storage/src/database/bootstrap/`](crates/auction-storage/src/database/bootstrap)、[`crates/auction-storage/src/database/rebuild/`](crates/auction-storage/src/database/rebuild)、[`crates/auction-storage/src/database/recovery_preflight/`](crates/auction-storage/src/database/recovery_preflight)、[`crates/auction-storage/src/database/retention_inventory/`](crates/auction-storage/src/database/retention_inventory) |
| 其他 | [`crates/auction-storage/src/database/rules/`](crates/auction-storage/src/database/rules)、[`crates/auction-storage/src/database/worlds/`](crates/auction-storage/src/database/worlds)、[`crates/auction-storage/src/database/equipment_secrets.rs`](crates/auction-storage/src/database/equipment_secrets.rs)、[`crates/auction-storage/src/database/public_equipment_link.rs`](crates/auction-storage/src/database/public_equipment_link.rs) |

[`crates/auction-storage/src/deployment_identity/`](crates/auction-storage/src/deployment_identity) 拥有连接参数规范化与部署目标比较。

## 维护约定

- SQL 只出现在本 crate；新增表或约束必须新增迁移文件，并同步[数据库契约](docs/contracts/database.md)与[保留政策](ops/deploy/retention-policy.v1.json)（`database_schema_version` 须等于最高迁移版本，否则 `db inventory` 拒绝运行）。
- 动态拼接 SQL 只允许编译期常量片段，业务值一律 `bind`；DDL 标识符经 `quote_identifier`，库名先过 `valid_ddl_database_name`（ASCII 字母数字与 `_-.`）。
- 新增 `pub` 接口前确认存在调用方，编译器不会对跨 crate 未使用的 `pub` 项告警。

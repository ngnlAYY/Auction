# auction-storage

[docs/README.md](../../docs/README.md) · [docs/contracts/database.md](../../docs/contracts/database.md) · [migrations/postgres/README.md](../../migrations/postgres/README.md)

PostgreSQL 18 适配器：实现应用层的仓储、事务与命令端口，拥有全部 SQL、迁移执行、连接池与运维数据库能力（建库、预检、盘点、重建）。

## feature

| feature | 作用 |
| --- | --- |
| `test-db` | PostgreSQL 18 集成测试夹具（临时集群、已迁移模板库与逐测试克隆库）；只在 `just test-db` 启用 |

## 对外入口

| 入口 | 用途 |
| --- | --- |
| `DatabaseStore::connect(url, profile)` / `migrate()` | 显式连接用途与池策略；在 SQLx 迁移锁内执行内嵌迁移，未知非空库拒绝 |
| `ProcessReadinessPort` | readiness：按迁移模式核对历史，再按审计模式通过门禁，见[迁移就绪](#迁移就绪)与[审计失败与就绪](#审计失败与就绪) |
| `Database*Commands` | 竞价、生命周期、结算、Admin 拍卖/录入/lot/邮件/规则/runtime/世界等命令组合器 |
| `initialize_empty_postgres` / `PostgresDatabaseBootstrap` / `PostgresDatabaseRebuildGuard` | 合并部署首次初始化、缺失才创建的建库、持锁重建空库 |
| `collect_recovery_preflight` / `collect_retention_inventory` / `collect_retention_dry_run`；`preflight_retention_purge` / `execute_retention_purge` | 只读预检与盘点；经演练回执与逐族令牌批准的运行态清理（只覆盖代码内闭集表） |
| `strict_deployment_database_identity` / `postgres_connect_options` / `postgres_expected_migration_version` | 不连接数据库的部署目标比较；连接选项解析；内嵌最高迁移版本 |

## 单一适配器结构

`DatabaseStore` 是唯一的 PostgreSQL 适配器：持有 `TrackedPool`、事务观测、`RuntimeSwitches`、发送节奏、影子/迁移记录器与按场次竞价队列等共享状态；各功能 owner 只以 `impl <端口 trait> for DatabaseStore` 提供实现，SQL 与锁序留在对应 `database/<owner>/` 目录。装配根 `connect` 一次后 `clone` 共享池；需要独立观测归属的路径用 `for_transaction(owner)`。

## 实现约定

- **事务**：竞价与关键生命周期在 Serializable 事务内完成，40001/40P01 由 owner 有界重试（`is_serialization_conflict` 统一判定）；独立 Serializable 写入共用 `retry_conflicts`（默认 `SERIALIZABLE_ATTEMPTS` 次，每次重开事务，只重跑明确中止，结果未知的提交不重试）。claim、进度与部分账本写入用默认隔离事务并按 owner 取行锁或校验租约/fencing；同一命令 ID 经事务级 advisory 锁协调（`lock_command_id`）。
- **场次写队列**：会写 `lots`/`auctions` 的场次级命令在进入 Serializable 事务前按场次排队（进程内互斥 + 会话 advisory lock）：只知道 lot 的用 `run_in_auction_lot_queue`，已知场次的用 `run_in_auction_queue`；两者必须落到同一把门。排队预算用尽返回可重试的 `QueueWaitTimeout`，取消/panic 由凭证 `Drop` 归还，绝不把可能仍持锁的连接放回池。
- **幂等账本与追加事实**：命令表保存命令 ID、请求摘要与结果；同 ID 同摘要重放，异摘要冲突。竞价、代理转换、黑名单历史、审计与投递尝试证明由触发器拒绝改写；审计 `INSERT` 失败仍传播并回滚，进程内另记分类失败数。
- **冻结策略与租约**：锁内快照按同一场次策略版本解码，未知版本失败关闭；Worker 的调度、结算、履约、投递、恢复、轮询与告警批次以 claim + 租约 + fencing 推进。
- **同快照比对**：告警读侧的新旧口径在同一语句级快照上配对读取（`operations_alert/snapshot/disposition.rs`）；影子模式只记录差异，不改变告警结果。
- **恢复与人工证据**：人工确认 recovery 用单条 `ANY` 查询按 UUID 升序锁定义务并逐条核对身份；`SettlementEquipmentLossPort`（`database/equipment_loss`）与 `DeliverySentConfirmationPort`（`database/sent_confirmation`）追加独立事实、复用共享强证明与原履约恢复 helper，不伪造实收或 `sent_verified`。契约见[docs/contracts/database.md#结算资金恢复与投递](../../docs/contracts/database.md#结算资金恢复与投递)与[docs/guide/settlement.md#sent-人工证据确认](../../docs/guide/settlement.md#sent-人工证据确认)。
- **规则上移**：Admin 投递动作、COD 实收与履约 claim 一致性以“存储按锁序读取 → 应用层类型化快照 `evaluate` → 闭集阻断项”判定；SQL 只保留关系完整性与不可绕过的约束。
- **运行时独占与只读快照**：外部 Cookie owner（邮件、论坛各一）与 Public realtime consumer 用 PostgreSQL session advisory lock，连接断开即失去所有权。报表与运维读取用可重复读只读事务，运维快照设 5 秒语句与 1 秒锁等待上限。
- **状态与边界**：停机时段证据（`shutdown_window`）只读 Final 场次与窗口内 lot，不加锁不写库；Public 会话活动按 60 秒节流且不延长期限；Admin 录入与邮件入库共用 `intake_target`；赛季 `intake_opens_at` 同时约束数据库时钟与邮件 `received_at`。
- **错误**：保留原有错误变体，SQL 通用失败只附加[安全上下文](#sql-错误安全上下文)，不携带 SQL、连接串或驱动原文；连接前固定显式端口并拒绝未知查询参数。

## 审计失败与就绪

`database/audit_health` 的 `AuditWriteResult` 只在原始审计 SQL 结果上分类后原样返回，不改变 `storage_error`、审计调用顺序、事务重试或结果未知的处置。完整 SQLSTATE/驱动分类表在[crates/auction-storage/src/database/operations/audit_health/mod.rs](src/database/operations/audit_health/mod.rs)的模块文档中（唯一字典）；unknown 是“未知风险处置”，permanent 与 unknown 在 classified 下保持粘滞、不提供清零接口。

`with_runtime_switches` 由三角色装配根注入共享快照；默认 shadow 沿用全部失败累计并观察差异，classified 仅投影 permanent + unknown，legacy 不计算影子。`check_database_readiness` 使用保留容量，readiness 始终有一个真实池连接；失败投影区分 `timeout` 与 `dependency_unavailable`。开关与切换步骤见[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)与[ops/README.md#审计就绪影子观察](../../ops/README.md#审计就绪影子观察)。

## SQL 错误安全上下文

[crates/auction-storage/src/database/backend.rs](src/database/backend.rs) 的 `storage_error_from` / `sql_error_context` 是字符串投影的唯一实现：保留原中文上下文，按固定顺序附加 `sqlx_kind`、`sqlstate_class`、`sqlstate`、`constraint`、`table`；缺失为 `none`、格式异常为 `invalid`。投影不读取驱动的 `message()`/`detail()`/`hint()`，不格式化 Display/Debug，不包含绑定值、SQL 或外部正文；错误文本**不得用于自动重试或重发判定**。排障见[ops/README.md#sql-失败诊断](../../ops/README.md#sql-失败诊断)。

## COD 实收资金化拒绝诊断

日志使用独立 target `auction::settlement::funding_diagnostics`；开关、过滤与 D1 核对见[ops/README.md#d1-诊断](../../ops/README.md#d1-诊断)，`cause_kind` 定义见[crates/auction-application/README.md#错误诊断](../auction-application/README.md#错误诊断)。诊断只附加日志，不改变判定顺序、锁、隔离、重试、提交/回滚边界或原错误。

### 收据拒绝字典

[crates/auction-storage/src/database/settlement/funding/receipt/diagnostics.rs](src/database/settlement/funding/receipt/diagnostics.rs) 的 `ReceiptRejection` 是收据与价格拒绝码的唯一枚举，**字典表（成员、`safe_kind` 与含义）随该枚举的文档维护**；所有拒绝位置经 `reject` 记录分类后返回原 `FundingError`，业务拒绝仍为 `Unproven`。

### 授权查询无行的只读诊断

`validate_receipt` 保留原授权 SELECT；只有无行后才可能在 target 的 WARN 启用时调用 `diagnose_relation`：沿用原 `expires_at`，剩余预算小于一秒跳过，否则同事务 `SET LOCAL statement_timeout='250ms'` 后执行只读谓词 SELECT。只认 `Some(true)`；任何失败、超时或缺行都记录 `funding_receipt_relation_unproven` 并返回原 `Unproven`，进入原回滚路径，不参与授权。

### 前置阶段

COD 实收路径的 `stage` 集合（`transaction_*`、`runtime_generation`、`world_lock` 等）以 `fulfillment_scheduler/internal/*.rs` 的固定字符串常量为唯一事实源；父 span 只携带 `obligation_id`、`delivery_id`、`command_id` 与闭集 `world`，事件只附加 `stage`、`safe_kind`、`cause_kind`、原 `defer_reason` 与结果类别，不输出正文、金额或快照。

## 迁移兼容层

[crates/auction-storage/src/database/migrations](src/database/migrations) 为少数历史迁移提供执行视图：保留文件原 checksum，只在执行时对固定版本做限定替换（`000005`/`000015` 保留字别名，`000044` 延迟约束，`000047` 列名加引号）。已发布的迁移文件永远不能修改。

## 迁移就绪

`database/migrations/readiness.rs` 的 `check_postgres_readiness` 是 readiness 专用入口：`with_migration_compatibility_mode` 接受启动策略（默认 shadow，实际仍要求完整历史匹配；compatible 才允许数据库超前）。检查使用 `REPEATABLE READ, READ ONLY` 快照，预算 750ms；SQL 与解码错误走安全投影并拒绝就绪。判定表与回退由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)与[ops/deploy/RECOVERY.md#程序回退](../../ops/deploy/RECOVERY.md#程序回退)拥有。

### 兼容声明验证

`tests/postgres/migration_compatibility.rs`（`just test-db-compat`）沿真实迁移链证明每个 `compatible` 登记属实：在 V-1 捕获旧程序依赖的契约，执行 V 后重新捕获并比对，配合 `INSERT ... DEFAULT VALUES` 探针与回滚事务中的 `LEGACY_WORKLOAD` 重放；任何结果变化都算破坏。内嵌/提交迁移夹具只在 `test-db` 可用。

## 源码结构

[crates/auction-storage/src/database](src/database) 按限界上下文分组，每个模块的 `//!` 注释说明边界；根目录只保留装配入口与跨分组基础：

| 分组 | 内容 |
| --- | --- |
| 基础 | [store.rs](src/database/store.rs)、[pool](src/database/pool)、[backend.rs](src/database/backend.rs)、[migrations](src/database/migrations)、[profiles](src/database/profiles) |
| 命令组合 | [commands](src/database/commands)（竞价、生命周期与 Admin 命令组合器） |
| 拍卖与公开读 | [auction](src/database/auction)（竞价事务、排期、慢速代理、录入目标、密钥信封）、[public](src/database/public)（拍卖/lot/历史/黑名单/补偿池/世界/规则） |
| Admin 与账户 | [admin](src/database/admin)、[accounts](src/database/accounts)（外部凭据、用户与登录会话） |
| 结算与资金 | [settlement](src/database/settlement)（现金账本、履约、调度、报表、策略、目录与补偿冲正） |
| 投递与恢复 | [delivery](src/database/delivery)（outbox、发送围栏、处置事实、装备损失、人工 Sent）、[recovery](src/database/recovery) |
| 邮件与论坛 | [mail](src/database/mail)（轮询、入站、检查点、邮箱维护与通知事实）、[forum](src/database/forum)、[lot_watch](src/database/lot_watch) |
| 实时与运维 | [realtime](src/database/realtime)、[operations](src/database/operations)（告警、资产对账、一致性巡检、SLI、审计与进程健康、运行时开关、通知模板修订） |
| 运维数据库 | [maintenance](src/database/maintenance)（初始化、最小权限、重建、恢复预检、密钥轮换、部署证据、停机窗口与保留清理） |

[crates/auction-storage/src/deployment_identity](src/deployment_identity) 拥有连接参数规范化与部署目标比较。

## 维护约定

- SQL 只出现在本 crate；新增表或约束必须新增迁移，并同步[docs/contracts/database.md](../../docs/contracts/database.md)与[ops/deploy/retention-policy.v2.json](../../ops/deploy/retention-policy.v2.json)（`database_schema_version` 须等于最高迁移版本）。
- 动态拼接 SQL 只允许编译期常量片段，业务值一律 `bind`；同一 owner 内共享的投影/过滤用字面量宏经 `concat!` 组合；DDL 标识符经 `quote_identifier`。
- 新增 `pub` 接口前确认存在调用方；跨 crate 未使用的 `pub` 项编译器不会告警。

## 运行时开关存储

`database/runtime_switches` 实现最新事实查询与 Read Committed 命令事务，锁序为 command advisory → switch advisory → 最新 revision CAS；事实、结果与追加审计同事务提交。未知名称在解码前忽略，已知损坏拒绝刷新。数据模型见[docs/contracts/database.md#运行时行为开关](../../docs/contracts/database.md#运行时行为开关)。

## 通知模板修订存储

`database/operations/mail_templates` 实现 `MailTemplateStore`：读取每个模板键的最新修订，以及 Read Committed 的写入事务，锁序为 command advisory → 模板键 advisory → 最新 revision CAS；修订事实、固定结果与追加审计同事务提交，提交回执丢失报告结果未知。校验、摘要、重放与版本规则都由应用层命令定义，这里只执行 SQL。`DatabaseStore::live_mail_templates` 返回本进程共享的生效快照句柄：竞价与生命周期事务开启时取一次快照，用它渲染并冻结被超提醒的正文。数据模型见[docs/contracts/database.md#出站通知模板](../../docs/contracts/database.md#出站通知模板)。

## 连接与观测边界

`profiles::DatabaseConnectionProfile` 由装配根传入，存储层不读配置；字段与池档位由[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)拥有。`TrackedConnection`/`TrackedTransaction` 持有 RAII 观测守卫（Executor、显式获取、事务与取消），不依赖 after_release；`raw_pool` 仅供原生迁移、LISTEN 与外部关闭边界。`retry_conflicts` 与独立 runner 保留原重试次数与返回值，重跑前等待改为带抖动的 `serializable_retry_backoff`（6ms 起、翻倍、24ms 封顶）。指标见[crates/auction-observability/README.md#数据库事务与连接观测](../auction-observability/README.md#数据库事务与连接观测)。

## SLI 只读快照

`database/sli` 实现 `SliSnapshotReader`，只发布完整数值快照：窗口候选先走事件时间或 slow_due 索引再作有界主键探测，超出候选上限不伪装为完整统计；每轮只借一个空闲连接，失败保留旧数据并丢弃连接。索引与窗口证据见[docs/contracts/database.md#sli-只读查询](../../docs/contracts/database.md#sli-只读查询)。

## 数据密钥重加密读取面

`database/maintenance/key_rotation/` 只为 `auction-cli keys` 提供按域分页扫描、以旧 key ID 作 CAS 条件替换信封、列出各域 key ID 数量并追加 `system.data_key_reencrypt` 审计；不接触明文、密钥材料或指纹。

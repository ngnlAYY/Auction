# auction-storage

[文档导航](../../docs/README.md) · [数据库契约](../../docs/contracts/database.md) · [迁移](../../migrations/postgres/README.md)

PostgreSQL 适配器：实现应用层的仓储、事务与命令端口，拥有全部 SQL、迁移执行、连接池与运维数据库能力（建库、预检、盘点、重建）。唯一支持的数据库是 PostgreSQL 18。

## feature

| feature | 作用 |
| --- | --- |
| （默认无） | 只编译 `DatabaseBackend`、部署目标身份与错误类型；应用以“无数据库骨架”启动 |
| `postgres` | SQLx PostgreSQL 驱动（rustls + webpki 根证书）与全部实现 |
| `test-db` | W6.2 PostgreSQL 18 集成测试夹具（临时集群、已迁移模板库与逐测试克隆库）；只在 `just test-db` 启用 |

## 对外入口

| 入口 | 用途 |
| --- | --- |
| `DatabaseStore::connect(url, profile)` | 显式连接用途与池策略，附带观测 |
| `DatabaseStore::migrate()` | 在 SQLx 迁移锁内执行内嵌迁移；空库或合法历史才允许，未知非空库拒绝 |
| `ProcessReadinessPort` | readiness：按迁移模式核对已知历史及兼容后缀，再按审计模式通过门禁，见[迁移就绪](#迁移就绪)与[审计失败与就绪](#审计失败与就绪) |
| `Database*Commands` | 竞价、生命周期、结算、Admin 拍卖/录入/lot/邮件/规则/runtime/世界等命令组合器 |
| `initialize_empty_postgres` | 合并部署首次启动：确认专用空库且无其他客户端后在迁移锁内建 schema |
| `PostgresDatabaseBootstrap` | 维护库连接上的“缺失才创建”建库（`auction-cli db create`） |
| `PostgresDatabaseRebuildGuard` | 持锁预检与 DROP/CREATE 同定义空库（`auction-cli db rebuild`） |
| `PostgresStore::collect_recovery_preflight` / `collect_retention_inventory` | 只读恢复预检与保留盘点 |
| `strict_deployment_database_identity` | 不连接数据库比较部署目标（主机/socket、端口、库名），拒绝隐式目标与 `options` |
| `postgres_connect_options` | `postgres` feature 下供连接装配与 CLI 目标/libpq 投影共用的 SQLx 选项解析；固定默认端口，保留驱动的 host/hostaddr 查询顺序与 socket 优先级，不连接数据库，错误脱敏 |
| `postgres_expected_migration_version` | 当前二进制内嵌的最高迁移版本 |

## 实现约定

- **事务**：竞价、关键生命周期等命令在 Serializable 事务内完成，序列化失败（40001）与死锁（40P01）由命令 owner 有界重试（`is_serialization_conflict` 是统一判定）。竞价、结算、规则与 Admin 邮件经各自的 `TransactionRunner` 按 `TransactionPolicy` 重试；其余独立 Serializable 写入共用 `retry_conflicts`（默认 `SERIALIZABLE_ATTEMPTS` 次，每次重开事务，只重跑 owner 判定的明确中止，结果未知的提交不重试）。运行期 claim、进度及部分账本写入使用默认隔离事务，并按各自 owner 取行锁或校验租约/fencing；不能把所有写入都视为 Serializable。同一幂等命令 ID 通过事务级 advisory 锁协调（`lock_command_id` 共用键空间）。会写 `lots` 的竞价命令先经 `database/competition/postgres/bid_queue/` 的按场次排队再进入 Serializable 事务：进程内按拍卖 ID 互斥（等待不占连接），并用 PostgreSQL 会话级 advisory lock 覆盖其它实例；隔离级别、行锁、账本与写入顺序都不变，排队预算用尽时返回可重试的 `QueueWaitTimeout`，取消与错误路径都经凭证 `Drop` 解锁或关闭会话。
- **场次写队列**：所有会写 `lots`/`auctions` 的场次级命令在进入 Serializable 事务之前按场次排队（进程内互斥 + 会话 advisory lock）：只知道 lot 的用 `TransactionRunner::run_in_auction_lot_queue`，已知场次的用 `TransactionRunner::run_in_auction_queue`；两者必须落到同一把门，否则同场次仍会 40001。排队只改变并发形状，隔离级别、行锁、账本与写入顺序不变；取消/panic 由凭证 `Drop` 归还，绝不把可能仍持锁的连接放回池中。
- **幂等账本**：各 owner 的命令表保存命令 ID、请求摘要与结果；同 ID 同摘要重放原结果，异摘要冲突。
- **追加式事实**：竞价、代理转换、黑名单历史、审计与投递尝试证明等由触发器拒绝改写；结算账本按 owner 追加。改名保留原名称证据，撤销时更新 `reverted_at`，不能把整张改名表视为不可更新。审计 `INSERT` 失败仍传播并让原事务回滚，进程内另记分类失败数，按[审计就绪策略](#审计失败与就绪)决定健康门禁。
- **冻结竞价策略**：锁内快照和账户代理降额能力按同一场次策略版本解码，未知版本失败关闭；公开普通最低价继续使用领域计算。新旧策略与追加迁移边界见[数据库契约](../../docs/contracts/database.md#拍卖与竞价)。
- **租约与 fencing**：Worker 的拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、告警批次都以“claim + 租约 + fencing 版本”推进，过期租约可被重新领取，发送中的外部写入以 fencing 防止重复。
- **恢复义务锁**：人工确认 recovery 时，用单条 `ANY` 查询按 UUID 升序锁定该 case 的 3～4 条义务，保持既有 owner 锁序；返回集合必须完整，且逐条核对拍卖、lot、世界和装备身份后才能推进，缺失义务继续拒绝确认。
- **装备损失 owner**：`database/equipment_loss` 实现 `SettlementEquipmentLossPort`；首次唯一库存读取失败才能通过正式命令终结原 COD，损失事实/原版本/人工声明永久追加。复用资金 owner 的 `record_operator_equipment_loss` 与既有补偿分配，释放正常卖家付款队列，不伪造实收或补偿完成。CAS、统一锁序、三次 Serializable 冲突重试与退休投递禁止复活详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递)。
- **人工 Sent owner**：`database/sent_confirmation` 实现 `DeliverySentConfirmationPort`；追加独立 operator 观察及责任声明，与原冻结发送尝试、唯一 MID 和当前 fence 绑定。装备继续经共享强证明谓词和原履约恢复 helper 取得真实 Sent collected/资金化/原金额卖方付款。现金只允许 main 已收款 lot 的原卖方正额货款（已领取须匹配原生金额回执并显式接受），精确绑定原 UID/lot/义务/投递/金额、`unknown` seller 分配及预留命令/分录；同事务复用自动恢复的 `finish_funded_delivery(Sent)` 记为 `spent`，同 ID 重放不重复支出，不创建来源或收款。被 Admin 对账判失败、但 Sent 证明已寄出的投递以追加 confirmation 更正（迁移 `000074` 扩展强谓词），原失败命令与审计不改写。不伪造 `sent_verified`，不重发邮件，原 Admin 手填成功仍拒绝。准入与分钟时窗见[结算指南](../../docs/guide/settlement.md#sent-人工证据确认)，永久保留事实见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递)。
- **规则上移**：Admin 投递动作、COD 实收资金化与履约 claim 一致性以“存储按锁序读取 → 应用层类型化快照 `evaluate` → 闭集阻断项”判定；SQL 只保留关系完整性、fenced 命令绑定与不可绕过的最低约束。阻断项到既有错误/拒绝码的映射固定，日志分类与资金错误变体不因上移放宽；新增或收窄 SQL 谓词需要迁移并同步正反例。
- **运行时独占**：外部 Cookie 的运行时 owner（邮件、论坛各一）和 Public realtime consumer 以 PostgreSQL session advisory lock 实现，连接断开即失去所有权；所有权失效后持有方必须退出。
- **只读快照**：报表与运维读取使用可重复读只读事务，运维快照设置 5 秒语句与 1 秒锁等待上限。
- **停机时段证据**：`database/shutdown_window` 的 `PostgresStore::collect_shutdown_window` 读取 Final 场次与截止不晚于保护窗口末端（含已过截止）的在拍 lot，沿用只读快照与上限，不加行锁/advisory lock、不写数据库；CLI 消费安全投影并处理强制声明。判定来源、窗口、失败关闭与输出见 [CLI 时段门禁](../../apps/auction-cli/README.md#停机时段门禁)。
- **Public 会话活动**：普通活动不延长期限，`last_activity_at` 按 60 秒节流写入；需要活动写入时，在语句完成后复用独立只读查询重新认证。行锁等待可能跨过期限或撤销，仅靠等待前的物化快照或 `UPDATE WHERE clock_timestamp()` 不能证明等待后的身份仍有效；无需活动写入的快路保持单条查询且不写会话行。
- **收件目标**：Admin 录入与邮件入库共用 `database/intake_target`：在已持有 world 锁后锁定收件目标与拍卖、校验一致性、按锁定版本 CAS 推进预览修订并写入装备 key；错误分类仍由各自 owner 决定。
- **邮件入库编号**：持有原 world/目标拍卖锁，在同一 Serializable 事务中按分类聚合已有编号，同时归并全局 lot 序号；空场次仍从 1 开始，分类编号各自递增，溢出和损坏分类快照继续失败关闭。
- **赛季收件边界**：新场次冻结的 `intake_opens_at` 同时约束锁后数据库实时钟和邮件原始 `received_at`；任一早于边界都返回 `IntakeClosed`，延迟轮询不能把旧赛季邮件加入新赛季。等于边界允许继续，历史 NULL 不增加限制；已持久接受命令仍先重放原决定。Admin 录入复核锁后当前时刻；恢复重列在既有 world/auction/recovery 等锁和证明复核后取 `clock_timestamp()`，不能使用事务开始时间或 Worker 选候选时的旧时刻替代换季判断。旧赛季识别同时保留原场次开拍和原始投稿收件事实，以二者较早时刻为起点，避免改期开拍绕过换季阻断。
- **错误**：保留原有错误变体，SQL 通用失败仅附加[安全上下文](#sql-错误安全上下文)；停机时段证据的诊断字段见 [CLI 时段门禁](../../apps/auction-cli/README.md#停机时段门禁)；不携带 SQL、连接串或驱动原文。
- **连接目标**：连接前固定显式端口（省略为 5432），拒绝未知查询参数；IPv6 主机名去括号后重新匹配 pgpass。

## 审计失败与就绪

`database/audit_health` 的 `AuditWriteResult` 仅接受 `Result<T, sqlx::Error>`，在原始审计 SQL 结果上分类，随后原样返回；不改变 `storage_error`、审计调用顺序、事务 owner 的 Serializable 重试、同事务事实/审计/事件或结果未知的处置。计数按失败 SQL 尝试累计，包含后来重试成功的尝试，不等同于已提交事实的审计缺失数量；不捕获未经过 helper 的事务开始或提交错误。

| 原始错误（按表中顺序匹配） | class / kind |
| --- | --- |
| `40001` / `40P01` | transient / `serialization_conflict`、`deadlock` |
| `57014` / `55P03` | transient / `query_canceled`、`lock_unavailable` |
| 整个 `08` 类（含 `08007`、`08P01`） | transient / `connection_exception`；只放行健康门禁，不允许自动重放结果未知事务 |
| sqlx `PoolTimedOut` / `Io` | transient / `pool_timed_out`、`connection_io` |
| `53300`（决策 71） | transient / `connection_capacity` |
| `57P01`（决策 71） | transient / `server_shutdown` |
| `57P02`（决策 71） | transient / `server_crash` |
| `57P03`（决策 71） | transient / `server_starting` |
| `23` 类 | permanent / `constraint_violation`，包括 `23505`，不扩大冲突重试 |
| `55000`、`09` / `27` / `P0` 类、`39P01/39P02/39P03` | permanent / `trigger_or_routine_rejected` |
| `42501` / `28` 类 | permanent / `permission_denied`、`authentication_rejected` |
| 其他 `42` 类、`3D` / `3F` 类 | permanent / `schema_or_sql_mismatch` |
| `21` / `22` / `44` 类 | permanent / `invalid_audit_data` |
| sqlx `Configuration/ConfigFile`、`InvalidArgument` | permanent / `driver_configuration`、`driver_invalid_argument` |
| sqlx `RowNotFound/TypeNotFound/ColumnIndexOutOfBounds/ColumnNotFound/ColumnDecode/Encode/Decode` | permanent / `driver_data` |
| sqlx `PoolClosed/WorkerCrashed/InvalidSavePointStatement` | permanent / `driver_state` |
| sqlx `Tls` / `Protocol` | unknown / `driver_tls`、`driver_protocol` |
| 未列出的、缺失或非法 SQLSTATE；其他及未来 sqlx 变体（含 `BeginFailed/AnyDriverError/Migrate`） | unknown / `unknown_sqlstate`、`unknown_driver`；依决策 71，`57P05`、`25P03/25P04` 仍按未知粘滞阻断 |

unknown 是“未知风险处置，不代表已证明不可重试”。permanent 与 unknown 在 classified 下保持粘滞，不提供清零接口，不写试探审计；修复后受控重启恢复。瞬时失败不锁死 classified readiness，但数据库/schema 当前不可用仍使依赖检查失败，恢复后可重新就绪。

`DatabaseStore::with_runtime_switches` 由三角色装配根注入共享快照；默认 classified 仅投影 permanent + unknown，显式 shadow 沿用全部失败累计门禁并观察新旧差异，legacy 不计算影子。HTTP、心跳与既有告警共享该模式的 `audit_failure_count`，不改 DTO 或 schema。纯内存审计指标由装配根注册，关闭 exporter 不影响计数；日志仅包含固定类别和 kind，不包含驱动原文。指标字典见 [auction-observability](../auction-observability/README.md#指标)，开关与切换步骤见[配置参考](../../ops/config/README.md#就绪策略)和[运维手册](../../ops/README.md#审计就绪影子观察)。

`check_database_readiness` 使用 `begin_read_snapshot_reserved`：`TrackedPool` 把普通业务连接限制在物理池扣除长期 owner 与 readiness 预留后的容量，readiness 始终保留一个真实池连接，Final 写饱和或匿名读突发不能把探针饿死；连接池关闭、连接失败与 schema/迁移失败仍使 readiness 失败。失败投影区分 `timeout`（有界预算内未完成检查）与 `dependency_unavailable`（池饱和或审计门禁），不再统一记为 `repository_failed`；审计门禁另在 `auction::storage::audit_health` 输出 `审计失败门禁阻断进程 readiness`。Worker 批量连接预算相应扣除 1 个 readiness 保留位。

## SQL 错误安全上下文

[`crates/auction-storage/src/database/backend.rs`](src/database/backend.rs) 的 `storage_error_from` 接收原始 SQLx 错误，返回增强文本的 `ApplicationError::Repository(RepositoryError::Failed(String))`；`sql_error_context` 是字符串投影的唯一实现，供健康错误和具名 SQL mapper 的原有 `Failed(String)` 出口复用。纯校验拒绝、非 SQLx 错误及没有原始错误的 `ok_or_else` 继续使用固定文本。已压缩为其他闭集错误的路径不从文本反推 SQL 原因。

文本保留原中文上下文，按固定顺序附加字段：

```text
原中文上下文 [sqlx_kind=Database sqlstate_class=23 sqlstate=23505 constraint=credential_keys_one_active_cookie_kind_idx table=credential_keys]
```

| 字段 | 允许的内容 |
| --- | --- |
| `sqlx_kind` | 静态 SQLx 变体名；未知新增变体为 `Other`，不读取变体负载 |
| `sqlstate_class` / `sqlstate` | SQLSTATE 前两位类别及完整五位代码；代码必须为五个 ASCII 大写字母或数字 |
| `constraint` / `table` | 驱动返回的 schema 标识，首字节为 ASCII 字母或下划线，后续仅含 ASCII 字母、数字或下划线，长度为 1–63 字节 |

缺失字段为 `none`，格式异常为 `invalid`。非 `Database` 错误只记录变体名，其余字段均为 `none`；例如 `PoolTimedOut`、`Io`、`ColumnDecode` 不携带连接、列值或底层原因。当前迁移中的对象名都是静态 schema 标识，不由业务值生成。PostgreSQL 不保证所有错误都带表名或约束名，现有部分触发器仅指定 SQLSTATE，因此字段缺失不表示约束没有执行。

投影不调用驱动的 `message()`、`detail()`、`hint()`，不格式化原错误的 Display/Debug、不遍历或保留 source，不包含绑定值、SQL 或外部正文。原有类型化 Conflict/SerializationConflict 判定与整事务重试顺序保持；`safe_kind()` 分类保持。错误文本仅供诊断，**不得用于自动重试或重发判定**。失败关闭与秘密隔离保持，缓存不参与错误投影或命令结果（不变量 6、7、9）。

本项不新增日志、影子模式或配置。增强文本可经既有 `%error` 日志和失败安全详情字段输出；只记录 `safe_kind` 或继续压缩错误的路径不会自动显示这些字段。回退为换回旧制品，无 schema 迁移；日志过滤器只能影响后续日志输出，不能撤销已写入日志或持久化详情的增强文本。排障步骤见[运维手册](../../ops/README.md#sql-失败诊断)。

## COD 实收资金化拒绝诊断

日志使用独立 target `auction::settlement::funding_diagnostics`；开关、过滤与 D1 只读核对见[运维手册](../../ops/README.md#d1-诊断)。原错误的 `cause_kind` 定义见[应用错误分类](../auction-application/README.md#资金与内部证据错误)。日志只附加诊断，不改变原判定顺序、锁、Serializable 隔离、三次重试、提交/回滚边界或原错误；不改人工 Sent、失败命令、冻结尝试及追加审计，不触发重发。

### 收据拒绝字典

[`crates/auction-storage/src/database/settlement_funding/receipt/diagnostics.rs`](src/database/settlement_funding/receipt/diagnostics.rs) 的内部 `ReceiptRejection` 是收据与价格拒绝码的唯一枚举；本表是其唯一文档字典。`as_str()` 穷尽匹配固定字面量，所有拒绝位置通过 `reject` 记录分类后返回原 `FundingError`。除注明的原 `Invalid` 和 SQL/解码错误外，业务拒绝仍为 `Unproven`；SQL/解码仍经原资金错误映射。原 `storage_error` 不变。

| 枚举成员 | `safe_kind` | 拒绝位置/含义 |
| --- | --- | --- |
| `SentMissing` | `funding_receipt_sent_missing` | 联合证据缺少 Sent |
| `SnapshotInvalid` | `funding_receipt_snapshot_invalid` | 联合快照结构或摘要校验不通过 |
| `PaymentMismatch` | `funding_receipt_payment_mismatch` | 收据付款义务不等于冻结付款 |
| `BuyerMismatch` | `funding_receipt_buyer_mismatch` | 收据买方金额不等于冻结金额 |
| `SentUidMismatch` | `funding_receipt_sent_uid_mismatch` | Sent 与已锁资金账户身份不等 |
| `SentWorldMismatch` | `funding_receipt_sent_world_mismatch` | Sent 世界不是 main |
| `CashUnexpected` | `funding_receipt_cash_unexpected` | record_only 却携带现金证据 |
| `CashMissing` | `funding_receipt_cash_missing` | strict_cash 缺少现金证据 |
| `CashWorldMismatch` | `funding_receipt_cash_world_mismatch` | 现金证据世界不是 main |
| `CashUidMismatch` | `funding_receipt_cash_uid_mismatch` | 现金证据与资金账户身份不等 |
| `MailIdInvalid` | `funding_receipt_mail_id_invalid` | 邮件标识不是规范正整数 |
| `EvidenceTimeInvalid` | `funding_receipt_evidence_time_invalid` | 数据库时间下证据未获证明 |
| `RelationUnproven` | `funding_receipt_relation_unproven` | 授权查询无行，诊断关闭、失败、超时、预算不足或未能进一步分类 |
| `QueryFailed` | `funding_receipt_query_failed` | 原收据 SQL 执行失败，原因只保留资金错误分类 |
| `RowDecodeFailed` | `funding_receipt_row_decode_failed` | 原收据行解码失败 |
| `KindInvalid` | `funding_receipt_kind_invalid` | 义务种类不是两种 COD |
| `CarrierInvalid` | `funding_receipt_carrier_invalid` | COD 种类对应的 carrier 不是 0 或 1 |
| `CodOverflow` | `funding_receipt_cod_overflow` | 买方金额加 carrier 溢出，原返回 Invalid |
| `AmountMismatch` | `funding_receipt_amount_mismatch` | 义务金额与冻结买方金额不等 |
| `BuyerNonpositive` | `funding_receipt_buyer_nonpositive` | 买方金额非正 |
| `RecipientMismatch` | `funding_receipt_recipient_mismatch` | Sent 与原投递收件人不等 |
| `SubjectMismatch` | `funding_receipt_subject_mismatch` | Sent 主题与 canonical 主题不等 |
| `PayloadMismatch` | `funding_receipt_payload_mismatch` | 原 payload 不等于完整重建值 |
| `OperationMismatch` | `funding_receipt_operation_mismatch` | metadata 操作不是 send |
| `TemplateMismatch` | `funding_receipt_template_mismatch` | metadata 模板不符 |
| `ReferenceUnproven` | `funding_receipt_reference_unproven` | metadata 外部引用未证明 |
| `SentProofUnproven` | `funding_receipt_sent_proof_unproven` | 旧精确证明与冻结强证明均不成立 |
| `StoreSentMissing` | `funding_receipt_store_sent_missing` | 持久收据前缺少 Sent |
| `StoreFailed` | `funding_receipt_store_failed` | 原收据 INSERT 失败 |
| `ObligationMissing` | `funding_receipt_obligation_missing` | 授权关联的付款义务缺失 |
| `EquipmentMissing` | `funding_receipt_equipment_missing` | 授权关联的装备缺失 |
| `FulfillmentMissing` | `funding_receipt_fulfillment_missing` | 授权关联的履约缺失 |
| `DeliveryMissing` | `funding_receipt_delivery_missing` | 授权关联的投递缺失 |
| `DeliveryMismatch` | `funding_receipt_delivery_mismatch` | 履约关联投递与收据投递不等 |
| `FulfillmentStateMismatch` | `funding_receipt_fulfillment_state_mismatch` | 履约尚非 satisfied |
| `ProofKindMismatch` | `funding_receipt_proof_kind_mismatch` | 履约证明种类不符 |
| `ProofReferenceMismatch` | `funding_receipt_proof_reference_mismatch` | 履约证明引用不符 |
| `ProofDigestMissing` | `funding_receipt_proof_digest_missing` | 履约证明摘要缺失 |
| `DeliveryStateMismatch` | `funding_receipt_delivery_state_mismatch` | 投递尚非 succeeded |
| `OriginKindMismatch` | `funding_receipt_origin_kind_mismatch` | 投递 origin kind 不是 settlement |
| `OriginIdMismatch` | `funding_receipt_origin_id_mismatch` | 投递 origin id 与付款义务不等 |
| `ExternalReferenceMismatch` | `funding_receipt_external_reference_mismatch` | 投递引用不符 |
| `PrepareCommandMismatch` | `funding_receipt_prepare_command_mismatch` | prepare 命令不等 |
| `PrepareDigestMismatch` | `funding_receipt_prepare_digest_mismatch` | prepare 摘要不等 |
| `ActivateCommandMismatch` | `funding_receipt_activate_command_mismatch` | activate 命令不等 |
| `ActivateDigestMismatch` | `funding_receipt_activate_digest_mismatch` | activate 摘要不等 |
| `OpeningSourceMissing` | `funding_receipt_opening_source_missing` | record_opening 或 reconciliation 边界来源缺失 |
| `SendingTimeMissing` | `funding_receipt_sending_time_missing` | 原 sending_at 缺失 |
| `SendBeforeBoundary` | `funding_receipt_send_before_boundary` | 原 sending_at 未严格晚于资金边界 |
| `PricingEndPriceMissing` | `funding_pricing_end_price_missing` | 冻结 end_price 缺失 |
| `PricingEndPriceInvalid` | `funding_pricing_end_price_invalid` | 冻结 end_price 无法构造非负金额 |
| `PricingListedPriceMissing` | `funding_pricing_listed_price_missing` | 冻结 listed_price 缺失 |
| `PricingListedPriceInvalid` | `funding_pricing_listed_price_invalid` | 冻结 listed_price 无法构造非负金额 |
| `PricingBuyerAmountMissing` | `funding_pricing_buyer_amount_missing` | 冻结 buyer_amount 缺失 |
| `PricingBuyerAmountInvalid` | `funding_pricing_buyer_amount_invalid` | 冻结 buyer_amount 无法构造非负金额 |
| `PricingSellerAmountMissing` | `funding_pricing_seller_amount_missing` | 冻结 seller_amount 缺失 |
| `PricingSellerAmountInvalid` | `funding_pricing_seller_amount_invalid` | 冻结 seller_amount 无法构造非负金额 |
| `PricingTaxAmountMissing` | `funding_pricing_tax_amount_missing` | 冻结 tax_amount 缺失 |
| `PricingTaxAmountInvalid` | `funding_pricing_tax_amount_invalid` | 冻结 tax_amount 无法构造非负金额 |
| `PricingCompensationAmountMissing` | `funding_pricing_compensation_amount_missing` | 冻结 compensation_amount 缺失 |
| `PricingCompensationAmountInvalid` | `funding_pricing_compensation_amount_invalid` | 冻结 compensation_amount 无法构造非负金额 |
| `PricingRowDecodeFailed` | `funding_pricing_row_decode_failed` | 冻结价格行解码失败 |
| `PricingHoldMissing` | `funding_pricing_hold_missing` | 冻结 hold_ppm 缺失 |
| `PricingAdjustmentMissing` | `funding_pricing_adjustment_missing` | 冻结 adjustment_ppm 缺失 |
| `PricingPoolInvalid` | `funding_pricing_pool_invalid` | 冻结买方奖池金额无效 |
| `PricingAdjustedOverflow` | `funding_pricing_adjusted_overflow` | adjusted price 加法溢出 |
| `PricingBuyerMismatch` | `funding_pricing_buyer_mismatch` | 冻结 buyer amount 与本次 buyer 不等 |
| `PricingHoldInvalid` | `funding_pricing_hold_invalid` | 冻结 hold_ppm 越界 |
| `PricingModeInvalid` | `funding_pricing_mode_invalid` | 冻结策略模式不能解码 |
| `PricingLineInvalid` | `funding_pricing_line_invalid` | 冻结结算明细不符合冻结策略 |

`stage` 分为 `receipt_validate`、`receipt_relation`、`receipt_pricing`、`receipt_store`。六个价格字段的缺失与非法金额分别拥有独立成员；旧三变量 payload 的兼容入口与冻结模式默认值保持原样。精确旧 Sent 证明与冻结强证明的 OR 顺序保持不变，不补写 `sent_verified`。

### 授权查询无行的只读诊断

`receipt::validate_receipt` 保留原授权 SELECT；只有该查询无行后才可能调用 `diagnose_relation`。target 的 WARN 未启用时立即返回原泛化类别，不执行额外语句。启用时沿用原证据的绝对 `expires_at`：剩余预算小于一秒就跳过；否则先在同一事务执行 `SET LOCAL statement_timeout = '250ms'`，成功后才执行诊断 SELECT。完整等待服务端结果，不包 tokio 超时、不取消 future，不改原 `transaction_timeout`。

额外 SELECT 在同一事务内执行，LEFT JOIN 原付款、装备、履约、投递与资金来源边界，只返回布尔谓词：关联行存在、投递匹配、satisfied、证明种类/引用/摘要存在、succeeded、origin、external reference、prepare/activate 命令与摘要、资金边界存在、sending_at 存在及严格晚于边界。按固定顺序，仅 `Some(true)` 算通过；FALSE 和 NULL 都记录首个失败谓词。SQL 失败、超时、缺行、解码失败或无法细分时，仍记录 `funding_receipt_relation_unproven`。

诊断 SELECT 不带业务写入或行锁，不 spawn/detach，不参与授权；无论诊断结果如何，调用方都返回原 `FundingError::Unproven`，进入原回滚路径。SET 失败时不再执行 SELECT；SET 或 SELECT 的 SQL 错误（包括 57014 语句超时）会使事务进入中止状态，但不妨碍随后原 ROLLBACK。此分支必回滚，LOCAL 设置随之撤销；避免客户端取消后 sqlx 在回滚时等待未读响应，继而触及事务超时并把原回滚结果变为未知。诊断不能证明提交/回滚结果，也不能直接解释 Worker 的 unpersisted 错误。

### 前置阶段

| owner 与符号 | `stage` | 新增诊断边界 |
| --- | --- | --- |
| `fulfillment_scheduler/internal/transaction.rs::run` | `transaction_begin`、`transaction_command`、`transaction_replay`、`transaction_budget`、`transaction_timeout`、`transaction_commit`、`transaction_rollback` | 原事务建立、命令、重放、预算、超时设置及提交/回滚错误；只在 collected 路径记录 |
| 同文件 `apply` | `runtime_generation`、`world_lock`、`account_lock`、`funding_scope`、`business_lock`、`credentials`、`delivery_lock`、`account_ready`、`funding_evidence`、`funding_deferred`、`satisfy_write`、`receipt_input`、`fund_payment`、`completion` | 每个原阶段独立固定 `collected_*` 类别；原资金门 false 仍为 `Ok(true)`，只在原提交后确认 FundingDeferred |
| 同文件 `validate_collected` | `collected_proof` | Sent/投递缺失，证明种类、摘要、引用、收件人、世界、义务种类、观察时间、主题及持久发送绑定各自分类 |
| `fulfillment_scheduler/internal/validation.rs::validate` | `claim_validate` | 冻结形状、快照、观察窗口、operation/world/purpose、原 claim SQL、用户读取、前置证明及最后证据时间；collected 路径才增加日志 |
| `fulfillment_scheduler/internal/graph.rs::validate_graph` | `graph_validate`、`graph_successors` | 原义务图未证明与后继状态冲突分别分类，查询 SQL 保持原样 |
| `settlement_funding/account.rs` | `account_verify`、`cash_gate`、`funding_evidence` | 账户事务/ready、模式、现金输入、缺口、在途/未知占用、身份/快照/时效与现金形状；缺口与未知占用均保持原 false |
| `settlement_funding/obligations.rs::fund_lot` | `fund_lot` | 原重放、行读取、冻结 B/C/S/T、付款证明、lot/分配/source/issue/transfer 写入、发行后覆盖、时效与完成；collected 路径才增加日志 |
| 同文件 `fund_collected_payment` | `payment_mutation` | 原 Sent 缺失、稳定请求摘要序列化与 lot 查询；原命令摘要与幂等身份不变 |

父 instrument span 仅传递 `obligation_id`、`delivery_id`、`command_id`、闭集 `world`，不跨 await 持有 entered guard。事件只附加 `stage`、`safe_kind`、`cause_kind`、原 `defer_reason` 与已确认的结果类别；不输出 UID、玩家名、地址、主题、MID、正文、payload、metadata、金额、凭据、快照或摘要。

两种持久化 defer reason 仍为原值，白名单与读取契约不变；没有迁移。D1 仍须部署后的分类日志及获批只读证据确认，不因新增诊断而标为已修复。

## 迁移兼容层

迁移文件位于 [`migrations/postgres/`](../../migrations/postgres)，编译期经 `sqlx::migrate!` 内嵌。[`crates/auction-storage/src/database/migrations/`](src/database/migrations) 为少数历史迁移提供“执行视图”：保留文件原 checksum，只在执行时对固定版本做限定替换（`000005`/`000015` 给保留字别名改名，`000044` 先立即执行延迟约束，`000047` 给保留字列名加引号）。仅 `000005`/`000015` 额外接受已部署的固定别名修正版 checksum；`000044`/`000047` 只接受原文件 checksum，其他差异均失败关闭。已发布的迁移文件永远不能修改。

## 迁移就绪

`database/migrations/readiness.rs` 的 `check_postgres_readiness` 是 readiness 专用入口：`DatabaseStore::with_migration_compatibility_mode` 接受装配根注入的启动策略，默认 shadow，实际仍要求完整历史匹配；只有 compatible 模式允许数据库超前。075 引入只追加登记表，本版登记 breaking；001–074 不回填，缺行不兼容。完整判定表与表契约见[数据库契约](../../docs/contracts/database.md#迁移兼容登记与就绪)，分级清单见[迁移说明](../../migrations/postgres/README.md#兼容分级与登记)。

检查使用一个 `REPEATABLE READ, READ ONLY` 快照，整个数据库检查预算 750ms；同版只读历史，超前时用实际迁移 LEFT JOIN 登记逐条检查连续版本、成功状态与闭集等级。SQL 查询与解码错误走 `storage_error_from` 安全投影并拒绝就绪，不计作审计写入失败。`postgres_history_matches` 保持严格：恢复、备份、盘点与迁移执行不因 readiness 开关接受未来版本。

三角色通过 `with_migration_readiness_recorder` 注入进程记录器；supervisor 的临时连接注入同一迁移模式，默认无指标 exporter。shadow 差异日志 target 为 `auction::storage::migration_readiness`，只包含固定 class、期望/数据库版本、strict/compatible 布尔结果；唯一预期差异为 `compatible_ahead`。指标名与闭集分支见[可观测性说明](../auction-observability/README.md#迁移就绪指标)。平时保持 shadow，换回已含该逻辑的旧程序前须显式选择 compatible，操作与保留新版 CLI 的要求见[程序回退](../../ops/deploy/RECOVERY.md#程序回退)。

## 源码结构

[`crates/auction-storage/src/database/`](src/database) 按功能 owner 划分子模块（每个模块的 `//!` 注释说明其边界），主要分组：

| 分组 | 模块 |
| --- | --- |
| 基础 | [`crates/auction-storage/src/database/store.rs`](src/database/store.rs)、[`crates/auction-storage/src/database/postgres_store.rs`](src/database/postgres_store.rs)、[`crates/auction-storage/src/database/pool/`](src/database/pool)、[`crates/auction-storage/src/database/backend.rs`](src/database/backend.rs)、[`crates/auction-storage/src/database/migrations/`](src/database/migrations)、[`crates/auction-storage/src/database/audit_health/`](src/database/audit_health) |
| 竞价与拍卖 | [`crates/auction-storage/src/database/competition/`](src/database/competition)、[`crates/auction-storage/src/database/auctions/`](src/database/auctions)、[`crates/auction-storage/src/database/lots/`](src/database/lots)、[`crates/auction-storage/src/database/bid_log/`](src/database/bid_log)、[`crates/auction-storage/src/database/price_history/`](src/database/price_history)、[`crates/auction-storage/src/database/slow_proxy_due.rs`](src/database/slow_proxy_due.rs)、[`crates/auction-storage/src/database/scheduler/`](src/database/scheduler)、[`crates/auction-storage/src/database/commands/`](src/database/commands) |
| 结算与资金 | [`crates/auction-storage/src/database/settlement/`](src/database/settlement)、[`crates/auction-storage/src/database/settlement_commands/`](src/database/settlement_commands)、[`crates/auction-storage/src/database/settlement_scheduler/`](src/database/settlement_scheduler)、[`crates/auction-storage/src/database/settlement_policy/`](src/database/settlement_policy)、[`crates/auction-storage/src/database/settlement_report/`](src/database/settlement_report)、[`crates/auction-storage/src/database/settlement_catalog/`](src/database/settlement_catalog)、[`crates/auction-storage/src/database/settlement_funding/`](src/database/settlement_funding)、[`crates/auction-storage/src/database/legacy_self_proof/`](src/database/legacy_self_proof)、[`crates/auction-storage/src/database/compensation/`](src/database/compensation)、[`crates/auction-storage/src/database/compensation_reversal/`](src/database/compensation_reversal) |
| 履约与投递 | [`crates/auction-storage/src/database/fulfillment_scheduler/`](src/database/fulfillment_scheduler)、[`crates/auction-storage/src/database/delivery_scheduler/`](src/database/delivery_scheduler)、[`crates/auction-storage/src/database/mail_delivery/`](src/database/mail_delivery)、[`crates/auction-storage/src/database/delivery_admin/`](src/database/delivery_admin)（列表、命令与单条证据详情）、[`crates/auction-storage/src/database/delivery_audit/`](src/database/delivery_audit) |
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

- SQL 只出现在本 crate；新增表或约束必须新增迁移文件，并同步[数据库契约](../../docs/contracts/database.md)与[保留政策 v2](../../ops/deploy/retention-policy.v2.json)（`database_schema_version` 须等于最高迁移版本，否则 `db inventory` 与 `db retention-dry-run` 拒绝运行）。
- 动态拼接 SQL 只允许编译期常量片段，业务值一律 `bind`；同一 owner 内多条查询共享的投影或过滤以字面量宏定义，经 `concat!` 组合成静态查询（如 lot 列表/详情、竞价日志、价格历史、账户代理），不在运行时拼接；DDL 标识符经 `quote_identifier`，库名先过 `valid_ddl_database_name`（ASCII 字母数字与 `_-.`）。
- 新增 `pub` 接口前确认存在调用方，编译器不会对跨 crate 未使用的 `pub` 项告警。


## 运行时开关存储

`database/runtime_switches` 实现最新事实查询和 Read Committed 命令事务，锁序为 command advisory → switch advisory → 最新 revision CAS，事实兼账本、结果与追加审计同事务提交。未知名称在解码之前忽略，已知损坏拒绝刷新；SQL 错误不回显驱动原文。审计指标源、心跳和探针共享 `RuntimeSwitches`，累计分类不重置。数据模型与 compatible 076 见[数据库契约](../../docs/contracts/database.md#运行时行为开关)。

## 连接与观测边界

`profiles::DatabaseConnectionProfile` 由装配根传入，不在存储层读配置。
默认 profiled：服务池默认 16，维护池默认 4；服务超时默认关闭。
legacy 完整保留 16/30 秒及会话继承，指标与模式无关。
名称来自 `DatabaseConnectionPurpose`，池与直连共用会话初始化。
字段、启用条件与重启回退由[配置参考](../../ops/config/README.md#数据库连接分档)拥有。

`pool::TrackedConnection` 与 `TrackedTransaction` 持有 RAII 观测守卫，
覆盖 Executor、显式获取、事务和取消；不依赖 after_release。
raw_pool 仅供原生迁移、LISTEN 与外部关闭边界；资金普通访问也走 TrackedPool。
所有权连接继续 close_on_drop，指标不能替代 session lock 或重建原会话。

`backend::is_serialization_conflict` 保持纯谓词；
`observed_serialization_conflict` 在映射前附加窄元数据，返回相同判断。
`retry_conflicts` 与独立 runner 保留全部原重试次数和返回值；
每次确认重跑前的等待改为 `serializable_retry_backoff` 的带抖动退避
（窗口 6ms 起、每次翻倍、24ms 封顶，实际等待为窗口一半加一半均匀抖动），
避免同时冲突的事务在同一时刻同步重试；预算仍是三次尝试，白名单与错误返回不变。
耗尽由 owner 原终止分支记录，不能用最后是否返回 Err 推测。
指标定义见[数据库观测](../auction-observability/README.md#数据库事务与连接观测)。
保护不变量 2、5、6、9：原事务、持久状态、失败关闭和缓存边界保持。


## SLI 只读快照

`database/sli` 实现 `SliSnapshotReader`，只发布完整数值快照。
窗口候选先走事件时间或 slow_due 索引，再作有界主键探测；
超出候选上限不能伪装为完整统计。
每轮只借一个现有空闲连接，池忙跳过；局部短超时、只读事务和总预算
均不改变业务会话分档、事务、审计或外写语义。
成功确认回滚后发布；失败保留旧数据并丢弃连接，不返回部分结果。
索引与八项历史窗口证据由[数据库契约](../../docs/contracts/database.md#sli-只读查询)说明，
指标及不可评估语义见[SLI 文档](../auction-observability/README.md#sli-与容量基线)。

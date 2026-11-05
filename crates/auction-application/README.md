# auction-application

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [业务规则](../../docs/business-rules.md)

用例层：定义窄端口（trait）、命令与安全投影，编排领域规则与事务边界。它不认识 PostgreSQL、HTTP、MoogleMail 或 E-Hentai 的具体实现——这些由 `auction-storage`、`auction-http`、`auction-integrations`、`auction-jobs` 作为适配器实现端口，`apps/` 中各程序按角色在装配根组合所需能力。

依赖：`auction-domain` 与少量通用库（serde、sha2/hmac、chrono-tz、url 等），不依赖 tokio 运行时。

`ProcessReadinessPort` 只返回有限成功/失败；`AuditFailureMode` 定义 legacy/shadow/classified 启动策略，具体 SQLSTATE 分类、计数由存储适配器拥有，HTTP 缓存由 HTTP 适配器拥有，不改变事务或命令重试规则。配置与语义见[就绪策略](../../ops/config/README.md#就绪策略)。

## 结构

| 路径 | 内容 |
| --- | --- |
| [`crates/auction-application/src/ports/`](src/ports) | 端口与值对象：每个功能 owner 一个子模块，由 [`crates/auction-application/src/lib.rs`](src/lib.rs) 统一导出 |
| [`crates/auction-application/src/services/`](src/services) | 用例服务：组合端口完成一个业务动作 |
| [`crates/auction-application/src/errors/`](src/errors) | `ApplicationError` 及各 owner 的低基数错误 |

### 端口分组

| 分组 | 主要内容 |
| --- | --- |
| [`crates/auction-application/src/ports/core/`](src/ports/core) | `Clock`、`CommandLedger`/`RequestHash`/`StoredCommand`（幂等账本）、事务策略、仓储错误、稳定编码（小写十六进制、旧版 Go JSON 兼容摘要） |
| [`crates/auction-application/src/ports/auction/`](src/ports/auction) | 公开查询、竞价命令、生命周期、慢速代理到期、冻结的策略模板 |
| [`crates/auction-application/src/ports/scheduling/`](src/ports/scheduling) | Worker 排期 bootstrap 与 `RecurringSchedule`；周排期、赛季早鸟与 IANA 时区规则在 `auction_domain::scheduling` |
| [`crates/auction-application/src/ports/public/`](src/ports/public) | 规则、黑名单、补偿池、装备链接、会话与登录时长偏好、装备标记与提醒偏好、读模型缓存 |
| [`crates/auction-application/src/ports/admin/`](src/ports/admin) | 拍卖创建/排期/列表、lot 修改、装备录入、论坛预览、邮件状态、规则、runtime 配置、凭据、用户、世界启停、审计、改名历史、结算目录 |
| [`crates/auction-application/src/ports/mail/`](src/ports/mail) | 轮询持久化、业务处理、注册/换绑、诊断证据、装备读取诊断、邮箱维护、展示投影 |
| [`crates/auction-application/src/ports/delivery/`](src/ports/delivery) | 投递 outbox Worker、发送节奏分组、Send/Take 期望冻结、Admin 投递列表与证据详情（尝试、Sent 观察/目击、领取、处置、来源义务与恢复）、人工 Sent 准入快照与判定 |
| [`crates/auction-application/src/ports/settlement/`](src/ports/settlement) | 结算命令、Worker 调度、履约、联合远端证据、资金、内部履约、报表 |
| [`crates/auction-application/src/ports/recovery/`](src/ports/recovery) | 付款超时恢复的调度、命令、健康与报表 |
| [`crates/auction-application/src/ports/forum/`](src/ports/forum) | 论坛主题同步、回帖导入、Public 镜像、装备标色规则、论坛私信、目标资料 |
| [`crates/auction-application/src/ports/lot_watch_pm/`](src/ports/lot_watch_pm) | 装备超价提醒私信的独立队列与人工对账 |
| [`crates/auction-application/src/ports/realtime/`](src/ports/realtime) | Public 回放、Worker 消费、Admin 运维 |
| [`crates/auction-application/src/ports/operations/`](src/ports/operations) | 运维告警、进程 readiness、Worker control 状态 |
| [external_metrics](src/ports/external_metrics) | 外部客户端观测闭集与 no-op |
| [shadow_run](src/ports/shadow_run) | W0.15 影子运行共享契约：owner/分支闭集、新旧比较 helper、结构化差异日志与 no-op 记录端口；旧路径独占裁决权 |

### 主要服务

| 服务 | 职责 |
| --- | --- |
| `CompetitionService` | 出价、创建/下调代理、慢速代理到期：锁内取快照 → 领域规划 → 原子写入事实、审计与 realtime |
| `AuctionLifecycleService` | Admin 与 Worker 共用的预览/开拍/Final/取消/提前结算；开拍时按排期日历计算后继场次 |
| `SettlementService` | 认领到期拍卖、计算 `SettlementPlan`、生成义务，结算事实一次提交；兼容重放旧版两阶段账本 |
| `SettlementFulfillmentService` | 履约 prepare → activate → reconcile；Bot 参与时走内部履约分支 |
| `MailProcessorService` | 按邮件主题路由：`auction` 投稿、`register`、`rebind`、结算退回邮件、其他邮件归档或退回 |
| `MailboxMaintenanceService` | 周期观察 compose 草稿、受控丢弃与 Sent 观察 |
| `RecoveryHealthService` / `RecoveryCommandService` | 余额与积压健康、Admin 恢复确认 |
| `RealtimeRelay` | Public 进程把持久化事件流转发到 WebSocket Hub |
| `PublicSessionService` / `AdminSessionService` | 会话签发与校验 |
| Admin 系列服务 | 凭据、用户、规则、runtime 配置、录入、世界、排期等后台命令 |
| `SettlementEquipmentLossPort` | 显式人工装备损失 preview/resolve；稳定命令绑定 actor/reason、UID、recovery/payment/delivery 版本和冻结卖家金额，安全投影不输出原载荷；外部买方补偿声明不等于完成回执，详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递) |
| `DeliverySentConfirmationPort` | 正式 CLI 与 Admin（`DeliverySentProofService` 选择已写入证据后）共用的人工 Sent preview/confirm；v1 观察绑定冻结正文摘要、身份、分钟时窗、金额与实际资产，已收取资产和旧尝试歧义分别显式接受；独立 operator proof 只恢复正常对账，详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递) |

## 外部客户端观测

`ExternalMetricsObserver` 是同步、无 I/O 的窄端口；指标实现由 `auction-observability`
提供，关闭 metrics 或非 Worker 角色使用 no-op。事件只含编译期闭集 label 与调用点
已有的时长/页数，不含玩家名、MID、主题/帖子 ID、URL、正文、错误原文或凭据。
记录不改变请求间隔、锁顺序、deadline、重试、取消、写入围栏或未知结果处理。
不同层（客户端、Worker、HTTP、数据库）的计数不能相加当作“总失败”。

论坛回帖端口增加两项只读投影：`ForumCommandStart.is_new` 表示本次事务是否首次
插入命令账本（`pending` 重试与已完成命令重放为 false）；`ForumReplyTopic.reply_phase`
来自领取时已连接的拍卖状态，仅用于指标分桶，不参与命令裁决，也不进入浏览器 DTO。
导入延迟终点是每个回帖第一条新命令账本提交得到确认，不是竞价最终接受。
完整指标契约见[可观测性](../auction-observability/README.md#外部账号客户端指标w04w05)。

## 横切约定

- **幂等账本**：竞价、生命周期及其他要求幂等重放的业务命令携带稳定 `command_id` 与 `RequestHash`（命令种类 + 聚合 ID + 请求 DTO 的 SHA-256）。同 ID 同摘要重放原结果，同 ID 不同摘要返回冲突。通用请求 JSON 直接流入摘要，不保留完整临时缓冲；邮件附件直接借用逐项序列化，避免再分配完整摘要投影列表，序列化字段顺序与字节编码保持不变。兼容旧系统的摘要统一经 `RequestHash::from_legacy_go_json` 等入口计算；登录、会话校验等端口使用各自的会话契约。
- **事务边界**：一个用例的状态变化、命令结果、追加式审计与 realtime 事件在同一事务提交；端口要求的隔离级别与重试（40001/40P01 有界重试）由存储实现负责。会写 `lots`/`auctions` 的场次级命令改用按场次排队入口：只知道 lot 的（普通出价、创建/降低代理、慢速代理执行、Admin draft lot 修改）用 `TransactionRunner::run_in_auction_lot_queue`，已知场次的（生命周期迁移、Admin 排期修改）用 `TransactionRunner::run_in_auction_queue`。两者必须落到同一把按场次的门：实现须在进入 Serializable 事务之前排队，隔离级别、行锁与账本语义不变；排队预算用尽返回 `QueueWaitTimeout`，不进入序列化重试。其余命令（World 启停、规则发布、创建新场次）继续用 `run`。设计、取得顺序、锁生命周期与实测见[数据库契约](../../docs/contracts/database.md#w121-落实按场次写队列竞价--生命周期--admin)。
- **竞价策略**：新建场次模板冻结统一加价与代理上限出价的 v2 策略；既有场次继续原版本。普通出价的事实 ID 数量与领域反击结果一致；慢速命令账本也接受有完整终态证据的耗尽结果。快照形状见[数据库契约](../../docs/contracts/database.md#拍卖与竞价)，金额规则见[业务规则](../../docs/business-rules.md#代理竞价)。
- **时间**：通过 `Clock` 注入；领域规划不读取系统时间。排期按本地日历偏移，偏移量或日期溢出返回 `InvalidSchedule`，不因构造时长而 panic。
- **赛季**：赛季前场次冻结 v2 恢复策略与 24 小时付款期限；配置按本地分钟先校验，具体场次物化和 Admin 改期共用 UTC 付款边界，要求实际计划结束加 24 小时严格早于换季日本地 00:00，防止 DST 前跳或改期突破预算。读取已有冻结来源仍沿用原解析语义，已冻结策略与命令结果不随新模板改写。早鸟查询逐场物化并在首个命中后停止，当前无效候选仍返回错误。同封投稿按冻结的世界与收件时间只选择一次品质列表，附件仍按原顺序逐件读取和验证。
- **取消与未知提交**：外写只能在持久 sending fence 后开始；内部证据的提交 guard、发送 fence 的取消 guard 在未确认提交或回滚时同步撤销原 owner。未知 Send 保留人工核对，Sent 未命中不能授权重发；已经接受的投稿优先恢复持久事实，不能因重试时远端附件变化改判退回。
- **Sent 收件人证明**：`mail_recipient_is_list_projection(list, full)` 只用于列表候选筛选，借用非空前缀并忽略 ASCII 大小写，允许列表末尾 U+FFFD、`…` 或 `...` 截断标记；空值和纯标记不匹配。`MoogleMailboxSentState.recipient` 必须来自详情完整收件人；冻结发送、历史结算和通知在记录证据或恢复前都逐字核对该完整身份（仅忽略 ASCII 大小写），不能以共享前缀完成证明。原正文、凭据、金额、附件和唯一性门禁保留；自动恢复时间窗从原 `sending_at` 所在分钟的起点（`sent_list_minute_start`）到 `outcome_recorded_at`；同分钟更早的同内容邮件由详情唯一匹配、MID 绑定唯一和存储侧整分钟歧义检查排除。
- **敏感值**：Cookie、装备 key、登录 key 等经专用值对象受控暴露，并对明文调试输出与丢弃清理设定边界；普通查询投影不含秘密。显式的凭据命令、Admin key 签发结果与投递端口可携带专用秘密类型，不能把这些类型直接映射为普通查询 DTO。
- **错误**：`ApplicationError` 区分能力缺失（`CapabilityUnavailable`，HTTP 层通常映射为 `501 feature_not_supported`）、领域错误、仓储错误与各 owner 的闭集错误；错误文本不回显输入中的秘密。

## 错误安全分类

[`crates/auction-application/src/errors/safe_kind.rs`](src/errors/safe_kind.rs) 的 `ApplicationError::safe_kind()` 返回下表闭集中的固定 `&'static str`，不分配字符串，不读取 Display、Debug、source 或错误正文，不包含秘密、金额、版本号或用户输入。分类仅用于诊断，不参与重试或业务判定；原错误、事务冲突检测与邮件重试分类保持原语义。既有分类值保持稳定，新增或细分分类时同步本表；本节是分类集合的唯一说明位置。

表中未细分的 owner 错误，其全部内层变体归入同一类别；事务错误与命令账本按下列精确原因分类。`RepositoryError` 当前只有 `Failed(String)`，不从文本推断冲突或可用性。

| 变体或内层原因 | `safe_kind` |
| --- | --- |
| `CapabilityUnavailable(_)` | `capability_unavailable` |
| `ForumIdentityRejected(_)` | `forum_identity_rejected` |
| `FulfillmentEvidenceNotObserved` | `evidence_not_observed` |
| `FulfillmentDependencyUnavailable` | `dependency_unavailable` |
| `FulfillmentAccountBusy` | `remote_busy` |
| `FulfillmentDependencyMismatch` | `dependency_mismatch` |
| `FulfillmentEquipmentSecretNotFound` | `equipment_secret_not_found` |
| `Domain(_)` | `domain_rejected` |
| `Repository(Failed(_))`；`CompetitionTransaction`、`SettlementTransaction`、`RecoveryCommand`、`AdminRulesTransaction`、`AdminMailTransaction` 的 `Failed(_)`；直接或嵌套的 `CommandLedgerError::Storage` | `repository_failed` |
| `DeliveryAdmin(_)` | `delivery_admin_error` |
| `CompetitionTransaction`、`SettlementTransaction`、`RecoveryCommand`、`AdminRulesTransaction`、`AdminMailTransaction` 的 `SerializationConflict` | `repository_serialization_conflict` |
| `CompetitionTransaction`、`SettlementTransaction`、`AdminRulesTransaction`、`AdminMailTransaction` 的 `Conflict` | `repository_conflict` |
| `CompetitionTransaction` 的 `NotFound`、`AdminIntakeUnavailable`、`IntakeClosed`、`AdminIntakeSellerBlacklisted`、`AdminIntakeEquipmentConflict`、`StaleState`、`InvalidState`、`BidderIneligible`、`StaleSlowProxy` | `competition_rejected` |
| `SettlementTransaction(NotFound)` | `settlement_not_found` |
| `RecoveryCommand(CommandConflict)`；直接或嵌套的 `CommandLedgerError::Conflict` | `command_conflict` |
| `RecoveryCommand` 的 `InvalidCommand`、`NotFound`、`StaleState`、`NotConfirmable`、`EvidenceUnresolved`、`InvalidStoredResult` | `recovery_rejected` |
| `RealtimeAdmin(_)` | `realtime_admin_error` |
| 直接或嵌套的 `CommandLedgerError::InvalidRecord` | `command_ledger_invalid` |
| `TransactionPolicy(_)` | `transaction_policy_invalid` |
| `Serialization(_)` | `serialization_failed` |
| `Session(_)` | `session_error` |
| `AdminRules(_)` | `admin_rules_error` |
| `AdminRulesTransaction(RevisionConflict { .. })` | `rules_revision_conflict` |
| `AdminMail(_)` | `admin_mail_error` |
| `AdminMailTransaction(NotFound)` | `mail_poll_state_not_found` |
| `AdminMailTransaction(StateConflict { .. })` | `mail_state_conflict` |
| `AdminIntake(_)` | `admin_intake_error` |
| `AdminForumPreview(_)` | `admin_forum_preview_error` |
| `ForumRemote`：`write_started == true` | `remote_write_started` |
| `ForumRemote`：`write_started == false && retryable == true` | `remote_retryable` |
| `ForumRemote`：`write_started == false && retryable == false` | `remote_rejected` |
| `AdminAudit(_)` | `admin_audit_error` |
| `AdminCredential(_)` | `admin_credential_error` |
| `AdminLotUpdate(_)` | `admin_lot_update_error` |
| `AdminRuntime(_)` | `admin_runtime_error` |
| `AdminRuntimeFile(_)` | `runtime_file_error` |
| `AdminRuntimeCommand(_)` | `runtime_command_error` |
| `AdminUser(_)` | `admin_user_error` |
| `AdminWorld(_)` | `admin_world_error` |
| `MailProcessing(_)` | `mail_processing_error` |
| `OperationsAlert(_)` | `operations_alert_error` |

命令账本的嵌套位置仅为 `CompetitionTransaction(CommandLedger(_))` 与 `SettlementTransaction(CommandLedger(_))`。论坛分类不直接返回 `ForumRemoteError::code()`；`remote_write_started` 只表示已开始写入，不推断最终结果。

首个调用方为 [`crates/auction-jobs/src/worker/runtime/settlement.rs`](../auction-jobs/src/worker/runtime/settlement.rs) 的 `process_fulfillment_claim`：仅在 reconcile 返回未持久化错误的原 warn 上增加 `safe_kind`、`delivery_id` 与固定 `phase="reconcile"`。日志不输出原错误或 claim 快照，保持原错误结果、租约和重试行为；HTTP、JSON、事件、配置及数据库契约不变。

### 资金与内部证据错误

`FundingError::safe_kind()` 与 `InternalEvidenceError::safe_kind()` 同样只穷尽匹配枚举并返回固定字面量，不读取错误正文，不参与资金、证据或重试判定；它们仍是独立端口错误，不新增 `ApplicationError` 变体。

| 错误变体 | `safe_kind` |
| --- | --- |
| `FundingError::Invalid` | `funding_invalid` |
| `FundingError::Conflict` | `funding_conflict` |
| `FundingError::Unavailable` | `funding_unavailable` |
| `FundingError::Unproven` | `funding_unproven` |
| `FundingError::Unsupported` | `funding_unsupported` |
| `FundingError::SerializationConflict` | `funding_serialization_conflict` |
| `FundingError::Storage` | `funding_storage_error` |
| `InternalEvidenceError::Unsupported` | `evidence_unsupported` |
| `InternalEvidenceError::Unproven` | `evidence_unproven` |
| `InternalEvidenceError::Unavailable` | `evidence_unavailable` |
| `InternalEvidenceError::Expired` | `evidence_expired` |
| `InternalEvidenceError::Revoked` | `evidence_revoked` |
| `InternalEvidenceError::InvalidRequest` | `evidence_request_invalid` |

### COD 实收 reconcile 位置分类

[`crates/auction-application/src/services/settlement/fulfillment/collected.rs`](src/services/settlement/fulfillment/collected.rs) 使用独立 target `auction::settlement::funding_diagnostics`；instrument span 仅携带义务、投递、稳定命令与闭集世界，不跨 await 持有 entered guard。每个位置使用下表固定 `safe_kind`，原错误类别仅进入 `cause_kind`，不记录错误文本或快照。

| 位置 | `safe_kind` |
| --- | --- |
| 缺少原 delivery | `collected_delivery_missing` |
| 未接内部端口时，原外部 satisfy 返回错误 | `collected_external_satisfy_failed` |
| 建立稳定命令失败 | `collected_command_invalid` |
| 原命令重放失败 | `collected_replay_failed` |
| 资金策略读取失败 | `collected_policy_read_failed` |
| 证据请求建立失败 | `collected_evidence_request_failed` |
| 无 canonical subject | `collected_subject_invalid` |
| 联合证据准备失败 | `collected_evidence_prepare_failed` |
| 准备失败后的原 legacy satisfy 也失败 | `collected_legacy_satisfy_failed` |
| 策略身份不等 | `collected_identity_mismatch` |
| check_current 失败 | `collected_evidence_current_failed` |
| begin_commit 失败 | `collected_commit_guard_failed` |
| 原 FundingDeferred 已确认提交 | `collected_funding_deferred` |
| 原 RolledBack 已确认回滚 | `collected_funding_rolled_back` |
| 事务返回 Err，尚无确认结果 | `collected_funding_outcome_unconfirmed` |
| 任一原 defer 持久化失败 | `collected_defer_persist_failed` |

`stage` 区分 `delivery`、`external_satisfy`、`command`、`replay`、`policy`、`evidence_request`、`subject`、`evidence_prepare`、`legacy_satisfy`、`identity`、`evidence_current`、`commit_guard`、`commit` 及 defer 失败时的 `funding_deferred`/`funding_rolled_back`。只记录有原错误的位置的 `cause_kind`；策略读取之前已丢失的内层原因不从文本反推。

身份不等时仍短路，不调用 check_current；原五处 defer 调用仍使用 `collected_funding_evidence_wait` 或 `collected_funding_wait`。原 Committed/FundingDeferred/RolledBack 的 guard 确认位置不变，Err 仍不确认并撤销 owner、返回原固定仓储错误。结果字段 `outcome` 只在已知分支写 `committed`、`rolled_back` 或 `unconfirmed`；不据日志自动重发或改写人工证明。

收据/价格拒绝码、前置存储阶段与辅助查询的唯一说明见 [Storage 资金拒绝字典](../auction-storage/README.md#cod-实收资金化拒绝诊断)。过滤与开关见[运维 D1 诊断](../../ops/README.md#d1-诊断)。仅增加日志，不改路由、JSON、事件、配置字段、数据库、持久化失败码或审计事实。

## 会话

- **Public**：玩家用登录 key 登录，原始 token 只在登录响应中出现一次，存储端只保存摘要。登录时长由账号偏好决定（默认永久，可设固定期限），活动不延长期限；每个用户的并发会话数受 `max_public_sessions`（默认 10）限制；满额时新登录在同一事务、同一用户行锁内注销最久未活动的有效会话（保留记录），不会拒绝登录。
- **Admin**：HTTP Basic 风格的 actor/secret 登录，会话与 CSRF token 以 HMAC 摘要保存，空闲与绝对过期滑动计算。服务启动时永久撤销既有 Admin 会话——重启 Admin 后须重新登录，配置凭据改回旧值也不能复活旧 token。

## 维护约定

- 新功能先定义端口与值对象，再在适配器 crate 中实现；不要在本 crate 引入 I/O 依赖。
- 新增命令必须定义稳定的命令种类字符串与请求摘要输入，一经发布不可修改。
- 投影与 DTO 的映射放在 `auction-http`，不要让端口类型直接成为对外协议。

`ExternalWriteLifecycle` 与逐工作项活动是无 Tokio 依赖的窄端口，由装配根桥接通用协调器。它只控制进程内取消，不决定持久租约、完成状态或重发资格。delivery/PM/discard 在围栏调用前同步登记，围栏调用开始即受保护；登记不授权外写，仍须确认围栏提交。论坛在已有持久 claim 下的首个实际 POST 前登记。交接与关键段同样计入活动、阻止排空结束，既有结果提交后才 complete；其余释放只由 drop 记为未收口。


## 运行时开关

`RuntimeSwitchName` 为审计模式、HTTP 就绪缓存、Worker SLI 汇总、Worker
邮件隔离和三角色指标转发拒绝绑定类型与角色。
`RuntimeSwitchCommand` 共用稳定摘要、重放输入及 CAS/到期规则。
`RuntimeSwitches` 保存完整不可变快照及 TOML 基线；刷新和到期仍由原服务负责。
SLI 开关不进入两项 readiness 缓存身份，不因观测切换使就绪缓存失效。
邮件隔离与指标转发由目标化 bool 访问器读取，不构造完整 `status()`。
`metrics_reject_forwarded_requests` 在命令校验层拒绝 `set false`，
只允许在线收紧；放宽须修改部署配置并重启。
先取最新事实再判期限，不恢复早期覆盖，不参与资金、资格或审计写入要求。
权威规则见[数据库契约](../../docs/contracts/database.md#运行时行为开关)。

## 安全错误记录

W0.2 由 I-A（jobs/application）与 I-B（integrations/HTTP）共同完成。
所有持久化 defer reason、failure_code、命令结果与对外协议保持原值。
同一函数不同原因用不同日志 kind/phase 和 safe_kind 区分，沿用 W0.7 的回退语义。

诊断入口为 `diagnostics::record_error`，只接收 `SafeErrorKind`、固定级别、
固定 kind/phase 和 `DiagnosticContext`，不能传入整个错误对象。
`DiagnosticError` 只投影类型化变体，不读取 Display、Debug、source 或负载文本。
固定安全类别使用关联常量，如 `SafeErrorKind::TIMEOUT`；`from_label` 仅用于
既有 `safe_kind()` 字符串进入闭集的边界映射，未知值归为 other。

kind 是简短的用例 owner，例如 `fulfillment_internal`、`mail_maintenance`，
不包含 crate 或模块路径前缀。phase 是语义步骤，例如 `read_identity`、`acquire_commit_guard`；
不使用函数名序号，同一 kind/phase 只对应同一种原因。超时与回执过期分别命名。
完整 owner 清单见[排障方式](../../ops/README.md#安全错误与未持久化指标)。
禁止把动态错误码、错误正文、URL/query、Cookie、密钥或请求/快照对象作为字段。
上下文只允许业务 UUID、闭集世界、框架路由模板和固定处置元数据。
`evidence_id` 是既有加密归档的 UUID；不输出证据正文、路径或密钥。
HTTP route 只同步借用框架 `MatchedPath`，不存进窗口；纯转换从安全业务 span 继承标识。
I-B 将闭集扩展为 113 类，外部自定义错误穷尽匹配，不能以 other 掩盖已知变体。
新增类别及边界见[集成记录规则](../auction-integrations/README.md#安全错误记录)。
`DiagnosticDisposition` 是处置闭集，当前仅有 `Unpersisted`，日志标签为 unpersisted。
它只表示未确认持久处置，不创建任何业务事实。

500、内部不变量和持久化处置失败用 ERROR；依赖、超时和未确认结果用 WARN；
正常证据等待和未启用能力用 INFO/DEBUG。日志目标为 `auction::diagnostics`。
Worker 的本地 warn/error/info/debug helper 统一字段顺序，`unpersisted_error` 添加处置枚举。
通过窄 `DiagnosticSink` 在原长期 owner 中复用 60 秒 `RateLimitedLog`。
指标 observer 先于日志过滤与去重，日志门只缓存固定类别与计数，不保存首条事件或业务标识。
未安装门时直接输出安全日志；门不可用或容量不足时放行，不影响业务。

`Unpersisted` 只由未确认对应持久处置的最终分支设置，
不能用返回 Err、WorkerResult::Error 或一般重试错误代替。
详见[指标口径](../auction-observability/README.md#未持久化错误)
与[排障方式](../../ops/README.md#安全错误与未持久化指标)。


## SLI 只读端口

`ports/sli` 拥有 `SliSnapshotReader`、固定世界/命令类型、数值快照及闭集错误。
storage 拥有全部 SQL，jobs 只负责唤醒，observability 只编码内存。
快照不包含业务标识或秘密，不参与业务判定，也不授权外部写入或重试。
`metrics_sli_snapshot_enabled` 仅 Worker 消费，TOML 基线默认 false；
沿用既有开关解析、刷新、期限和审计，不进入 readiness 缓存身份。
完整指标口径见[SLI 文档](../auction-observability/README.md#sli-与容量基线)。

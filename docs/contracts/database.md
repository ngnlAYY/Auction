# 数据库契约

[文档导航](../README.md) · [迁移](../../migrations/postgres/README.md) · [auction-storage](../../crates/auction-storage/README.md) · [保留政策](../../ops/deploy/retention-policy.v1.json)

唯一支持的数据库是 **PostgreSQL 18**。运行期 SQL 由 `auction-storage` 拥有，schema 由 [`migrations/postgres`](../../migrations/postgres) 中的追加式迁移定义（当前最高版本 `000075`，另有 SQLx 管理的 `_sqlx_migrations`），编译期内嵌进启用 `postgres` 的制品。

## schema 身份与版本

- `auction_schema_identity` 是单行表，记录应用名与基线版本，用来区分“Auction 的库”与其他库（备份与恢复都会核对它）。迁移只在空库或已有迁移记录的库上执行，没有迁移记录却存在用户对象的库被拒绝。
- `_sqlx_migrations` 记录已执行的迁移。已知版本逐条核对完整序列、成功状态及 checksum，少数历史摘要仅接受[迁移兼容层](../../migrations/postgres/README.md#执行兼容层)显式允许的值。readiness 默认保持完全匹配；仅显式兼容模式允许下述已登记的未来后缀。恢复预检、备份、盘点及迁移执行的严格版本边界不放宽。
- 普通启动**从不**隐式迁移。迁移只能通过 `auction-cli db migrate`（或 `service start/restart`）以及合并部署首次初始化执行，见 [auction-cli](../../apps/auction-cli/README.md#数据库)。
- 迁移只向前，不提供回滚；已发布的迁移文件永不修改。

### 迁移兼容登记与就绪

`public.schema_migration_compatibility` 由迁移 owner 拥有：`version bigint` 为主键且必须大于 74；`level text` 非空、默认 `breaking`，闭集为 `compatible` / `breaking`；`recorded_at timestamptz` 非空、默认 `CURRENT_TIMESTAMP`，表示登记事务时间。迁移名仍由 SQLx history 提供，不重复存储。UPDATE、DELETE 与 TRUNCATE 均由触发器以 `55000` 拒绝；不得改写旧等级，修正只能新增迁移。该表永久保留，不授权清理。

001–074 不回填，**缺行即 breaking，绝不视为 compatible**；075 建表并登记自身为 `breaking`。此后每个迁移在自身 SQLx 事务内、以末尾唯一一条规范 INSERT 登记本版本。SQLx 随后在同事务写入本版本成功记录，因此不建立即时外键。登记编写与分类依据见[迁移说明](../../migrations/postgres/README.md#兼容分级与登记)。

令 `E` 为二进制内嵌最高版本、`D` 为数据库最高版本：

| 数据库状态 | schema 就绪判定 |
| --- | --- |
| `D < E` | 所有模式均拒绝，不能用兼容登记补足未执行迁移 |
| `D = E` | 所有模式均要求完整历史严格匹配，失败记录、缺项及未知摘要均拒绝 |
| `D > E` | `strict` 拒绝；`shadow` 实际仍拒绝、同时观测兼容判定；`compatible` 仅在已知前缀严格匹配且 `E+1…D` 连续、全部成功、每个版本均登记为 `compatible` 时通过 |

超前判定以 `_sqlx_migrations` 实际记录 LEFT JOIN 登记表驱动，在同一个有界可重复读只读快照中完成；缺表、缺行、未知等级、breaking 或查询失败均拒绝。旧二进制无法核对未来文件的内嵌 checksum；未来后缀依据同事务成功记录与兼容声明接受，不能据此放宽已知版本摘要校验。

模式配置见[就绪策略](../../ops/config/README.md#就绪策略)；回退前必须显式切换 `compatible`，且目标旧程序须已包含本逻辑，详见[程序回退](../../ops/deploy/RECOVERY.md#程序回退)。审计、Public relay 等其他就绪条件继续生效，Worker control 的运行状态契约不扩展。

## 表分组

分组与[保留政策](../../ops/deploy/retention-policy.v1.json)的数据族一致。保留政策当前对所有业务数据族都是 `retain_forever`：`auction-cli db inventory` 只统计、不授权任何归档或删除。

### 配置与 schema

| 表 | 内容 |
| --- | --- |
| `auction_schema_identity` | schema 身份（单行） |
| `schema_migration_compatibility` | 迁移兼容声明，只追加、永久保留；缺行失败关闭 |
| `worlds` | 业务世界（`main`、`isekai`）及启用状态 |
| `rules_documents`、`rules_admin_commands` | 各语言规则文档与发布命令账本 |
| `runtime_config_admin_commands` | runtime 配置更新命令账本（文件写入的恢复依据） |

### 拍卖与竞价

| 表 | 内容 |
| --- | --- |
| `auctions` | 场次：世界、期号、状态、冻结的经济/结算/恢复策略快照、计划/有效截止、`state_version`、早鸟窗口内场次的最早收件时刻 `intake_opens_at`（赛季开始，也是旧赛季装备不能重列的边界；迁移前的场次保持 NULL，不回填）。身份字段（世界、期号等）由触发器禁止修改 |
| `lots` | lot：装备、编号、状态（pending/live/sold/unsold/withdrawn）、起拍价、当前价、领先者、有效截止、慢速代理到期、`state_version` |
| `equipment` | 装备公开属性（世界内按外部装备 ID 唯一）；`attributes.stats` 保持名称到 value/Base 的对象，HTML 新快照按[来源分组规则](../../crates/auction-integrations/README.md#关键行为)保留完整属性键，历史键不改写 |
| `equipment_secrets` | 加密保存的装备 key（`equipment_data_key` 信封） |
| `auction_commands` | 竞价与生命周期等命令的幂等账本（命令 ID、种类、聚合、请求摘要、结果） |
| `bids` | 被接受的出价事实：金额、机制（ordinary/proxy）、接受后的截止与版本；**只追加**，同一命令内事实序号连续 |
| `bid_proxy_authorizations` | 代理授权当前状态 |
| `bid_proxy_authorization_transitions` | 代理状态转换事实，只追加、序号连续 |
| `bid_proxy_attempts`、`bid_proxy_outbid_uses` | 代理反超尝试与“设置代理次数”计数 |
| `auction_submissions` | 经邮件投稿进入场次的装备来源（邮箱、邮件 ID、卖家） |
| `auction_intake_targets`、`auction_preview_items` | 各世界当前接收投稿的草稿场次；预览目录项 |
| `scheduled_rounds` | 周排期与早鸟场次的轮次身份（常规周按世界+本地周唯一，早鸟按 `slot_key` 唯一；`early-bird:YYYY-MM-DD:N`，N 为 0 时是赛季前场次） |
| `auction_forum_sync`、`forum_sync_admin_commands`、`auction_forum_bid_commands`、`auction_forum_catalog_pages` | 论坛主题同步状态（含租约与 fencing）、人工授权、回帖竞价命令、目录分页帖 |

`auctions.policy_snapshot` 的竞价版本在创建场次时冻结：既有 `schema_version=4` 解码为竞价 v1；新建场次写入 `schema_version=5`、数值 `bidding_version=2`，统一最低增量并允许代理用尽上限反超。迁移 `000069` 仅放宽快照 CHECK，拒绝 v5 缺少版本、版本非数值 2 或未知 schema，不回填或改写任何旧场次。普通竞价、慢速到期、Final 与账户代理降额能力共用该版本；历史事实和命令结果保持原样。价格语义见[业务规则](../business-rules.md#代理竞价)。

### 装备标记

| 表 | 内容 |
| --- | --- |
| `lot_watches` | 用户对 lot 的标记与提醒渠道 |
| `lot_watch_leadership`、`lot_watch_notifications` | 连续领先周期与“被超价”提醒记录 |
| `lot_watch_pm_deliveries` | 论坛私信提醒队列（租约、fencing、人工对账） |

### 身份与安全

| 表 | 内容 |
| --- | --- |
| `users` | 用户：名称、状态（active/blacklisted）、登录 key 摘要、游戏身份、论坛账号、登录时长偏好 |
| `user_login_key_envelopes` | 加密保存的登录 key（`user_login_data_key` 信封，用于邮件重发） |
| `user_blacklist`、`user_blacklist_history` | 黑名单当前状态与只追加历史 |
| `user_renames`、`user_game_identity_history`、`forum_identity_history`、`forum_bidder_identities` | 改名、游戏身份与论坛身份的历史 |
| `user_admin_commands`、`user_mail_commands` | Admin 用户命令与邮件注册/换绑命令账本 |
| `credential_keys`、`credential_admin_commands` | 加密保存的邮件/论坛 Cookie（`credential_data_key` 信封）及其命令账本；每种 kind 同时最多一个 active |

### 结算、资金、恢复与投递

| 表 | 内容 |
| --- | --- |
| `auction_settlements`、`lot_settlements` | 场次与 lot 的结算结果（成交价、买方实付、卖方所得、税费、补偿） |
| `compensation_pool`、`compensation_ledger` | 两个世界共用的补偿池（单行，main Credits）与只追加流水 |
| `settlement_pools` | 迁移 000071 合并前各世界补偿池的期末余额与版本（触发器冻结，只作证据） |
| `compensation_pool_reversals` | 成交撤销后的补偿池冲正（每个恢复记录最多一条，与 `resolution` 落定同事务写入） |
| `settlement_obligations` | 结算义务（种类闭集、世界、收款人、金额、前置义务、业务状态） |
| `settlement_obligation_fulfillments` | 义务履约执行与证据 |
| `settlement_internal_commands` | Bot 内部履约的命令与完整证明 |
| `settlement_funding_scope`、`settlement_funding_accounts`、`settlement_funding_sources`、`settlement_funding_entries`、`settlement_funding_allocations`、`settlement_funding_lots`、`settlement_funding_receipts`、`settlement_funding_returns`、`settlement_funding_commands` | Bot 资金范围、账户、来源、分录、用途占用、lot 绑定、收款绑定、返还与命令账本；账户策略（strict_cash/record_only）由触发器禁止改写 |
| `settlement_legacy_self_reconciliations`、`settlement_legacy_bot_seller_claims` | 历史 Bot 自交易结算的核对证明（只追加） |
| `settlement_equipment_losses` | 装备损失的人工责任声明与原 owner 快照；command/lot/recovery/payment/delivery/seller/refund 各自唯一，只追加，永久退役原投递 |
| `settlement_recoveries`、`settlement_recovery_actions`、`settlement_recovery_commands`、`settlement_recovery_health`、`settlement_recovery_return_consumptions` | 付款超时恢复的状态、动作、人工确认、健康与 COD 退回计数 |
| `deliveries` | 出站邮件 outbox：来源、收件人、负载、状态（prepared → pending → claimed → sending → succeeded / failed / manual_reconcile）、租约与 fencing |
| `delivery_send_attempts`、`delivery_sent_bindings`、`delivery_take_attempts` | 发送/收取尝试的摘要与远端邮件 ID 绑定（只追加），用于结果未知时的对账 |
| `delivery_sent_confirmations` | 正式 CLI 的人工 Sent 观察命令、请求摘要、接受项、原 owner 快照与结果；只追加，按当前 delivery/fence 和 world/MID 唯一 |
| `delivery_send_cadence`、`delivery_admin_commands` | 同类邮件发送节奏；Admin 投递操作账本 |

装备损失只能经 `SettlementEquipmentLossPort` 正式运维命令处置。准入为 Main 已结算成交、原 `winner_equipment_cod` 的首次且唯一库存读取失败（`attempt_count=1`、`fencing_version=1`、`not_observed`、`inventory/read/write_started=false`）、无 claim、无 Sent 绑定、无恢复动作或既有处置、无资金 lot 或资产冲突；历史多次尝试缺少逐次未外写证明，失败关闭。账户必须唯一且为 ready/record_only，strict_cash 不支持。

同一 Serializable 事务写入只追加损失事实及审计，CAS 三个期望版本；相同 command ID 同参先重放，异参或其他 ID 重处置拒绝，序列化冲突/死锁最多尝试三次。锁序为 command advisory → runtime generation → worlds → funding scope/account → auctions → recoveries → lots → equipment → users → obligations → recovery actions → deliveries → fulfillments。原 delivery 终结为 failed/equipment_destroyed 并递增 fencing，Admin 所有新动作均拒绝该退休投递。payment 以 `operator_equipment_loss_v1:<command_id>` 及请求摘要满足；这只是人工处置证明。refund 终结为 failed/operator_external_buyer_compensation 并保留同一人工证明，表示其执行责任转由操作者承担，不表示买方已获补偿。

`settlement_funding_lots.source_kind=operator_equipment_loss`；资金来源 `record_operator_equipment_loss` 只记录操作者承担的冻结 buyer_amount，原 compensation_amount 继续来自 `record_compensation`，按冻结 B+C=S+T 分配 seller/tax，不创建 collected_payment 或实收 receipt。recovery 完成为 completed/resolution=NULL，不触发成交撤销、补偿池冲正、重列或黑名单。卖家只从 blocked 释放为 ready，由原正常 SellerPayout/Worker 流程真实付款；原 lot/auction settlement、补偿账本与冻结策略保持不可变。资金预览仅在损失事实、退休投递、人工证明及资金 lot 完整匹配时识别 refund 的终结和原投递的退休。

人工 Sent 收口只经 `DeliverySentConfirmationPort`，支持 main/settlement 的单件 `winner_equipment_cod`、`unsold_return`、`withdrawn_return` 及已寄出货款的 `seller_payout`。准入二选一且均须当前无租约/claim：结果未知的 `manual_reconcile/write_unknown/write_started=true`；或 Admin 对账判失败的 `failed` 投递（`result_metadata.resolution=admin_reconciled_failed`，命令账本中最新一条 `reconcile` 命令结果为 `failed`、fence 与失败码仍等于当前行，履约以同一失败码 `failed`），预览以 `corrects_admin_failure` 标明。两者都须具备同 command/hash/sending_at 的不可变当前发送尝试；退休装备损失、已有 MID、其他相同内容未绑定投递或付款提醒以外的恢复动作均拒绝，恢复只允许处于 `awaiting_payment`/`payment_overdue` 且无处置。原凭据必须仍为 active 同版本，sender UID 绑定唯一 ready 资金账户，有预期 player_name 时逐字匹配。正文原摘要、世界、完整收件人、主题与冻结资产均核对，历史正文不按当前模板重渲染。

装备分支继续拒绝既有收款、资金 lot 或占用，核对冻结 item_name、零 Credits、精确 COD 与单件 EID。货款分支只接受完整正额 Credits、零 COD 与空 EID：未领取时原额仍在且回执为空，已领取时剩余为零并带唯一原生 `Removed attachment: <千分位冻结金额> Credits` 回执；金额、收件人、payload 必须与冻结卖方义务和结算一致。原资金 lot 必须为 `collected/seller`，原 `seller/unknown` allocation 的 UID、lot、义务、delivery 和金额，以及 reserve command、reserve/unknown 分录均须精确匹配。

`ManualSentEvidence` v1 严格拒绝未知字段；`sent_minute` 为 UTC 整分钟，结果未知时半开区间 `[minute,minute+60s)` 必须与原 `sending_at..completed_at` 相交；Admin 失败更正时须落在投递激活所在分钟至失败结论（`completed_at`）之间，早于当前尝试即要求 `accept_attempt_history`。观察不能早于原发送完成或晚于数据库当前时间。未收取时原/观察正文摘要一致、实际剩余资产精确相等且 receipt 为空；装备已收取时实际资产全零、EID 数组为空，唯一原生 `Removed attachment: <冻结名称>` 及可选精确 `CoD Paid: <千分位金额> Credits` 回执绑定原期待，观察正文摘要必须非零且不同于原摘要。此时须显式 `accept_collected_assets` 承担当前 EID 已不可见的人工关联责任；旧未绑定尝试须另行 `accept_attempt_history`，全部旧尝试及原未知结果保留。货款已领取同样须显式 `accept_collected_assets`；附件现金在发送时已离开账户，领取不改变资金结论。

同一 Serializable 事务按 command advisory → runtime generation → worlds → funding scope/account → auctions → recoveries → lots → equipment → credential → users → obligations → recovery actions → deliveries → fulfillments 顺序锁定，最多三次重试序列化冲突/死锁。同 ID 同参优先重放，异参、旧 fence 或冲突拒绝；追加 confirmation 与审计、引用实际 sending fence 的唯一 MID binding，原 delivery 保持当前 fence 改为 succeeded。独立标签 `sent_operator_confirmed_v1` 带 `external_reference_proven=true`，不产生 `sent_verified=true`。`delivery_has_operator_sent_proof` 必须匹配不可变 confirmation、原 snapshot、当前 attempt、binding、当前 delivery 与原摘要；迁移 `000074` 起原 snapshot 可为上述 Admin 失败结论，此时须在命令账本按命令 ID、结果状态、fence 与失败码找到该失败命令且其后无新命令；共享 `delivery_has_frozen_sent_proof` 保持原自动证明并接受这一强谓词，履约 claim、内部收款、资金收据与恢复/退货消费方均不可仅凭 operator 标签成功。

人工命令复用发送恢复 helper 把原 fulfillment 从 manual_reconcile（Admin 失败更正时从 failed）恢复 enqueued，不创建新投递或收款来源；更正移除投递上的失败标记，原失败结论保留在命令账本、审计与 confirmation 快照中。装备分支随后由 Worker 读取真实当前 Sent：未领取继续等待；已领取由原资金 owner 使用同账号会话证明记录 COD 实收，并按原冻结金额放行卖方付款。货款分支在同事务复用自动 recovered-send 的 `finish_funded_delivery(...Sent)`，将原 unknown allocation 记为 spent，追加一次以原 delivery 身份绑定的支出分录；原金额及资金来源不变，同 ID 重放不重复入账。收件人领取仍由正常履约流程核验，原 Admin 手填引用成功的 Send 禁止保持不变。

### 邮件来源

| 表 | 内容 |
| --- | --- |
| `inbound_mail` | 每封已处理入站邮件的种类、内容摘要、决定与安全元数据（邮箱 + 邮件 ID 唯一） |
| `mail_commands`、`mail_admin_commands` | 邮件业务命令与 Admin 邮件操作账本 |
| `mail_poll_state` | 各邮箱的轮询游标、暂停状态与租约 |

### 实时与运维

| 表 | 内容 |
| --- | --- |
| `realtime_streams`、`realtime_events` | 事件流与事件（流内序号连续），见[实时事件](realtime.md) |
| `realtime_consumer_offsets`、`realtime_failures` | 各 consumer 游标与转发失败记录 |
| `operations_alert_runtime_state`、`operations_alert_signals`、`operations_alert_batches`、`operations_alert_batch_items` | 运维告警的运行状态、信号、批次与批次项 |
| `service_heartbeats` | 三角色进程心跳（TTL 30 秒），是唯一被标记为“待决定是否短期保留”的数据族；`audit_failure_count` 在发布进程为 legacy/shadow 时是全部审计失败累计，classified 时仅为阻断就绪的 permanent + unknown 累计，成功写入不清零、重启归零；结构不变，完整分类计数在进程指标中保留，见[就绪策略](../../ops/config/README.md#就绪策略) |
| `audit_records` | 追加式审计：操作者类型（public_user/admin/worker/system）、动作、实体、结果、来源与安全元数据；触发器禁止修改与删除 |

### 当前态投影

`public_sessions`、`admin_sessions`、`auction_forum_reply_cursors`、`mail_poll_pending_reads`、`mailbox_sweep_cursors`、`auction_intake_targets`、`mail_runtime_generation` 只表示当前运行状态，由各自 owner 的生命周期维护，不作为历史事实。

## 共有约束

- **幂等账本**：每个写命令 owner 都有命令表，以 `command_id` 为键保存 32 字节请求摘要与结果。同 ID 同摘要重放、异摘要冲突。
- **乐观版本**：可变聚合（拍卖、lot、用户、义务、投递等）带 `state_version` 或 `fencing_version`，命令以期望版本做 CAS。
- **只追加事实**：出价、代理转换、黑名单历史、审计、发送/收取尝试及 Sent 绑定、COD 退回计数、历史核对证明、场次与 lot 结算结果、补偿账本及冲正由触发器拒绝 UPDATE/DELETE。资金账户仅冻结 `accounting_mode`/`policy_version`，其他运行状态可变；改名的原始名称证据保留，但撤销会更新 `user_renames.reverted_at`。
- **租约与 fencing**：Worker 领取的工作项（拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、私信、告警）都有 `lease_token`/`lease_expires_at` 与单调递增的 fencing 版本；租约过期可被重新领取，处于“发送中”的外部写入在结果未知时转为人工对账，绝不自动重发。
- **事务**：竞价、关键生命周期等命令使用 Serializable 隔离，序列化失败（40001）与死锁（40P01）由 owner 有界重试；同一幂等命令 ID 使用共用事务级 advisory lock 协调。运行期 claim、进度及部分账本写入也使用默认隔离事务，以行锁、租约/fencing、唯一约束等保障各自不变量，并非所有写入都是 Serializable。
- **失败诊断**：SQL 通用失败在原错误变体中附加 SQLSTATE 类别/代码及可用的约束名、表名，不保留驱动原文或业务值；字段可能缺失，错误文本不得决定自动重试或重发。格式与安全边界见 [Storage SQL 错误安全上下文](../../crates/auction-storage/README.md#sql-错误安全上下文)，不改变数据库 schema 或事务契约。
- **运行时独占**：Public realtime consumer、邮件 Cookie 与论坛 Cookie 的运行时 owner 使用 session advisory lock；运维工具的建库/重建使用集群级 advisory lock。
- **唤醒**：新的可发送投递插入后触发 `pg_notify('auction_delivery_ready')`，Worker 投递循环据此提前唤醒（轮询仍是兜底）。
- **CHECK 约束**：金额非负/为正、状态闭集、文本已修剪且非空、时间有序、互斥字段二选一等规则在数据库层强制，不依赖应用代码。

## 变更规则

1. 新增表、列、约束或索引只能新增迁移文件（编号连续），见[迁移说明](../../migrations/postgres/README.md)。
2. 每次新增迁移都把[保留政策](../../ops/deploy/retention-policy.v1.json)的 `database_schema_version` 更新为新的最高版本（否则 `db inventory` 拒绝运行，发布身份也会变化）；新增表还须加入对应数据族。
3. 同步更新本页的表分组。
4. 迁移须能在生产数据上安全执行：不得静默改写历史事实；需要修复数据时只修复有确凿证据的部分，其余保留原状并交给人工核对。

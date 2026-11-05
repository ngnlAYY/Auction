# 数据库契约

[文档导航](../README.md) · [迁移](../../migrations/postgres/README.md) · [auction-storage](../../crates/auction-storage/README.md) · [保留政策](../../ops/deploy/retention-policy.v1.json)

唯一支持的数据库是 **PostgreSQL 18**。运行期 SQL 由 `auction-storage` 拥有，schema 由 [`migrations/postgres`](../../migrations/postgres) 中的追加式迁移定义（当前最高版本 `000087`，另有 SQLx 管理的 `_sqlx_migrations`），编译期内嵌进启用 `postgres` 的制品。

## schema 身份与版本

- `auction_schema_identity` 是单行表，记录应用名与基线版本，用来区分“Auction 的库”与其他库（备份与恢复都会核对它）。迁移只在空库或已有迁移记录的库上执行，没有迁移记录却存在用户对象的库被拒绝。
- `_sqlx_migrations` 记录已执行的迁移。已知版本逐条核对完整序列、成功状态及 checksum，少数历史摘要仅接受[迁移兼容层](../../migrations/postgres/README.md#执行兼容层)显式允许的值。readiness 默认保持完全匹配；仅显式兼容模式允许下述已登记的未来后缀。恢复预检、备份、盘点及迁移执行的严格版本边界不放宽。
- 普通启动**从不**隐式迁移。迁移只能通过 `auction-cli db migrate`（或 `service start/restart`）以及合并部署首次初始化执行，见 [auction-cli](../../apps/auction-cli/README.md#数据库)。迁移与建库、重建、恢复一样必须用迁移 owner 凭据（`--owner-secrets`）；服务运行账号没有 DDL 权限。
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
| `runtime_switch_events` | 行为开关的只追加事实兼命令账本；最新事实解析、期限与重放见下文 |

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

论坛回帖导入游标不单独建表：增量扫描以 `auction_forum_bid_commands` 中该主题 `post_number` 最高的行作为“已处理到的最大帖子 ID”，页号由楼层与每页 20 楼换算。每轮仍读取固定楼层所在页，并先核对游标页仍包含该 `post_id`；帖子被删除或楼层前移时回退完整扫描。迁移 `000005` 建立的 `auction_forum_reply_cursors` 仍无人读写，其 `(scope, last_scan_at)` 结构无法表示按主题的帖子与页码，留给 W8.2 评估清理，本项不追加迁移。

`auctions.policy_snapshot` 的竞价版本在创建场次时冻结：既有 `schema_version=4` 解码为竞价 v1；新建场次写入 `schema_version=5`、数值 `bidding_version=2`，统一最低增量并允许代理用尽上限反超。迁移 `000069` 仅放宽快照 CHECK，拒绝 v5 缺少版本、版本非数值 2 或未知 schema，不回填或改写任何旧场次。普通竞价、慢速到期、Final 与账户代理降额能力共用该版本；历史事实和命令结果保持原样。价格语义见[业务规则](../business-rules.md#代理竞价)。

### 装备标记

| 表 | 内容 |
| --- | --- |
| `lot_watches` | 用户对 lot 的标记与提醒渠道 |
| `lot_watch_leadership`、`lot_watch_notifications` | 连续领先周期与“被超价”提醒记录 |
| `lot_watch_pm_deliveries` | 论坛私信提醒队列（租约、fencing、人工对账） |
| `lot_watch_events` | 已提交竞价/新建代理的提交后处理 outbox：事件身份、`pending`/`processing`/`done` 进度、租约与同 lot `state_version` 顺序屏障；不是业务状态来源 |

出价被超提醒的发送资格只到对应 lot 的有效截止（含软截止延长，`lots.effective_end_at`）为止：截止后尚未开始发送的 MoogleMail 投递在领取预检中退役为 `failed`，`failure_code`/`reason_code` 为 `lot_watch_invalidated` 并带 `outcome_unknown=false`、`preflight_blocked=true`；论坛私信队列对应行退役为 `suppressed`、`failure_code='source_invalid'`。两者都不写 `sending_at`，也不删除来源记录；已经 `sending_at` 非空的行不参与退役，其外部结果仍只由原 sender 对账。提醒保持不合并、不限额（决策 1、决策 14）。

### 身份与安全

| 表 | 内容 |
| --- | --- |
| `users` | 用户：名称、状态（active/blacklisted）、登录 key 摘要、游戏身份、论坛账号、登录时长偏好 |
| `user_login_key_envelopes` | 加密保存的登录 key（`user_login_data_key` 信封，用于邮件重发） |
| `user_blacklist`、`user_blacklist_history` | 黑名单当前状态与只追加历史 |
| `user_renames`、`user_game_identity_history`、`forum_identity_history`、`forum_bidder_identities` | 改名、游戏身份与论坛身份的历史 |
| `user_admin_commands`、`user_mail_commands` | Admin 用户命令与邮件注册/换绑/轮换命令账本；`inbound_mail.mail_kind` 含 `rekey` |
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
| `delivery_dispositions` | 投递异常终态的统一处置事实（迁移 `000088`）：种类闭集 `auto_sent_proof`、`operator_sent_confirmation`、`admin_failed`、`equipment_loss`、`legacy_self`、`preflight_retired`、`operator_acknowledged`；每种恰好一个来源引用（自动 Sent 绑定、人工 Sent 证明命令、Admin 账本命令、装备损失命令、历史自寄修复命令、操作者确认审计），预检退役以投递行自身为证；只追加，不改变投递状态、资金、履约或资产 |

装备损失只能经 `SettlementEquipmentLossPort` 正式运维命令处置。准入为 Main 已结算成交、原 `winner_equipment_cod` 的首次且唯一库存读取失败（`attempt_count=1`、`fencing_version=1`、`not_observed`、`inventory/read/write_started=false`）、无 claim、无 Sent 绑定、无恢复动作或既有处置、无资金 lot 或资产冲突；历史多次尝试缺少逐次未外写证明，失败关闭。账户必须唯一且为 ready/record_only，strict_cash 不支持。

同一 Serializable 事务写入只追加损失事实及审计，CAS 三个期望版本；相同 command ID 同参先重放，异参或其他 ID 重处置拒绝，序列化冲突/死锁最多尝试三次。锁序为 command advisory → runtime generation → worlds → funding scope/account → auctions → recoveries → lots → equipment → users → obligations → recovery actions → deliveries → fulfillments。原 delivery 终结为 failed/equipment_destroyed 并递增 fencing，Admin 所有新动作均拒绝该退休投递。payment 以 `operator_equipment_loss_v1:<command_id>` 及请求摘要满足；这只是人工处置证明。refund 终结为 failed/operator_external_buyer_compensation 并保留同一人工证明，表示其执行责任转由操作者承担，不表示买方已获补偿。

`settlement_funding_lots.source_kind=operator_equipment_loss`；资金来源 `record_operator_equipment_loss` 只记录操作者承担的冻结 buyer_amount，原 compensation_amount 继续来自 `record_compensation`，按冻结 B+C=S+T 分配 seller/tax，不创建 collected_payment 或实收 receipt。recovery 完成为 completed/resolution=NULL，不触发成交撤销、补偿池冲正、重列或黑名单。卖家只从 blocked 释放为 ready，由原正常 SellerPayout/Worker 流程真实付款；原 lot/auction settlement、补偿账本与冻结策略保持不可变。资金预览仅在损失事实、退休投递、人工证明及资金 lot 完整匹配时识别 refund 的终结和原投递的退休。

人工 Sent 收口只经 `DeliverySentConfirmationPort`，支持 main/settlement 的单件 `winner_equipment_cod`、`unsold_return`、`withdrawn_return` 及已寄出货款的 `seller_payout`。准入二选一且均须当前无租约/claim：结果未知的 `manual_reconcile/write_unknown/write_started=true`；或 Admin 对账判失败的 `failed` 投递（`result_metadata.resolution=admin_reconciled_failed`，命令账本中最新一条 `reconcile` 命令结果为 `failed`、fence 与失败码仍等于当前行，履约以同一失败码 `failed`），预览以 `corrects_admin_failure` 标明。两者都须具备同 command/hash/sending_at 的不可变当前发送尝试；退休装备损失、已有 MID、其他相同内容未绑定投递或付款提醒以外的恢复动作均拒绝，恢复只允许处于 `awaiting_payment`/`payment_overdue` 且无处置。原凭据必须仍为 active 同版本，sender UID 绑定唯一 ready 资金账户，有预期 player_name 时逐字匹配。正文原摘要、世界、完整收件人、主题与冻结资产均核对，历史正文不按当前模板重渲染。

装备分支继续拒绝既有收款、资金 lot 或占用，核对冻结 item_name、零 Credits、精确 COD 与单件 EID。货款分支只接受完整正额 Credits、零 COD 与空 EID：未领取时原额仍在且回执为空，已领取时剩余为零并带唯一原生 `Removed attachment: <千分位冻结金额> Credits` 回执；金额、收件人、payload 必须与冻结卖方义务和结算一致。原资金 lot 必须为 `collected/seller`，原 `seller/unknown` allocation 的 UID、lot、义务、delivery 和金额，以及 reserve command、reserve/unknown 分录均须精确匹配。

`ManualSentEvidence` v1 严格拒绝未知字段；`sent_minute` 为 UTC 整分钟，结果未知时半开区间 `[minute,minute+60s)` 必须与原 `sending_at..completed_at` 相交；Admin 失败更正时须落在投递激活所在分钟至失败结论（`completed_at`）之间，早于当前尝试即要求 `accept_attempt_history`。观察不能早于原发送完成或晚于数据库当前时间。未收取时原/观察正文摘要一致、实际剩余资产精确相等且 receipt 为空；装备已收取时实际资产全零、EID 数组为空，唯一原生 `Removed attachment: <冻结名称>` 及可选精确 `CoD Paid: <千分位金额> Credits` 回执绑定原期待，观察正文摘要必须非零且不同于原摘要。此时须显式 `accept_collected_assets` 承担当前 EID 已不可见的人工关联责任；旧未绑定尝试须另行 `accept_attempt_history`，全部旧尝试及原未知结果保留。货款已领取同样须显式 `accept_collected_assets`；附件现金在发送时已离开账户，领取不改变资金结论。

同一 Serializable 事务按 command advisory → runtime generation → worlds → funding scope/account → auctions → recoveries → lots → equipment → credential → users → obligations → recovery actions → deliveries → fulfillments 顺序锁定，最多三次重试序列化冲突/死锁。同 ID 同参优先重放，异参、旧 fence 或冲突拒绝；追加 confirmation 与审计、引用实际 sending fence 的唯一 MID binding，原 delivery 保持当前 fence 改为 succeeded。独立标签 `sent_operator_confirmed_v1` 带 `external_reference_proven=true`，不产生 `sent_verified=true`。`delivery_has_operator_sent_proof` 必须匹配不可变 confirmation、原 snapshot、当前 attempt、binding、当前 delivery 与原摘要；迁移 `000074` 起原 snapshot 可为上述 Admin 失败结论，此时须在命令账本按命令 ID、结果状态、fence 与失败码找到该失败命令且其后无新命令；共享 `delivery_has_frozen_sent_proof` 保持原自动证明并接受这一强谓词，履约 claim、内部收款、资金收据与恢复/退货消费方均不可仅凭 operator 标签成功。

统一处置事实只经 `DeliveryDispositionPort` 与存储层单一写入模块追加，写入方必须处在产生该终态的同一事务内：Admin 对账判失败、装备损失退役、`confirm-sent` 人工证明、维护扫描的自动 recovered-send、发送围栏前的预检退役，以及操作者确认。`resolves_alert` 在写入时按领域闭集固定，不是读侧派生：交付与通知的失败水位只按“当前 fence 上没有解除计数的事实”过滤，不再按来源拼接专用排除谓词。当前口径沿用迁移 `000088` 之前的排除范围——预检退役中只有 `lot_watch_invalidated` 解除失败计数，改名、投稿场次已开拍与赛季边界已过的退役仍计入通知失败；放开属有意改变判定，须用户批准后新增迁移。

`operator_acknowledged` 由操作者命令写入，命令 ID 与请求摘要在该行内保存，同 ID 同参重放返回原结果、异参冲突。准入在最终 Serializable 事务内重新读取：投递必须是 `failed`、不是结算或恢复来源、没有履约义务、冻结载荷中 Credits 与 COD 均为零且没有附件装备；原因必填，操作者来源只能是 `admin` 或 `cli`。该命令不改变投递状态、资金、履约或资产事实，只追加处置事实与同事务审计（审计 ID 是处置的来源引用）。处置表只追加（触发器拒绝 `UPDATE`/`DELETE`），运行账号只获得 `SELECT`、`INSERT`。

人工命令复用发送恢复 helper 把原 fulfillment 从 manual_reconcile（Admin 失败更正时从 failed）恢复 enqueued，不创建新投递或收款来源；更正移除投递上的失败标记，原失败结论保留在命令账本、审计与 confirmation 快照中。装备分支随后由 Worker 读取真实当前 Sent：未领取继续等待；已领取由原资金 owner 使用同账号会话证明记录 COD 实收，并按原冻结金额放行卖方付款。货款分支在同事务复用自动 recovered-send 的 `finish_funded_delivery(...Sent)`，将原 unknown allocation 记为 spent，追加一次以原 delivery 身份绑定的支出分录；原金额及资金来源不变，同 ID 重放不重复入账。收件人领取仍由正常履约流程核验，原 Admin 手填引用成功的 Send 禁止保持不变。

### 邮件来源

| 表 | 内容 |
| --- | --- |
| `inbound_mail` | 每封已处理入站邮件的种类、内容摘要、决定与安全元数据（邮箱 + 邮件 ID 唯一） |
| `mail_commands`、`mail_admin_commands` | 邮件业务命令与 Admin 邮件操作账本 |
| `mail_poll_state` | 各邮箱的轮询游标、暂停状态与租约；`cursor_value` 是 Inbox 的唯一权威游标 |
| `mail_scan_checkpoints` | 按凭据、凭据版本、世界与 endpoint origin 保存 Sent / Read Mail 的最高 MID、水位时间与最近完整校验时间；只服务增量前缀，不是发送、已读、领取或完成证明，作用域不匹配或损坏时回退完整扫描（迁移 `000083`） |

### 实时与运维

| 表 | 内容 |
| --- | --- |
| `realtime_streams`、`realtime_events` | 事件流与事件（流内序号连续）；竞价事件与出价同事务写入，`stream_sequence` 在提交后分配，未分配行为 NULL 且不进入回放，见[实时事件](realtime.md) |
| `realtime_consumer_offsets`、`realtime_failures` | 各 consumer 游标与转发失败记录 |
| `operations_alert_runtime_state`、`operations_alert_signals`、`operations_alert_batches`、`operations_alert_batch_items` | 运维告警的运行状态、信号、批次与批次项 |
| `operations_alert_acknowledgements` | 迁移 `000089` 新增的告警台确认/静默只追加事实：`action`（`acknowledge`/`silence`）、唯一 `command_id` 与请求摘要、操作者、原因、静默秒数与到期时间。BEFORE UPDATE/DELETE 触发器与权限层拒绝改写；不改变 signal 状态、计数、阈值或业务数据 |
| `operations_alert_signals` 的确认/静默投影 | 迁移 `000089` 新增 `acknowledged_at`、`acknowledged_by`、`silenced_until`、`resolution_notified_at`。前三者是当前活动周期的快速投影，观察事务在 signal 从已恢复重新变为活动时清空；`resolution_notified_at` 只标记已恢复的每日摘要类信号已进入下一封摘要，两者都由追加事实表保存历史 |
| `operations_alert_signals` 的 kind / scope / source / identity 闭集 | 迁移 `000005`/`000015`/`000020` 建立，`000085` 扩展为 26 个 kind、9 个 source，`000091` 再扩展为 29 个 kind、11 个 source：Worker 扫描的 `process`、`delivery`、`fulfillment`、`recovery`、`mail`、`credential`、`lifecycle`、`forum`、`notification`，以及主机侧 `auction-cli alert report` 报告的 `backup`（`disk_capacity_low`/`backup_failed`，scope `backup`）与 `wal`（`wal_upload_stalled`，scope `wal`）。`source_unavailable` 仍要求 scope 等于来源名；各来源的 scope 是 `mail`、`mail_cookie`/`external_api`、`auction`、`forum`、`notification`、`backup`、`wal`。应用层 `OperationsAlertKind::ALL`/`OperationsAlertSource::ALL` 是持久化闭集，Worker 每轮提交的完整闭集是 `OperationsAlertSource::WORKER`、主机侧 CLI 提交的是 `OperationsAlertSource::HOST`；未登记取值在数据库层失败关闭。信号只读各 owner 已提交的安全水位或 CLI 读取的主机证据，不参与竞价、结算或投递正确性 |
| `service_heartbeats` | 三角色进程心跳（TTL 30 秒），是唯一被标记为“待决定是否短期保留”的数据族；`audit_failure_count` 在发布进程为 legacy/shadow 时是全部审计失败累计，classified 时仅为阻断就绪的 permanent + unknown 累计，成功写入不清零、重启归零；结构不变，完整分类计数在进程指标中保留，见[就绪策略](../../ops/config/README.md#就绪策略) |
| `audit_records` | 追加式审计：操作者类型（public_user/admin/worker/system）、动作、实体、结果、来源（public/admin/worker/system/cli）与安全元数据；触发器禁止修改与删除 |

### 当前态投影

`public_sessions`、`admin_sessions`、`auction_forum_reply_cursors`、`mail_poll_pending_reads`、`mailbox_sweep_cursors`、`mail_scan_checkpoints`、`auction_intake_targets`、`mail_runtime_generation` 只表示当前运行状态，由各自 owner 的生命周期维护，不作为历史事实。

## 共有约束

- **幂等账本**：每个写命令 owner 都有命令表，以 `command_id` 为键保存 32 字节请求摘要与结果。同 ID 同摘要重放、异摘要冲突。
- **乐观版本**：可变聚合（拍卖、lot、用户、义务、投递等）带 `state_version` 或 `fencing_version`，命令以期望版本做 CAS。
- **只追加事实**：出价、代理转换、黑名单历史、审计、发送/收取尝试及 Sent 绑定、COD 退回计数、历史核对证明、场次与 lot 结算结果、补偿账本及冲正由触发器拒绝 UPDATE/DELETE，运行账号在权限层同样只有 `INSERT`（见[数据库账号与权限](#数据库账号与权限)）。资金账户仅冻结 `accounting_mode`/`policy_version`，其他运行状态可变；改名的原始名称证据保留，但撤销会更新 `user_renames.reverted_at`。
- **租约与 fencing**：Worker 领取的工作项（拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、私信、告警）都有 `lease_token`/`lease_expires_at` 与单调递增的 fencing 版本；租约过期可被重新领取，处于“发送中”的外部写入在结果未知时转为人工对账，绝不自动重发。
- **事务**：竞价、关键生命周期等命令使用 Serializable 隔离，序列化失败（40001）与死锁（40P01）由 owner 有界重试；同一幂等命令 ID 使用共用事务级 advisory lock 协调。运行期 claim、进度及部分账本写入也使用默认隔离事务，以行锁、租约/fencing、唯一约束等保障各自不变量，并非所有写入都是 Serializable。
- **失败诊断**：SQL 通用失败在原错误变体中附加 SQLSTATE 类别/代码及可用的约束名、表名，不保留驱动原文或业务值；字段可能缺失，错误文本不得决定自动重试或重发。格式与安全边界见 [Storage SQL 错误安全上下文](../../crates/auction-storage/README.md#sql-错误安全上下文)，不改变数据库 schema 或事务契约。
- **运行时独占**：Public realtime consumer、邮件 Cookie 与论坛 Cookie 的运行时 owner 使用 session advisory lock；运维工具的建库/重建使用集群级 advisory lock。
- **唤醒**：插入新的可发送投递的事务提交后，由完成插入的进程单独执行 `public.signal_delivery_ready()`（独立自动提交的 `pg_notify('auction_delivery_ready')`），Worker 投递循环据此提前唤醒（轮询仍是兜底）。通知不携带任务、不代替领取，租约与 fencing 仍是唯一的领取依据；通知不能在对应投递提交前可见。
- **CHECK 约束**：金额非负/为正、状态闭集、文本已修剪且非空、时间有序、互斥字段二选一等规则在数据库层强制，不依赖应用代码。

## 数据库账号与权限

迁移 owner 与运行账号分离（决策 35，W9.9）。owner 是应用数据库 owner，只供 `auction-cli` 的
`db create`、`db migrate`、`db rebuild`、`backup restore` 与 `service start/restart` 使用，
凭据固定在 owner-only 的独立敏感 TOML（`--owner-secrets`）。Public、Admin 与 Worker 共用一个
运行账号 `auction_runtime`，角色本身在集群中预先创建，权限全部由迁移 `000081` 授予：

| 对象面 | 运行账号权限 |
| --- | --- |
| `public` schema | 只有 `USAGE`；`CREATE` 被撤销，因此不能建表、索引、视图、触发器或函数 |
| 普通可变表 | `SELECT`、`INSERT`、`UPDATE`、`DELETE`；任何表都没有 `TRUNCATE` |
| 只追加事实表（触发器拒绝 `UPDATE`/`DELETE`）：`auction_settlements`、`audit_records`、`bid_proxy_authorization_transitions`、`bids`、`compensation_ledger`、`compensation_pool_reversals`、`delivery_dispositions`、`delivery_send_attempts`、`delivery_sent_bindings`、`delivery_sent_confirmations`、`delivery_take_attempts`、`lot_settlements`、`runtime_switch_events`、`settlement_equipment_losses`、`settlement_legacy_bot_seller_claims`、`settlement_legacy_self_reconciliations`、`settlement_recovery_return_consumptions`、`user_blacklist_history` | 只有 `SELECT`、`INSERT` |
| 冻结证据与登记：`settlement_pools`、`schema_migration_compatibility`、`_sqlx_migrations` | 只有 `SELECT` |
| 序列 | 只有 `USAGE`、`SELECT`；不授予 `UPDATE`，运行账号不能 `setval` |

迁移 `000081` 以固定角色名授权；`auction_runtime` 不存在、可登录属性不合格、是 superuser /
`CREATEDB` / `CREATEROLE` / `BYPASSRLS` / `REPLICATION`、拥有 `public` schema 或其对象、或可继承
owner/superuser 时以明确错误终止，不放宽授权。`ALTER DEFAULT PRIVILEGES` 让 owner 之后新建的表
默认只获得 `SELECT`（序列另加 `USAGE`），**新增写入面必须在对应迁移里显式 `GRANT`**，未授权即
失败关闭。权限收紧只影响新角色，既有以 owner 运行的程序读写不变，因此登记 `compatible`。

公共文件的 `[database_roles] owner` 声明迁移 owner 账号名；`config check --strict` 要求三角色的
`[database].username` 都是 `auction_runtime`、`owner` 已声明且与之不同，否则部署预检失败。
`db migrate` 等命令还会核对 `--owner-secrets` 的同库目标与账号一致性。

## 变更规则

1. 新增表、列、约束或索引只能新增迁移文件（编号连续），见[迁移说明](../../migrations/postgres/README.md)。新表对运行账号默认只读，新增或变更写入面必须在同一迁移里显式 `GRANT`（只追加表只授 `INSERT`）。
2. 每次新增迁移都把[保留政策](../../ops/deploy/retention-policy.v1.json)的 `database_schema_version` 更新为新的最高版本（否则 `db inventory` 拒绝运行，发布身份也会变化）；新增表还须加入对应数据族。
3. 同步更新本页的表分组。
4. 迁移须能在生产数据上安全执行：不得静默改写历史事实；需要修复数据时只修复有确凿证据的部分，其余保留原状并交给人工核对。

## 运行时行为开关

迁移 `000076` 纯新增 `runtime_switch_events`，登记 `compatible`：旧程序不读取该表，既有业务约束、审计闭集与冻结事实不变。表的 UPDATE、DELETE、TRUNCATE 都被触发器拒绝，数据族为 `schema_and_configuration`，永久保留且不授权清理。

迁移 `000077` 只新增五个历史 SLI 索引，登记 `compatible`：旧程序只读写既有表，索引不改变查询结果、约束或写入语义；索引候选入口见 [SLI 只读查询](#sli-只读查询)。

表按非零 command_id 保存版本 1 的 32 字节请求摘要、名称、递增 revision、
expected_revision、set/clear/expire、布尔或枚举值、UTC 期限、操作者、入口、
原因、审计关联及固定结果；每名称 revision 唯一，同一设置最多追加一次 expire。
数据库对名称只检查 1–64 字节及 snake_case，应用写入使用 Rust 闭集。
已知名称增加 Worker 专用 bool `metrics_sli_snapshot_enabled` 与
`mooglemail_poll_isolation_enabled`，以及三角色 bool
`metrics_reject_forwarded_requests`；原审计模式和 Public/Admin 的
`readiness_cache_enabled` 保持。
新名称复用解析、摘要、最新事实、到期和审计，不新增迁移。

读取先取每名称最新事实，再判断该 set 的期限。未设置、clear、expire 或最新 set 已过期均回落本角色启动时的 TOML 基线，绝不从更早事实恢复覆盖。未知名称在值解码之前忽略；已知名称的类型、枚举或结果投影损坏则拒绝整次刷新，保留原成功快照。新增名称须保持旧程序忽略规则；扩展已知名称的取值须另行评估跨版本兼容，不能默认兼容。

写事务为 Read Committed，锁序为共用 command advisory → 带命名空间的 switch advisory → 最新事实版本 CAS；该 owner 不取得拍卖等业务锁。锁后读取最新版本，同事务追加事实、可重放结果与审计。同 ID 同摘要先返回原结果，即使原设置已过期或已有后续命令也不续期、不改写；异摘要冲突。只重试明确中止的 `40001`/`40P01`，最多三次；提交回执未知要求原 ID、原参数重试。CLI 审计使用 `actor_type=admin`、`source=cli`，metadata 标注 `entrypoint=cli`、`os_uid`、可选 `sudo_user` 与 `executor=auction-cli`；Admin 写入保持 `source=admin`。`--actor` 是人工说明，不替代主机身份。

三角色启动立即读取、每五秒刷新，单次预算一秒。成功时原子替换完整不可变快照；首次失败用 TOML，后续失败保留最后成功快照及原期限。期限来自数据库时间，进程用查询开始时刻和单调时钟保守推算，不因传输延迟延长期限。Worker 每周期对最新且已到期 set 追加 expire，其命令 UUID 由原 set UUID 与固定 `runtime-switch-expire-v1` 派生；竞争时重新读取，结果未知保持原身份。实际回落不等待 expire 追加成功，原 set 的期限已是持久证据。

此缓存只选择已经就绪且正确的运行路径，不决定金额、资格、幂等、锁、会话或
审计写入。运行时模式只改变审计分类累计的投影，不清零计数；HTTP 缓存绑定
策略版本及期限状态，旧版本在途检查不得作为新版本缓存或返回成功。
指标转发拒绝只允许命令层在线收紧，放宽须修改部署配置并重启；
直接写库绕过命令不在本批威胁模型内。操作步骤见
[运行时开关](../../ops/README.md#运行时开关)。

## 连接档位与事务观测

连接分档不改变表、迁移、隔离级别、锁序、命令身份、追加审计或持久事件。
三服务共享各自物理池，长期 owner 包含在上限内：Public 为 1；
Worker 最坏为邮件所有权、论坛所有权与投递 LISTEN 共 3；Admin 为 0。
LISTEN 仅唤醒，持久任务、租约与游标仍是状态来源，不建立第二套状态机。

连接模式、超时字段与启用前置条件由
[配置参考](../../ops/config/README.md#数据库连接分档)拥有。
默认 profiled、服务超时关闭；维护档不继承服务短超时，保留 owner 局部限界。
会话参数修改需重启；事务 `SET LOCAL` 只覆盖该事务，结束后恢复会话值。
W0.7 `diagnose_relation` 的 250ms 限界与原回滚路径保持不变。

数据库指标仅为附加观测，完整名称、闭集标签与桶见
[数据库指标](../../crates/auction-observability/README.md#数据库事务与连接观测)。
一个事务操作包含原有内部重试，每次实际 BEGIN 获取阶段计一个尝试；
同一尝试的 `40001` 与 `40P01` 在映射前区分，重复映射不重复计数。
只有实际进入下一次尝试才计 retry，只有 owner 确认预算耗尽才计 exhausted。
现金 fence 的 `Deferred` 与资金的 `RolledBack(SerializationConflict)`
即使以 Ok 返回，也在原终止分支计耗尽。单次 Read Committed 领取不新增重试。

原最多三次尝试与全部错误返回保持不变；确认重跑前的等待改为带抖动退避
（窗口 6ms 起、每次翻倍、24ms 封顶，取窗口一半加一半均匀抖动），
使同时冲突的事务不再同步重试，尝试次数与重试白名单不变。
超时、锁等待失败、连接中断和未知 COMMIT 不扩大自动重试白名单；
数据库错误不能证明外部请求未执行，结果未知仍按原命令核对，不自动重发。
Final 标签只来自本次事务已读权威快照；取快照前失败保留 unknown，
不能据此宣称 Final 零耗尽。实时序号分配点明确失败才标记 realtime_sequence；
COMMIT 报告的 SSI 冲突不推测为序号热点。


## 价格历史读路径

`crates/auction-storage/src/database/price_history/` 是公开成交价格历史 SQL 的唯一 owner；
应用端口为 `PublicPriceHistoryReader`，HTTP 只映射 DTO。可见集合固定为
`a.status = 'settled'`、`l.status = 'sold'`、`l.leader_user_id IS NOT NULL` 且 lot 与场次
结算记录同时存在的 lot；游标页按 `l.effective_end_at DESC, a.sequence DESC, l.lot_number, l.id`
取顶部若干行，编号页在同一快照内返回完整总数、最低/最高价与中位价。

迁移 `000084` 只新增普通索引 `lots_sold_history_effective_end_idx`
（`(effective_end_at DESC, id)`，`INCLUDE (auction_id, current_price, lot_number)`，
谓词 `status = 'sold' AND leader_user_id IS NOT NULL`），登记 `compatible`：
它只提供候选入口，不改变可见集合、排序、统计、金额、资格、锁序、审计或事件。

测量口径与结果（一次性隔离 PG18、合成 500 场次 × 20 件成交 = 10000 条价格历史、
每场景对同一份生产查询原文执行 `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`，
先在有索引的库上测一次，再 `DROP INDEX` 后测同一批查询作为基线）：

| 场景 | 无索引 | 有索引 | 缓冲区（无 → 有） |
| --- | --- | --- | --- |
| 游标首屏 | 38.0ms | 0.77ms | 6087 → 1910 |
| 游标 boundary 页 | 44.8ms | 0.87ms | 6087 → 3537 |
| 游标 + 世界过滤 | 22.1ms | 0.70ms | 6095 → 2573 |
| 游标 + 价格区间 | 3.1ms | 1.3ms | 2640 → 2640 |
| 编号页（closed_desc，全量统计） | 48.7ms | 48.2ms | 8874 → 8874 |

游标页因此不再顺序扫描并排序全部已成交 lot；编号页与价格区间过滤不受索引影响，符合预期：
前者必须覆盖全部匹配行才能给出总数与中位价，后者的搜索键不是截止时间。为价格区间再加一个
以 `current_price` 为前导的索引只会增加写入成本，本批不新增。

另在一个 2 万条已成交 lot 的独立数据集上评估过"把装备/买家/卖家连接改写为 `EXISTS`
半连接"：无过滤编号页 96.6ms → 76.9ms，但带名称过滤反升到 124.5ms、带卖家过滤反升到
85.4ms（哈希连接被逐行主键探测替换）。因此不采用该改写，保留原连接顺序与共享的
`price_history_scope_sql!` 定义，两条分页路径继续使用同一份可见集合条件。

迁移 `000087` 只新增四个投递 Admin 检索候选索引，登记 `compatible`：
`deliveries_admin_recipient_idx`（`lower(recipient)` 前导）、
`deliveries_admin_template_idx`（`payload->>'template'` 前导，部分索引）、
`deliveries_admin_origin_id_idx`（结算来源 `origin_id` 前导，仅两种结算 origin）、
`deliveries_admin_category_idx`（不可变函数 `public.delivery_send_category(origin_kind, payload)`
前导）。世界筛选复用 `deliveries_mailbox_sweep_order` 的 `COALESCE(payload->>'world','main')`
前导。索引只提供候选入口：筛选语义、计数、排序、分页上限与安全投影仍由
`crates/auction-storage/src/database/delivery_admin/reads.rs` 与应用层校验决定，不改变金额、
状态迁移、租约、审计、事件、冻结载荷或发送节奏。

复现入口：`cargo test -p auction-storage --features test-db --test perf -- --ignored --nocapture`
（默认 `just test-db` 不运行；报告写入 `AUCTION_PERF_REPORT_DIR`，默认 `target/perf/<unix 秒>`）。

## 热点与连接竞争测量

同一夹具还量化了 W12.1（同场 `realtime_streams` 序号热点）、W12.8（出价事务内
`pg_notify`）与 W12.4（匿名读与出价共享连接池）三项。口径：一次性隔离 PG18，16 个并发
Serializable 事务在同一瞬间对同一场次的不同 lot 递增同一 `streams` 行，事务语句顺序与出价
一致（`SET TRANSACTION ISOLATION LEVEL SERIALIZABLE` → 序号 upsert → lot 行更新 →
可选 `pg_notify`），重试预算与生产相同（3 次）。

- 立即重试（旧行为）：32 个事务中 12 个成功提交、20 个耗尽预算。
- 带抖动退避（本批实现）：18 个成功、14 个耗尽。抖动把"同时重试再次碰撞"的概率降下来，
  但同一瞬间争用同一序号的尝试仍会耗尽——这正是 W12.1 的量化依据。
- 提交后分配序号（W12.1 已实现）：同一数据集与并发下，事务内只写待定事件行（`stream_sequence`
  为 NULL）与自己的 lot 行，提交后再由分配器补号；基准的 `deferred_sequence` 分支断言并报告
  零冲突、零耗尽。`stream_sequence` 连续性与回放契约不变，见[实时事件](realtime.md)。
- `pg_notify`（隔离 PG18）：比较事务内通知与提交后发送。
  - 直接投递入队基准（16 并发 × 25 行，5 次重复）：事务内触发器提交 p95 1180–2113µs、
    p50 789–843µs；提交后发送的提交 p95 1007–1169µs、p50 817–922µs，发送本身 p95
    3652–4041µs（完全在提交之外）。
  - 出价热点夹具（8 并发，3 次重复）：无通知提交 p95 188–198µs；事务内通知 p95
    194–218µs；提交后通知 p95 221–382µs。该并发下差异落在运行间抖动内；旧锁只在多个
    写入者同时提交时显著，因此不能只凭这一组数字判断锁的开销。
  迁移 `000093` 删除 `delivery_ready_after_insert` 触发器与 `notify_delivery_ready`，唤醒改由
  完成投递插入的进程在提交后单独调用 `signal_delivery_ready()`：提交不再持有 NOTIFY 全局锁，
  发送失败或进程在发送前崩溃由 Worker 轮询兜底；通知仍只表示“已有提交的待发送投递”，不携带
  任务、不决定领取顺序。
- 匿名读突发：同一物理池上 24 个并发价格历史首屏把出价 p95 从 36.8ms 抬到 170.1ms
  （4.6×）；把读并发限到 4 后出价 p95 回到 32.1ms（0.87× 对照），读 p95 120ms。
  这量化了 W12.4 读限流/并发门的必要性：读必须受限，否则会拖慢出价。

### W12.1 残留：跨 lot 的 Serializable 误报

出价事务的真实形状（命令账本 → 锁内快照 → 资格 → 竞价事实 → 实时事件 → lot 更新 → 审计）
在 Final 阶段并发出价时仍会被 40001 中止。测量证据（隔离 PG18，同一 `final` 场次、不同 lot、
不同用户、8 并发、同一瞬间进入事务，重试预算 3 次）：

- `enforce_bid_fact_sequence()` 旧实现每写一条竞价事实都做
  `SELECT max(fact_index) FROM bids WHERE command_id = …`。这是范围扫描：在 `bids` 索引页
  取得页级谓词锁，而所有并发出价都向同一索引插入，两个逻辑无关的命令形成双向 rw 反依赖。
  8 并发下该触发器单独就把冲突率推到 88%（禁用触发器后同一夹具为 0%）。
- 命令账本先对不存在的 `command_id` 做 `SELECT … FOR UPDATE`（写侧页谓词锁），随后
  `INSERT … ON CONFLICT`；8 并发同为 88%，改成普通读取后降到 17%。
- `SELECT auction_id FROM lots` 与 `lot_watch_leadership` 的锁定读取未命中都会叠加谓词锁；
  小表上的顺序/位图扫描会升级为关系级/页级锁。

迁移 `000090` 因此把事实连续性计数提升为 `auction_commands.last_bid_fact_index`
（触发器只读写本事务自己的命令行，连续性仍由数据库强制），并让 lot 创建事务直接写入
`lot_watch_leadership`；命令账本改为非锁定读取 + INSERT 仲裁，只有并发同 ID 已提交时才
锁定读取并要求调用方按账本结果重放。

剩余的多表重叠来自被超提醒与自动标记：出价事务内写 `lot_watches`、`lot_watch_leadership`、
`lot_watch_notifications`/`lot_watch_pm_deliveries` 与 `deliveries`，并触发
`notify_delivery_ready`。迁移 `000092` 改为持久化提交后 outbox：

- 出价 Serializable 事务只向 `lot_watch_events` 追加一条身份确定的事件（`bid_accepted` 或
  `proxy_created`），不读标记、不写领先周期、不插入投递；`bids`、实时事件、lot 最终状态、
  命令账本与命令/竞价审计仍与出价同事务提交。
- 提交成功后，同一进程立即用独立 READ COMMITTED 事务处理本次事件；失败只记录告警，事件仍
  持久待处理。Worker 的 Auction 循环按 `batch_limit` 扫描并处理未完成事件。
- 同一 lot 的竞价事件按 `state_version` 顺序领取：更早未完成（含崩溃遗留的过期租约）时，
  后续事件不可领取；领取用租约 + `FOR UPDATE SKIP LOCKED`，处理事务与 `done` 标记同一事务
  提交，因此崩溃只会重试。提醒身份仍是 `(lot, user, leadership_cycle)`，重复应用受既有唯一
  约束与领先周期判定保护，不会产生第二条提醒。
- 迁移 `000093` 删除 `delivery_ready_after_insert` 触发器：投递入队事务不再在同事务内
  `pg_notify`，因此提交不持有 NOTIFY 全局锁；处理事务提交且本轮确实入队了 `lot_watch`
  投递时，由处理进程单独调用 `public.signal_delivery_ready()` 发送空载荷通知。通知失败只
  记录告警，Worker 轮询仍会领取；崩溃发生在提交后、通知前同样由轮询兜底，唤醒不会造成
  重复领取（claim 仍使用租约与 `FOR UPDATE SKIP LOCKED`）。
- 与旧实现的语义差异只有时间与读取时点：标记/提醒在提交后应用，目标渠道、收件人与截止判断
  读取提交后的最新事实；正文使用处理进程的公开站点配置（Worker 配置了 origin，Public API
  进程未配置时与旧实现相同地省略链接）。崩溃或长期停机时提醒延迟，不会丢失。

事后测量入口同上；隔离 PG18、8 并发、同一瞬间进入事务，旧实现平均 1–2 笔以 40001 结束，
`000092` 后连续 4 次均为 8 成功、0 序列化冲突。回归测试
`postgres_lot_watch_outbox_round_trip_is_exactly_once` 与
`postgres_lot_watch_outbox_reclaims_lease_and_preserves_state_version_order` 覆盖事件恰好一次、
重复处理不重复提醒、过期租约回收与同 lot 顺序屏障。

```bash
cargo test -p auction-storage --features test-db --test postgres \
  -- postgres_final_cross_lot_bids_do_not_exhaust_serializable_retries --ignored --nocapture
```

复现入口：`cargo test -p auction-storage --features test-db --test perf -- --ignored --nocapture`
（默认 `just test-db` 不运行；报告写入 `AUCTION_PERF_REPORT_DIR`）。

### W12.1 残余的实测归因（未消除，属于待决策的设计权衡）

`000090`/`000092` 之后跨 lot 出价仍会以 40001 耗尽。逐语句隔离测量（隔离 PG18、真实 schema
与真实事务语句顺序、8 并发、每笔只写自己的 lot/账本/事件/审计行、6 轮共 48 笔，同一瞬间进入
事务）得到：

| 事务形状（除注明外只保留该语句） | 40001 / 48 |
| --- | --- |
| 完整出价事务（Serializable，当前实现） | 42 |
| 只保留 `UPDATE lots … WHERE id = 本 lot AND state_version = 前版本`（8 个不同 lot） | 42 |
| 只保留 `INSERT auction_commands` + `INSERT bids`（含 `000090` 触发器） | 5 |
| 只保留 `INSERT bids` + `INSERT lot_watch_events` | 1 |
| 只保留 `INSERT bids` + `INSERT realtime_events` | 5 |
| 只保留 `INSERT bids` + `INSERT audit_records` | 7 |
| 四张追加表（无 lot 更新） | 7 |
| 只保留锁内快照（`FOR SHARE OF world/auction` + `FOR UPDATE OF l`） | 0 |
| 只保留 `SELECT auction_commands`（未命中） | 0 |

- 主导项是 `UPDATE lots`：同一 `lots` 页上的八笔**互不相交**行更新即可让 SSI 互相中止——每笔
  更新前都要读自己那一行，页级谓词锁与同页写入形成双向 rw 反依赖。场次通常只有数行到数十行，
  必然同页，`vacuum`/索引/锁序都不改变这一点。
- `bids`/`lot_watch_events`/`realtime_events`/`audit_records` 的追加索引页产生少量同类误报。
  把 `000090` 触发器改成单语句 CAS（`UPDATE … WHERE last_bid_fact_index = fact_index - 1`，
  仅错误路径再区分 23503/23514）实测不改变冲突数（账本+事实 10/48 对 5/48，完整事务 42/48 对
  42/48），因此**不需要迁移**。
- `find_command` 未命中读取、锁内快照与 `last_bid_fact_index` 触发器单独都不产生冲突；日志里以
  它们为「pivot 发现点」的记录属于同一 doomed 事务被顺带命中的语句，不是它们制造了冲突。

结论：在不改变隔离级别的前提下，读/写删减、推迟写入、锁序调整与索引/触发器调整都无法消除这
部分残余——它是 PostgreSQL SSI 的页级谓词锁误报，而竞价事务必须更新 lot 行并在事务内写入事实、
审计与事件。当时列出的三条路线中，**路线 3「进入 Serializable 前按场次排队」已批准并落地**：
保留 Serializable 与全部行锁，只把同一场次的重叠写入退化为串行（不采用 Read Committed，也不
接受 Final 阶段耗尽）。

### W12.1 落实：按场次写队列（竞价 / 生命周期 / Admin）

队列覆盖**所有会写 `lots`/`auctions` 的场次级命令**，它们必须落到同一把按场次的门：

| 入口 | 命令 | 排队键 |
| --- | --- | --- |
| `TransactionRunner::run_in_auction_lot_queue(lot_id, …)` | 普通出价、创建/降低代理、Worker 慢速代理执行、Admin draft lot 修改 | lot → `lots.auction_id`（一次只读解析，结果缓存） |
| `TransactionRunner::run_in_auction_queue(auction_id, …)` | 生命周期迁移（预览/开拍/取消/最终阶段/提前结算）、Admin 排期修改 | 场次 ID |

其余路径继续用不带队列的 `run`（World 启停、规则发布、Admin 创建新场次——新建场次在提交前无人
可见，无需排队）。实现在
[`crates/auction-storage/src/database/competition/bid_queue/`](../../crates/auction-storage/src/database/competition/bid_queue)，
只改变并发形状：

- **进程内**：按拍卖 ID 的异步互斥量。同一实例的重叠命令在这里排队，等待期间不取连接、不开事务。
- **跨实例**：PostgreSQL 会话级 advisory lock（键 `hashtextextended('auction_bid_queue:' || auction_id, 0)`）。
  文本前缀是**跨版本稳定**的常量：改名会让滚动重启期间的新旧进程用不同锁键，混跑时失去互斥。
  等待方每次 `pg_try_advisory_lock` 前临时取一条队列专用连接，失败立即归还并退避（2ms 起，上限
  25ms），因此等待期间不占连接。
- **为什么不是 `pg_advisory_xact_lock`**：Serializable 的快照在 `BEGIN` 之后第一条语句取得；先等
  锁再读取会用到早于持锁方提交的快照，SSI 仍以 40001 中止。会话锁必须在 `BEGIN` 之前取得。
- **不变的部分**：隔离级别、`FOR SHARE OF world/auction` 与 `FOR UPDATE OF l`、`auction_commands`
  幂等仲裁、事实/实时事件/审计的同事务写入、提交后补号与 `lot_watch_events` 处理顺序全部不变。
  lot 行锁仍是唯一的价格串行化依据；队列只保证同一场次不会有两个写事务同时处于 SSI 冲突窗口。
- **取得顺序与死锁**：命令只在进入事务**之前**取得**一把**场次门（进程内 → 跨实例会话锁），
  取得后不再申请第二把场次门；跨场次写入（如开拍/取消创建后继场次）在同一事务内完成，后继场次
  在提交前不存在、也无人能对它排队。因此不存在「持 A 门等 B 门」与反向形成的环：竞价队列与
  生命周期/Admin 写入之间只有先后，没有交叉等待。世界行的 `FOR UPDATE`（生命周期）与
  `FOR SHARE`（竞价）仍按 world → auction → lot 的既有顺序取得。
- **连接配额**：队列凭证的连接取自独立信号量，不占普通业务配额——持锁连接随后还要申请事务连接，
  共用同一配额会互相等待。上限为普通配额的 1/4（至少 1、至多 4），且不超过物理池在长驻 owner 与
  readiness 保留位之外的剩余位（默认档 16 物理、普通 12 → 队列 2）。
- **释放**：正常路径 `release()` 先 `pg_advisory_unlock` 并校验返回 true，再把连接归还池；取消、
  panic 与解锁失败一律关闭该会话（会话结束即释放全部会话锁），绝不把可能仍持锁的连接放回池中。
  进程内凭证与场次互斥量条目在同一时刻回收。
- **预算与错误**：进程内等待与跨实例等待共用一个 5 秒截止时间；用尽返回
  `CompetitionTransactionError::QueueWaitTimeout`，HTTP 映射为既有的 `temporarily_unavailable`
  （503），不进入 `serialization_conflict` 重试。命令账本此时没有任何写入，客户端可用同一
  `command_id` 原样重试。
- **观测**：`auction_bid_queue_wait_seconds{stage}`（`in_process`/`cross_instance`）与
  `auction_bid_queue_requests_total{outcome}`（`acquired`/`timed_out`/`canceled`/`failed`）。

实测 A：同一 `final` 场次的不同 lot 并发出价（隔离 PG18、真实 schema、`seed_history` 生产量级
填充、同一瞬间进入、6 轮）：

| 并发 | 模式 | 成功 | 40001 耗尽 | p50 | p95 | p99 | 连接池峰值 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 直接（队列关闭对照） | 48/48 | 0 | 29–32ms | 56–58ms | 66ms | 9–10 |
| 8 | 按场次排队 | 48/48 | 0 | 77–81ms | 206–218ms | 263–273ms | 7 |
| 12 | 直接（队列关闭对照） | 67/72 | 5 | 36ms | 72–82ms | 81–91ms | 14–15 |
| 12 | 按场次排队 | 72/72 | 0 | 115–118ms | 285–290ms | 365–375ms | 11 |

（8 并发为三次运行的范围；12 并发为两次运行的范围。成功与耗尽计数每次都一致，分位差异来自
单笔事务耗时的抖动。）

实测 B：同一 `live` 场次的 Admin 最终阶段迁移 + 8 笔 Final 并发出价 + 另一世界 2 个 Admin lot
修改同一瞬间进入（6 轮，单次运行，无填充）：

| 模式 | 出价成功 | 生命周期成功 | 40001/40P01 耗尽 | 排队超时 | 出价 p99 | 生命周期 p99 | Admin 无关场次 p99 | 死锁增量 | 连接池峰值 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 直接（对照） | 23/48 | 1/6 | 30 | 0 | 217ms | 204ms | 119ms | 0 | 16/16 |
| 按场次排队 | 48/48 | 6/6 | 0 | 0 | 615ms | 143ms | 94ms | 0 | 12/16 |

- 对照组里最终阶段迁移与同场出价几乎每轮互相中止（6 轮只有 1 次提交成功），8 笔出价在 5 次
  重试内只成功 23 笔；排队后两者都 100% 成功、零耗尽，且另一世界的 Admin 修改延迟更低
  （不受 main 场次的门影响，也少了一层 SSI 争用）。
- 排队组出价 p99 615ms ≈ 8 笔同场写入 × 约 70ms/笔；生命周期自身 p99 143ms，说明它在队列最前面
  时几乎不受影响。若生命周期命令排队等待（最坏 8 × 70ms ≈ 0.6s），仍远低于 5 秒预算。

- 8 并发时该夹具本就未复现残余（填充数据让快照与追加索引走索引路径），队列的价值出现在并发
  更高的形状：12 并发下对照组 5/72 以 40001 结束，排队组 0 笔；连接池峰值同时从 14–15/16 降到
  11，即等待方不再占着连接。
- 代价是确定性的排队延迟，量级为「(同场并发 − 1) × 单笔事务耗时」：8 并发 ≈ 7 × 33ms ≈ 230ms、
  12 并发 ≈ 11 × 33ms ≈ 360ms，与实测 p99 一致。它随同场并发线性增长，但排队预算固定 5 秒，
  且等待不消耗数据库资源；若 Final 阶段 p99 超出 SLO，先用 W0.13 采样确认是同场串行导致，
  再考虑按 lot 拆分队列或调整预算，而不是放宽隔离级别。

回归测试（`just test-db` 覆盖除演练外的全部条目；演练为显式入口）：

- `postgres_bid_queue_serializes_cross_lot_across_instances`：两个独立连接池模拟两个 Public 实例，
  一个实例持锁时另一个实例的跨 lot 出价必须等待，放行后两笔都成功且会话锁归零。
- `postgres_bid_queue_releases_after_rejected_command`：事务内被领域规则拒绝后锁归零，同场次随后
  仍能出价。
- `postgres_bid_queue_releases_when_command_is_canceled`：命令被取消后会话锁归零（或随会话关闭
  回收），同场次随后仍能出价。
- `postgres_auction_queue_serializes_lifecycle_against_final_bids`：`live` 场次的最终阶段迁移与
  4 路 Final 并发出价全部成功，场次提交为 `final`、每个 lot 恰一条事实、死锁计数与队列凭证归零。
- `postgres_admin_lot_queue_serializes_same_auction_and_leaves_others_free`：同一 draft 场次两个
  lot 的 Admin 修改互相等待（场次级而不是 lot 级），同瞬间另一世界的 Admin 修改必须立即完成。
- `postgres_auction_queue_releases_lifecycle_when_canceled`：生命周期任务被取消后会话锁归零，
  同场次随后仍能提交迁移。
- `postgres_admin_lot_queue_shares_auction_key_with_bids`：出价持门期间同场次 Admin 修改不得进入
  事务；门释放后按状态/版本门禁失败（不是继续等待）。
- `postgres_bid_queue_drill_reports_latency_and_pool_usage` 与
  `postgres_auction_queue_drill_reports_latency_and_pool_usage`（`--ignored`）：上表两次演练。

```bash
cargo test -p auction-storage --features test-db --test postgres \
  -- --ignored --nocapture postgres_auction_queue_drill
```

仍未覆盖（后续单独评估）：Admin 装备录入（`admin_auction_intake_item`）按 **世界** 的 intake
target 解析场次，应用层拿到的是 world-scoped 聚合 ID，需要新增 world → auction 的排队入口后才能
纳入；结算 owner（`SettlementTransactionRunner`）、恢复 relist 与调度 claim 仍按各自的租约/
fencing 与行锁保护，不进本队列；W12.8 的提交侧 `pg_notify` 串行；队列预算与退避没有做成配置项。

复现方式：在一次性隔离 PG18 上执行全部迁移后，按本节「事务形状」列出的语句序列各跑 8 个并发
会话（每会话一笔、同一瞬间开始、跳过不需要的语句），统计 `could not serialize access`。探针
脚本与原始日志不入库，也不随部署分发。

## SLI 只读查询

`crates/auction-storage/src/database/sli/` 是 W0.13 SQL 的唯一 owner。
应用端口 `SliSnapshotReader` 只传递数值和快照时间，不暴露 SQL、业务 ID 或正文。
Worker 任务与业务循环分离；`/metrics` 不读取数据库。
迁移 `000077` 只新增历史 SLI 的五个全局时间候选索引，不修改业务隔离、锁序、账本、审计、事件或外写规则；索引存在不等于派生查询已实现。

`TrackedPool::try_acquire` 只尝试现有空闲连接，不等待或扩池，保留原池占用观测。
每轮串行至多一个连接；新建只读 Repeatable Read 事务，无 FOR UPDATE。
局部 statement_timeout=1000ms、lock_timeout=100ms，不改 W0.6 会话档位。
包含 BEGIN、全部读取和确认回滚的总预算两秒；成功回滚后才发布完整快照。
错误、超时或候选超限时丢弃该连接并保留旧指标，不触发业务重试或 readiness。
任务取消沿用 SQLx 事务取消清理，不补写业务或审计事实。

已实施的候选路径：

- `capacity::read` 用迁移 033 的 `realtime_events_created_at_idx`
  按七天时间范围及 created_at/event_id 倒序取候选。
  在 kind/replay 过滤之前限制至 10001 行；超过 10000 则整轮 limited。
  用主键关联 bid、auction_commands、lot、auction；LATERAL/LIMIT 保留点查边界。
  按 command_id 去重，只返回 main/isekai 与三类已接受 Final 命令的数值。
- `pending::read` 用迁移 001 的 `lots_due_slow_idx`，按 slow_due_at/id
  取当前到期 live lot，再按 auction 主键查世界，同样使用 10001 行哨兵。
  当前积压包含过期未清除字段，不能证明历史世代都已执行。

成本边界来自索引前缀、候选上限、主键关联和总时间预算；
本批没有执行 EXPLAIN 或连接数据库，不能声称线上计划或耗时已验证。

历史耗时与提醒余量的候选入口由迁移 `000077` 提供：

- `inbound_mail_sli_received_at_idx`：全局入站时间窗口；现有轮询索引需要已知 mailbox_key。
- `deliveries_sli_succeeded_completed_at_idx`：全部成功投递的完成时间；领取/lease 部分索引不覆盖历史完成记录。
- `lot_watch_pm_sli_succeeded_completed_at_idx`：已成功 PM 的完成时间，包括后来失效的通知。
- `auction_settlements_sli_settled_at_idx`：全局场次结算窗口。
- `audit_records_sli_slow_proxy_tick_idx`：固定 `lot.slow_proxy_tick` 的全部历史审计。

这些索引是八项只读窗口的候选入口；查询先按成功完成时间 MATERIALIZED LIMIT 10001，
再做类型/关联分类；结算的支付义务扇出另有 10001 哨兵，任一溢出整轮不可评估。
首/末封付款集合只含 COD 与 carrier 付款义务；`winner_equipment_handoff` 是付款后的
isekai 装备交付，不是付款邮件，不进入该集合；`bot_internal_v1` 内部满足的义务
同样不要求邮件。所有 Send 样本（注册、拒收、结算付款、payout、MM 提醒）统一用
`delivery_has_frozen_sent_proof`；候选先保留在完成窗口内，缺强证明的投递不进入分位，
结算按场次、outbid 按提醒、payout 按货款发送分别累计 unknown。当前所有 flow 的 incomplete 覆盖尚未实现
（`auction_sli_window_incomplete_available=0`），不发布 incomplete/oldest；
待各 flow 的完整未完成候选查询落地后再启用，不能返回缩窗计数或伪零。
成功样本按终点落入七天窗口；`auction_sli_window_*` 发布样本数和 p50/p95/p99，
提醒另发布 `auction_sli_outbid_*` 的 p01/p05 与非正余量。没有样本时不生成分位序列，
`unknown`/`incomplete` 不能当作零延迟；坏 metadata 或无法归属的候选使整轮失败关闭。
不以全表扫描加超时、当前任务集或任意截断代替完整历史窗口。
完整逐项状态、起终点、单位及指标见
[SLI owner 文档](../../crates/auction-observability/README.md#sli-与容量基线)。

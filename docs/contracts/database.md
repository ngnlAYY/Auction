# 数据库契约

[文档导航](../README.md) · [迁移](../../migrations/postgres/README.md) · [auction-storage](../../crates/auction-storage/README.md) · [保留政策](../../ops/deploy/retention-policy.v1.json)

唯一支持的数据库是 **PostgreSQL 18**。运行期 SQL 由 `auction-storage` 拥有，schema 由 [`migrations/postgres`](../../migrations/postgres) 中的追加式迁移定义（当前 68 个、88 张应用表，另有 SQLx 管理的 `_sqlx_migrations`），编译期内嵌进启用 `postgres` 的制品。

## schema 身份与版本

- `auction_schema_identity` 是单行表，记录应用名与基线版本，用来区分“Auction 的库”与其他库（备份与恢复都会核对它）。迁移只在空库或已有迁移记录的库上执行，没有迁移记录却存在用户对象的库被拒绝。
- `_sqlx_migrations` 记录已执行的迁移。进程 readiness 逐条核对完整版本序列、成功状态及 checksum，要求恰好等于本二进制内嵌迁移集；失败、缺失、额外版本或未知摘要都会导致未就绪。少数历史摘要仅接受[迁移兼容层](../../migrations/postgres/README.md#执行兼容层)显式允许的值。
- 普通启动**从不**隐式迁移。迁移只能通过 `auction-cli db migrate`（或 `service start/restart`）以及合并部署首次初始化执行，见 [auction-cli](../../apps/auction-cli/README.md#数据库)。
- 迁移只向前，不提供回滚；已发布的迁移文件永不修改。

## 表分组

分组与[保留政策](../../ops/deploy/retention-policy.v1.json)的数据族一致。保留政策当前对所有业务数据族都是 `retain_forever`：`auction-cli db inventory` 只统计、不授权任何归档或删除。

### 配置与 schema

| 表 | 内容 |
| --- | --- |
| `auction_schema_identity` | schema 身份（单行） |
| `worlds` | 业务世界（`main`、`isekai`）及启用状态 |
| `rules_documents`、`rules_admin_commands` | 各语言规则文档与发布命令账本 |
| `runtime_config_admin_commands` | runtime 配置更新命令账本（文件写入的恢复依据） |

### 拍卖与竞价

| 表 | 内容 |
| --- | --- |
| `auctions` | 场次：世界、期号、状态、冻结的经济/结算/恢复策略快照、计划/有效截止、`state_version`、早鸟窗口内场次的最早收件时刻 `intake_opens_at`（赛季开始，也是旧赛季装备不能重列的边界；迁移前的场次保持 NULL，不回填）。身份字段（世界、期号等）由触发器禁止修改 |
| `lots` | lot：装备、编号、状态（pending/live/sold/unsold/withdrawn）、起拍价、当前价、领先者、有效截止、慢速代理到期、`state_version` |
| `equipment` | 装备公开属性（世界内按外部装备 ID 唯一） |
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
| `settlement_pools`、`compensation_ledger` | 各世界补偿池与只追加流水 |
| `settlement_obligations` | 结算义务（种类闭集、世界、收款人、金额、前置义务、业务状态） |
| `settlement_obligation_fulfillments` | 义务履约执行与证据 |
| `settlement_internal_commands` | Bot 内部履约的命令与完整证明 |
| `settlement_funding_scope`、`settlement_funding_accounts`、`settlement_funding_sources`、`settlement_funding_entries`、`settlement_funding_allocations`、`settlement_funding_lots`、`settlement_funding_receipts`、`settlement_funding_returns`、`settlement_funding_commands` | Bot 资金范围、账户、来源、分录、用途占用、lot 绑定、收款绑定、返还与命令账本；账户策略（strict_cash/record_only）由触发器禁止改写 |
| `settlement_legacy_self_reconciliations`、`settlement_legacy_bot_seller_claims` | 历史 Bot 自交易结算的核对证明（只追加） |
| `settlement_recoveries`、`settlement_recovery_actions`、`settlement_recovery_commands`、`settlement_recovery_health`、`settlement_recovery_return_consumptions` | 付款超时恢复的状态、动作、人工确认、健康与 COD 退回计数 |
| `deliveries` | 出站邮件 outbox：来源、收件人、负载、状态（prepared → pending → claimed → sending → succeeded / failed / manual_reconcile）、租约与 fencing |
| `delivery_send_attempts`、`delivery_sent_bindings`、`delivery_take_attempts` | 发送/收取尝试的摘要与远端邮件 ID 绑定（只追加），用于结果未知时的对账 |
| `delivery_send_cadence`、`delivery_admin_commands` | 同类邮件发送节奏；Admin 投递操作账本 |

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
| `service_heartbeats` | 三角色进程心跳（TTL 30 秒），是唯一被标记为“待决定是否短期保留”的数据族 |
| `audit_records` | 追加式审计：操作者类型（public_user/admin/worker/system）、动作、实体、结果、来源与安全元数据；触发器禁止修改与删除 |

### 当前态投影

`public_sessions`、`admin_sessions`、`auction_forum_reply_cursors`、`mail_poll_pending_reads`、`mailbox_sweep_cursors`、`auction_intake_targets`、`mail_runtime_generation` 只表示当前运行状态，由各自 owner 的生命周期维护，不作为历史事实。

## 共有约束

- **幂等账本**：每个写命令 owner 都有命令表，以 `command_id` 为键保存 32 字节请求摘要与结果。同 ID 同摘要重放、异摘要冲突。
- **乐观版本**：可变聚合（拍卖、lot、用户、义务、投递等）带 `state_version` 或 `fencing_version`，命令以期望版本做 CAS。
- **只追加事实**：出价、代理转换、黑名单历史、审计、发送/收取尝试及 Sent 绑定、COD 退回计数、历史核对证明由触发器拒绝 UPDATE/DELETE。资金账户仅冻结 `accounting_mode`/`policy_version`，其他运行状态可变；改名的原始名称证据保留，但撤销会更新 `user_renames.reverted_at`。
- **租约与 fencing**：Worker 领取的工作项（拍卖调度、结算、履约、投递、恢复、邮件轮询、论坛同步、私信、告警）都有 `lease_token`/`lease_expires_at` 与单调递增的 fencing 版本；租约过期可被重新领取，处于“发送中”的外部写入在结果未知时转为人工对账，绝不自动重发。
- **事务**：竞价、关键生命周期等命令使用 Serializable 隔离，序列化失败（40001）与死锁（40P01）由 owner 有界重试；同一幂等命令 ID 使用共用事务级 advisory lock 协调。运行期 claim、进度及部分账本写入也使用默认隔离事务，以行锁、租约/fencing、唯一约束等保障各自不变量，并非所有写入都是 Serializable。
- **运行时独占**：Public realtime consumer、邮件 Cookie 与论坛 Cookie 的运行时 owner 使用 session advisory lock；运维工具的建库/重建使用集群级 advisory lock。
- **唤醒**：新的可发送投递插入后触发 `pg_notify('auction_delivery_ready')`，Worker 投递循环据此提前唤醒（轮询仍是兜底）。
- **CHECK 约束**：金额非负/为正、状态闭集、文本已修剪且非空、时间有序、互斥字段二选一等规则在数据库层强制，不依赖应用代码。

## 变更规则

1. 新增表、列、约束或索引只能新增迁移文件（编号连续），见[迁移说明](../../migrations/postgres/README.md)。
2. 每次新增迁移都把[保留政策](../../ops/deploy/retention-policy.v1.json)的 `database_schema_version` 更新为新的最高版本（否则 `db inventory` 拒绝运行，发布身份也会变化）；新增表还须加入对应数据族。
3. 同步更新本页的表分组。
4. 迁移须能在生产数据上安全执行：不得静默改写历史事实；需要修复数据时只修复有确凿证据的部分，其余保留原状并交给人工核对。

# auction-storage

持久化适配器边界：PostgreSQL 完整后端与 SQLite Lite 后端。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；连接池、迁移和后端适配实现集中在
`src/database/mod.rs`；PostgreSQL 竞价事务按职责放在
`src/database/competition/mod.rs`，后续领域事务继续按同一规则拆分。
Worker 的 `scheduled_rounds` due claim、租约和失败状态由
`src/database/scheduler/mod.rs` 持有；它不把调度状态放进内存或 Redis。
Admin 结算报告读取由 `src/database/settlement_report/mod.rs` 持有，只在 PostgreSQL
完整档读取同一快照内的结算摘要、补偿账本和 lot 金额；SQLite Lite 明确不支持该能力。
Admin recovery 报告读取由 `src/database/recovery_report/mod.rs` 持有，在同一 Serializable
快照内读取 recovery/action 安全投影；`recovery_commands/mod.rs` 持有 PostgreSQL 完整档的
Serializable 确认写入、证据重校验、退款激活、guarded relist、命令账本、审计和预览目录事件；
SQLite Lite 对确认能力明确返回不支持。
Public 规则文档和黑名单安全投影分别由 `src/database/rules/mod.rs` 与
`src/database/blacklist/mod.rs` 持有，PostgreSQL 与 SQLite 均只返回公开字段。
Public 补偿池与单场账本由 `src/database/compensation/mod.rs` 持有；PostgreSQL 完整档在
同一 Serializable 快照中读取结算摘要、账本和最新余额，SQLite Lite 明确返回能力不支持。
Public 论坛镜像由 `src/database/forum/mod.rs` 读取 PostgreSQL 的本地同步投影；远程发布
仍由后续 Worker/集成 owner 负责，SQLite Lite 不提供该完整档表。
Admin lot 列表由 `src/database/admin_lots/mod.rs` 持有，在同一 PostgreSQL Serializable
快照中读取拍卖生命周期、lot、装备和投稿来源安全投影；SQLite Lite 因没有完整 Admin 生命周期
列明确返回能力不支持。
Admin 拍卖列表由 `src/database/admin_auctions/mod.rs` 持有，在同一 PostgreSQL Serializable
快照中读取 world/status 筛选下的生命周期和 lot 数量；SQLite Lite 明确返回能力不支持，不能
用空列表或默认能力掩盖完整档缺失。
Admin 拍卖创建由 `src/database/commands/mod.rs` 与 PostgreSQL
`src/database/competition/mod.rs` 共同持有：应用服务使用 `admin_auction_create` 命令账本，
事务锁定 world 和 intake target，写入 draft、策略快照、审计及 `auction.created` 事件；SQLite
Lite 明确返回能力不支持。A03 请求中的 `scheduled_end_at` 当前冻结在策略快照，生命周期列在
真正开放前保持空值，以兼容旧系统创建行为。
Admin 规则读取与发布由 `src/database/rules/mod.rs` 和 `src/database/commands/mod.rs` 持有：
PostgreSQL 使用独立 Serializable 事务、规则 revision CAS、`rules_admin_commands` 幂等账本和
追加审计；SQLite Lite 读取规则安全快照，但发布明确返回能力不支持。
Admin 装备录入由 `src/database/commands/admin_intake.rs` 与
`src/database/admin_intake/mod.rs` 持有：PostgreSQL 在 Serializable 事务中按 world、intake
target、玩家和外部装备身份加锁，写入装备、`equipment_secrets`、后台投稿、lot、版本、命令
账本、审计和 realtime 事件；SQLite Lite 因缺少密文表与完整录入能力明确返回不支持。
Admin draft lot 修改由 `src/database/commands/admin_lot.rs` 与 PostgreSQL
`src/database/competition/mod.rs` 持有：事务按 world → auction → lot 加锁，完成 lot/auction
版本 fence、预览 revision、命令账本、审计和 `auction.preview_updated` 事件；withdrawn 同一
事务创建 `withdrawn_return` obligation 与 pending fulfillment，SQLite Lite 明确返回不支持。
Admin 审计读取由 `src/database/admin_audit/mod.rs` 持有：两套数据库都只读取追加式
`audit_records`，支持安全精确筛选、keyset/数字分页和 metadata object 校验；数字页的总数与
记录在同一读取事务内取得，不返回原始数据库行。
Admin 改名历史读取由 `src/database/admin_renames/mod.rs` 持有：两套数据库都在同一读取事务内
按 `user_id`/用户名筛选并返回 `user_renames` 的安全投影；回滚写命令仍由后续 users owner
切片负责。
Admin 邮件 A16-A19 由 `src/database/admin_mail/mod.rs` 与
`src/database/commands/admin_mail.rs` 持有：PostgreSQL 读取 `mail_poll_state`、
`inbound_mail` 的安全投影，并在 Serializable 事务内用 `mail_poll_state` 的版本 fence、
`mail_admin_commands` 命令账本和追加审计完成暂停/恢复/A20 触发；`src/database/mail_poll/`
又持有 PostgreSQL `000008_mooglemail_polling` 的邮箱租约、失败退避，以及
`inbound_mail`、`mail_commands`、cursor 的同事务幂等提交。SQLite Lite 明确返回能力不支持，
不创建伪造的空邮件状态。该模块不打开 MoogleMail 连接，远程读取仍由 Worker/集成 owner
负责。
服务心跳由 `src/database/health/mod.rs` 持有：PostgreSQL 使用数据库时钟和
observation sequence 条件 upsert，SQLite Lite 使用本地数据库时钟和同一 sequence fence；
两个后端都只返回 dashboard 所需的安全字段，details 原文不会离开存储边界。

`database_url` 的 scheme 决定实际后端，也决定部署档位：

| scheme | 档位 | 能力 |
| --- | --- | --- |
| `postgres://` / `postgresql://` | 完整生产档 | 完整持久化能力、Serializable + 明确行锁、并发写入、多进程 |
| `sqlite:` | Lite 档 | 单主机、每角色一个实例、Worker 并发为一 |

## Cargo feature

默认不带任何数据库驱动：

- `postgres` → `sqlx/postgres` + rustls
- `sqlite` → `sqlx/sqlite`

发布制品只打开实际使用的后端。CI 分别检查两条路径，并用 `--all-features` 检查组合图。

## 边界约束

SQL 模型、查询、连接池、事务和后端锁策略只放在本 crate；业务决定留在领域/应用层。

不使用 `sqlx::Any` 抹平后端差异——锁、事务、能力缺失和迁移必须显式可审查。PostgreSQL
竞价和关键生命周期使用 Serializable、最多 3 次序列化冲突重试并配合明确行锁；队列路径
才可使用 Read Committed 加显式锁。SQLite 使用明确的串行写事务，不能假设存在 `FOR UPDATE`。

## SQLite 连接选项

文件数据库启用 WAL：读事务不阻塞单个写事务，符合 Lite 档「低并发但仍要可用」的定位。
写入串行由 SQLite 自身的写锁保证，`busy_timeout`（5 秒）决定等锁而不是立刻返回
`SQLITE_BUSY`。文件库连接上限为 5（多读 + 一写）。

`create_if_missing(true)`：Lite 档面向本地与单主机部署，数据库文件不存在时创建而不是启动
失败。SQLx 的默认值是 `false`，因此这一项必须显式设置。

`foreign_keys(true)`：SQLite 的外键约束默认关闭，不显式开启会让 Lite 档静默接受违反引用
完整性的写入。

内存数据库（`:memory:` 或 `mode=memory`）不落盘，WAL 无意义，且**必须保持单连接**——每个新
连接都会得到独立的空库，池化会让数据看起来随机消失。

「轻量」只缩减系统能力，不放宽已声明支持功能的数据正确性：Lite 已提供的竞价写入仍必须保证
`command_id` 幂等、事务原子性、审计追加和凭据安全。

## 迁移

`migrations/postgres/` 维护完整生产 schema，`migrations/sqlite/` 只维护 Lite 能力子集。
两个迁移树不要求一一对应，共有对象必须同名同义。迁移只允许追加，已发布文件禁止修改。

新增持久化能力前必须在 `docs/architecture.md` 标明它属于「完整档」「Lite 档」还是「两者」。

当前 PostgreSQL `000001` 至 `000007` 已提交核心竞价、结算/履约、生命周期、身份/
外部命令、恢复/可观测性和论坛同步 schema；`PostgresCompetitionTransaction` 已接入竞价、
生命周期审计/realtime 事务，结算事务也已接入首片。SQLite `000001_lite_core.up.sql`、
`BEGIN IMMEDIATE` 竞价和生命周期写适配器均已接入；结算报告属于 PostgreSQL 完整档，SQLite
不提供结算表；PostgreSQL 完整档的 Worker 结算 workflow 由
`src/database/settlement_scheduler/mod.rs` 持有，结算命令装配由
`src/database/settlement_commands/mod.rs` 持有；SQLite Lite 明确不启用该 workflow，外部投递
仍未装配外部传输实现。PostgreSQL 完整档的
`src/database/fulfillment_scheduler/mod.rs` 持有结算履约 claim、fencing、准备/激活状态推进，
`src/database/delivery_scheduler/mod.rs` 持有 delivery outbox 的领取、发送中 fencing、成功、
失败和人工对账终态；SQLite Lite 对这些完整档能力明确返回不支持。realtime Admin 的
lag、failure 安全投影和 replay 事务由 `src/database/realtime_admin/mod.rs` 持有。

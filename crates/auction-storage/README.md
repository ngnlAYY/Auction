# auction-storage

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [对象契约](docs/contracts/schema.md)

拥有 PostgreSQL 的 SQL、连接池、查询、事务、锁与迁移，实现应用窄端口。业务决定留在领域/应用层，外部请求留在
集成层，SQL 模型与行映射不越过存储边界。表、迁移与查询归属见[对象契约](docs/contracts/schema.md)，
锁序、幂等与审计语义见[领域不变量](docs/contracts/invariants.md)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [database/store.rs](crates/auction-storage/src/database/store.rs)、[pool](crates/auction-storage/src/database/pool) | 连接、readiness 与连接池观测 |
| [competition](crates/auction-storage/src/database/competition)、[commands](crates/auction-storage/src/database/commands) | 竞价、生命周期、管理事务与命令装配 |
| [draft_soft_close](crates/auction-storage/src/database/draft_soft_close) | 未发布草稿只读检查及显式 Serializable/CAS 修正、账本重放和追加审计 |
| [settlement](crates/auction-storage/src/database/settlement)、[settlement_policy](crates/auction-storage/src/database/settlement_policy) | 结算事实与冻结策略解码 |
| [scheduler](crates/auction-storage/src/database/scheduler)、[slow_proxy_due.rs](crates/auction-storage/src/database/slow_proxy_due.rs) | 持久轮次、领取与慢速代理候选 |
| [realtime_consumer](crates/auction-storage/src/database/realtime_consumer) | 独占消费、连续游标与失败恢复 |
| [migrations](crates/auction-storage/src/database/migrations)、[initialization](crates/auction-storage/src/database/initialization) | 追加迁移、兼容执行与首次空库初始化 |
| [legacy_v28](crates/auction-storage/src/database/legacy_v28) | canonical v28 格式防腐与接管 |
| [deployment_identity](crates/auction-storage/src/deployment_identity) | 脱敏数据库目标身份与部署校验 |

## 用户身份与消费 owner

[users](crates/auction-storage/src/database/users) 在写锁内复核注册信封绑定的 UUID；论坛数字账号是稳定身份，
`username` 只是可变显示名和 MM 收件人，二者不能互换。[sessions](crates/auction-storage/src/database/sessions) 保存会话
摘要：Admin 启动撤销旧 Admin 会话，Public key 撤销/重签与旧会话永久撤销同事务提交，只读 inspect 不刷新过期时间。
用户、论坛身份及登录 key 信封的快照更新时间以已存创建/更新时间为下界；`000044` 只修复 MM 注册账本与论坛身份历史
共同证明的时间倒退，推进版本并追加更正审计，保留创建时间与所有凭据，未知来源异常阻断整次迁移
（见[用户快照时间修复升级](ops/README.md#用户快照时间修复升级)）。

凭据与 realtime 消费 owner 持有专用物理 session：操作复用原持锁连接，失效后不能从池补连接继续，guard 退出关闭物理
连接、不带锁归池；凭据网络调用不跨越数据库事务，部署身份不含连接原文、账号或密码。`credentials` 由 `000045` 持久化
可选预期 `player_name` 并传递当前 state_version（旧记录保持 NULL，不是 verified 标记，非空值必须精确匹配）；
`000046` 的 `mail_runtime_generation` 只含全局 `mail_cookie` 行，取得原 session advisory lock 后每个 mail owner 都先原子
推进 generation 并确认提交，才允许远端操作。[credentials/evidence.rs](crates/auction-storage/src/database/credentials/evidence.rs)
提供 generation 首锁、active 凭据版本/世界/名称锁验，以及最终写入前 `clock_timestamp()` 的 30 秒证据时效门禁。
证据 helper 返回 `MailEvidenceTransactionError::{Sqlx, Rejected}`：外层先按 SQLSTATE 40001/40P01 分类整事务重试，
再转换脱敏应用错误，不通过错误文本决定重试。

## 竞价事务和后端入口

竞价按 world → auction → lot 加锁：只读 world/auction 使用共享行锁，lot、活动代理、参与者资格和账本保留排他锁，
生命周期使用对应排他锁；取得锁后读取数据库时间，同一计划的代理、领先时间、截止、审计及接受事实共用该时刻。
命令账本、状态、审计和 realtime/outbox 同事务提交，Final 与轮次完成标记也原子写入。`competition/postgres/` 按
bidding、lifecycle、admin 拆分；旧竞价账本重放仍在一致快照中取得冻结策略、授权版本和转换历史，不重新执行当前状态
门禁。竞价及关键生命周期使用 Serializable、明确行锁，最多三次尝试（含首次）；队列领取才使用 Read Committed 加锁，
只重试已分类的序列化冲突/死锁，未知提交结果不能盲目重放。

## 装备标记与超价提醒

[lot_watch](crates/auction-storage/src/database/lot_watch) 独占标记 CAS、领先周期、通知来源和发送资格证明：
普通/论坛/快慢代理的每条已接受竞价事实都推进周期，新代理 Created 在同事务默认标记，读取、降额与重放不恢复标记；
同值新命令也推进版本 fence，取消保留 tombstone，迁移只补已有有效代理标记。通知跨渠道按 lot/用户/领先周期唯一，
MM 复用 mail_delivery owner 入队（`40001`/`40P01` 交回竞价 runner 沿用最多三次整事务尝试）。领取只读 proof，
sending 先锁 lot 再锁 delivery 并复核；取消/重新领先使旧资格永久失效，未开始发送的旧任务收口为 failed 并追加审计，
已 sending 的未知结果进入 manual_reconcile。正文不含代理上限，完整标记列表超过 10000 条明确失败。
`lot_watch/preference.rs` 只锁 user 做独立偏好 CAS。[lot_watch_pm](crates/auction-storage/src/database/lot_watch_pm) 独占
PM claim、lot→任务 sending fence、原 sender 对账和 CLI 安全投影：`000042` 保留旧 MM payload/digest，PM 缺 UID 明确
suppressed 且不回退 MM；sending 永不回 pending，过期 sending 只转 manual_reconcile。

## 持久调度与慢速代理

scheduler 保存 due、租约、尝试次数和失败状态；完成/失败须匹配 owner、精确租约及版本，Admin 改期清除旧租约并推进
版本，陈旧任务不能收口。已存在轮次按冻结事实读取，配置变更不改写旧命令或时间；自动 Draft 不提前物化运行期结束
时间，后继 SQL 规则列与快照同取新模板。slow proxy 扫描在 LIMIT 前排除已到截止的 lot，执行由竞价事务复核
generation、due、授权与资格；调度字段与确定性命令持久化，不另建内存游标或 Redis 锁。

## 报表与恢复

| 职责 | 入口 |
| --- | --- |
| 公开目录、lot、账户、竞价日志 | `auctions/`、`lots/`、`account/`、`bid_log/` |
| 已提交结算事实与报告 | `price_history/`、`compensation/`、`settlement_report/` |
| 结算与履约 | `settlement_scheduler/`、`settlement_commands/`、`fulfillment_scheduler/` |
| 确认、提醒、退款与重挂 | `recovery_report/`、`recovery_commands/`、`recovery_scheduler/` |
| 邮件轮询、入站事实、outbox 与审查 | `mail_poll/`、`mail_intake/`、`mail_delivery/`、`admin_mail/` |
| 外部发送状态、fencing 与人工动作 | `delivery_scheduler/`、`delivery_admin/` |
| 论坛、凭据与告警 | `forum/`、`credentials/`、`operations_alert/` |
| 规则、审计与 readiness 水位 | `rules/`、`admin_audit/`、`health/`、`audit_health/` |

编号页 total 与当前页来自一致快照，禁用 world 的活动投影不能公开，已 settled 历史按契约读取。
[auctions/countdown.rs](crates/auction-storage/src/database/auctions/countdown.rs) 用单条 SQL 读取同场公开时间及数据库
时钟，不加载装备、凭据或论坛正文，不公开草稿、取消场次或被遮蔽的预览，无需新增表和迁移。Public 拍卖、竞价日志、
成交历史与补偿批次的游标查询仅对额外探测行使用饱和加法，不截断请求数量（见[分页契约](docs/contracts/routes.md#public-分页世界与金额)）。
Admin 读取不为补全投影临时写入；邮件展示和诊断 sidecar 均非权威数据，不参与幂等、完成证明或 cursor。
`leader_since` 按全部接受事实判断连续领先是否中断（普通出价后被原领先者快速代理反超也重新计时）；`000037` 只重建
可由追加式 bid 证明的未结算 live lot 投影，不改已结算金额、终态 lot 或历史审计。

Delivery 进入 sending 前须证明命令已提交，未知发送结果进入 manual_reconcile，不自动重寄。
[delivery_scheduler/cash](crates/auction-storage/src/database/delivery_scheduler/cash) 统一新旧 sending 入口：有证据时按
generation→排序 worlds→资金 scope/account→必要业务/凭据→义务→delivery→fulfillment→allocation 加锁，无证据时早锁
scope 及持久 allocation 关联；完整 payload、recipient、command/hash 及 token/fencing/实时 lease 锁内精确匹配。
现金不足只提交冻结并返回 Deferred；现金 until 入口沿用 prepared 原单调截止与 UTC 证据窗口的交集，PostgreSQL 18
`transaction_timeout` 在业务锁/审计前设置并覆盖 COMMIT。已有 allocation 的五种新 Admin 动作全部返回 Invalid 且不写
状态，原稳定命令先重放。compose 安全由 transport 在 sending fence 前实际只读检查，不再按历史失败永久阻断整个 world。
未开始写入的有效 claim 释放回 pending 时撤销本次领取 attempt_count 并在同事务追加 release 审计。续租仅允许当前有效
claimed/sending，先取 delivery 行锁再复核 token/fencing 及 `clock_timestamp()`，新截止不得缩短已有租约。

`forum/catalog.rs` 拥有 `auction_forum_catalog_pages` 续页身份/正文（`000040` 增加主记录目录页数），发布和刷新在主
记录 claim/revision CAS 的同一事务内更新所有续页；`catalog_layout` 冻结历史单页/新分页方案，仅从未尝试、无远端身份
的队列项及新主题启用分页。`000043` 为 forum owner 增加纳入标记和 nullable 终态确认版本，历史 settled 仅豁免；
`forum/sync/snapshot.rs` 在同一 Repeatable Read 中读取结算汇总、lot 明细和最终领先事实，
`forum/reply/fixed_posts.rs` 只在全部固定帖确认后以 auction 共享锁核对 settled 版本与无 pending，再按 sync
lease/fencing/revision CAS 收口；`forum/reply/queries.rs` 从既有命令账本投影 `last_error_code` 与有界详情，不回填历史。论坛领取可按 Worker 首轮水位重新领取旧凭据失败和未知写入，但不领取
`write_authorization_required`。Delivery Admin 重试由 `recovery_commands/commands/delivery_retry.rs` 在原 Serializable
命令事务恢复匹配的失败投影并推进 fencing，不生成成功证明。

## 内部履约原子事务设计

[fulfillment_scheduler/internal](crates/auction-storage/src/database/fulfillment_scheduler/internal) 实现独立
`bot_internal_v1` 满足事务；迁移 `000048_internal_fulfillment` 只新增稳定命令与非敏感完整证明表，不回填旧记录、
不改写冻结金额。

- 原命令先重放（同负载原结果、异负载冲突），Serializable 整事务最多三次，只按类型化 40001/40P01 重试；
  确认 ROLLBACK 才返回 RolledBack，未知提交/取消由应用 opaque guard 撤销原 owner。
- 锁序为 generation→排序 worlds→资金 scope/account→auction→recovery→lot/equipment→credentials→users→排序 obligations
  →actions→deliveries→fulfillments→proof/ledger/audit；旧 prepare/activate/satisfy 同步此业务前缀。同 EID 可能关联多个
  本地 equipment UUID，均核对；当前按 UUID 锁全部 deliveries 的成本须在集成评审关注。
- 复核当前用户版本、有效 claim token/fencing/state_version 及数据库实时时效；新 pending 必须从未准备、激活或关联
  delivery。`conflicts.rs` 按真实 origin/payload 闭集 schema 排除已验证无装备通知：论坛 `forum_auction_notice` 仍属
  mail_intake，仅同 command/world 的 `auction_forum_bid_commands` notify 账本、零命令金额及完整 producer origin 重建
  一致时才识别，关联只读；
  缺失/畸形/未知 schema 仍阻断，不凭前缀或 succeeded 放行。
- 库存只消费完整正向 Armory 快照，所有资金 SQL 由 settlement_funding 拥有。满足、清 claim/版本、独立 proof、资金动作、
  直接 blocked 依赖和追加审计同事务；外部 COD 在任何满足/资金化/解依赖前以已锁账户和同一有效 Main cash 检查既有占用，
  缺口确定提交冻结并返回 FundingDeferred，通过后才保存原 Sent collected 满足并释放原依赖。

Recovery action 终态先锁父 case 再锁 action，确认路径按 UUID 预锁全部 fulfillment。Admin 旧重试由 recovery owner 提供
`lock_recovery_delivery_retry_scope` / `lock_recovery_delivery_retry_actions` 两阶段锁，只用于无 allocation 的
RetryFailed/ReconcileRetry，SQLSTATE 原样交 runner。源码静态结果见[状态页](docs/status.md)，迁移、SQL 执行、并发与
大库锁成本仍需独立运行证据。

## 真实现金账户与事务 helper

[settlement_funding](crates/auction-storage/src/database/settlement_funding) 独占迁移 `000047` 的资金 scope、稳定 UID
账户、唯一 source/receipt、用途 allocation、追加 entry 及命令结果；初始 0/unreconciled，不回填历史现金。helper 在
generation/world 之后、业务锁之前取得 scope/account，令牌绑定当前事务。所有 Credits Send 须经
reserved→in_flight→spent/unknown，失败/人工 failed 不释放，精确返还另追加事实；`lock_funded_delivery_account` 在
delivery 锁之前按持久 allocation 关联早锁原账户，孤儿/歧义失败关闭，delivery owner 不从 metadata 推断账号。
责任 preview 使用 Serializable READ ONLY，apply 锁完整责任集合、核对预览摘要/版本和真实现金，既有 allocation 全部扣占。
未资金化付款已外发但收款未闭合（或关联 delivery 缺失）时 preview/apply 增加 `payment_collection_unclosed` blocker；
新 COD 要求精确 MID/当前账号实收绑定、全局 payment/source 唯一且发送晚于最近 opening 边界。
`delivery_scheduler/capability.rs` 是当前构建唯一 V1 提供者，非 postgres 档位返回 None；`delivery_admin` 仅为明确
失效的 watch preflight 保留无 sending 时间的严格投影。

## 后端选择、连接池与验证

配置层将 `[database]` 或兼容的 `postgres://` / `postgresql://` URL 交给本 crate，不在存储层解析 TOML；
默认不编译驱动，运行须显式启用 `postgres` feature，使用 SQLx PostgreSQL/rustls。
`DatabaseStore::pool_metrics_source` 暴露借出、空闲、总数、上限及真实 acquisition 累计指标，普通查询、事务与显式
acquire 均经过 `TrackedPool`，不额外查询 SQL，也不暴露数据库名或连接串。readiness 检查数据库、内嵌迁移及审计写入
失败计数，异常失败关闭。

[retention_inventory](crates/auction-storage/src/database/retention_inventory) 在只读快照按版本化政策盘点关系、依赖和
realtime 水位，不授予删除资格；[recovery_preflight](crates/auction-storage/src/database/recovery_preflight) 只核对恢复
目标，不运行 pg_restore 或覆盖对象；[bootstrap](crates/auction-storage/src/database/bootstrap) 在维护库身份、权限和集群
锁下创建缺失库，不提供 DROP 或 migration；[rebuild](crates/auction-storage/src/database/rebuild) 是独立破坏性边界，
要求精确目标与受限目录。备份、人工确认、停止进程和 receipt 归 `auctionctl`（见
[数据库恢复与保留](ops/README.md#数据库恢复与保留)）。

```bash
cargo check --locked -p auction-storage --no-default-features --features postgres
```

仅静态编译，不执行迁移或行为测试。完整门禁见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。

## 迁移

[PostgreSQL 迁移](migrations/postgres/README.md)只追加；普通启动不隐式迁移，执行门禁见[执行边界](migrations/postgres/README.md#执行边界)。
`initialization/` 只供确认后的合并首次空库初始化：同一专用连接持 SQLx 迁移锁，拒绝已有用户对象与其他客户端，
不建库、不删对象、不升级已有库。普通迁移与初始化共用受限执行视图：`000005`/`000015` 修正历史别名，`000044` 在
ALTER 前执行已排队的 `user_forum_identity_account_id_check` 延迟约束，`000047` 为保留字列 `authorization` 加双引号。
不重复加锁，不修改已发布文件、checksum、列名或原迁移事务边界。`recovery_preflight/catalog.rs` 拥有空库对象盘点
（subscription 仅计当前库，default ACL 等残留仍阻断）；`migrations/history.rs` 核对完整 SQLx 历史前缀及 checksum，
旧 archive 恢复不能只比较最大版本。canonical v28 必须走[专用接管](ops/deploy/LEGACY_V28_ADOPTION.md)。
冻结结算策略解码不应用新模板门禁，后继创建另由应用/领域校验。

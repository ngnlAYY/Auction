# auction-storage

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

拥有 PostgreSQL 的 SQL、连接池、查询、事务、锁与迁移，实现应用窄端口。业务决定留在领域/应用层，
外部请求留在集成层；SQL 模型与行映射不越过存储边界。表与能力归属见[对象契约](../../docs/contracts/schema.md)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [database/store.rs](src/database/store.rs)、[pool](src/database/pool) | 连接、readiness 与连接池观测 |
| [competition](src/database/competition)、[commands](src/database/commands) | 竞价、生命周期、管理事务与命令装配 |
| [draft_soft_close](src/database/draft_soft_close) | 未发布草稿只读检查及显式 Serializable/CAS 修正、账本重放和追加审计 |
| [settlement](src/database/settlement)、[settlement_policy](src/database/settlement_policy) | 结算事实与冻结策略解码 |
| [scheduler](src/database/scheduler)、[slow_proxy_due.rs](src/database/slow_proxy_due.rs) | 持久轮次、领取与慢速代理候选 |
| [realtime_consumer](src/database/realtime_consumer) | 独占消费、连续游标与失败恢复 |
| [migrations](src/database/migrations)、[initialization](src/database/initialization) | 追加迁移、兼容执行与首次空库初始化 |
| [legacy_v28](src/database/legacy_v28) | canonical v28 格式防腐与接管 |
| [deployment_identity](src/deployment_identity) | 脱敏数据库目标身份与部署校验 |

## 用户身份与消费 owner

[users](src/database/users) 在写锁内复核注册信封绑定的 UUID。论坛数字账号是稳定身份；`username` 是可变显示名和
MM 收件人，不能互换。[sessions](src/database/sessions) 保存会话摘要：Admin 启动撤销旧 Admin 会话，Public key
撤销/重签与旧会话永久撤销同事务提交；只读 inspect 不刷新过期时间。
用户、论坛身份及登录 key 信封的快照更新时间以已存创建/更新时间为下界，不因时钟差异倒退；审计/命令时间不据此改写。
`000044` 只修复 MM 注册账本与论坛身份历史共同证明的用户时间倒退，推进版本并追加更正审计，保留创建时间与所有凭据。
未知来源异常由新增时间顺序约束阻断整次迁移，Admin 投影校验不放宽；部署边界见[运维手册](../../ops/README.md#用户快照时间修复升级)。

凭据与 realtime 消费 owner 持有专用物理 session；操作复用原持锁连接，失效后不能从池补连接继续。
guard 退出关闭物理连接，不带锁归池。凭据网络调用不跨越数据库事务；部署身份不包含连接原文、账号或密码。
`credentials` 通过 `000045` 持久化可选预期 `player_name` 并传递当前 state_version，旧记录保持 NULL。
该字段不是 verified 标记；NULL 不阻断真实联合身份，非空值必须精确匹配。
`000046` 的 `mail_runtime_generation` 只含全局 `mail_cookie` 行；取得原 session advisory lock 后，
包括配置 Cookie 的每个 mail owner 都先原子推进 generation 并确认提交，再允许任何远端操作。
[credentials/evidence.rs](src/database/credentials/evidence.rs) 提供 generation 首锁、active 凭据版本/世界/名称锁验，
以及最终写入前 `clock_timestamp()` 的 30 秒证据时效门禁；不反向取得 world、资金、拍卖或 claim 锁。
内部事务持 generation 行锁到确定结束，新 owner 必须等待推进；未知提交取消由上层 opaque guard 永久撤销旧 owner。
配置 Cookie 没有数据库 ID/version，虽参与换主 fence，仍不具备内部提交能力。
三个证据事务 helper 返回 `MailEvidenceTransactionError::{Sqlx, Rejected}`；外层先按 SQLSTATE 40001/40P01 分类整事务重试，
再转换脱敏应用错误，不通过错误文本或提前擦除 SQLx 类型来决定重试；Sent 观察也进入 scope/同账号/时效锁验。

## 竞价事务和后端入口

竞价按 world → auction → lot 加锁：只读 world/auction 使用共享行锁，lot、活动代理、参与者资格和账本
保留排他锁，生命周期使用对应排他锁。取得锁后读取数据库时间，同一计划的代理、领先时间、截止、审计及接受事实共用该时刻。
命令账本、状态、审计和 realtime/outbox 同事务提交；Final 与轮次完成标记也原子写入。

`competition/postgres/` 按 bidding、lifecycle、admin 拆分。旧竞价账本重放仍在一致快照中取得冻结策略、授权版本和转换历史，
不重新执行当前状态门禁。竞价及关键生命周期使用 Serializable、明确行锁，最多三次尝试（含首次）；队列领取才使用
Read Committed 加锁。只重试已分类的序列化冲突/死锁，未知提交结果不能盲目重放，详见[不变量](../../docs/contracts/invariants.md)。

## 装备标记与超价提醒

[lot_watch](src/database/lot_watch) 独占标记 CAS、领先周期、通知来源和发送资格证明。
普通/论坛/快慢代理的每条已接受竞价事实都推进周期；新代理 Created 在同事务默认标记，读取、降额与重放不恢复标记。
同值的新命令也推进版本 fence，取消保留 tombstone。迁移只补已有有效代理标记，不补发过去提醒。
通知跨渠道按 lot/用户/领先周期唯一，MM 复用 mail_delivery owner 入队；冻结 payload、收件人和摘要提供独立 proof。
内部入队接口保留类型化 SQLx 错误，`40001`/`40P01` 交回竞价 runner，沿用最多三次整事务尝试；
不局部重排入队，独立邮件仓储端口仍在边界脱敏，不暴露 SQL 错误或改变发送重试规则。
领取只读 proof，sending 先锁 lot 再锁 delivery 并复核；取消/重新领先使旧资格永久失效，未开始发送的旧任务收口为 failed 并追加审计。
已 sending 的未知结果仍进入 manual_reconcile；已接受标记命令在拍卖后来隐藏后仍能重放原响应，新命令遵守当前公开可见性。
正文不包含代理上限。完整标记列表超过 10000 条明确失败。
`lot_watch/preference.rs` 只锁 user 做独立偏好 CAS；竞价普通快照读取渠道，不晚加旧 leader user 锁。
[lot_watch_pm](src/database/lot_watch_pm) 独占 PM claim、lot→任务 sending fence、原 sender 对账和 CLI 安全投影。
`000042` 保留旧 MM payload/digest；PM 缺 UID 明确 suppressed，不吞 SQL 错误、不回退 MM。
sending 永不回 pending，sender/sending_at 由数据库触发器保护；过期 sending 只转 manual_reconcile。
领取按 `COALESCE(claimed_at,created_at),created_at,notification_id` 持久轮转；准备失败释放时保留领取时间，
防止不存在或持续拒绝的收件人独占每轮新发送名额，不增加并发、配置或 sending 后重寄。
CLI 请求重放/CAS/审计同事务，仅申请 Worker 只读对账。迁移和发送行为未在本轮执行。
`delivery_admin` 仅为明确失效的 watch preflight 保留无 sending 时间的严格投影，普通失败和未知发送边界不变；
人工 release/retry 也在 lot → delivery 锁内复核 watch proof，不恢复旧周期。

## 持久调度与慢速代理

scheduler 保存 due、租约、尝试次数和失败状态；完成/失败须匹配 owner、精确租约及版本。Admin 改期清除旧租约并推进版本，
陈旧任务不能收口。已存在轮次按冻结事实读取，配置变更不改写旧命令或时间。
自动 Draft 不提前物化运行期结束时间；后继 SQL 规则列与快照同取新模板，不复制或改写前驱经济策略。

slow proxy 扫描在 LIMIT 前排除已到截止的 lot；执行由竞价事务复核 generation、due、授权与资格。
调度字段与确定性命令持久化，不另建内存游标或 Redis 锁。

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

编号页 total 与当前页来自一致快照。禁用 world 的活动投影不能公开；已 settled 历史按契约读取。
[auctions/countdown.rs](src/database/auctions/countdown.rs) 用单条 SQL 读取同场公开时间及数据库时钟；
不加载装备、凭据或论坛正文，不公开草稿、取消场次、被活动场次遮蔽的预览或预览计划截止；无需新增表和迁移。
Public 拍卖、竞价日志、成交历史与补偿批次的游标查询仅对额外探测行使用饱和加法；不截断请求数量，保留既有快照和 has_more 语义。大页读取成本见[分页契约](../../docs/contracts/routes.md#public-分页世界与金额)。
Admin 读取不为补全投影临时写入。邮件展示和诊断 sidecar 均非权威数据，不参与幂等、完成证明或 cursor。

Delivery 进入 sending 前须证明命令已提交；未知发送结果进入 manual_reconcile，不自动重寄。
[delivery_scheduler/cash](src/database/delivery_scheduler/cash) 统一新旧 sending 入口：
有证据时 generation→排序 worlds→资金scope/account→必要业务/凭据→义务→delivery→fulfillment→allocation；
无证据时早锁scope及持久allocation关联，任何受保护账户或已有绑定均不能绕过。
完整payload、recipient、command/hash及token/fencing/实时lease锁内精确匹配；reserved→in_flight与sending同事务。
现金不足只提交冻结并返回Deferred，其余拒绝整事务回滚；SQLSTATE/A/B类型保留至最多三次重试边界。
现金 until 入口沿用 prepared 原单调截止与 UTC 证据窗口的交集，每次重试只用原剩余整毫秒，零预算拒绝；
PostgreSQL 18 `transaction_timeout` 在业务锁/审计前设置并覆盖 COMMIT，截止后回执不授权发送。
旧 authorized(Some) 缺精确单调期限明确拒绝，None 保留原受保护 legacy 门；未知提交由 guard 撤销 owner。
Sent结果同事务记spent，failed/unknown记unknown；过期sending逐条先定位原资金账户再锁delivery，不从metadata猜UID。
已有allocation的五种新Admin动作全部返回Invalid且不写状态；原稳定命令先重放，无绑定legacy保留原语义。
本轮不提供资金返还或未知重发授权，任何占用不会因TTL或人工失败释放。
compose 安全改由 transport 在 sending fence 前实际只读检查，不再按历史失败永久阻断整个 world。
未开始写入的有效 claim 释放回 pending 时撤销本次领取 attempt_count，并在同事务追加 release 审计；
租约 token/fencing 和调用时刻、数据库实时有效期仍须一致，历史 sending/manual_reconcile 不进入该释放路径。
续租仅允许当前有效 claimed/sending；先取得 delivery 行锁，再复核 token/fencing 及数据库 clock_timestamp()，避免锁等待跨越有效期。
新截止必须仍在未来且不缩短已有租约；sending fence 保持 lot → delivery 锁序，释放同样在行锁后检查实时有效期；
准备期间可续租，过期租约、旧 owner 和 manual_reconcile 均不能复活。Worker 在 fence 前后确认续租后才开始外部写入。
主动 Send 要求新增、唯一且完整匹配的 Sent 证明；流拍/撤拍逐件 Send 原装备，历史冻结 Return 不改写。
论坛未知 POST 先精确只读对账，再按持久人工授权重试；真实处理楼层独立于内容版本和 post ID。
`forum/catalog.rs` 拥有 `auction_forum_catalog_pages` 续页身份/正文；迁移 `000040` 增加主记录目录页数。
发布和刷新在主记录 claim/revision CAS 的同一 PostgreSQL 事务内更新所有续页，领取检查页数、连续楼层及互异身份。
`catalog_layout` 冻结历史单页/新分页方案；旧主题保持原楼层并继续处理出价，仍受原正文长度上限约束。
仅从未尝试、无远端身份的队列项及新主题启用分页，已尝试或未知写入不得借升级更换布局。页数冻结后不在扫描时移动系统日志。
`000043` 为 forum owner 增加明确纳入标记和 nullable 终态确认版本；历史 settled 仅豁免，不伪造成功，发布恢复和回帖领取都排除它们。
`forum/sync/snapshot.rs` 在同一 Repeatable Read 中读取结算汇总、lot 明细和最终领先事实；
`forum/sync/settlement.rs` 核对持久落锤价、赢家和来源，不重算曲线或补偿。
`forum/reply/fixed_posts.rs` 仅在全部固定帖确认后，以 auction 共享锁核对 settled 版本和无 pending，再按 sync lease/fencing/revision CAS 收口。
普通扫描期间结算也不能以未发布快照完成；部分进度不写终态版本，未知写入保留原门禁。
`forum/reply/commands.rs` 在 auction 共享锁下拒绝非 live/final 新命令或重试，不删除已有 pending。
`forum/reply/queries.rs` 从既有命令账本投影 `last_error_code` 与有界详情，不回填或改写历史拒绝。
新拒绝分类由 jobs owner 提供，仍经原 pending → rejected 条件更新收口；无需新增列或迁移。
论坛领取可按 Worker 首轮水位重新领取旧凭据失败和未知写入，原失败码进入 claim；不领取 `write_authorization_required`，
不伪造远端证明，updated_at 超过水位后只能走正常 due/lease 门禁。
Delivery Admin 重试由 `recovery_commands/commands/delivery_retry.rs` 在原 Serializable 命令事务恢复匹配的 Recovery
父子失败投影并推进 fencing；不生成成功证明、不清除其他动作阻断。证明规则见[不变量](../../docs/contracts/invariants.md)。

## 内部履约原子事务设计

[fulfillment_scheduler/internal](src/database/fulfillment_scheduler/internal) 实现独立 `bot_internal_v1` 满足事务；
迁移 `000048_internal_fulfillment` 只新增稳定命令与非敏感完整证明表，不回填旧记录清白或改写冻结金额。

- 原命令先重放；同负载原结果、异负载冲突。Serializable整事务最多三次，只按类型化40001/40P01重试。
  确认ROLLBACK才返回RolledBack；未知提交/取消由应用opaque guard撤销原owner。
- 锁序为generation→排序worlds→资金scope/account→auction→recovery→lot/equipment→credentials→users→
  排序obligations→actions→deliveries→fulfillments→proof/ledger/audit；旧prepare/activate/satisfy同步此业务前缀。
  同EID可能关联多个本地equipment UUID，均核对；当前实现为完整孤儿排查按UUID锁全部deliveries，成本须在集成评审关注。
- 复核当前用户版本、有效claim token/fencing/state_version及数据库实时时效；新pending必须从未准备、激活或关联delivery。
  同EID/world其它义务、恢复动作、出站及孤儿记录无法排除冲突时等待，旧unknown不解除。
  `conflicts.rs` 按真实 origin/payload 闭集 schema 排除已验证无装备通知；普通 attachment_ids 按外部 EID/world，
  settlement secret 引用按实际 equipment EID/world 核对。论坛 `forum_auction_notice` 仍属 mail_intake，
  仅同 command/world 的 `auction_forum_bid_commands` notify 账本、零命令金额及完整 producer origin 重建一致，
  且 origin_id=external_mail_id、模板/结果码相符、零 Credits/COD、空附件、无 key/正文时，才识别为无装备通知；
  关联只读，不新增反向业务行锁。缺失/畸形/未知 schema 仍阻断，不凭前缀或 succeeded 放行；
  原精确 recovered_handoff 例外、来源关联检查和统一锁序保持。
- 库存只消费A完整正向Armory快照；所有资金SQL由settlement_funding拥有。缺真实C、自有用途或来源不内部满足。
  原合法退款仅在equipment_recovered及原外部handoff成功/精确取回证明闭合后转移原S/T，未新增内部取消退款。
- 满足、清claim/版本、独立proof、资金动作、直接blocked依赖和追加审计同事务；contingent退款不通用解锁。
  外部 COD 在任何满足/资金化/解依赖前，以已锁账户和同一有效 Main cash 检查既有占用；
  缺口确定提交冻结并返回 FundingDeferred，不满足原付款、不发行、不解依赖。通过后才保存原 Sent collected 满足、
  资金化并做发行后覆盖校验，最后释放原依赖；无账户且无保护账户仍为原外部语义，不产生 allocation。
  外部退款用途预留只授权原退款allocation，不满足义务或伪造外发结果。

Recovery action终态先锁父case再锁action，确认路径按UUID预锁全部fulfillment。
Admin旧重试由recovery owner提供 `lock_recovery_delivery_retry_scope` / `lock_recovery_delivery_retry_actions` 两阶段锁；
delivery_admin 已在资金之后先锁recovery，完成所需 watch lot 锁后锁actions，再锁delivery并调用原reset；
只用于无allocation的 RetryFailed/ReconcileRetry，SQLSTATE原样交runner，原授权/CAS不变。
源码静态结果见[状态页](../../docs/status.md)；迁移、SQL执行、并发与大库锁成本仍需独立运行证据。

## 真实现金账户与事务helper

[settlement_funding](src/database/settlement_funding) 独占迁移 `000047` 的资金scope、稳定UID账户、
唯一source/receipt、用途allocation、追加entry及命令结果。初始0/unreconciled，不迁移回填历史现金。
helper在generation/world之后、业务锁之前取得scope/account，令牌绑定当前事务；业务owner随后锁原冻结事实。
内部B及实际C转原seller S/受限T，seller retained不自动再投资；合法退款撤销互斥S/T后按原用途还原。
所有Credits Send须经reserved→in_flight→spent/unknown，失败/人工failed不释放，精确返还另追加事实。
`lock_funded_delivery_account`在delivery锁之前按持久allocation关联早锁原账户，供结果回写、过期claim和人工门禁使用；
spent、返还后的关联及冻结账户仍返回原令牌，孤儿/歧义失败关闭。只有确无关联才返回None，scope仍持锁；
delivery owner 不从metadata推断账号或把非ready账户降级为legacy。

完整责任preview使用Serializable READ ONLY；旧无UID责任只计保守隔离，不改归属。apply锁完整责任集合、
核对预览摘要/版本和真实现金，只有真实余量可建立本次reconciliation来源；既有allocation全部扣占，
同余额不能反复开额度，差额冻结账户。未资金化付款已外发但收款事实未闭合（或关联 delivery 缺失）时，
preview/apply 共用 snapshot 增加 `payment_collection_unclosed` blocker，Main 装备 COD 与跨世界现金 carrier 均覆盖；
正常尚未外发的 pending/prepared 付款不因此阻断。补偿与隔离新增需求明确不低于零，既有占用仍全部计入 allocated，不能释放。
新COD要求精确MID/当前账号实收绑定、全局payment/source唯一，
且发送晚于最近opening边界；跨边界历史款继续核对。仅完整发送门装配后可启用；
`delivery_scheduler/capability.rs` 是当前构建唯一V1提供者，非postgres档位返回None。
启用仍要求相关程序同版、停止旧Worker、专用账号无外部资金写入及mail owner独占；构建能力不替代运行验收。

## 连接池可观测性

`DatabaseStore::pool_metrics_source` 暴露借出、空闲、总数、上限及真实 acquisition 累计指标。
普通查询、事务与显式 acquire 均经过 `TrackedPool`，不额外查询 SQL，也不暴露数据库名或连接串；外部原生 SQLx 池不计入累计值。
readiness 检查数据库、内嵌迁移及审计写入失败计数，异常失败关闭。

## 只读盘点与重建

[retention_inventory](src/database/retention_inventory) 在只读快照按版本化政策盘点关系、依赖和 realtime 水位，不授予删除资格。
[recovery_preflight](src/database/recovery_preflight) 只核对恢复目标，不运行 pg_restore 或覆盖对象。
[bootstrap](src/database/bootstrap) 在维护库身份、权限和集群锁下创建缺失库，不提供 DROP 或 migration；
[rebuild](src/database/rebuild) 是独立破坏性边界，要求精确目标、受限目录，并保留受支持的数据库属性。
备份、人工确认、停止进程、外部工具与 receipt 归 `auctionctl`；步骤见[数据库恢复与保留](../../ops/README.md#数据库恢复与保留)。

## 后端选择

配置层将 `[database]` 或兼容的 `postgres://` / `postgresql://` URL 交给本 crate；不在存储层解析 TOML。
默认不编译驱动，运行须显式启用 `postgres` feature，使用 SQLx PostgreSQL/rustls。

## 迁移

[PostgreSQL 迁移](../../migrations/postgres/README.md)只追加；普通启动不隐式迁移，执行门禁见[迁移导航](../../migrations/README.md)。
`initialization/` 只供确认后的合并首次空库初始化：同一专用连接持 SQLx 迁移锁，拒绝已有用户对象与其他客户端；
不建库、不删对象、不升级已有库。普通迁移与初始化共用受限执行视图：`000005`/`000015` 修正历史别名，
`000044` 在 ALTER 前执行已排队的 `user_forum_identity_account_id_check` 延迟约束，避免用户时间修复产生待处理事件。
约束失败仍回滚用户更正、版本及审计；不重复加锁，不修改已发布文件、checksum 或原迁移事务边界。

`recovery_preflight/catalog.rs` 拥有空库对象盘点；subscription 仅计当前库，default ACL 等残留仍阻断。
`migrations/history.rs` 核对完整 SQLx 历史前缀及 checksum，旧 archive 恢复不能只比较最大版本。
canonical v28 必须走[专用接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)。

## 连续领先时间

`leader_since` 按全部接受事实判断连续领先是否中断，普通出价后被原领先者快速代理反超也重新计时。
`000037` 只重建可由追加式 bid 证明的未结算 live lot 投影；无证据不猜测，不改已结算金额、终态 lot 或历史审计。
冻结结算策略解码不应用新模板门禁，后继创建另由应用/领域校验。

## 维护与验证

```bash
cargo check --locked -p auction-storage --no-default-features --features postgres
```

仅静态编译，不执行迁移或行为测试。完整门禁见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。

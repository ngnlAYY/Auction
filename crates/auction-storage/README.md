# auction-storage

持久化适配器边界：PostgreSQL 完整后端与 SQLite Lite 后端。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；数据库装配、迁移和后端适配入口位于
`src/database/mod.rs`，SQLx 连接池 acquisition 追踪位于独立的
`src/database/pool/{mod,metrics}.rs`；PostgreSQL 竞价事务按职责放在
`src/database/competition/postgres.rs` 只保留事务装配与稳定端口；生命周期、管理命令和共享映射
分别位于同目录的 `postgres/{lifecycle,admin,mapping}.rs`，竞价 owner 拆入
`postgres/bidding/{mod,ledger,snapshot,eligibility,plan,proxy,facts,audit,realtime}.rs`；其中管理事务再按
拍卖创建、lot 更新和行映射拆到 `postgres/admin/{create,mapping}.rs` 与 `postgres/admin/lot/`；
`lot/` 再按事务适配、锁内读取、状态变更、withdrawn-return 义务和审计/realtime 事件拆到
`{mod,transaction,read,mutation,obligation,events}.rs`。SQLite Lite 竞价事务
由 `src/database/sqlite/competition/transaction.rs` 分派，具体生命周期和映射仍由
`transaction/{lifecycle,mapping}.rs` 持有；竞价 owner 进一步拆入
`transaction/bidding/{mod,ledger,snapshot,eligibility,plan,proxy,facts,audit,realtime}.rs`，分别负责命令账本、
锁内快照、参与资格、计划编排、代理/lot 写入、审计和 realtime 事件；两后端的 `snapshot.rs` 还会在同一
竞价事务内按序读取完整代理转换历史，供应用层生成并核验旧版 `ProxyResult` 账本载荷；历史
`accepted + CompetitionPlan` 只读重放还会从同一事务读取拍卖冻结策略中的 proxy quota，不使用当前
生命周期或时间 gate。`BEGIN IMMEDIATE` 事务边界仍由父文件统一维护；
`src/database/competition_audit.rs` 只拥有两后端共用的 Auction-BF 外部竞价命令级审计映射，
普通 bid、代理创建/降低各写唯一旧 action，内部代理转换继续由追加式 transition 表保存；后续领域事务
继续按同一规则拆分。
Public 竞价日志由 `src/database/bid_log/mod.rs` 装配，公共 cursor/结果校验、PostgreSQL 查询/映射
和 SQLite Lite 查询/类型转换分别位于 `bid_log/{common,postgres,sqlite}.rs`；两套后端继续共享
时间、lot、状态版本、事实序号和 bid ID 的 keyset 排序语义，但保留各自 SQL 与能力边界。
数据库根入口的后端错误与 URL 识别位于 `database/backend.rs`，运行时 Store 选择、迁移目标、
readiness 和连接池入口位于 `database/store.rs`；`database/migrations/mod.rs` 为 PostgreSQL
`000005`/`000015` 提供只替换保留字别名、但默认保留历史 checksum 的受限执行视图；它还只读识别
早期别名修正版已经记录的两组已知 checksum，不改写迁移历史；连接池指标适配位于
`database/pool/metrics.rs`；
PostgreSQL/SQLite 的连接池与后端专属边界分别位于 `database/postgres_store.rs` 和
`database/sqlite_store.rs`。`database/mod.rs` 只负责模块装配、兼容导出和保留的根级领域测试，
不改变 `DatabaseBackend`、`DatabaseStore`、`PostgresStore` 与 `SqliteStore` 的公共路径。
PostgreSQL 破坏性重建由 `database/rebuild/` 持有：`model.rs` 定义预检/结果/错误契约，`catalog.rs`
负责维护库目录采集与安全预检，`guard.rs` 负责 advisory lock、删库/建库和后置身份校验，`sql.rs`
只负责数据库名、locale/provider、tablespace 和连接上限的 SQL quoting；`rebuild/mod.rs` 只保留稳定导出。
`src/database/legacy_v28/` 只负责 canonical Auction-BF PostgreSQL v28 的只读预检、事务 bridge
和 SQLx 基线接管；`model.rs`、`preflight.rs`、`source_checks.rs`、`adoption.rs` 与 `mapping.rs`
分别持有安全模型、预检、来源事实检查、接管 bridge 和错误映射；备份、人工确认及不可覆盖回执仍由
`auctionctl` 拥有。实际接管在取得 migration advisory lock 后，使用同一物理连接的只读事务重新
执行完整预检，避免把连接池自身误计为外部 session。
Worker 的 `scheduled_rounds` due claim、租约和失败状态由
`src/database/scheduler/` 持有：`mod.rs` 只做端口分派，`claim.rs` 负责 PostgreSQL/SQLite
领取，`resolution.rs` 负责完成/失败 fence，`mapping.rs` 负责阶段、命令和 Lite 映射；它不把
调度状态放进内存或 Redis。
慢速代理使用 lot 自身的 `slow_authorization_id`、`slow_generation` 和 `slow_due_at` 作为持久化
generation；`src/database/slow_proxy_due.rs` 按数据库时钟无锁有界扫描，随后由两后端竞价事务
重新锁定 lot/活动代理/用户、核对截止时间并原子写入命令账本、计划、事件和
`lot.slow_proxy_tick` 审计。它沿用旧版确定性身份，不另建租约表或内存游标。
Admin 结算报告读取由 `src/database/settlement_report/mod.rs` 持有，只在 PostgreSQL
完整档读取同一 Serializable 快照内的结算摘要、补偿账本、lot 金额、挂牌价、持有比例和调整比例，对序列化冲突/死锁最多
重试三次；已存在的结算摘要不会因补偿账本损坏而被静默隐藏，后续闭集校验会失败关闭。SQLite Lite
明确不支持该能力。
Admin recovery 报告读取由 `src/database/recovery_report/mod.rs` 持有，在同一 Serializable
快照内读取 recovery/action 安全投影；`recovery_commands/commands.rs` 只做 PostgreSQL 完整档恢复命令分派；
确认写入、范围快照、guarded relist 和共享映射分别位于 `recovery_commands/commands/{confirm,scope,relist,mapping}.rs`，
其中 confirm 又按 Admin 确认、Worker relist、buyer refund 和共享 fence/审计辅助拆入
`confirm/{command,relist,refund,common}.rs`，scope 又按数据库行模型、命令回放、锁定读取和状态/证据校验拆入
`scope/{model,replay,locks,validation}.rs`；
共同完成 Serializable 确认写入、证据重校验、退款激活、命令账本、审计和预览目录事件；
SQLite Lite 对确认能力明确返回不支持。
结算事务在 `src/database/settlement/mod.rs` 装配，由 `transaction.rs`、`facts.rs`、`obligations.rs`
和 `mapping.rs` 分别负责锁内流程、事实/账本、义务/恢复和映射；它根据冻结的 recovery 策略，为
每个获胜 lot 与结算义务同事务建立 `settlement_recoveries`。补偿账本通过结算命令的权威
`aggregate_id` 关联拍卖；命令账本、lot/池/账本、义务、恢复、
审计、实时事件及终态统一使用 workflow 冻结的 `due_at` 作为业务事实时间；已 settled 的旧版命令
重放只读取结算摘要、lot 明细和义务聚合并失败关闭，不会重写历史或再次插入结算事实。
PostgreSQL 结算提交只对映射为序列化冲突或死锁的错误执行有界重试；连接、约束和其他提交错误
立即按原分类返回，不能把未知提交结果当作可安全重放的事务。
`src/database/settlement_commands/legacy.rs` 为 `auctionctl` 在
`SERIALIZABLE READ ONLY DEFERRABLE` 事务中装配专用核验入口；账本读取与 settled 快照均不请求
`FOR UPDATE`，缺失旧 complete 账本或任何身份/事实不一致都失败关闭。SQLite Lite 明确不提供该能力。
`src/database/recovery_scheduler/scheduler.rs`
只负责调度分派，推进、action 和映射分别位于同目录的
`scheduler/{advance,actions,mapping}.rs`，退回 owner 位于 `scheduler/returns/`，再按事务编排、
证据模型、校验、action 幂等写入和行映射拆到 `returns/{mod,record,model,validation,action,mapping}.rs`；
其中 action 再按领取、投递和收口拆到
`scheduler/actions/{claim,resolution}.rs` 与
`scheduler/actions/delivery/{mod,prepare,activation,observation}.rs`，负责
PostgreSQL 完整档的 due case 推进、付款超时 `payment_reminder` action 创建、退回邮件证据绑定、
evidence mismatch 阻断、ordinal recovery Take、持久化 claim/fencing、recovery delivery 的
`pending → prepared → enqueued` 两阶段身份和结果收口；prepare/activation 前还会读取 Users
owner 的 `username`（当前 MM 收件人）与 `active` 资格，非 active 收件人只写入持久化退避，不会直接投递。
`src/database/users/admin/` 负责 recovery 触发的 Users 黑名单幂等命令、黑名单历史和审计；
其中 `mod.rs` 持有跨后端分派与共享校验，PostgreSQL `postgres.rs` 只负责子模块装配，
`postgres/commands/{mod,blacklist,create,mutation,revert}.rs` 持有按命令职责拆分的写入流程，
`postgres/reads.rs` 与 `postgres/mapping.rs` 分别持有查询读取和行/幂等结果映射，
SQLite `sqlite.rs` 同样只负责子模块装配，`sqlite/commands/{mod,blacklist,create,mutation,revert}.rs` 持有
按命令职责拆分的串行写入，`sqlite/{reads,mapping}.rs` 持有对应的查询读取和行/事务映射，
并由 recovery owner 记录完成 proof。`recovery_commands/commands/{confirm,relist}.rs` 同时持有 PostgreSQL 完整档的
relist/refund resolution 调度：重挂目标、原拍卖、原 lot/equipment 和 custody action 在
Serializable 事务中重新校验，目标暂缺时写入 `next_attempt_at` 退避，buyer refund fulfillment
满足后再以时间和状态 fence 关闭 recovery；两种推进都追加 worker 审计。SQLite Lite 对 recovery
完整档明确返回不支持，不能由报告读取适配器临时补写。
Public 规则文档由 `src/database/rules/` 持有：`mod.rs` 只做模块装配，`reads.rs` 负责
Public/Admin 的跨后端安全快照读取与 SQLite 时间映射，PostgreSQL 规则发布事务位于
`transaction.rs`；规则 revision、公开字段校验和 SQLite Lite 只读边界保持显式。黑名单安全
投影由 `src/database/blacklist/mod.rs` 持有，PostgreSQL 与 SQLite 均只返回公开字段。
Public lot 读取由 `src/database/lots/` 持有：`mod.rs` 只做后端分派，PostgreSQL 与 SQLite
分别由各自子目录的 `mod.rs`、`list.rs`、`detail.rs` 和 `mapping.rs` 拥有列表、详情与行映射；
顶层 `mapping.rs` 继续负责跨后端安全投影、装备字段校验和 Lite 时间/UUID 转换。两套后端的
Lite/完整能力差异在各自 SQL 适配器中显式保留。Public 的 `next_bid` 与普通竞价事务统一使用
完整加价阶梯；论坛帖专属的代理末口 `+1K` 投影不得进入浏览器竞价契约。
`src/database/equipment_secrets.rs` 统一拥有 PostgreSQL 装备密文信封的只读 SQL 与结构校验，
Worker 履约和 Public 真实装备 URL 分别通过自己的应用端口映射错误；Public 列表使用一次有界批量
读取，避免逐 lot 查询。`src/database/public_equipment_link.rs` 只返回密文投影，SQLite Lite 固定
报告能力缺失，明文 key 不会越过存储边界。
Public 拍卖目录读取由 `src/database/auctions/` 持有：`mod.rs` 只做跨后端端口装配，
`postgres.rs` 与 `sqlite.rs` 分别负责查询，`mapping.rs` 负责行到安全投影的转换和列表参数
校验；两后端还按启用世界和公开状态闭集提供 WebSocket 预检，不读取事件历史，也不把 draft/cancelled
伪装成可订阅 projection。cursor、总数和 Lite/完整档字段差异仍由对应适配器明确表达。
Public 成交价格历史由 `src/database/price_history/` 持有：`mod.rs` 只做端口分派，
`postgres.rs` 从 `auction_settlements` 与 `lot_settlements` 的已提交事实链查询并负责行映射，
`mapping.rs` 负责混合游标、名称筛选、统计结果及公共字段校验。SQLite Lite 没有结算事实 owner，
对应端口固定返回 `CapabilityUnavailable`；不得把可手工写入的 `settled`/`sold` 状态行伪装成完整成交历史。
会话持久化由 `src/database/sessions/` 持有：`mod.rs` 只做 Admin/Public 端口装配，
`admin.rs` 与 `public.rs` 分别负责对应会话的 PostgreSQL/SQLite 写入与刷新，`common.rs` 负责
共享错误和 Lite 时间/UUID 映射；会话真值不进入 Redis。
Public 账户竞价与代理历史由 `src/database/account/` 持有：`mod.rs` 只做端口分派，
`postgres.rs` 与 `sqlite.rs` 分别负责快照查询；`mapping/` 下的 `bids.rs`、`proxy.rs` 和
`common.rs` 分别维护普通/代理竞价事实、代理状态投影以及跨后端读取与分页校验，`mapping/mod.rs`
只负责稳定导出，公开账户边界、代理能力和结果不变量保持不变。
Public 补偿池与单场账本由 `src/database/compensation/` 持有；`mod.rs` 只负责后端分派，
`postgres/{rounds,movements}.rs` 分别拥有批次/余额和单场账本查询。PostgreSQL 完整档在同一
Serializable 快照中读取结算摘要、账本和最新余额，SQLite Lite 明确返回能力不支持。
Public 论坛镜像由 `src/database/forum/mod.rs` 读取 PostgreSQL 的本地同步投影；同目录的
`sync.rs` 只做主题同步端口装配，领取租约、快照读取、发布/失败状态推进和输入校验分别位于
`sync/{claims,snapshot,publishing,validation}.rs`，`sync/authorization.rs` 单独拥有未知写入清单、
幂等人工再授权、命令重放和追加审计事务；`reply.rs` 只做回帖端口装配，已发布主题与
lot/代理查询、论坛账号键与 MM 玩家名称解析、命令账本、扫描游标和固定楼层内容 CAS 分别位于
`reply/{queries,identity,commands,cursor,fixed_posts,validation}.rs`。`identity.rs` 在 Users-owner
Serializable 事务内用数字账号键定位论坛映射并同步到唯一的 `users.forum_account_id`；
`users.username` 是可变的 MM 收件人/页面名称，论坛已绑定主体改名时随作者链接更新。active-key
同名用户可以建立首个映射，revoked 用户只能沿用另一世界已证明的同一数字键，否则为尚未注册 MM
的作者建立 revoked-key `forum_only` 临时主体及 world 映射。`000026` 保证同一 world/provider 的一个
Users 主体只有一个论坛账号，`000027` 增加 Users 级唯一论坛账号 ID，并以延迟约束保证 Users 与该
主体所有论坛映射在事务提交时一致；Admin 修正数字 ID 时也在同一事务更新全部映射。旧
`game_player_id` 仅作为 `username` 的数据库兼容镜像，数字键不会进入 MoogleMail 收件人。`000025` 在同一同步行保存固定 #1/#2
的真实 post ID、#2 规范正文和内容版本，并通过独立 due 水位、claim/lease/fencing 串行化远端
编辑；历史 published 行保持未对账，不猜测楼层身份。
主题同步只为已经实际开放的 `live` 拍卖初始化一次，已发布记录不会随 `state_version` 变化再次领取；
快照只输出装备 ID、外部 ID 和不含 key 的目标，真实 URL 由 Worker 的外部链接解析边界生成。
`credential_unavailable` 保持非写入失败；只有当前 Worker 确认内存凭据可用时，领取 SQL 才越过一年
人工退避，并且仅放行早于该进程首轮恢复水位的记录。失败写回会越过该水位，防止同一进程紧循环；
`write_unknown`、发布证明失败和人工授权状态不经过此分支。
回帖账本接管会同时核对命令 ID 与冻结自然键；自然键和完整业务负载一致时复用既有旧版命令 ID，
两种身份命中不同记录或任一负载字段漂移时失败关闭。
`000028` 为账本保存可证明的真实楼层号：既有空值只在同一自然键和完整负载再次匹配时补齐，历史
未知值不会从远端 post ID 推算。Public 竞价日志据此区分论坛普通竞价与 Web 代理事实，代理来源不再
暴露 fast/slow 子类型。
远程读写仍由 Worker/集成 owner 负责，SQLite Lite 不提供论坛完整档。
Admin lot 列表由 `src/database/admin_lots/mod.rs` 持有，在同一 PostgreSQL Serializable
快照中读取拍卖生命周期、lot、装备和投稿来源安全投影；SQLite Lite 因没有完整 Admin 生命周期
列明确返回能力不支持。
Admin 拍卖列表由 `src/database/admin_auctions/mod.rs` 持有，在同一 PostgreSQL Serializable
快照中读取 world/status 筛选下的生命周期和 lot 数量；SQLite Lite 明确返回能力不支持，不能
用空列表或默认能力掩盖完整档缺失。
拍卖生命周期事务由 `src/database/competition/postgres/lifecycle.rs` 持有，父文件只负责装配；其 `lifecycle/` 子目录的
`context.rs`、`plan/` 和 `record.rs` 分别负责上下文锁定、生命周期计划写入和审计/实时事件记录，其中
`plan/{preview,open,cancel,final_phase,early_settlement}.rs` 按状态动作拆分 PostgreSQL 写入：管理命令锁定拍卖后校验
`expected_state_version`，人工提前开放还会用装配时冻结的策略和锁内 `scheduled_open_at` 失败关闭；
Preview 在修改状态和转移 intake target 前还要求至少一个 pending lot，防止形成永远无法 Open 的空活跃场次；
人工 Final 只收紧较晚的 live lot 截止时间并保留软截止竞价；提前结算只在 PostgreSQL 完整档把截止
时间收紧到事务时间，使既有 settlement scheduler 可领取，结算事实仍由 Worker 事务写入。取消动作同时写入取消时间、原因和操作者。SQLite Lite 通过追加迁移提供同名取消元数据列和形状
触发器；SQLite `src/database/sqlite/competition/transaction/lifecycle.rs` 也只负责端口装配，
`transaction/lifecycle/{context,plan/{mod,preview,open,cancel,final_phase},record}.rs` 分别负责上下文/资格读取、
按状态动作的计划写入和审计/realtime 事件记录，并保留 `BEGIN IMMEDIATE` 的单写者边界。两后端通过
`src/database/lifecycle_audit.rs` 把父命令映射为旧版 action 专属 metadata，审计表 `request_id`
保持 NULL，HTTP 关联值只进入 metadata；Final slow/失格代理只追加一条
`lot.proxy_final_transition` 命令级审计，不再为同一子命令重复写逐 bid/transition 通用审计。内部
`final_fast` 状态仍保留，只有旧版审计边界投影为 `mode=fast/status=active`。
Admin 拍卖创建由 `src/database/commands/mod.rs` 与 PostgreSQL
`src/database/competition/postgres/admin/create.rs` 共同持有：应用服务使用 `admin_auction_create` 命令账本，
事务锁定 world 和 intake target，写入 draft、策略快照、审计、拍卖专属流中的 `auction.created`
事件及公开发现流的持久化派生别名；SQLite
Lite 明确返回能力不支持。策略模板会在同一事务中同时写入 schema v4 `policy_snapshot` 和
竞价/生命周期仍读取的 `default_start_price`、软截止、取消截止、slow 间隔列，避免配置与
遗留规则分叉。A03 请求中的 `scheduled_end_at` 当前冻结在策略快照，生命周期列在真正开放前
保持空值，以兼容旧系统创建行为。
Admin 规则读取与发布由 `src/database/rules/` 和 `src/database/commands/mod.rs` 持有：
`rules/{mod,reads,transaction}.rs` 分别保留模块装配、跨后端读取和 PostgreSQL 写事务；
PostgreSQL 使用独立 Serializable 事务、规则 revision CAS、`rules_admin_commands` 幂等账本和
追加审计；SQLite Lite 读取规则安全快照，但发布明确返回能力不支持。
Admin 装备录入由 `src/database/commands/admin/intake.rs` 与
`src/database/admin_intake/{mod,write,record}.rs` 持有：PostgreSQL 在 Serializable 事务中按 world、intake
target、玩家和外部装备身份加锁，`write.rs` 写入装备、`equipment_secrets`、后台投稿、lot、版本和命令
账本，`record.rs` 追加审计与 realtime 事件；内部 `lot_number` 保持场次内全局顺序，
`category_number` 在装备分类内从 1 独立递增并作为安全展示编号。SQLite Lite 因缺少密文表与完整录入能力明确返回不支持。
Admin draft lot 修改由 `src/database/commands/admin/lot.rs` 与 PostgreSQL
`src/database/competition/postgres/admin/lot/` 持有：`transaction.rs` 负责事务端口适配，`read.rs`、
`mutation.rs`、`obligation.rs` 和 `events.rs` 分别负责锁内读取、lot 状态/价格变更、withdrawn-return
义务及审计/realtime 事件；事务按 world → auction → lot 加锁，完成 lot/auction 版本 fence、预览
revision 和命令账本。withdrawn 同一事务创建 `withdrawn_return` obligation 与 pending fulfillment，
SQLite Lite 明确返回不支持。
Admin runtime 命令账本由 `src/database/commands/admin/runtime.rs` 装配，跨后端分派、PostgreSQL
只读查询、pending/applied/unknown/reopen 事务、行映射和错误/SQLite Lite 能力边界分别位于
`admin/runtime/{dispatch,queries,mutations,mapping,errors}.rs`；命令摘要、状态 fence、审计
追加和未知结果重放语义保持不变。
Admin 审计读取由 `src/database/admin_audit/` 持有：`mod.rs` 只做请求校验与后端分派，
`postgres.rs`、`sqlite.rs` 和 `mapping.rs` 分别负责两套查询与 SQLite 行/时间转换；两套数据库
都只读取追加式 `audit_records`，支持安全精确筛选、keyset/数字分页和 metadata object 校验；
数字页的总数与记录在同一读取事务内取得，不返回原始数据库行。
Admin 改名历史读取由 `src/database/admin_renames/mod.rs` 持有；用户管理写入由
`src/database/users/admin/` 持有。两套数据库都在用户命令事务内完成 state-version fence、
命令账本、用户投影、密文 key 信封、`user_renames`、`user_blacklist_history` 和追加审计；
PostgreSQL 使用 Serializable/advisory lock，SQLite 使用 `BEGIN IMMEDIATE`。MoogleMail 注册/换绑
仍按其独立能力清单只支持 PostgreSQL，不能把两者混写成完全对等。
Users Mail owner 由 `src/database/users/mail.rs` 只做端口分派；`mail/postgres/{commands,reads,mapping}.rs`
分别负责 MoogleMail 注册/换绑命令、幂等与信封读取、结果/身份/事务映射，`mail/sqlite.rs` 只负责
Lite 登录 key 信封读取。这样 PostgreSQL 完整档的外部身份命令与 SQLite Lite 的本地信封能力保持
显式边界，明文 key 仍不进入存储或 HTTP 投影。
Admin A32 凭据管理由 `src/database/credentials/mod.rs` 装配：`runtime.rs` 负责 active/health
读取，`read.rs` 负责列表与命令查询，`write.rs` 负责导入/修改/删除事务，`mapping.rs` 与
`common.rs` 负责安全投影、结果和错误映射。PostgreSQL 在 Serializable 事务中完成
`mail_cookie`/`forum_cookie` 的安全投影、密文信封、命令幂等、版本 fence 和追加审计；
删除会先保留命令账本与审计，再删除凭据行，外键使用 `ON DELETE SET NULL`。Worker 读取 active
凭据时也由该模块按 world/shared 返回密文信封；dashboard 聚合同时返回 scope 最近更新时间和
凭据命令账本最近变更时间，供 HTTP 层结合 Worker 心跳判断重启状态，并按旧版要求检查 MoogleMail
`main`/`isekai` 各自的 active/invalid 水位；`shared` 邮件凭据会继承到两个 world，更新时间取有效
scope 的最新值，任一有效 scope 存在 invalid 行都会使对应 world 降级；SQLite Lite 明确返回能力不支持，不把凭据导入、
解密或外部 Cookie 能力伪装成 Lite 功能。
Admin 邮件 A16-A19 由 `src/database/admin_mail/mod.rs` 与
`src/database/commands/admin/mail.rs` 持有：PostgreSQL 读取 `mail_poll_state`、
`inbound_mail` 的安全投影，并在 Serializable 事务内用 `mail_poll_state` 的版本 fence、
`mail_admin_commands` 命令账本和追加审计完成暂停/恢复及历史兼容命令；`src/database/mail_poll/`
又持有 PostgreSQL `000008_mooglemail_polling` 的邮箱租约、失败退避，以及
`inbound_mail`、`mail_commands`、cursor 的同事务幂等提交；其中 `state.rs` 负责状态/租约，
`poll.rs` 负责 claim/commit/fail，`inbound.rs` 负责入站与命令账本幂等校验。入站重放会严格比较
远端身份、内容摘要和业务决定，但保留首次入站轮询写入的 `processed_at`，不把后续本轮时间或
PostgreSQL 微秒精度规范化当作内容冲突。SQLite Lite 明确返回能力不支持，
不创建伪造的空邮件状态。该模块不打开 MoogleMail 连接，远程读取仍由 Worker/集成 owner
负责；每个已启用 world 通过 mailbox_key 定向 claim，避免多个 world poller 领取彼此的邮箱状态。
`admin_mail/transaction.rs`、`reads.rs` 和 `mapping.rs` 分别拥有 PostgreSQL 邮件事务/命令写入、
轮询与入站查询、行和安全 metadata 映射；父 `mod.rs` 只保留跨后端端口分派。
MoogleMail 拍卖入站事务由 `src/database/mail_intake/mod.rs` 装配，`intake.rs` 持有命令账本、world/auction/玩家锁、
装备与密钥信封、pending lot 和预览版本写入，`record.rs` 持有追加审计与 preview realtime 事件；父模块只保留
端口适配；同批不同分类使用独立分类计数器，撤回条目也保留已占用编号。所有写入仍在同一个
PostgreSQL 竞价事务中完成，明文邮件正文和装备 key 不进入 storage。
服务心跳由 `src/database/health/` 持有：`mod.rs` 只负责 `HeartbeatStore` 端口分派，
`postgres.rs` 与 `sqlite.rs` 分别负责两个后端的条件 upsert/读取，`mapping.rs` 负责闭集
身份、状态和安全错误映射。PostgreSQL 使用数据库时钟和 observation sequence 条件 upsert，
SQLite Lite 使用本地数据库时钟和同一 sequence fence；两个后端都只返回 dashboard 所需的安全字段，
details 原文不会离开存储边界。
`src/database/audit_health/mod.rs` 对当前所有追加审计 INSERT 的返回错误进行进程内饱和计数，
不保存 SQL、错误原文或业务数据。`DatabaseStore` 的进程 readiness 每次重新检查数据库和全部
内嵌 migration，并在该计数非零后失败关闭；同一窄端口同时供 HTTP readiness 与服务心跳使用。

连接池状态通过 `DatabaseStore::pool_metrics_source` 以窄端口交给
`auction-observability`：只暴露当前借出、空闲、总数和最大连接数，以及在本 crate 的 SQLx
池边界真实记录的 acquisition 累计指标；不执行额外 SQL，也不暴露连接串、数据库名或路径。
普通查询、事务和显式 `acquire()` 都必须经过 `TrackedPool`，避免只统计少数调用点或用快照值
冒充累计数据。绕过该边界取得的外部原生 SQLx 池不会被这组累计指标覆盖。

PostgreSQL retention 盘点由 `src/database/retention_inventory/mod.rs` 装配，`model.rs`、`policy.rs`、
`realtime.rs`、`catalog.rs` 和 `mapping.rs` 分别负责安全模型/错误、政策聚合、realtime 水位、目录
查询与共享映射；它使用编译进制品的
`ops/deploy/retention-policy.v1.json` 作为版本化保留政策矩阵；单个
`REPEATABLE READ READ ONLY` 事务读取 `current_database()`、`auction_schema_identity`、SQLx
迁移历史、public 关系大小/行数估计和政策声明表覆盖，并输出数据族汇总、realtime 诊断水位、
public 外键依赖及待人工决定项。SQL 使用 5 秒 statement timeout、1 秒 lock timeout，关系数量
超过安全上限时失败关闭而不静默截断；realtime stream 和外键列表分别带覆盖/截断标记。返回值只
包含安全统计，固定标记不评估资格且不授予授权；该模块没有归档、删除、恢复或写事务入口。

PostgreSQL 恢复目标的只读预检由 `src/database/recovery_preflight/{mod,model,query}.rs` 持有：`mod.rs` 只负责
同一 `REPEATABLE READ READ ONLY` 事务中的编排，检查当前数据库身份、主节点/默认只读状态、其他连接、数据库
大小、用户 schema、类型、关系、函数、扩展及其他 PostgreSQL 用户对象、schema identity 和迁移
历史，并固定区分“基础条件通过”和“archive/恢复内容已验证”。它不调用 `pg_restore`，不停止
外部连接，不创建/删除数据库，也不覆盖已有对象。user mapping 盘点通过普通 owner 可读的
`pg_catalog.pg_user_mappings` 视图完成，只统计对象且不读取敏感 option，不要求 superuser。
`PostgresStore::close` 只供运维装配在预检后
显式释放自身连接池，实际外部命令和 receipt 仍由 `auctionctl` 拥有。

PostgreSQL 破坏性重建 SQL 由 `src/database/rebuild/mod.rs` 单独持有，只提供 PostgreSQL 18
维护库预检、集群 advisory lock 和经过 identifier/literal quoting 审计的
`DROP DATABASE ... WITH (FORCE)`/`CREATE DATABASE` 边界。它拒绝维护库、未知 locale provider、
非 UTF8、自定义 database ACL、database/role setting、活动连接、prepared transaction、subscription
和活动 logical slot；重建时保留 owner、locale provider、tablespace 与连接上限。备份授权、人工确认、
模式选择、迁移/空库后置证明和 receipt 属于 `auctionctl`，不能由存储适配器自行推断。自定义
tablespace 仍要求显式 `CREATE` 权限；`pg_default` 通过省略显式 TABLESPACE 子句保持普通
`CREATEDB` owner 的可重建语义。
PostgreSQL 初始数据库 bootstrap 由独立的 `database/bootstrap/` 持有：`model.rs` 定义创建/已存在
结果和脱敏失败分类，`sql.rs` 负责数据库名、应用角色校验及安全 identifier quoting，`guard.rs`
负责维护库身份、writable primary、`CREATEDB`/owner membership、集群 advisory lock、catalog
检查、单一 `CREATE DATABASE` 和应用连接后置验证。该适配器没有 DROP、覆盖或 migration 入口；
已存在目标只有在 owner 与应用 URL 用户一致且应用 URL 可连接时返回 `already_exists`。bootstrap
与 rebuild 共用数据库级 advisory lock，命令层另行持有 deployment state-dir 停机锁。

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
PostgreSQL `000005`/`000015` 的历史字节保持不变；当前 PostgreSQL 解析器所需的保留字别名修正
只发生在编译期内嵌迁移的受限执行视图中，并以版本、描述、替换片段和数量失败关闭，不能借此
改写任意 SQL 或迁移 checksum。fresh/原始历史使用不可变文件 checksum；曾运行早期别名修正版的
数据库可继续使用其已记录 checksum，但只在它精确等于固定替换后 SQL 的 SHA-384 时放行。

新增持久化能力前必须在 `docs/architecture.md` 标明它属于「完整档」「Lite 档」还是「两者」。

当前 PostgreSQL `000001` 至 `000028` 已提交核心竞价、结算/履约、生命周期、身份/
外部命令、凭据兼容修正、恢复/可观测性、论坛同步及 `forum_sync_admin_commands` 人工授权账本；
`PostgresCompetitionTransaction` 已接入竞价、
生命周期审计/realtime 事务，结算事务也已接入首片；`000018` 恢复 live/sold lot 的有效结束
时间 shape，`000019` 为旧 Auction-BF bid 写入补齐同一有效结束时间兼容防线，`000020` 扩展
operations alert 的退信 signal 闭集，`000021` 建立非敏感运行状态与成功扫描水位，`000022`
将配置状态发布时间和扫描水位拆为两条独立单调时钟，避免扫描时间阻断后续启停发布，`000023`
则为 HV 战斗页导致的余额暂不可读增加 `busy` 状态，并继续保留最后成功余额；`000024` 为 lot
增加分类键与分类编号唯一性/不可变防线，并冻结既有论坛主题的编号方案；`000025` 追加论坛固定
#1/#2 的 post proof、内容版本和独立刷新 lease/fencing，`000026` 固定同一 world/provider 的单用户
论坛账号归属，`000027` 增加 Users 级稳定论坛账号 ID、跨表提交约束并把名称权威收敛到 `username`，`000028`
保存论坛竞价来源的真实楼层号。SQLite
`000001_lite_core.up.sql`、`000002` 至 `000010` 的调度、用户管理、取消/
生命周期、Final phase、领先时间与同义 lot 结束时间防线，以及
`BEGIN IMMEDIATE` 竞价和生命周期写适配器均已接入；结算报告属于 PostgreSQL 完整档，SQLite
不提供结算表；PostgreSQL 完整档的 Worker 结算 workflow 由
`src/database/settlement_scheduler/mod.rs` 装配，due claim/租约和完成/失败 fence 分别由
`settlement_scheduler/{claim,resolution}.rs` 持有：接管路径局部识别旧
`settlement_claim/OK` 账本，claim/final 审计恢复旧 action、身份和 NULL request，失败进度在
Serializable 最多三次的同一事务中原子递增 attempt、计算固定退避并追加 retry 审计。结算命令装配由
`src/database/settlement_commands/mod.rs` 持有，并通过独立 `legacy.rs` 暴露只读旧 complete 核验；
SQLite Lite 明确不启用该 workflow，结算履约
外部传输由 Worker/集成 owner 执行，结算 Sent/Balance 回执对账已由 Worker 接入。PostgreSQL 完整档的
`src/database/fulfillment_scheduler/mod.rs` 装配结算履约 owner；`claims.rs`、`delivery.rs`、
`reconcile.rs` 和 `mapping.rs` 分别持有 claim、prepared/activation、证据收口和共享映射；
claim 以 obligation owner 顺序锁定并支持旧版缺失 execution 行的惰性补齐；claim 同时保留装备来源
世界和外部派发世界，custody 按前者核对、余额与发送按后者核对。坏的可选上下文
只隔离当前 claim 为 `manual_reconcile`，不阻断同批其他任务，execution/delivery 事实损坏则失败关闭。
该 owner 持有结算履约 claim、fencing、准备/激活状态推进；`WinnerPaymentCarrierCod` 的义务
来源世界保持 `isekai`、履约 dispatch 固定为 `main`，异世界 recovery 案件仍保留其来源世界；
`src/database/recovery_scheduler/scheduler.rs` 持有 recovery case/action 的 PostgreSQL 完整档 due
推进、提醒 outbox 关联和结果 fence，
`src/database/delivery_scheduler/mod.rs` 装配 delivery outbox owner；`claims.rs`、`state.rs`、
`outcome.rs` 和 `mapping.rs` 分别负责领取、发送前安全释放/发送中续租、成功/失败终态和共享映射。
`src/database/mail_delivery/mod.rs` 只负责 MailDeliveryEnqueuer 的跨后端分派；命令闭集校验、普通
outbox 入队、recovery 两阶段激活和 PostgreSQL 错误/载荷辅助分别位于
`mail_delivery/{validation,outbox,activation,common}.rs`，保留命令摘要幂等、`prepared → pending`
激活和外部结果后置边界。`mail_intake` 新记录使用 operation 后缀的规范 `origin_id`；幂等复读
只在请求摘要、收件人和完整 payload 仍相同时兼容既有 `delivery-*` 后缀，不接受通用 origin 改写。
目标 world 暂无凭据时只能从 `claimed` 以完整 lease/fencing
释放回 `pending`。过期 `claimed` 可以安全重领；过期 `sending` 只有在额外 sending grace 到期后
才批量隔离为 `manual_reconcile`，不能作为 pending 重发。失败写回会保留 transport 已校验的可选外部引用和有界安全
metadata，同时由 storage owner 覆盖 `safe_detail` 和 `outcome_unknown`，不允许原始响应越过该边界。
delivery 的创建/准备/激活、Worker claim/发送/释放/终态、过期 sending 隔离和 Admin 人工动作
都会在同一事务追加安全审计；审计失败会使对应状态变更整体回滚，幂等重放不重复追加记录。
Admin 交付命令由 `src/database/delivery_admin/` 持有：`mod.rs` 负责端口校验和后端分派，
`reads.rs` 负责安全列表读取，`commands.rs` 负责幂等人工命令、状态迁移、结算履约联动和追加审计，
`mapping.rs` 负责安全快照、响应元数据投影及旧 Go 完整账本 payload 防腐映射，`transaction.rs`
负责 Serializable 事务、SQLSTATE 重试闭集和 10ms/20ms 有限退避；SQLite Lite 明确返回能力不支持。
SQLite Lite 对这些完整档能力明确返回不支持。realtime Admin 由
`src/database/realtime_admin/` 分层持有：`mod.rs` 负责端口分派，`snapshot.rs`、`lag.rs`、
`failures.rs` 与 `replay/` 分别负责安全快照、lag、failure 读取和 replay；replay 再按
`replay.rs` 公共结果映射、`replay/postgres.rs` 完整档事务和 `replay/sqlite.rs` Lite 事务
拆分，`common.rs` 负责共享错误、时间/UUID 映射与 Lite 事务边界。Worker realtime consumer 由
`src/database/realtime_consumer/` 分层持有：`mod.rs` 负责端口分派，`stream.rs`、`read.rs`、
`acknowledgement.rs`、`failure/` 分别负责 stream 目录、事件读取、连续游标确认和 failure
生命周期；`failure/mod.rs` 负责稳定导出，`failure/record.rs` 负责失败记录，
`failure/resolve.rs` 负责 replay 收口，`common.rs` 负责共享校验与 Lite 串行写事务。
operations alert 由 `src/database/operations_alert/` 分层持有：`mod.rs` 负责 source/store
端口和后端分派，`snapshot.rs` 负责 signal/fingerprint 与冻结批次快照，`claims/` 负责
批次 lease/fencing（`claims/mod.rs` 负责导出，`create.rs` 负责新批次领取，`reclaim.rs` 负责
过期租约回收，`mapping.rs` 负责 PostgreSQL 行映射），`delivery.rs` 负责三次 Create 尝试、cooldown 与 delivery 终态投影，
`health.rs` 负责运行状态发布与水位健康快照，`common.rs` 负责共享的安全映射；配置状态发布时间
倒退会失败关闭，成功扫描只推进独立的 `last_observed_at`，不会改写配置状态时间；普通失败只释放为无错误码的
pending，cooldown 只在 delivery 入队确认事务中生效。退信水位只统计同时具有
`failure_code=recipient_not_found`、同名 reason code 和布尔型 `take_required=true` 的
`manual_reconcile` delivery，并排除 operations alert 自身 delivery；SQLite Lite 不提供该完整档能力。

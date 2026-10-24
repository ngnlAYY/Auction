# auction-storage

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [对象契约](docs/contracts/schema.md)

拥有 PostgreSQL 的 SQL、连接池、查询、事务、锁与迁移，实现应用窄端口。业务决定留在领域/应用层，外部请求留在
集成层，SQL 模型与行映射不越过存储边界。表、迁移与查询归属见[对象契约](docs/contracts/schema.md)，
锁序、幂等与审计语义见[领域不变量](docs/contracts/invariants.md)。

PostgreSQL 连接入口独立校验 URI：拒绝缩略形式、fragment、控制字符和无效编码，host/socket 与库名必须由 URI/query
明确给出；省略端口固定为 5432，不能由 PG* 环境补齐目标。首次驱动解析前按 SQLx 当前支持的查询键闭集拒绝未知参数，
错误不保留原 URI、查询值或驱动解析错误链。普通连接保留合法 `options`，strict 部署身份继续拒绝它；
凭据、pgpass、IPv6、socket 与 TLS 语义不变，字段规则见[连接参数](ops/config/README.md#postgresql-连接参数)。

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

[delivery_scheduler/cadence.rs](crates/auction-storage/src/database/delivery_scheduler/cadence.rs) 与 `000059` 独占七类 Send 起点；
在原业务锁前锁分组、数据库实时时钟复核，Sending/原审计与高水位同事务提交。领取按超价优先及时间/UUID 排序，
准备失败不消耗间隔，未知/失败发送不退还间隔；投递负载和历史结果不改写。
[delivery_scheduler/wakeup.rs](crates/auction-storage/src/database/delivery_scheduler/wakeup.rs) 独占固定空载荷频道订阅；
只做提交后提示，断线不改变持久队列。SQL 映射、触发器、保留策略及迁移范围见[对象契约](docs/contracts/schema.md#结算恢复与交付)。

[delivery_scheduler/attempts.rs](crates/auction-storage/src/database/delivery_scheduler/attempts.rs) 与 `000061` 拥有
`delivery_send_attempts`、`delivery_sent_bindings`：实际发送身份按 delivery/fence 只追加，正文只存摘要；
Sending、节奏起点和原审计同事务提交。正常成功和维护恢复共享 world/MID 唯一绑定，同一绑定重放无新副作用，
管理员重试不覆盖旧尝试。恢复按当前观测 generation→资金→凭据→delivery 取锁，原发送 generation 只作历史身份；
冻结记录精确比对后同事务收口状态、资金、履约和审计，跨批次存在相同内容/时窗的候选时保留未知。
[mailbox_sweep/recovery.rs](crates/auction-storage/src/database/mailbox_sweep/recovery.rs) 的新证明供下游
`delivery_has_frozen_sent_proof` 强谓词消费，不把新 proof 标签直接加入旧字符串白名单。升级先停止旧 Worker，
迁移不回填历史渲染身份；无身份的旧行仅保留原安全证明，未知 Send 不自动重排。
`000062` 为 delivery_scheduler 的升级前 Sent 引用碰撞核对新增 `deliveries.external_reference` 非唯一部分索引，
仅收录非空引用；不改写历史引用、不把索引当唯一证明，原完整绑定谓词及 Serializable 幻行冲突保护保持不变。

[delivery_scheduler/take_attempts.rs](crates/auction-storage/src/database/delivery_scheduler/take_attempts.rs) 与 `000063` 拥有
只追加的 `delivery_take_attempts`：Take 在 Sending 前冻结 world/MID 与原凭据引用，和 Sending、审计同事务提交。
`delivery_current_take_attempt` 按原 `sending_at`、命令及摘要解析 `taking_fencing_version`，当前 delivery CAS fence
仍单独匹配；过期隔离推进当前 fence 不覆盖原尝试。维护只接受同 credential ID/version，允许重启后的新 generation，
但本次观测 generation 必须锁内仍为当前 owner。无旧尝试或配置 Cookie 的空凭据绑定不自动判成功、也不自动重排；
不以当前账号回填历史身份。升级停止旧 Worker 后追加迁移，Send 的 `000061` 和 Return 恢复边界不变。
授权入口按 `DeliveryPreparedExpectation::{Send, Take}` 区分动作，现金 until 授权仅允许 Send。

## 用户身份与消费 owner

[users](crates/auction-storage/src/database/users) 在写锁内复核注册信封绑定的 UUID；论坛数字账号是稳定身份，
`username` 只是可变显示名和 MM 收件人，二者不能互换。[sessions](crates/auction-storage/src/database/sessions) 保存会话
摘要：Admin 启动撤销旧 Admin 会话，Public key 撤销/重签与旧会话永久撤销同事务提交，只读 inspect 不刷新过期时间。
Admin 启动、轮换及注销和 Public 注销的撤销时间至少为该会话创建时间，时钟回拨不妨碍永久撤销。
Admin/Public touch 先物化并锁定会话行，再取请求时间与数据库实时时钟的较大值检查截止；
等锁跨过截止不认证、不续期。Public inspect 和偏好读取也在查询时复核截止，连接池等待不能复活身份。
Public [sessions/public/preference.rs](crates/auction-storage/src/database/sessions/public/preference.rs) 拥有账号时长读取及命令事务：
Serializable 按 user → 当前 session → 账本取锁，锁后和写入时复核期限，偏好、当前截止、账本与审计同事务提交。
`000058` 新增独立偏好版本和可空的成对截止；默认只影响新登录，旧截止和撤销事实不回填。永久会话也参与原有并发计数，
认证仍要求账号及 Key active；未知重放不延长截止，HTTP 刷新 Cookie 前须另读当前会话。SQL owner 与约束见[数据库契约](docs/contracts/schema.md)。
Recovery 拉黑仍按历史 claim 时间保存稳定命令、证明与审计；当前用户/黑名单、代理及转换、lot 与会话副作用分别以下辖行
已有创建/更新时间及最后转换时间为下界，不因 claim 之后新建的会话或代理使整笔拉黑回滚。
人工和 Recovery 拉黑统一按 lot → proxy → user → session 取得锁，先冻结授权，再锁用户并撤销会话；
序列化冲突和死锁保留结构化分类，以原命令最多尝试三次完整事务，不重新签发密钥或重建摘要。
用户、论坛身份及登录 key 信封的快照更新时间以已存创建/更新时间为下界；`000044` 只修复 MM 注册账本与论坛身份历史
共同证明的时间倒退，推进版本并追加更正审计，保留创建时间与所有凭据，未知来源异常阻断整次迁移
（见[用户快照时间修复升级](ops/README.md#用户快照时间修复升级)）。
MM 注册对已有用户名的大小写修正与换绑一样，在同一事务追加 `user_game_identity_history`；
仅重签 key 而名称不变不生成虚假改名历史，已接受命令重放也不重复追加。
Admin 用户列表只对当前页 UUID 批量读取黑名单历史，每人仍保留完整次数及按时间/ID 倒序的最近十条；
列表、计数和历史保持同一 Serializable 快照，不逐用户另取连接或查询。

凭据与 realtime 消费 owner 持有专用物理 session：操作复用原持锁连接，失效后不能从池补连接继续，guard 退出关闭物理
连接、不带锁归池；凭据网络调用不跨越数据库事务，部署身份不含连接原文、账号或密码。`credentials` 由 `000045` 持久化
可选预期 `player_name` 并传递当前 state_version（旧记录保持 NULL，不是 verified 标记，非空值必须精确匹配）；
`000046` 的 `mail_runtime_generation` 只含全局 `mail_cookie` 行，取得原 session advisory lock 后每个 mail owner 都先原子
推进 generation 并确认提交，才允许远端操作。[credentials/evidence.rs](crates/auction-storage/src/database/credentials/evidence.rs)
提供 generation 首锁、active 凭据版本/世界/名称锁验，以及最终写入前 `clock_timestamp()` 的 30 秒证据时效门禁。
证据 helper 返回 `MailEvidenceTransactionError::{Sqlx, Rejected}`：外层先按 SQLSTATE 40001/40P01 分类整事务重试，
再转换脱敏应用错误，不通过错误文本决定重试。
凭据 owner 的首次永久撤销通过结构化日志记录凭据类型及闭集原因
（`probe_failed`、`session_unavailable`、`check_cancelled`、`internal_use_revoked`）；不记录连接错误原文、凭据或账号。
后续成功探测不能恢复已撤销 owner，重建必须由新进程重新取得 advisory lock 并推进邮件 generation。
合法删除凭据后，命令账本的外键可变为 NULL；历史导入、改名和启停仍按原类型、摘要及安全结果快照重放，
不读取当前凭据重建结果，不因删除而重新执行外部获取或导入。
Admin 凭据改名、启停及 Worker 失效回写的快照时间以该行 `created_at`/`updated_at` 为下界；
首次 `rotated_at` 使用同一下界，角色时钟落后或回拨不能阻止撤销，既有轮换时间、密文和命令审计时间保持原样。

Admin realtime replay 在追加替代事件的同一事务复用 consumer 恢复入口：先锁 failure，再锁绑定的 consumer/stream 游标；
只有游标恰为原事件前一序号才推进到原事件，较早的未处理序号使整笔 replay 回滚，已超过边界不倒退。
替代事件及其间其他事件仍按正常连续顺序消费；同一原事件的其他 consumer failure 和游标保持独立，重复命令只返回原账本结果。
连接入口仍复用 owner 原物理会话；同一 failure/replay 的显式恢复可补齐旧版只写 resolved 而未推进游标的窗口。
正常 consumer acknowledge 同样先收口并锁定 failure，再锁连续游标，避免与 Admin replay 形成反向锁等待；
后续 event 身份或连续性校验失败仍回滚整笔事务，不会留下提前解决的 failure。

## 竞价事务和后端入口

竞价按 world → auction → lot 加锁：只读 world/auction 使用共享行锁，lot、活动代理、参与者资格和账本保留排他锁，
生命周期使用对应排他锁；取得锁后读取数据库时间，同一计划的代理、领先时间、截止、审计及接受事实共用该时刻。
命令账本、状态、审计和 realtime/outbox 同事务提交，Preview/Open/Final 与轮次完成标记也原子写入。`competition/postgres/` 按
bidding、lifecycle、admin 拆分；旧竞价账本重放仍在一致快照中取得冻结策略、授权版本和转换历史，不重新执行当前状态
门禁。竞价及关键生命周期使用 Serializable、明确行锁，最多三次尝试（含首次）；队列领取才使用 Read Committed 加锁，
只重试已分类的序列化冲突/死锁，未知提交结果不能盲目重放。
竞价命令账本和 MM 超价通知直接借用原 32 字节请求摘要绑定 `bytea`，不为每次写入另建临时字节向量；
摘要算法、字段内容、幂等比较及原事务边界不变。
Admin 规则、邮箱管理、Recovery 命令及周轮次 bootstrap 的已有重试边界也按 `40001`/`40P01` 识别明确中止，
保持原最多三次整事务尝试；唯一约束、未知提交及普通连接失败不因此重试。
规则发布仅允许 `expected_revision = 0` 新建；非零版本只更新匹配旧版本的既有语言文档，缺失时返回当前版本 0，
不通过 UPSERT 创建跳号文档，失败不提交规则、命令账本或审计。
Admin 新建拍卖先由 `lock_admin_auction_creation` 完成原 world → intake target 锁前缀，再由应用层读取数据库
实时时钟并校验预览窗口；写入复用同事务锁和该时间，命令重放不重新执行当前时间门禁。
Admin/MM 录入以 Users 的大小写不敏感唯一名称定位并锁定卖家；查不到大小写完全相同的展示名不能绕过黑名单。
取得 world/target/auction 锁后才读取数据库实时时钟写本地事实及聚合更新时间，更新时间保持单调；
来源 `collected_at` 继续表达采集事实，不因录入跨越 Open/取消后的 target 切换而写回新草稿的较早时间。
人工生命周期动作不得抢占仍有效的轮次 Worker 租约；有轮次时必须按顺序精确更新一行，未登记轮次的合法历史场次
保持可操作。阶段提交后的 Worker complete 只确认同轮次/拍卖/命令完成证据，提交与进程崩溃之间不留下永久重试窗口。


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

`mail_poll/reads.rs` 实现详情读取的 `MailPollReadJournal`；`000052` 的 `mail_poll_pending_reads` 只保存 mailbox/world、MID、
发件人、主题与收件时间，身份列有界且默认调试输出脱敏。短事务锁定当前邮箱 claim，并在取得行锁后及提交前重验实时租约；
同 MID 重复登记须身份一致，不能覆盖旧值。队列按持久序号返回最早前缀；`poll::commit` 在入站事实、命令账本和 cursor 的
同一事务删除完成前缀，失败、取消、租约丢失和未完成后缀均保留。更改 allowlist 或 mailbox/world 后遇旧待读项时失败关闭，
不由启动或迁移丢弃队列。查询使用运行时绑定 SQL，无新增 SQLx 宏离线元数据。
轮询续租和失败退避先取得邮箱行锁，再用 `clock_timestamp()` 校验实时租约；入站提交的最终 cursor CAS
也使用实时数据库时间，不能用调用前时间或事务起始时间完成已过期租约。token、owner、版本和暂停快照规则不变。
论坛发布/内容确认、告警批次、recovery action、履约失败及 PM 收口同样先按原锁序取得目标行，
再在后续语句复核实时租约；单独在首次 UPDATE 谓词中调用实时时钟不构成锁后检查。
过期保留原 stale 分类，未知外写不会因此重新入队。

scheduler 保存 due、租约、尝试次数和失败状态；完成/失败须匹配 owner、精确租约及版本，Admin 改期清除旧租约并推进
版本，陈旧任务不能收口。已存在轮次按冻结事实读取，配置变更不改写旧命令或时间；自动 Draft 不提前物化运行期结束
时间，后继 SQL 规则列与快照同取新模板。slow proxy 扫描在 LIMIT 前排除已到截止的 lot，执行由竞价事务复核
generation、due、授权与资格；调度字段与确定性命令持久化，不另建内存游标或 Redis 锁。
周轮次首次创建在取得 world 行锁后读取 `clock_timestamp()` 判断窗口是否已结束；等待跨过截止时只登记 missed，
不因 `CURRENT_TIMESTAMP` 固定于事务起点而建立过期 Draft。

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

公开补偿池的 `balance` 是全部世界 `settlement_pools.balance` 的当前合计，`updated_at` 是资金池最近更新时间；
批次列表继续展示不可变结算摘要，不能用“最后结算的某一世界余额”冒充全局余额，不改写任何历史结算或账本。

[fulfillment_scheduler/claims/pricing.rs](crates/auction-storage/src/database/fulfillment_scheduler/claims/pricing.rs)
在履约领取中投影并验证不可变 `lot_settlements`，向应用层传递可选 `SettlementLine`；买家 COD 正文据此冻结明细，
不从当前策略或可变竞价状态补算。此投影不新增表或迁移，持久载荷兼容范围见
[交付载荷契约](docs/contracts/schema.md#结算恢复与交付)。
领取先选择至多请求批量的到期候选，再按 UUID 锁义务和 execution；不锁全部 ready 义务，锁竞争的候选留待后续批次。

编号页 total 与当前页来自一致快照，禁用 world 的活动投影不能公开，已 settled 历史按契约读取。
`bid_log/postgres.rs` 的明细及 total 均保留同一命令内的普通出价和即时代理回应，不因后续代理事实而隐藏触发报价。
原命令白名单、拍卖/world 可见性、五字段 keyset 排序和分页边界不变；结果仍只含公开金额、玩家和来源，
不读取或暴露代理秘密上限。`valid` 只标识当前领先投影，完整事实不会因不再领先而从历史中删除。
公开黑名单、Admin delivery 及 realtime failure/lag 的计数与当前页共用 Repeatable Read 只读事务，
避免并发状态变化造成同一响应的总数与列表不一致；筛选、排序、空页总数和分页上限保持原契约。
Realtime dashboard 的全量 lag 聚合、有界明细与 failure 水位共用同一只读一致快照；健康和总积压使用全量值，
不能从排序前 200 条明细推断全局无积压，wire 字段见[Dashboard 契约](docs/contracts/routes.md#dashboard-与-readiness)。
账户页只对当前页授权批量读取完整转换历史和最近代理竞价，详情共用同一归属查询；转换按授权/序号排列，
最近竞价仍以同用户/lot 的下一 usage 创建时间截断，缺失或不连续转换继续失败关闭。页校验直接移交结果，不复制整页。
授权历史以 `updated_at DESC, lot_id, authorization_id` 形成严格全序，时间和 lot 相同也有唯一分页尾键。
Recovery 报告同样只对本报告 case 批量读取 action，并按各 case 原有 kind/ID 顺序映射，不截断完整报告。
Public 的 `DatabaseStore::public_lot_reader` 接受启动时冻结的装备标色规则，由应用共用判定生成稀有/融合分组。
[lots/postgres/selection.rs](crates/auction-storage/src/database/lots/postgres/selection.rs) 在选择分组、等级上限或隐藏无等级时流式扫描
本场可见装备的身份字段（不读取 stats），严格解码后复用应用等级/分组条件，先计类型总数，再按类型及 offset/limit 保留当前页 UUID；
同一 Serializable 事务按 UUID 读取完整页并保持原排序，`total` 包含全部筛选。内存只保留计数和当前页 UUID，
计算量随本场装备数量增长；无这些条件时保留原 SQL 统计和分页。None 等级不补零；预览、当前及历史某场目录一致，
详情不应用列表筛选。不增加持久字段或迁移，装备事实、金额及竞价语义不变。
完整装备映射借用原 attributes JSON 解码，不复制整棵临时 Value；筛选仍消费窄身份投影，严格校验和原始属性保留不变。
[auctions/countdown.rs](crates/auction-storage/src/database/auctions/countdown.rs) 用单条 SQL 读取同场公开时间及数据库
时钟，不加载装备、凭据或论坛正文，不公开草稿、取消场次或被遮蔽的预览，无需新增表和迁移。Public 拍卖、竞价日志、
成交历史与补偿批次的游标查询仅对额外探测行使用饱和加法，不截断请求数量（见[分页契约](docs/contracts/routes.md#public-分页世界与金额)）。
`price_history/postgres/numbered.rs` 在同一 Serializable 快照内按完整筛选集计算统计和服务端排序分页；
编号页与 cursor 共用名称字面包含词条及 `ILIKE ALL`：空白或逗号分隔的缩写逐词 AND，仍转义 `%`、`_` 与反斜杠。
排序仅接受时间、价格升降序，价格相同后沿用时间、场次、内部 lot 顺序与 UUID 保持稳定。
中位数选中间一至两笔价格后按 `numeric` 求均值，`median_exact` 返回规范十进制字符串，旧 `median` 显式半数取偶保留整数兼容口径；
不先相加 bigint、不经浮点数，旧 cursor 继续固定时间倒序。
Admin 读取不为补全投影临时写入；邮件展示和诊断 sidecar 均非权威数据，不参与幂等、完成证明或 cursor。
`operations_alert/snapshot/reads.rs` 的履约待处理水位与 Worker 领取的业务状态一致，只计 `ready` 义务的
`pending/prepared/enqueued` 阶段；最老积压按这批义务及独立的未终结 `blocked` 义务计算。
`contingent` 退款保持未触发事实，不进入待处理与积压年龄；`blocked/failed/manual_reconcile` 保持各自计数，
Dashboard 与 operations alert 共用该投影，字段契约见[Dashboard 与 readiness](docs/contracts/routes.md#dashboard-与-readiness)。
Delivery 告警、内部履约冲突/锁定及资金分类查询只在首次调用时组合固定的修复证明 SQL，后续复用不可变文本；
每次仍读取当前事务事实并取得原锁集合，不缓存水位/资格、不放宽 `TRUSTED_DELIVERY_PREDICATE`，也不拼接业务输入。
结算报告在同一 Serializable 快照内读取汇总与明细，序列化冲突最多尝试三次；明细排除不参与结算的
`withdrawn` 标的，其余 lot 缺失结算事实时失败关闭。赢家累计领先时长由追加式 bid 重建，无领先事实时为零。
`leader_since` 按全部接受事实判断连续领先是否中断（普通出价后被原领先者快速代理反超也重新计时）；`000037` 只重建
可由追加式 bid 证明的未结算 live lot 投影，不改已结算金额、终态 lot 或历史审计。
`settlement` 的 `ReadOnlySettled` 历史核验继续使用 `leader_since` 旧连续区间，不扫描并丢弃未使用的 bids 累计时长；
正常新结算仍按追加事实累计领先，历史金额、策略兼容及核验边界不变。

Delivery 进入 sending 前须证明命令已提交，未知发送结果进入 manual_reconcile，不自动重寄。
Fulfillment 激活后保留原 claim 的 token/fencing 继续对账，领取期间 `next_attempt_at` 始终为空；
COD 与卖家货款的下次共享追踪窗口由对账延期在清除 claim 的同一事务持久化，不在激活事务提前写入。
BuyerRefund 的 prepare/activate（含旧 prepared）借用真实 Armory 联合证据及原单调截止；
generation、credential、claim 与数据库时效在原业务锁序内复核，连接池等待与 COMMIT 均不能延长预算。
应用持 opaque guard 至确定结果，不再以装备详情可读替代持有证明。
[delivery_scheduler/cash](crates/auction-storage/src/database/delivery_scheduler/cash) 统一新旧 sending 入口：有证据时按
generation→排序 worlds→资金 scope/account→必要业务/凭据→义务→delivery→fulfillment→allocation 加锁；非现金发送的
持久凭据同样先锁 generation，再锁 scope/持久 allocation 关联及 active 凭据。完整 payload、recipient、command/hash
与 token/fencing/实时 lease 锁内精确匹配，无冻结身份的旧入口不得授权 Send。
现金不足只提交冻结并返回 Deferred；现金 until 入口沿用 prepared 原单调截止与 UTC 证据窗口的交集，PostgreSQL 18
`transaction_timeout` 在业务锁/审计前设置并覆盖 COMMIT。已有 allocation 的五种新 Admin 动作全部返回 Invalid 且不写
状态，原稳定命令先重放。compose 安全由 transport 在 sending fence 前实际只读检查，不再按历史失败永久阻断整个 world。
未开始写入的有效 claim 释放回 pending 时撤销本次领取 attempt_count 并在同事务追加 release 审计。续租仅允许当前有效
claimed/sending，先取 delivery 行锁再复核 token/fencing 及 `clock_timestamp()`，新截止不得缩短已有租约。

`mailbox_sweep` 的受证明恢复以 Serializable 事务按当前观测 generation→资金 scope/account→凭据→delivery 加锁；
真实 Sent 将原分配从 unknown/in_flight 原子收口为 spent，并同时恢复原 fulfillment 的对账排队与追加投递审计。
通知、take 成功与 take 重排队也同事务追加审计，仅实际迁移才返回 true；重放、引用已占用和并发输家不重复计数或审计。
所有 Send 恢复共享不可变 world/MID 绑定，并排除全库同内容/时窗的其它未绑定尝试；明确序列化冲突/死锁最多尝试三次，
未知/失败提交不宣告成功，不据此重发。Send 不进入维护重排队入口，观察元数据本身不迁移状态。
维护 Requeue 只接受原 Take 尝试及当前凭据证明匹配的 `mail_intake/take`；回到 pending 时一并清除 `claimed_at`，
保持待领取投影完整，不把原 sending 或未知 Send 伪装成从未发送。
`mailbox_sweep_cursors`（`000060`）只持久记录逐条维护尝试的 `(created_at,id)` 环形位置，不表示投递完成；
预算未处理的后缀不推进。一次性 discard 同时匹配当前 fence/冻结身份，拒绝全库内容歧义，只阻止未知草稿删除重放，不授权 Send。
COD 重排队使用冻结 v1 七天窗口，并在清旧投影前同事务追加 `delivery.cod_return_resend` 安全历史审计。
Admin 列表和命令读取只为精确收件人不存在的人工核对退信接受 MID 引用，不把它当作发送成功。
Recovery 来源不完整时仍持久化已证明的积压/证据过期异常及不可用信号，但不关闭未出现的历史异常。

`forum/catalog.rs` 拥有 `auction_forum_catalog_pages` 续页身份/正文（`000040` 增加主记录目录页数），发布和刷新在主
记录 claim/revision CAS 的同一事务内更新所有续页；`catalog_layout` 冻结历史单页/新分页方案，仅从未尝试、无远端身份
的队列项及新主题启用分页。`000043` 为 forum owner 增加纳入标记和 nullable 终态确认版本，历史 settled 仅豁免；
`forum/sync/snapshot.rs` 在同一 Repeatable Read 中读取领域状态闭集、结算汇总、lot 明细和最终领先事实；
Preview 读取 pending，Cancelled 读取 withdrawn，只有 Settled 投影持久结算明细。已发布主题可刷新
Preview/Live/Final/Settling，以及尚未完成终态证明的 Settled/Cancelled；旧 settled 豁免不变。
取消只刷新已持久化有效主题，不新建主题、不导入新竞价；全部固定帖确认后停止轮询。
`forum/reply/fixed_posts.rs` 以 auction 共享锁核对已渲染的 settled/cancelled 版本与无 pending，再按 sync
lease/fencing/revision CAS 收口；部分进度不会写终态标记，`forum/reply/queries.rs` 的竞价资格仍仅 live/final。
`forum/reply/queries.rs` 从既有命令账本投影 `last_error_code` 与有界详情，不回填历史。
论坛标题候选有界、去重且不含当前标题；状态来自权威快照而非日期，不改变公开或命令 wire。
论坛领取可按 Worker 首轮水位重新领取旧凭据失败和未知写入，但不领取
`write_authorization_required`。Delivery Admin 重试由 `recovery_commands/commands/delivery_retry.rs` 在原 Serializable
命令事务恢复匹配的失败投影并推进 fencing，不生成成功证明。
论坛同步、回帖及 PM 领取在 LIMIT 前按实际可用 world 过滤。命令历史按稳定帖子身份核对，不因当前位置改变改写账本；
完整扫描的派生楼层可在原 claim/revision CAS 下下降，pending 删除/编辑仍阻止后续命令和终态完成。

## 内部履约原子事务设计

[fulfillment_scheduler/internal](crates/auction-storage/src/database/fulfillment_scheduler/internal) 实现独立
`bot_internal_v1` 满足事务；迁移 `000048_internal_fulfillment` 只新增稳定命令与非敏感完整证明表，不回填旧记录、
不改写冻结金额。

- 原命令先重放（同负载原结果、异负载冲突），Serializable 整事务最多三次，只按类型化 40001/40P01 重试；
  确认 ROLLBACK 才返回 RolledBack，未知提交/取消由应用 opaque guard 撤销原 owner。
- 锁序为 generation→排序 worlds→资金 scope/account→auction→recovery→lot/equipment→credentials→users→排序 obligations
  →actions→deliveries→fulfillments→proof/ledger/audit；旧 prepare/activate/satisfy 同步此业务前缀。同 EID 可能关联多个
  本地 equipment UUID，均核对。delivery 行锁仅覆盖当前投递及同 EID/world 未闭合的显式引用或 owner 关联，最多 256 条；
  额外探测行表示溢出并拒绝整事务，不静默忽略冲突。未知 schema、孤儿及其它历史仍经过完整冲突谓词读取，
  Serializable SIREAD 保留幻行保护，既有 world/资产业务锁及三次冲突重试不变。
- 资产冲突的普通投递分类和已闭合入站退回历史逐行读取完整载荷，只累计通过原闭集证明的 UUID；
  不整批保存正文或附件，也不先复制 JSON 树再解码。所有候选、未知 schema 的失败关闭及原事务快照保持不变。
- UnsoldReturn/WithdrawnReturn 均不套成交资金图；已证明的同标的来源 Take 按同一闭集规则闭合，
  不放宽未知历史、其它标的或其它装备的冲突防线。
- 复核当前用户版本、有效 claim token/fencing/state_version 及数据库实时时效；新 pending 必须从未准备、激活或关联
  delivery。`conflicts.rs` 按真实 origin/payload 闭集 schema 排除已验证无装备通知：论坛 `forum_auction_notice` 仍属
  mail_intake，仅同 command/world 的 `auction_forum_bid_commands` notify 账本、零命令金额及完整 producer origin 重建
  一致时才识别，关联只读；
  缺失/畸形/未知 schema 仍阻断，不凭前缀或 succeeded 放行。
- 库存只消费完整正向 Armory 快照，所有资金 SQL 由 settlement_funding 拥有。满足、清 claim/版本、独立 proof、资金动作、
  直接 blocked 依赖和追加审计同事务；外部 COD 在任何满足/资金化/解依赖前以已锁账户和同一有效 Main cash 检查既有占用，
  缺口确定提交冻结并返回 FundingDeferred，通过后才保存原 Sent collected 满足并释放原依赖。
- COD 实收与 enqueued 旧满足只按不可变用户 UUID 复核身份；地址必须匹配原 delivery/Sent，不受之后改名影响。
  外部退款资金化保留原 v1 命令 ID、完整 payload 和摘要格式，新证明附存原收件地址；
  改名重放只从该冻结地址或同 UUID 的既有改名历史重建完整原 v1 payload，逐字节匹配旧摘要才返回已提交，
  不再次执行资金动作。历史地址无法重建时持久化 `manual_reconcile`，不改写旧账本、释放原资金或猜测补发。

Recovery action 终态先锁父 case 再锁 action，确认路径按 UUID 预锁全部 fulfillment。Admin 旧重试由 recovery owner 提供
`lock_recovery_delivery_retry_scope` / `lock_recovery_delivery_retry_actions` 两阶段锁，只用于无 allocation 的
RetryFailed/ReconcileRetry，SQLSTATE 原样交 runner。源码静态结果见[状态页](docs/status.md)，迁移、SQL 执行、并发与
大库锁成本仍需独立运行证据。

`000051` 追加 `settlement_recovery_return_consumptions`，以退件 `return_mail_id` 全局唯一地保存消费事实，
禁止更新或删除；记录、计数和后续 action 在同一 Serializable 事务提交。重寄清空 recovery 当前指针仍排除旧邮件，
升级只回填当前指针、回收动作和历史 take 投递可证明的消费，不修正旧计数。新 take/activate 身份按真实退件区分，
同一 MID 的重试保持稳定；`mail_delivery/preparation.rs` 只读恢复旧 prepare 已提交但尚未关联 action 的窗口，
完整来源/摘要/payload 不符失败关闭，已有 prepared/enqueued 沿用原身份。

## 真实现金账户与事务 helper

[settlement_funding](crates/auction-storage/src/database/settlement_funding) 独占迁移 `000047` 的资金 scope、稳定 UID
账户、唯一 source/receipt、用途 allocation、追加 entry 及命令结果；初始 0/unreconciled，不回填历史现金。helper 在
generation/world 之后、业务锁之前取得 scope/account，令牌绑定当前事务。所有 Credits Send 须经
reserved→in_flight→spent/unknown，失败/人工 failed 不释放，精确返还另追加事实；`lock_funded_delivery_account` 在
delivery 锁之前按持久 allocation 关联早锁原账户，孤儿/歧义失败关闭，delivery owner 不从 metadata 推断账号。
责任 preview 使用 Serializable READ ONLY，apply 锁完整责任集合、核对预览摘要/版本和真实现金，既有 allocation 全部扣占。
纯加锁查询完整执行有序 `SELECT ... FOR UPDATE`，但不收集随后丢弃的行集；内部履约沿用同样方式，
需要校验数量的有界 delivery 冲突探针仍保留结果。锁集合、锁序、Serializable 及重试边界不变。
未资金化付款已外发但收款未闭合（或关联 delivery 缺失）时 preview/apply 增加 `payment_collection_unclosed` blocker；
新 COD 要求精确 MID/当前账号实收绑定、全局 payment/source 唯一且发送晚于最近 opening 边界。
[settlement_funding/receipt.rs](crates/auction-storage/src/database/settlement_funding/receipt.rs) 在同一查询读取冻结结算明细，
重建买家装备 COD 的扩展变量并完整比较 payload；精确旧三变量和 carrier 原两变量保持兼容，不接受 payload 自报价格替代事实。
`delivery_scheduler/capability.rs` 是当前构建唯一 V1 提供者，非 postgres 档位返回 None；`delivery_admin` 仅为明确
失效的 watch preflight 保留无 sending 时间的严格投影。

`000055` 将 `strict_cash`/`record_only` 策略持久化至 UID 账户，旧账户默认前者。
仅记账初始化复用原责任预览、稳定命令、追加账本和审计，不读取现金；锁内复核策略及版本，各进程不能按配置暗切换。
内部购买和补偿从原标的冻结 B/C 生成唯一有限的会计来源，旧责任先隔离再按精确原收款认领；原有真实现金路径保持现金门。
跨世界外部付款的 1C 载体绑定原 Isekai 义务、完整 canonical payload 和 delivery 唯一来源，只记入对应 carrier 用途。
仅记账不因个人用款或其他标的的现金未知而冻结全局，原标的的用途、未知占用、禁止重发及装备冲突仍受原事务保护。

[settlement_legacy_repair](crates/auction-storage/src/database/settlement_legacy_repair) 独占 `000053` 的旧自寄核对。
单笔 Serializable 事务验证原版本及完整快照、真实 UID 和正向库存，追加原投递/履约指针与新证明；流拍留存满足
原义务，成交自寄明确被拒后的修复仅把当前执行投影恢复为无外发 pending。原投递及错误证据保留，后续内部付款
受持久修复事实的 UID 约束。库存与资金预览只按完整匹配的事实解除对应旧投递占用。

`fulfillment_scheduler/internal/conflicts/intake.rs` 仅为已核验新鲜正向库存的竞得装备付款与流拍留存，
从当前标的投稿精确绑定已接受入站及成功领取，闭合该次入库和更早由各自拒收账本绑定的已完成退回。
世界、装备、MID、发件人、附件集合与数量、动作证明及完成先后均需匹配；未知、后续或其他标的动作继续阻断，
历史投递与入站事实保持原样，卖家留款等无库存动作不使用该例外。

[settlement_funding/legacy](crates/auction-storage/src/database/settlement_funding/legacy) 独占 `000054` 的旧 Bot
卖家款认领：锁定既有隔离用途、原付款/卖家义务与同账号精确 Sent-collected 证明，逐标的将隔离责任转为卖家留存，
不再次创建旧收款 source 或发行 B。两种修复保存稳定命令、唯一目标、完整证明及追加审计，未知提交只查询原命令。
旧款认领另接受原维护产生的 `sent_recovered_reference_v1` 闭集事实，精确核对原 MID、收件人及恢复时资产矩阵；
恢复时尚未付款的历史保留原状，本次仍须用同一真实会话证明该 MID 已收款且无剩余资产。普通新收款接受
实时 `sent_exact_payload_v1`，或经过不可变尝试和唯一 MID 绑定核验的 `sent_recovered_frozen_v1`；旧 reference
恢复证明不改标、不新增来源，开户前时间边界、完整原载荷和 prepare/activate 绑定保持不变。
资金预览仅对严格解码且零 Credits、零 COD、无附件的纯通知排除资金未知阻塞，保留通知本身的人工核对状态。
该分类流式读取历史载荷，只保留通过原闭集解码器的 UUID；不省略任何候选，也不把正文整批保存在内存。

## 后端选择、连接池与验证

配置层将 `[database]` 或兼容的 `postgres://` / `postgresql://` URL 交给本 crate，不在存储层解析 TOML；
默认不编译驱动，运行须显式启用 `postgres` feature，使用 SQLx PostgreSQL/rustls。
`DatabaseStore::pool_metrics_source` 暴露借出、空闲、总数、上限及真实 acquisition 累计指标，普通查询、事务与显式
acquire 均经过 `TrackedPool`，不额外查询 SQL，也不暴露数据库名或连接串。readiness 检查数据库、内嵌迁移及审计写入
失败计数，异常失败关闭。
readiness 直接查询完整迁移历史，不再先做独立 `SELECT 1`；仅缓存二进制内嵌迁移生成的只读验证视图，
每次仍逐行核对当前数据库版本、success 与 checksum，不缓存健康或匹配结果。
心跳写入借用命令中的 details JSON 编码，不在每次发布前复制整棵树；观测序号、数据库 TTL 和同序号重放规则不变。

[retention_inventory](crates/auction-storage/src/database/retention_inventory) 在只读快照按版本化政策盘点关系、依赖和
realtime 水位，不授予删除资格；[recovery_preflight](crates/auction-storage/src/database/recovery_preflight) 只核对恢复
目标，不运行 pg_restore 或覆盖对象；[bootstrap](crates/auction-storage/src/database/bootstrap) 在维护库身份、权限和集群
锁下创建缺失库，不提供 DROP 或 migration；[rebuild](crates/auction-storage/src/database/rebuild) 是独立破坏性边界，
要求精确目标与受限目录。备份、人工确认、停止进程和 receipt 归 `auctionctl`（见
[数据库恢复与保留](ops/README.md#数据库恢复与保留)）。
`retention_inventory` 的 `migration.current` 也复用完整版本、success 和受限兼容 checksum 校验，
最高版本相同不能替代完整迁移历史；盘点和 schema 60 的维护游标登记均不授予删除权限。

```bash
cargo check --locked -p auction-storage --no-default-features --features postgres
```

仅静态编译，不执行迁移或行为测试。完整门禁见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。

## 迁移

[PostgreSQL 迁移](migrations/postgres/README.md)只追加；普通启动不隐式迁移，执行门禁见[执行边界](migrations/postgres/README.md#执行边界)。
`000056` 仅扩展入站装备诊断的 parse/protocol_drift 原因闭集，保留历史诊断及原子入站/游标语义；
新版 Worker 启动前必须完成迁移，字段和值域见[数据库契约](docs/contracts/schema.md)。
`000057` 仅增加账户按用户筛选的竞价 `(created_at DESC, id DESC)` 和代理 `(updated_at DESC, lot_id ASC)` 索引，
覆盖相同用户谓词的列表/计数访问，不修改分页可见性、授权状态或历史记录。
`initialization/` 只供确认后的合并首次空库初始化：同一专用连接持 SQLx 迁移锁，拒绝已有用户对象与其他客户端，
不建库、不删对象、不升级已有库。普通迁移与初始化共用受限执行视图：`000005`/`000015` 修正历史别名，`000044` 在
ALTER 前执行已排队的 `user_forum_identity_account_id_check` 延迟约束，`000047` 为保留字列 `authorization` 加双引号。
不重复加锁，不修改已发布文件、checksum、列名或原迁移事务边界。`recovery_preflight/catalog.rs` 拥有空库对象盘点
（subscription 仅计当前库，default ACL 等残留仍阻断）；`migrations/history.rs` 核对完整 SQLx 历史前缀及 checksum，
旧 archive 恢复不能只比较最大版本。canonical v28 必须走[专用接管](ops/deploy/LEGACY_V28_ADOPTION.md)。
普通迁移与 v28 接管在申请 session 迁移锁前即标记专用池连接退出时关闭；
取消、提前错误及解锁失败都不得把持锁物理会话归池，实际迁移执行不共享只读验证视图。
冻结结算策略直接从借用的 JSON 子树解码，不先复制整份策略；仍只校验历史结构，不应用新模板门禁，
后继创建另由应用/领域校验。

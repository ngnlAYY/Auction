# PostgreSQL 迁移

[docs/README.md](../../docs/README.md) · [docs/contracts/database.md](../../docs/contracts/database.md) · [crates/auction-storage/README.md](../../crates/auction-storage/README.md)

本目录是 Auction schema 的唯一定义。迁移文件在编译期由 `sqlx::migrate!` 内嵌进各二进制，并由 `auction-cli db migrate`（或 `service start/restart`、合并部署首次初始化）在 SQLx 迁移锁内执行。目标数据库为 PostgreSQL 18。

## 规则

1. **只追加。** 已发布的迁移文件永远不能修改、重命名或删除——数据库按版本记录 checksum，任何改动都会让已部署环境的迁移校验失败。修正错误只能新增迁移。“已发布”指文件已存在于任一 `v<版本>` 发布 tag；CI 的 `published-migrations` 检查逐字节比对 tag 与当前工作树，发布工作流在构建前强制执行。
2. **命名**：`NNNNNN_<snake_case 描述>.up.sql`，编号连续递增；只有向前迁移，没有 `down` 文件。
3. **一个迁移一个主题**，文件开头用中文注释说明目的、owner 与不变量。
4. **不静默改写历史事实。** 修复数据时只修复有确凿证据的部分（例如能由追加式事实重建的投影），无法证明的保持原样并留给人工核对；需要前置条件时在迁移内检查并以明确的错误终止（`RAISE EXCEPTION`），而不是猜测。
5. **并发安全**：修改被运行中进程写入的表时，先 `LOCK TABLE … IN SHARE ROW EXCLUSIVE MODE` 等方式阻止并发写，再检查与改写。迁移本身只在停机门禁下执行。
6. **约束在数据库层强制**：状态闭集、金额范围、互斥字段、只追加（触发器）等规则写成 CHECK、唯一约束或触发器，不只依赖应用代码。
7. **同步更新**：每次新增迁移都把[ops/deploy/retention-policy.v2.json](../../ops/deploy/retention-policy.v2.json)的 `database_schema_version` 更新为新的最高版本（否则 `auction-cli db inventory`、`db retention-dry-run` 与 `db retention-purge` 拒绝运行）；新增表还须加入对应数据族；同步[docs/contracts/database.md](../../docs/contracts/database.md)。
8. **运行账号授权**：迁移 `000081` 起 Public/Admin/Worker 共用的 `auction_runtime` 只有读取面与显式写入面，权限矩阵见[docs/contracts/database.md#数据库账号与权限](../../docs/contracts/database.md#数据库账号与权限)。owner 之后新建的表默认只获得 `SELECT`（序列 `USAGE`/`SELECT`），因此**新增表或新增写入面的迁移必须按该矩阵显式 `GRANT`**，授权对象固定为 `auction_runtime`。新增可变表还必须加入 `public.apply_auction_runtime_privileges()` 的可变表列表并重新执行该函数，否则 `pg_restore --no-owner` 后运行账号只有默认 `SELECT`。
9. **部署顺序**：迁移若要求旧 Worker 先停止（例如新增发送证明字段），在文件注释中写明；分体部署的 `service restart` 与合并部署的停服迁移都满足这一点。
10. **兼容登记**：075 起每个文件在自身事务末尾恰好登记本版本一次；规范与默认 breaking 规则见下节，CI 静态门禁强制。

## 兼容分级与登记

001–074 保持原文件，不回填兼容表；缺行即 `breaking`。075 建立只追加登记表并将自身登记为 `breaking`。后续迁移默认采用 `breaking`，只有证明旧程序读写不受影响的纯新增才可选择 `compatible`：

| 情况 | 等级 |
| --- | --- |
| 纯新增表、列、普通索引，旧程序不读取、不依赖且读写结果不受影响；新增列允许旧写入省略，新表自身保护约束不改变旧业务 | 可经审阅声明 `compatible` |
| 收紧约束、新增会拒绝旧写入的唯一性要求、修改既有触发器、改变冻结策略或要求停止旧角色以防行为冲突 | `breaking` |
| 删除、改名、改变类型或既有读写语义，无法证明兼容 | `breaking` |
| 放宽 CHECK 后会写入旧程序无法解析的新状态、种类、失败码、告警来源或策略版本 | `breaking`；只有旧程序对未知值已有明确容错证据才可另行评估 |

069 放宽快照闭集但旧程序不认识新竞价策略，071 冻结旧补偿池写入，均属于 `breaking`。注释须说明 owner、保护的不变量、等级依据和必要停机顺序；兼容承诺要覆盖实际允许回退的旧程序范围，不能只考虑最新程序。

`compatible` 不是只靠审阅：`mise run test-db-compat`（在 `mise run test-db` 与 CI 测试作业中运行）沿迁移链在 V-1 捕获旧程序依赖的契约、权限与旧版语句结果，执行 V 后要求不收缩；新增 NOT NULL 列、删除或改类型、改变既有约束/触发器语义、收回权限都会指出具体迁移。把迁移写成真正的纯新增是让该门禁通过的唯一正确方式，不要为通过而修改夹具的基线。

每个新文件的最后一条顶层 SQL 固定为以下单行 VALUES 形式，版本使用文件编号对应的十进制整数，等级必须显式填写：

```sql
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (76, 'breaking');
```

不要手工 BEGIN/COMMIT/ROLLBACK，不使用 `-- no-transaction`；SQLx 将文件 SQL、登记与本版本迁移成功记录同事务提交。禁止 upsert、INSERT SELECT、多行或过程内登记；等级事实不能 UPDATE/DELETE/TRUNCATE。修正追加新迁移，不编辑已发布 SQL 或历史等级。

`bash ci/checks/check.sh migration-compatibility` 已纳入 `all` / `mise run verify`，词法及正则约束见[ci/checks/README.md#迁移兼容登记](../../ci/checks/README.md#迁移兼容登记)。门禁只验证登记格式，兼容声明仍须审阅。readiness 判定由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)定义，程序回退步骤见[ops/deploy/RECOVERY.md#程序回退](../../ops/deploy/RECOVERY.md#程序回退)。

## 执行兼容层

少数早期迁移的原文在新版 PostgreSQL 或新的执行环境下需要微调。[crates/auction-storage/src/database/migrations](../../crates/auction-storage/src/database/migrations) 为它们提供“执行视图”：保留文件原 checksum，只对固定描述、原片段与出现次数做限定替换：`000005`/`000015` 给保留字别名改名，`000044` 在修改用户时间戳前立即执行排队的延迟约束，`000047` 为保留字列名 `authorization` 加引号。仅 `000005`/`000015` 额外接受已经部署的固定别名修正版 SHA-384；`000044`/`000047` 仍只接受原文件 checksum，其他差异均失败关闭。

## 索引

当前 head 为 `000131`，与 [ops/deploy/retention-policy.v2.json](../../ops/deploy/retention-policy.v2.json) 的 `database_schema_version` 一致；新增迁移必须同时推进这两处。

| 版本 | 主题 |
| --- | --- |
| 000001 | 核心竞价：世界、用户、装备、拍卖、lot、命令账本、出价、代理授权与转换、审计、realtime 流/事件/游标、schema 身份 |
| 000002 | 结算核心：入站邮件、投稿、场次与 lot 结算、补偿池与流水、结算义务与履约、投递 outbox |
| 000003 | 生命周期与代理历史：投稿目标、预览项、代理尝试与设置次数、装备密钥、周排期轮次 |
| 000004 | 身份与外部凭据：会话、凭据与各 Admin 命令账本、规则文档、心跳、黑名单、改名、登录 key 信封、邮件轮询状态 |
| 000005 | 恢复、可观测与论坛：论坛同步与回帖命令、论坛身份、运维告警、realtime 失败、付款恢复 |
| 000006 | Worker 排期轮次的租约与失败状态 |
| 000007 | realtime 重放：重放事件可引用同一原始出价事实 |
| 000008 | MoogleMail 轮询的租约、失败退避与 Admin 立即触发 |
| 000009 | 凭据管理的兼容修正 |
| 000010 | 论坛发布的持久 token、owner 与 fencing |
| 000011 | realtime 公开发现流（`auction.created` 的派生流）；该流已停用，只保留历史事件、不再写入 |
| 000012 | Final 阶段复用周轮次租约 |
| 000013 | 回填 lot 领先起点（`leader_since`） |
| 000014 | 生命周期跨行约束 |
| 000015 | 运维告警批次状态机约束 |
| 000016 | 论坛主题写入结果未知时的人工授权 |
| 000017 | isekai 载体 COD 从 main 派发 |
| 000018 | live/sold lot 必须有有效截止时间 |
| 000019 | 旧版出价写入缺少有效截止时间的兼容 |
| 000020 | 运维告警：退信资产需人工取回 |
| 000021 | 运维告警运行状态（Admin 只读健康投影） |
| 000022 | 运维告警两类时钟拆分 |
| 000023 | 恢复余额读取的“角色忙”状态 |
| 000024 | lot 按装备分类独立编号 |
| 000025 | 论坛固定楼层的远端身份 |
| 000026 | 同一世界一个用户只对应一个论坛账号 |
| 000027 | 用户论坛账号 ID |
| 000028 | 论坛命令记录真实楼层号 |
| 000029 | 论坛固定楼层扫描的健康证据 |
| 000030 | 每种外部 Cookie 同时只能一个 active |
| 000031 | 论坛已处理楼层水位 |
| 000032 | Preview 期间继续接收投稿 |
| 000033 | Admin realtime 事件查询索引 |
| 000034 | FinalFast 代理可降低上限 |
| 000035 | 入站邮件展示快照 |
| 000036 | 入站装备诊断 |
| 000037 | 修复同命令内被快速代理反超时的领先起点 |
| 000038 | 流拍装备改为逐件寄回（Send） |
| 000039 | 排期轮次冻结 IANA 本地计划；Final 完成补齐 |
| 000040 | 论坛目录分页帖 |
| 000041 | 装备标记、连续领先周期与提醒 |
| 000042 | 账号级提醒渠道与论坛私信队列 |
| 000043 | 论坛终态刷新 |
| 000044 | 修复用户时间戳倒退 |
| 000045 | 邮件凭据的预期玩家名 |
| 000046 | 邮件运行时代际 fence |
| 000047 | Bot 资金账户 |
| 000048 | 内部履约命令与证明 |
| 000049 | COD 退回后的重寄与上限收口 |
| 000050 | COD 退回上限可配置（每个恢复记录冻结） |
| 000051 | 退件消费记录（只追加） |
| 000052 | 邮件轮询待读登记 |
| 000053 | 历史 Bot 自寄结算的核对证明 |
| 000054 | 历史 Bot 卖家收款认领 |
| 000055 | 资金账户只记账模式 |
| 000056 | 装备解析诊断原因码闭集 |
| 000057 | 账户页读取索引 |
| 000058 | Public 登录时长偏好 |
| 000059 | 投递分组发送节奏 |
| 000060 | 邮箱维护游标 |
| 000061 | 发送尝试证明与远端邮件 ID 绑定 |
| 000062 | 投递外部引用索引 |
| 000063 | 收取（Take）尝试证明 |
| 000064 | HoldCurveV2：卖方全额结算 |
| 000065 | 赛季早鸟场次轮次身份 |
| 000066 | 赛季前场次的时段键（早鸟场次序号 0） |
| 000067 | 恢复策略 v2：赛季前场次 24 小时付款期限 |
| 000068 | 早鸟场次的最早收件时刻（赛季开始前停止接收装备） |
| 000069 | 新场次 schema v5 / 竞价策略 v2：统一加价并允许代理上限出价，既有场次保留 v1 |
| 000070 | 成交撤销后的补偿池冲正（只追加）；结算结果与补偿账本加只追加触发器 |
| 000071 | 各世界补偿池合并为一个 main Credits 全局池；`settlement_pools` 冻结为合并前证据 |
| 000072 | 装备损失人工责任事实（只追加、唯一退休原投递）；record_only 操作者承担买方责任，释放冻结卖家款到正常队列 |
| 000073 | 人工 Sent 观察命令与冻结 attempt/MID 绑定（只追加），独立 operator proof 恢复正常履约对账；不直接确认收款或付款 |
| 000074 | operator proof 强谓词扩展到被 Admin 对账判失败、但原生 Sent 证明已寄出的结算发送；只替换函数，原失败命令与快照不改写 |
| 000075 | 迁移兼容等级登记（只追加）；001–074 不回填，本版保守登记 breaking |
| 000076 | 运行时行为开关只追加事实兼命令账本；纯新增、旧程序不读，登记 compatible |
| 000077 | 历史 SLI 的五个全局时间候选索引；仅新增普通索引，不改写事实，登记 compatible |
| 000078 | CLI 运维审计来源显式区分 `cli`；扩展 `audit_records.source` CHECK，不修改历史行，登记 breaking（旧程序与旧 Admin 前端拒绝 cli） |
| 000079 | `rekey` 自助轮换命令与入站种类；扩展 CHECK 后登记 breaking |
| 000080 | 慢速代理等待期间的待定被超提醒（只追加、单向收口）；纯新增，登记 compatible |
| 000081 | 数据库最小权限：运行账号 `auction_runtime` 只有 USAGE/无 DDL、无 TRUNCATE，只追加表仅 INSERT，序列与 `_sqlx_migrations` 只读，默认权限只授读取面；只改授权，登记 compatible |
| 000082 | 玩家通知来源：卖方成交汇总与换季 isekai 提醒；放宽 `deliveries.origin_kind` CHECK、扩展发送类别、新增身份唯一索引；旧程序不认识新来源，登记 breaking |
| 000083 | Sent / Read Mail 扫描检查点：按凭据版本、世界与 endpoint origin 持久化最高 MID、水位时间与最近完整校验时间；只服务增量前缀，Inbox 仍以 `mail_poll_state.cursor_value` 为权威游标；把新表加入运行账号可变表授权，登记 compatible |
| 000084 | 公开成交价格历史的已成交 lot 截止时间候选索引（`effective_end_at DESC, id` + `INCLUDE`，谓词 `status='sold' AND leader_user_id IS NOT NULL`）；只新增普通索引，可见集合与统计仍由查询决定，登记 compatible |
| 000085 | 运维告警来源补齐：扩展 `operations_alert_signals` 的 kind/scope/source/identity 闭集（收件轮询停滞、凭据失效、生命周期与慢速代理逾期、队列延迟、论坛发布与私信失败、非结算通知失败含邮资不足、账号持续忙）；只放宽 CHECK、不新建表，旧程序无法解析新 kind/source，登记 breaking |
| 000086 | 竞价事件提交后分配序号：`realtime_events.stream_sequence` 允许暂时 NULL，部分索引与单次分配触发器；序号仍连续，旧程序不分配待定行，登记 breaking |
| 000087 | Admin 投递检索：收件人、模板、结算义务参考号与发送类别四个只读候选索引；纯新增，登记 compatible |
| 000088 | 统一处置事实：`delivery_dispositions` 只追加表、七类闭集、来源引用与 fence 唯一、追加触发器、运行账号只读/只插；并回填可精确证明的历史终态，登记 breaking |
| 000089 | 告警台：确认与静默只追加事实表，signal 增加当前活动周期投影与已恢复摘要标记；新表只授 SELECT/INSERT，旧程序忽略新列，登记 compatible |
| 000090 | `auction_commands.last_bid_fact_index` 取代 `bids` 上的 `max(fact_index)` 范围聚合读取（连续性仍由触发器强制）；lot 创建事务直接建立 `lot_watch_leadership` 并回填缺失行，消除首笔竞价的锁定读取未命中；替换既有触发器，登记 breaking |
| 000091 | 主机侧运维告警来源：`operations_alert_signals` 闭集新增 `backup`/`wal` 来源与 `disk_capacity_low`/`backup_failed`/`wal_upload_stalled` 种类，由 `auction-cli alert report` 在主机上采集磁盘水位、备份回执与 WAL 暂存事实后提交；只放宽 CHECK、不新建表、不改变授权，旧程序无法解析新 kind/source，登记 breaking |
| 000092 | 被超提醒与自动标记改为提交后 outbox（`lot_watch_events` 只追加处理进度、租约与顺序屏障）；出价事务只插入事件，提醒在独立事务应用；把新表加入运行账号可变表授权；纯新增表/索引与授权，登记 compatible |
| 000093 | 投递唤醒移出业务事务：删除 `delivery_ready_after_insert` 触发器与 `notify_delivery_ready`，新增提交后调用的 `signal_delivery_ready()`；修改既有触发器，登记 breaking |
| 000094 | 投递热键列化：`deliveries.payload` 的 `world`/`operation`/`template` 提升为 `GENERATED ALWAYS … STORED` 生成列并由整表重写回填，`payload` 仍是唯一真相；两个候选索引换成列形式，整行证据比较两侧剔除派生列；旧程序回退后整行证明会失败关闭，登记 breaking |
| 000095 | 资产对账与偿付能力报告：放宽 `operations_alert_signals` 的 kind/scope/source/identity 闭集，新增 `reconciliation` 来源与 `custody_drift`/`solvency_shortfall`/`reconciliation_stale` 三类信号；为 `audit_records` 的 custody/solvency 报告实体增加部分索引，报告仍只追加到既有审计表；登记 breaking |
| 000096 | 恢复后外部核对：`delivery_external_quarantines` 只追加隔离事实、投递/fence 与 world/MID 唯一；写入与 pending/claimed 改人工对账同事务；新表只授 SELECT/INSERT，登记 compatible |
| 000097 | 删除无人读写的 `bid_proxies` 视图与 `bid_proxy_attempts`、`bid_proxy_outbid_uses`、`auction_forum_reply_cursors` 三张表；同步 `apply_auction_runtime_privileges()` 去掉已删除对象的写入授权，保留策略 v2 移除对应表并升级 `database_schema_version`；不做数据保留，登记 breaking |
| 000098 | `rekey` 回信发送类别修复：082 重定义 `delivery_send_category` 时漏回 `rekey_key`/`rekey_result`，本迁移补回并重建表达式索引；只改变发送类别投影，函数语义与索引均变化，登记 breaking |
| 000099 | 放宽 `operations_alert_signals` 的 kind/identity 闭集，新增 `recovery_payment_mail_unsent`；付款期限已过但付款投递没有自动/人工发送证明时产生独立告警，不改变恢复阶段、期限、资金或投递事实；旧程序无法解析新 kind，登记 breaking |
| 000100 | 放宽 `delivery_dispositions` 的 kind/来源闭集，新增 `cod_returned` 与 `settlement_recovery_id`；COD 退回达到 recovery 冻结上限时在原事务追加处置，读侧仅对同一恢复记录的 payment/seller/refund 义务解除失败；旧程序无法解析新 kind，登记 breaking |
| 000101 | Sent/论坛远端分页达到上限 80% 与换季未完成义务的告警来源；只扩展告警 kind/identity 闭集并为扫描检查点/论坛同步补充观察列，旧程序无法解析新 kind，登记 breaking |
| 000102 | 运维告警独立 webhook 通道；新增持久投递账本、`next_webhook_at` 冷却列与运行账号写入授权，登记 compatible |
| 000103 | webhook 投递按活动周期隔离；`operations_alert_signals.webhook_generation` 在 inactive→active 时递增，投递记录 `signal_generation`，open 唯一索引改为 `(fingerprint, signal_generation)`；上一周期 `manual_reconcile` 不再阻断新事件，登记 compatible |
| 000104 | `operations_alert_signals` 的 kind/identity 闭集新增 `restore_drill_overdue`（`backup` 来源与 scope）；主机侧 `alert report` 据运维留存的成功恢复回执与通过的迁移演练回执判断月度演练逾期，旧程序无法解析新 kind，登记 breaking |
| 000105 | 竞价警告渠道偏好改为集合：`users.lot_watch_channel` 闭集扩为 none/mm/pm/both（MM、PM 各自独立，可都启用或都关闭）；`lot_watch_notifications` 与 `lot_watch_pending_notifications` 的唯一性由“每 lot × 用户 × 领先周期一条”改为“每 lot × 用户 × 领先周期 × 渠道一条”，同一渠道仍不重发；不改写既有偏好、来源、待定事实与投递；旧程序不认识新取值且 ON CONFLICT 目标失配，登记 breaking |
| 000106 | 站点文档：`rules_documents` 增加 `kind`（rules/about），文档身份由 locale 改为（kind，locale），新增“关于”文档种类；`rules_admin_commands` 增加 `document_kind` 并以（document_kind，locale）外键指向文档；既有行原样归入 rules，不改写版本与正文；旧程序按 locale 单列读写不再成立，登记 breaking |
| 000107 | 预置规则：把 2026-11-13 线上发布的五种语言规则文档（en、zh-CN、zh-TW、ja、ko）作为初始内容写入 `rules_documents`（kind=rules，版本 1，发布者 system）；只在该语言还没有规则时写入，已有规则的库不改任何行；实际写入的语言记入迁移审计；不预置“关于”；登记 compatible |
| 000108 | 出站通知模板迁入数据库：新增只追加的 `mail_template_events`（修订事实兼命令账本，每个模板键以最高 `revision` 为准，`reset` 表示使用内置默认正文），运行账号只授 `INSERT`；模板不再从 runtime 文件或 Worker 配置读取 |
| 000109 | Public 竞价日志按新建代理命令查被排位击败的授权：`bid_proxy_authorization_transitions` 新增部分索引 `bid_proxy_transitions_ranked_below_origin_idx`（`origin_command_id`，仅 `exhausted`/`ranked_below`），逐行查找由顺序扫描变为索引探测；不改结构、约束与数据，登记 compatible |
| 000110 | `operations_alert_signals` 的 kind/identity 闭集新增 `fulfillment_collection_overdue`（`fulfillment` 来源、`auction` scope）：已寄出、只等收件人领取的 Send 履约寄出时长超过运行配置阈值时告警，只提示人工跟进，不改变义务、投递、资金或资产，也不触发任何自动动作；同时补上 000095 漏登记的 scope `reconciliation`（对账报告缺失时的 `source_unavailable` 此前违反 scope CHECK，整轮扫描回滚）；履约新增 `dormant` 标记（从未激活的或有义务的初始 `pending` 履约；结算写入时置真，激活或装备损失直接收口时同事务置假，CHECK 与延迟约束触发器拒绝漏清，迁移按同一谓词回填），新增部分索引 `settlement_obligation_fulfillments_open_idx`（尚未满足且不带标记的履约）与 `settlement_obligations_blocked_idx`（`blocked` 义务），供告警扫描与看板只读活动履约；`runtime_config_admin_commands.digest_version` 闭集放宽为 1、2（新命令以版本 2 记录原始请求摘要，升级前的版本 1 行原样保留）；旧程序无法解析新 kind，也不认版本 2 的命令行，登记 breaking |
| 000111 | 结算退件证据：新增只追加的 `settlement_return_evidence`（以入站邮件 ID 为主键，记录处置 `untrusted`/`unbound`/`fulfillment_terminal`/`manual_reconcile`、原因码、原义务与原投递），与恢复流程的退件消费按“来源可证明且绑定恢复付款/交付义务”互斥分区；只留证与把未终结履约转人工对账，不领取、不退回、不重发、不改变资金；`inbound_mail` 增加结算退件部分索引；运行账号只授 `INSERT`；纯新增，登记 compatible |
| 000112 | Admin 确认收回/取消的成交撤销的历史投影修复：只修能证明确认、退件、Take 与被处置发送尝试彼此一致的记录——命令账本有该确认；原付款履约为 `enqueued` 且无未过期领取，原付款投递是唯一派发且已成功寄出；恢复记录当前退件来源可证明、绑定原付款义务并已被消费，确认种类对应的 Take 取回了这封退件且证明与 custody 一致；原付款义务每一次可能已到达远端的发送都由本恢复记录已消费的系统退件逐一归属（计数相等，只有投递当前发送能凭 `failed` 与明确的 `write_started=false`、`outcome_unknown=false` 证明未外写）、任何发送都早于当前退件截止（消费时刻与收件时刻加 3 分钟取早）且最早的发送早于第一封退件的截止，确认后没有新的 Admin 命令；截止之前的两次发送、同分钟多次发送或时钟偏差下落在截止之前的重发都保持原样；依赖履约未开始。满足时把原付款履约改为 `failed`（`recovery_reclaimed`/`recovery_cancelled`）、未开始的卖方货款与 isekai 装备交付改为 `failed`（`recovery_sale_voided`），并在原付款投递当前 fence 追加 `cod_returned` 处置与审计；结果未知（`manual_reconcile`、写入未知）、确认后新 fence、新派发等其余记录保持原样留给人工核对，不改投递、证明、资金或恢复记录；改写既有行并写入旧读侧不认识的失败码，登记 breaking |
| 000113 | `delivery_dispositions` 的种类闭集新增 `recovery_equipment_recovered`（来源仍为 `settlement_recovery_id`，写入时固定解除告警）：isekai 已付款交付被退回、装备经精确 Take 证明收回后，Admin 确认 `equipment_recovered` 在同一事务内把从未开始的卖方货款（`recovery_sale_voided`）与已寄出的交付（`recovery_equipment_recovered`）收口为 `failed`，并在原交付投递当前 fence 上追加该处置；不改写既有行，不改变资金或资产；旧程序无法解析新种类，登记 breaking |
| 000114 | 结算退件案件：新增可变投影 `settlement_return_cases`（每条退件证据一个案件，含 untrusted/unbound/fulfillment_terminal；冻结补寄上限 3、案件版本乐观并发；同一原派发只绑定一个案件）与持久 Take 投影 `settlement_return_actions`，只追加的 `settlement_return_commands`（command_id 幂等账本）、`settlement_return_events`（每个案件版本一条，含 Take 结果未知后的原会话观察 `take_observed` 与凭 `assets_claimable` 观察再次领回的 `take_retry_requested`）、`settlement_return_takes`（精确领回证明只消费一次，原派发、Take 投递与邮件各唯一）与 `settlement_return_verifications`（只读重新核验读到的完整证据，改绑的案件以 `verified_version` 引用，入站证据原文不改写）；`deliveries.origin_kind` 新增受限来源 `settlement_return_action`（只做 Take）；为已有证据补建案件与开案事件；运行账号对投影授读写、对事实只授 `INSERT`；旧程序不认识新来源与新互斥，登记 breaking |
| 000115 | 结算退件补寄与资金返还：履约新增当前派发编号 `dispatch_no`（既有行为 1），只追加的派发历史 `settlement_fulfillment_dispatches`（`(义务, 派发编号)` 主键，补寄引用授权它的退件案件）与资金投递历史绑定 `settlement_funding_delivery_bindings`（分配指针换代后旧派发仍受资金保护）；`settlement_funding_returns` 增加入站/Take/来源唯一绑定（NOT VALID 只约束新行）并改为只追加、运行账号只授 `INSERT`；补登既有派发与绑定；旧程序按单指针定位资金，登记 breaking |
| 000116 | `operations_alert_signals` 的 kind/scope/identity 闭集新增 `settlement_return_pending`（已绑定待处理的退件案件）与 `settlement_return_quarantined`（隔离或待重新核验的退件案件），沿用 `fulfillment` 来源与 `auction` scope；另新增 `settlement_return_take_unknown`（Take 结果未知的退件案件），scope 为结果未知的原因闭集（`assets_claimable`、`assets_unclaimable`、`assets_changed`、`mail_missing`、`receipt_unproven`、`session_changed`、`take_unverifiable`、`disposition_conflict`、`awaiting_observation`），每个原因一个信号；按未结案案件计数，不改变案件、义务、投递或资金；旧程序无法解析新 kind，登记 breaking |
| 000117 | 资金分录新增 `(uid,id)` 复合唯一键与分录复合外键，阻止 debit/credit/source 跨 UID 归属；延迟约束触发器把结算退件 action 绑定到来源、身份、prepare/activate 命令、世界、Take 操作及外部 MID 均匹配的唯一 delivery，已绑定后不可换代或删除；新增唯一性、外键和触发器限制旧程序可能的写入，登记 breaking |
| 000118 | 代理转换历史连续性与当前投影、realtime 来源完整性和终态拍卖/lot 跨行一致性；历史 recovery 重挂来源按命令账本或 Worker 审计证明并保留，`origin_command_id` 的 `auction_commands` 外键仅强制后续写入；依赖 000079 的命名 CHECK 基线缺失时失败关闭，登记 breaking |
| 000119 | 入站邮件展示快照 CHECK 安全校验 JSON 数值类型、整型范围与载荷大小，不以窄化 cast 引发范围异常；登记 breaking |
| 000120 | 资金 issue/return 总额不得超源。只记账开账来源发行绑定同 UID 的开账命令与 quarantine allocation，其余 seller 来源绑定同 UID、付款义务、lot、冻结拆分及 seller payout allocation。历史预检核对 equipment loss、collected receipt 与冲正。资金分录只追加，UPDATE/DELETE/TRUNCATE 均由触发器拒绝。登记 breaking |
| 000121 | 早鸟排期日期使用 Gregorian 日历校验，冻结恢复策略要求 JSON 数值类型，runtime switch expire 只能引用同名、当前且已到期的 set 事实；登记 breaking |
| 000122 | 运行账号权限重建覆盖所有数据库对象 ownership、继承角色、PUBLIC 写权限、各对象创建者默认 ACL 与旧 SECURITY DEFINER 函数授权；保留 webhook INSERT/UPDATE、撤销 DELETE，并将 funding entries 限为 INSERT；登记 breaking |
| 000123 | 外部 Sent quarantine 与 delivery 当前 fence 及状态转换同事务绑定；000112 关账 Send/退件逐一按时间区间归属；修复可唯一证明的 000114 earliest backfill，并约束退件 action 生命周期；登记 breaking |
| 000124 | 邮箱 sweep cursor 新增 sweep/attempt 身份，只有带新 attempt 且无剩余后缀候选的本轮扫描可回绕；登记 breaking |
| 000125 | 结算退件 Take 的原会话完整领取证明可将 action 从 `manual_reconcile` 直接收口为 `satisfied`；数据库同时核对唯一 Take 事实、案件/义务/邮件身份、成功投递及当前 fencing attempt，登记 breaking |
| 000126 | 场次结算池余额 CHECK 改用 NUMERIC 中间值，避免 bigint 加法在约束求值时溢出；登记 breaking |
| 000127 | 将引用终态 delivery 的 pending/claimed 告警批次按 delivery 权威结果收口；只放行身份一致且未触碰的 prepared delivery，其他非终态关联失败关闭；登记 breaking |
| 000128 | 以复合唯一键和外键绑定 lot 分类快照与 equipment 分类，阻止已有 lot 的分类漂移；登记 breaking |
| 000129 | 新建 buyer_blacklist recovery action 必须有理由，保留历史 NULL 理由；登记 breaking |
| 000130 | `cod_returned` disposition 绑定同一 recovery 的付款 Send 投递与 fence，身份引用写入后不可改写；登记 breaking |
| 000131 | 资金来源累计 issue/return 由来源行原子计数并受冻结额度约束，已使用来源金额不可修改；运行账号仅可列级 INSERT 来源，不可写累计计数器，权限重放保持该限制；复合外键绑定补偿冲正的 recovery、auction 与 world，历史不一致时失败关闭；登记 breaking |

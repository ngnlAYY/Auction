# PostgreSQL 完整档迁移

[文档导航](../../docs/README.md) · [对象 owner](../../docs/contracts/schema.md) · [执行与兼容映射](../../crates/auction-storage/src/database/migrations) · [v28 专用接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)

维护 PostgreSQL 完整 schema，历史字节与 checksum 不变；结构修正通过新迁移追加。

## 本页导航

- [迁移文件索引](#迁移文件索引)
- [原则与基线](#原则与基线)
- [追加迁移语义](#追加迁移语义)
- [历史 checksum 兼容](#历史-checksum-兼容)
- [历史格式与后端边界](#历史格式与后端边界)
- [连续领先时间修复](#连续领先时间修复)

## 迁移文件索引

| 迁移 | 职责 |
| --- | --- |
| [000001_core_competition.up.sql](000001_core_competition.up.sql) | 核心身份、拍卖、竞价与代理 |
| [000002_settlement_core.up.sql](000002_settlement_core.up.sql) | 结算、补偿与履约基础 |
| [000003_lifecycle_proxy_history.up.sql](000003_lifecycle_proxy_history.up.sql) | 生命周期、代理历史与目录 |
| [000004_identity_external_admin.up.sql](000004_identity_external_admin.up.sql) | 身份、外部凭据及管理命令 |
| [000005_recovery_observability_forum.up.sql](000005_recovery_observability_forum.up.sql) | 恢复、可观测性与论坛基础 |
| [000006_worker_scheduler.up.sql](000006_worker_scheduler.up.sql) | Worker 轮次租约与失败状态 |
| [000007_realtime_replay.up.sql](000007_realtime_replay.up.sql) | Realtime replay 追加边界 |
| [000008_mooglemail_polling.up.sql](000008_mooglemail_polling.up.sql) | MoogleMail 持久轮询 |
| [000009_credential_admin_compat.up.sql](000009_credential_admin_compat.up.sql) | 凭据闭集与安全重放 |
| [000010_forum_sync_fencing.up.sql](000010_forum_sync_fencing.up.sql) | 论坛同步 fencing |
| [000011_realtime_discovery_alias.up.sql](000011_realtime_discovery_alias.up.sql) | 发现流持久别名 |
| [000012_scheduled_final_phase.up.sql](000012_scheduled_final_phase.up.sql) | Final phase 调度 fence |
| [000013_leader_since_backfill.up.sql](000013_leader_since_backfill.up.sql) | 可证明的领先时间回填 |
| [000014_lifecycle_database_invariants.up.sql](000014_lifecycle_database_invariants.up.sql) | 生命周期数据库不变量 |
| [000015_operations_alert_state_invariants.up.sql](000015_operations_alert_state_invariants.up.sql) | 告警 signal 与批次不变量 |
| [000016_forum_sync_write_authorization.up.sql](000016_forum_sync_write_authorization.up.sql) | 未知论坛写入的人工再授权 |
| [000017_settlement_carrier_dispatch_world.up.sql](000017_settlement_carrier_dispatch_world.up.sql) | Carrier dispatch world 修正 |
| [000018_lot_effective_end_invariant.up.sql](000018_lot_effective_end_invariant.up.sql) | Lot 有效结束时间不变量 |
| [000019_legacy_bid_effective_end_compat.up.sql](000019_legacy_bid_effective_end_compat.up.sql) | 历史 bid INSERT 有效结束时间映射 |
| [000020_operations_alert_returned_mail.up.sql](000020_operations_alert_returned_mail.up.sql) | 退信独立告警 signal |
| [000021_operations_alert_runtime_state.up.sql](000021_operations_alert_runtime_state.up.sql) | 告警运行状态与扫描水位 |
| [000022_operations_alert_runtime_clock_split.up.sql](000022_operations_alert_runtime_clock_split.up.sql) | 配置状态与扫描时钟分离 |
| [000023_recovery_balance_busy.up.sql](000023_recovery_balance_busy.up.sql) | Recovery 余额 busy 状态 |
| [000024_lot_category_numbering.up.sql](000024_lot_category_numbering.up.sql) | 分类内标的编号 |
| [000025_forum_fixed_posts.up.sql](000025_forum_fixed_posts.up.sql) | 论坛固定帖子身份与刷新租约 |
| [000026_forum_identity_owner_unique.up.sql](000026_forum_identity_owner_unique.up.sql) | 同世界论坛身份唯一 owner |
| [000027_user_forum_account_identity.up.sql](000027_user_forum_account_identity.up.sql) | Users 论坛账号身份 |
| [000028_forum_command_post_number.up.sql](000028_forum_command_post_number.up.sql) | 论坛命令真实楼层 |
| [000029_forum_sync_health_identity.up.sql](000029_forum_sync_health_identity.up.sql) | 论坛同步健康与身份历史 |
| [000030_single_active_external_cookie.up.sql](000030_single_active_external_cookie.up.sql) | 单一 active 外部 Cookie |
| [000031_forum_processed_floor.up.sql](000031_forum_processed_floor.up.sql) | 连续处理楼层水位 |
| [000032_preview_intake_handoff.up.sql](000032_preview_intake_handoff.up.sql) | Draft/Preview intake 交接 |
| [000033_realtime_event_admin_index.up.sql](000033_realtime_event_admin_index.up.sql) | Admin 跨流事件索引 |
| [000034_proxy_transition_final_fast_lowering.up.sql](000034_proxy_transition_final_fast_lowering.up.sql) | FinalFast 同状态降额 |
| [000035_mail_intake_display_snapshot.up.sql](000035_mail_intake_display_snapshot.up.sql) | 入站邮件脱敏展示投影 |
| [000036_mail_intake_equipment_diagnostic.up.sql](000036_mail_intake_equipment_diagnostic.up.sql) | 装备读取失败诊断投影 |
| [000037_live_leader_since_repair.up.sql](000037_live_leader_since_repair.up.sql) | 未结算 live lot 连续领先时间修复 |

## 原则与基线

PostgreSQL 迁移必须保持只追加。当前 `000001` 至 `000037` 已按 owner 追加核心身份、
拍卖、竞价、代理、结算/履约、生命周期、身份/外部命令、恢复/可观测性和论坛同步对象；
它们是当前目标 schema，但不能单独证明全部业务表面已经完成。历史名称
`bid_proxies` 由末尾只读兼容视图提供，权威写入只进入新授权与历史表。

## 追加迁移语义

`000006` 为 Worker 轮次表追加租约、尝试次数和失败状态，供 `SKIP LOCKED` 领取使用。
`000007` 为 realtime replay 放宽原始 bid fact 的唯一性边界：唯一性只约束原始事件，
不阻止带 `replay_of_event_id` 的追加修复事件。
`000008` 为 MoogleMail 轮询追加持久化游标；`000009` 修正 A32 凭据闭集并允许删除
凭据后保留命令账本，支持 `forum_cookie` 和安全重放。
`000010` 为论坛主题同步追加 lease/fencing 证明，防止多 Worker 重复发布或回写过期状态。
`000011` 为已有和后续 `auction.created` 事件建立持久化的 `auction:discovery` 派生流，
供 Public 发现范围跨进程恢复；该流仍不是拍卖事实的第二份权威写入。
`000012` 为持久化轮次追加 Final phase 完成 fence；`000013` 只从追加式 bid 事实重建早期
Rust 数据缺失的当前连续 `leader_since`，无法证明的行保持空值并由结算失败关闭。
`000014` 先补齐早期 Rust 迁移遗漏但可唯一恢复的既有 draft intake target，再恢复拍卖身份
不可变、前序顺序以及 draft/target 一致性的数据库防线，并移除旧 v28 接管后不再属于数据库
owner 的论坛入队触发器。
`000015` 统一 operations alert 的 signal identity、批次 claim/retry/terminal 状态和错误码闭集，
恢复成功入队后才生效的 cooldown，并为迁移后的新批次建立 delivery 引用约束；旧 v28 合法的
历史终态孤儿保留为可审计事实，只有目标不存在孤儿时才验证整张表的外键。
`000016` 新增 `forum_sync_admin_commands`，把论坛远端写入未知后的再次 POST 限制为显式、幂等、
逐字确认目标并追加审计的人工授权；只读精确标题对账本身不需要授权，也不会创建远端主题。
`000017` 保留 `winner_payment_carrier_cod` 结算义务的 `isekai` 来源世界，仅将履约
dispatch 世界固定为 `main`，并回填已有的异世界旧路由记录；该修正不改变异世界 recovery
案件的来源世界。
`000018` 在锁内拒绝无法解释的旧投影，并恢复 `live`/`sold` lot 必须具有
`effective_end_at` 的旧 schema 不变量；迁移不会猜测或回填业务截止时间。
`000019` 为滚动升级窗口保留旧 v28 的固定 bid INSERT：旧事务已先更新并锁定 lot，兼容
trigger 只在遗漏 `bids.effective_end_at` 时复制该权威值；新版显式写入不经过兼容分支，
无法解析的遗漏值继续按约束错误失败关闭。
`000020` 为已证明的 MoogleMail 收件人不存在退信追加 `delivery_returned_mail` signal kind，
保留既有 `delivery_manual_reconcile` 总量 identity 和历史 fingerprint；该迁移只扩展 PostgreSQL
完整档 operations alert 闭集，不为 SQLite Lite 伪造外部邮件能力。
`000021` 新增 operations alert 单例运行状态：Worker 发布非敏感启用状态，成功 scan 与 signal
变更在同一事务推进扫描水位；Admin 不再从表是否为空推断配置，也不读取 Worker 私有 TOML。
`000022` 移除配置状态发布时间与扫描水位之间不存在的先后约束：状态发布继续按自身时间单调
校验并在倒退时失败关闭，成功 scan 只单调推进 `last_observed_at`，不会阻止之后的启停发布。
`000023` 扩展 recovery 余额状态闭集：HV 返回战斗页时写入 `busy`，保留有效 Cookie 与最后成功
余额，待战斗结束后由后续 Worker 刷新恢复正常状态。
`000024` 为 lot 固化分类键和分类内展示编号，并为已经开始发布的论坛主题冻结旧编号方案；
`000025` 保存固定 #1 拍卖目录与 #2 系统日志的 exact post ID、规范正文和单调内容版本，并使用
独立的 due 水位、claim token、owner、lease 与 fencing 串行化远端固定帖刷新。
`000026` 为同一 world/provider 下的 Users 主体增加唯一论坛账号归属，阻止同名并发建立多重映射。
`000027` 将唯一、规范十进制的论坛账号 ID 提升到 Users owner，并以延迟约束保证该主体与全部
论坛映射在事务提交时一致；`000028` 为论坛竞价命令保存由完整主题读取证明的真实楼层号。
`000029` 保存逐主题固定帖同步健康，并以追加式历史记录论坛数字账号和可变玩家名称的变化；
`000030` 通过迁移前失败关闭检查与 partial unique index，保证邮件和论坛 Cookie 各自最多一条 active。
`000031` 单独保存每个主题已连续处理到的真实楼层，初始为固定系统帖 #2，禁止用正文版本或远端
post ID 推算该水位。`000032` 把唯一 intake target 的状态闭集扩展为 Draft/Preview；迁移拒绝历史
Preview 与后继 Draft 同时存在的切换窗口，并以跨状态唯一索引、精确复合外键和延迟触发器保证
正式开拍前目标仍是本期，Open 后才切换到下一期 Draft。`000033` 为 Admin 最近事件的跨流
新到旧读取追加 `(created_at, event_id)` 索引，不改变事件内容、顺序或消费游标。
`000034` 仅替换代理转换矩阵中的降额规则，允许 `active → active` 与 Fast 模式的
`final_fast → final_fast`；其余转换分支、历史行和已发布迁移保持原语义。
`000035` 为 MoogleMail `inbound_mail` 追加 nullable `sender`、`subject` 和 `message_display`，
用于 PostgreSQL Admin 邮件列表的 scalar/has 投影和详情脱敏 snapshot。它不改变业务事实、幂等或 cursor，
不为 SQLite Lite 增加对应列；普通 replay 不补历史 NULL。
`000036` 为被拒绝装备邮件追加 nullable `equipment_diagnostic` sidecar；它只保留首个失败的低基数诊断和
证据留存状态，不进入决定 metadata、命令摘要、完成证明或 cursor。成功读取不会落盘准备好的密文，SQLite Lite
不增加该列。


## 历史 checksum 兼容

`000005` 与 `000015` 的历史文件字节和 SQLx checksum 保持不可变。两者曾使用 PostgreSQL
保留字作为未加引号的查询别名；`auction-storage` 的迁移执行视图只对这两个固定版本、描述和
精确片段执行计数受限的别名替换。fresh 数据库继续记录原始 checksum；曾运行早期别名修正版的
数据库只在既有值精确等于同一固定替换后 SQL 的 SHA-384 时沿用该值，既不改写历史也不接受第三种
迁移身份。任何其它历史 SQL 漂移仍由 SQLx 失败关闭。


## 历史格式与后端边界

canonical PostgreSQL v28 历史格式的接管不伪装成一条普通追加迁移。专用 bridge 位于
`auction-storage/src/database/legacy_v28/`，只在精确格式预检、已验证备份、零连接和显式确认后
建立 SQLx 等价基线，再继续执行本目录尚未覆盖的前向迁移。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
完整生产能力和 [`../../docs/architecture.md`](../../docs/architecture.md) 中的不变量一致，
不受 SQLite Lite 能力子集限制。两边共有的领域对象保持同名同义即可，不要求与
[`../sqlite/`](../sqlite/) 迁移一一对应。已经发布的迁移文件禁止修改；结构修正应通过
新的追加迁移完成。


## 连续领先时间修复

`000037` 在排除并发业务写入后，从追加式 bid 重建尚未结算 live lot 的连续领先起点，
修复同命令内普通出价与代理反超没有重置 `leader_since` 的旧投影；已结算记录不改写。

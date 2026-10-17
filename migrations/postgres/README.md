# PostgreSQL 完整档迁移

[文档导航](../../docs/README.md) · [对象 owner](../../docs/contracts/schema.md) · [执行与兼容映射](../../crates/auction-storage/src/database/migrations) · [v28 专用接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)

PostgreSQL 完整 schema 的追加迁移；结构修正只能增加新文件。

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
| [000038_unsold_return_send.up.sql](000038_unsold_return_send.up.sql) | 未领取且未冻结的流拍返还改为逐件 Send |
| [000039_schedule_source_and_final_completion.up.sql](000039_schedule_source_and_final_completion.up.sql) | 冻结本地排期来源并补齐可证明的 Final 完成标记 |
| [000040_forum_catalog_pages.up.sql](000040_forum_catalog_pages.up.sql) | 每帖 150 件的论坛目录续页身份、正文和目录页数 |
| [000041_lot_watches.up.sql](000041_lot_watches.up.sql) | 标记版本、连续领先周期及独立 MM 通知来源；只补有效代理标记 |
| [000042_lot_watch_pm.up.sql](000042_lot_watch_pm.up.sql) | 账号渠道独立 CAS、跨渠道统一来源、PM 租约/sender/不可重寄 fence；不改历史 MM payload |
| [000043_forum_terminal_refresh.up.sql](000043_forum_terminal_refresh.up.sql) | forum 终态纳入与真实全页完成版本；升级前 settled 明确豁免，不伪造成功 |
| [000044_user_timestamp_order.up.sql](000044_user_timestamp_order.up.sql) | Users 时间顺序约束；仅修复有 MM 注册/论坛同步双重证据的快照倒退并追加审计 |
| [000045_mail_player_identity.up.sql](000045_mail_player_identity.up.sql) | 邮件凭据 nullable 预期完整玩家名称；不代表远端核验，不回填旧记录 |

## 原则与基线

当前 `000001`–`000045` 覆盖完整档数据库对象；文件存在只表示 schema 定义，不代表全部业务表面完成验收。
`bid_proxies` 是只读兼容视图，权威写入进入授权与转换历史表。
执行门禁见[迁移导航](../README.md)，对象归属见[数据库契约](../../docs/contracts/schema.md)。

## 追加迁移语义

上表提供文件入口，完整含义集中在[迁移语义与兼容边界](../../docs/contracts/schema.md#迁移语义与兼容边界)，避免维护第二份逐项历史。
迁移不得猜测业务事实：重复 active Cookie、冲突 intake target 或无法解释的旧投影应失败关闭，由所属命令修复。

`000035`/`000036` 的邮件展示与装备诊断为非权威投影，不参与幂等、完成证明或 cursor。
`000038` 仅转换从未领取、未冻结、未关联 delivery 的初始流拍返还，已开始或未知 Return 保留身份并人工核对。
`000039` 冻结后续轮次的本地排期来源，只补有唯一成功 Final 证明的完成标记，不能用当前时区猜测历史。
`000040` 增加目录续页镜像及冻结布局；历史已尝试主题保持一页并继续导入，只有从未尝试/无远端身份的队列项及新主题启用分页。
不回填或猜测远端 post ID，不移动已有日志/用户回帖，未知 POST 保持原布局对账。

`000041` 为已有 active/final_fast 代理补标记并追加审计，只初始化当前领先周期，不创建过去超价提醒。
取消保留 tombstone；通知有独立来源及每领先周期唯一约束，领取/发送依赖 watch owner proof。
`000042` 保持旧 watch.channel=mm 占位与旧 MM 内容，账号偏好默认 mm/1，独立 PM 来源与队列不冒充 MM delivery。
PM sending_at/sender 由触发器保护不可清空或替换，sending 后不回 pending；缺收件人 suppressed，不补发。
`000043` 不改经济事实、旧迁移或历史帖子；升级前 settled 的终态纳入标记为 false、完成版本仍为 NULL，其余及新记录默认纳入。
执行前受控停止全部旧 writer，详见[升级门禁](../../ops/README.md#装备标记与超价提醒升级)，此轮无迁移验收。
`000044` 保留用户创建时间与原始证据，仅提升有证明的异常更新时间并推进版本；每条更正形成追加审计。
其余倒退行使时间顺序约束校验失败并回滚迁移，不批量猜测历史时区；执行前见[用户时间升级门禁](../../ops/README.md#用户快照时间修复升级)。

`000045` 仅由 credentials owner 增加 mail-only 的预期名称和边界约束，无 verified 状态或内部结清表。
旧凭据保持 NULL/原身份/密文，禁止默认删除重导入补绑；手动升级见[截止与邮件隔离](../../ops/README.md#截止规则与邮件发送隔离升级)。

## 历史 checksum 兼容

`000005`/`000015` 历史文件与原始 checksum 保持不可变。
[auction-storage 执行视图](../../crates/auction-storage/src/database/migrations)只对固定版本、描述与精确计数的片段替换保留字别名。
新库继续记录原始 checksum；早期别名修正版数据库只在既有值精确匹配固定替换 SQL 的 SHA-384 时沿用该值，不改写历史。
其他 SQL/checksum 漂移均失败关闭。

该兼容由存储迁移入口提供，不保证原始 SQL 可交给任意执行器直接运行。
普通迁移和合并首次初始化共用原 SQLx 迁移锁，不重复加锁；源码兼容不替代[空库运行证据](../../docs/status.md)。

## 历史格式与后端边界

canonical PostgreSQL v28 使用[专用接管手册](../../ops/deploy/LEGACY_V28_ADOPTION.md)与 `legacy_v28` bridge：
精确预检、可验证备份、零外部连接及显式确认后建立等价 SQLx 基线，再应用后续迁移，不能伪装为普通追加文件。

SQL、索引与锁均按 PostgreSQL 契约维护。

## 连续领先时间修复

`000037` 在排除并发业务写入后，从追加式 bid 重建未结算 live lot 的连续领先起点，
涵盖同命令普通出价后被原领先者快速代理反超的情况。无法证明不猜测，已结算金额、终态记录与历史审计不改写。

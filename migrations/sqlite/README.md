# SQLite Lite 迁移

[文档导航](../../docs/README.md) · [能力矩阵](../../docs/architecture.md#部署档位与能力) · [串行写适配](../../crates/auction-storage/src/database/sqlite) · [完整档边界](../postgres/README.md)

只维护 Lite 声明支持的 schema；本地文件、单主机与显式串行写入是部署前提。

## 本页导航

- [迁移文件索引](#迁移文件索引)
- [范围与约束](#范围与约束)
- [迁移序列](#迁移序列)
- [连续领先时间修复](#连续领先时间修复)

## 迁移文件索引

| 迁移 | 职责 |
| --- | --- |
| [000001_lite_core.up.sql](000001_lite_core.up.sql) | Lite 核心对象 |
| [000002_worker_scheduler.up.sql](000002_worker_scheduler.up.sql) | Worker 轮次租约与失败状态 |
| [000003_user_admin.up.sql](000003_user_admin.up.sql) | 用户管理命令 |
| [000004_auction_cancellation.up.sql](000004_auction_cancellation.up.sql) | 拍卖取消元数据 |
| [000005_lifecycle_metadata.up.sql](000005_lifecycle_metadata.up.sql) | 生命周期与前后轮元数据 |
| [000006_scheduled_final_phase.up.sql](000006_scheduled_final_phase.up.sql) | Final phase 调度 fence |
| [000007_leader_since_backfill.up.sql](000007_leader_since_backfill.up.sql) | 可证明的领先时间回填 |
| [000008_lot_effective_end_invariant.up.sql](000008_lot_effective_end_invariant.up.sql) | Lot 有效结束时间不变量 |
| [000009_lot_category_numbering.up.sql](000009_lot_category_numbering.up.sql) | 分类内标的编号 |
| [000010_user_forum_account_identity.up.sql](000010_user_forum_account_identity.up.sql) | 用户论坛账号身份 |
| [000011_realtime_event_admin_index.up.sql](000011_realtime_event_admin_index.up.sql) | Admin 跨流事件索引 |
| [000012_live_leader_since_repair.up.sql](000012_live_leader_since_repair.up.sql) | 未结算 live lot 连续领先时间修复 |
| [000013_schedule_source_and_final_completion.up.sql](000013_schedule_source_and_final_completion.up.sql) | 冻结本地排期来源并补齐可证明的 Final 完成标记 |

## 范围与约束

已发布迁移禁止修改，修正只追加。两后端共有对象必须同名同义，不要求与 [PostgreSQL](../postgres/README.md) 的文件、序号或完整能力一一对应。
Lite 只缩减系统能力，已实现竞价仍遵守幂等、事务、追加审计与凭据安全。

竞价及相关写入经 SQLx 跟踪的 `BEGIN IMMEDIATE`；文件库使用 WAL、外键、锁等待和单写者约束，
每角色最多一个实例，Worker 并发一，不使用网络共享文件系统。连接与消费 owner 边界见[存储实现](../../crates/auction-storage/README.md#sqlite-连接选项)。

## 迁移序列

`000001`–`000013` 提供共有身份、拍卖、竞价/代理、会话、审计、realtime、调度及其修正，具体文件见上表。
结算、恢复、交付和外部论坛/邮件对象属于完整档，Lite 不建表；保留 settled/sold 状态用于模型兼容不代表具有结算事实链，
Public 成交价格历史因此明确返回能力不支持。

`000013` 与 PostgreSQL `000039` 同义：新增可空本地排期来源，历史轮次保留 NULL 与 UTC 周期；
仅补齐可唯一证明成功 Final 的完成标记。业务与历史事实边界见[对象契约](../../docs/contracts/schema.md#追加迁移与历史事实)。
执行门禁及验证限制见[迁移导航](../README.md)。

## 连续领先时间修复

`000012` 与 PostgreSQL `000037` 同义，只从追加式接受事实修正未结算 live lot 的连续领先起点；
不扩大 Lite 结算能力，不重写已保留的终态记录、金额或审计。

# SQLite 迁移

SQLite 迁移必须保持只追加。本目录只维护 Lite 档明确支持能力所需的 schema 子集，允许
使用 SQLite 专属的 SQL 和事务初始化方式，不要求与 [`../postgres/`](../postgres/) 的
完整生产 schema 或迁移序号一一对应。两边共有的领域对象必须保持同名同义。

SQLite 的串行写入、锁等待、本地文件系统、备份和单节点假设必须在涉及竞价写入的迁移与
架构文档中明确说明。Lite 可以减少系统能力，但已支持的竞价写入仍必须保持事务、幂等和
追加式审计不变量。已经发布的迁移文件禁止修改；结构修正应通过新的追加迁移完成。

当前 `000001_lite_core.up.sql` 已提交共有身份、拍卖、竞价、代理、审计、realtime、会话和
调度对象，`000002_worker_scheduler.up.sql` 追加调度租约、尝试次数和失败状态，
`000003_user_admin.up.sql` 追加用户管理命令账本和可选的登录密钥信封，`000004_auction_cancellation.up.sql`
追加拍卖取消时间、原因、操作者及形状触发器，`000005_lifecycle_metadata.up.sql` 补齐预览、开放、
Final phase 和前后轮关系，`000006_scheduled_final_phase.up.sql` 持久化 Final phase 完成 fence，
`000007_leader_since_backfill.up.sql` 从追加式 bid 事实修正可证明的当前连续领先时间，
`000008_lot_effective_end_invariant.up.sql` 以触发器和失败关闭预检恢复 live/sold lot 的有效结束
时间 shape，`000009_lot_category_numbering.up.sql` 固化分类内 lot 编号，
`000010_user_forum_account_identity.up.sql` 保存论坛账号身份，
`000011_realtime_event_admin_index.up.sql` 为 Admin 最近事件的跨流新到旧读取追加索引；结算、恢复、交付、
以及外部论坛/邮件对象仍属于 PostgreSQL 完整档，Lite 不建表。
因此 Lite 不声明 Public 成交价格历史能力；`settled`/`sold` 状态列用于共有模型兼容，不构成可供
历史查询采信的结算事实链，对应 HTTP 路由固定返回 `501 feature_not_supported`。
SQLite 竞价事务必须使用 `BEGIN IMMEDIATE` 的串行写入口，不能因为 migration 已存在就绕过
该适配器边界。

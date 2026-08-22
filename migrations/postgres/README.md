# PostgreSQL 迁移

PostgreSQL 迁移必须保持只追加。当前 `000001` 至 `000007` 已按 owner 追加核心身份、
拍卖、竞价、代理、结算/履约、生命周期、身份/外部命令、恢复/可观测性和论坛同步对象；
它们仍是迁移中的目标 schema，不等于旧系统全部行为已经完成。旧名
`bid_proxies` 由末尾只读兼容视图提供，权威写入只进入新授权与历史表。
`000006` 为 Worker 轮次表追加租约、尝试次数和失败状态，供 `SKIP LOCKED` 领取使用。
`000007` 为 realtime replay 放宽原始 bid fact 的唯一性边界：唯一性只约束原始事件，
不阻止带 `replay_of_event_id` 的追加修复事件。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
完整生产能力和 [`../../docs/architecture.md`](../../docs/architecture.md) 中的不变量一致，
不受 SQLite Lite 能力子集限制。两边共有的领域对象保持同名同义即可，不要求与
[`../sqlite/`](../sqlite/) 迁移一一对应。已经发布的迁移文件禁止修改；结构修正应通过
新的追加迁移完成。

# PostgreSQL 迁移

PostgreSQL 迁移必须保持只追加。当前 `000001` 至 `000015` 已按 owner 追加核心身份、
拍卖、竞价、代理、结算/履约、生命周期、身份/外部命令、恢复/可观测性和论坛同步对象；
它们仍是迁移中的目标 schema，不等于旧系统全部行为已经完成。旧名
`bid_proxies` 由末尾只读兼容视图提供，权威写入只进入新授权与历史表。
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

Auction-BF v28 的接管不伪装成一条普通追加迁移。专用 bridge 位于
`auction-storage/src/database/legacy_v28/`，只在精确旧库预检、已验证备份、零连接和显式确认后
建立 SQLx 等价基线，再继续执行本目录尚未覆盖的前向迁移。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
完整生产能力和 [`../../docs/architecture.md`](../../docs/architecture.md) 中的不变量一致，
不受 SQLite Lite 能力子集限制。两边共有的领域对象保持同名同义即可，不要求与
[`../sqlite/`](../sqlite/) 迁移一一对应。已经发布的迁移文件禁止修改；结构修正应通过
新的追加迁移完成。

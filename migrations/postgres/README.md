# PostgreSQL 迁移

PostgreSQL 迁移必须保持只追加。当前 `000001` 至 `000020` 已按 owner 追加核心身份、
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

`000005` 与 `000015` 的历史文件字节和 SQLx checksum 保持不可变。两者曾使用 PostgreSQL
保留字作为未加引号的查询别名；`auction-storage` 的迁移执行视图只对这两个固定版本、描述和
精确片段执行计数受限的别名替换。fresh 数据库继续记录原始 checksum；曾运行早期别名修正版的
数据库只在既有值精确等于同一固定替换后 SQL 的 SHA-384 时沿用该值，既不改写历史也不接受第三种
迁移身份。任何其它历史 SQL 漂移仍由 SQLx 失败关闭。

Auction-BF v28 的接管不伪装成一条普通追加迁移。专用 bridge 位于
`auction-storage/src/database/legacy_v28/`，只在精确旧库预检、已验证备份、零连接和显式确认后
建立 SQLx 等价基线，再继续执行本目录尚未覆盖的前向迁移。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
完整生产能力和 [`../../docs/architecture.md`](../../docs/architecture.md) 中的不变量一致，
不受 SQLite Lite 能力子集限制。两边共有的领域对象保持同名同义即可，不要求与
[`../sqlite/`](../sqlite/) 迁移一一对应。已经发布的迁移文件禁止修改；结构修正应通过
新的追加迁移完成。

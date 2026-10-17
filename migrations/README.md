# 数据库迁移

[文档导航](../docs/README.md) · [PostgreSQL 文件索引](postgres/README.md) · [执行 owner](../crates/auction-storage/src/database/migrations)

当前只执行 PostgreSQL `000001`–`000045`。迁移只追加，已发布 SQL 和 checksum 不可修改或删除；
退役后端原件仅作历史保留，不注册、不嵌入、不执行。
表与写入归属见[对象契约](../docs/contracts/schema.md)，业务及历史修正见其[迁移章节](../docs/contracts/schema.md#追加迁移与历史事实)。

`000042` 扩展 users 账号警告偏好、统一跨渠道通知来源及独立 PM 队列；不重写历史 MM payload/digest，
只在受控停机门禁和备份确认后执行，不允许新旧通知 writer 混跑。本轮未执行迁移。
`000043` 仅为 forum owner 登记终态刷新纳入及完成证明；升级前 settled 主题明确豁免且完成版本保持 NULL。
`000044` 为用户时间顺序增加约束，仅更正可证明的 MM 注册/论坛同步时间倒退并追加审计，不重写创建时间、凭据或旧命令。
`000045` 增加 nullable 邮件预期完整玩家名称，不回填旧凭据、不生成身份已验证状态或内部履约证明。
`000041`—`000045` 均须使用内嵌最新迁移的 auctionctl 走受控停机升级，普通重启不执行迁移。
草稿软截止修正是[显式单目标命令](../apps/auctionctl/README.md#草稿软截止显式修正)，不放入自动迁移；本轮无 `000046`/`000047`。

## 执行边界

- 分体：受保护的 `auctionctl migrate`，或 `deployment start/restart` 严格停机后迁移。
- 合并升级：停止 `auction-server` 后同账号执行 `migrate-managed --config-dir <绝对目录>`，共享锁证明已停止。
- 普通启动不隐式迁移；网页明确确认后的首次空库初始化不用于旧库接管。
- 普通迁移与首次初始化复用[历史 checksum 兼容](postgres/README.md#历史-checksum-兼容)，在原迁移锁内执行。失败保留现场，不改历史身份。

权限、备份、精确确认及恢复步骤见[部署手册](../ops/README.md)和[CLI 手册](../apps/auctionctl/README.md)。
默认仅静态检查，不执行迁移或行为测试；运行证据另记[验证状态](../docs/status.md)。

# 数据库迁移

[文档导航](../docs/README.md) · [PostgreSQL 文件索引](postgres/README.md) · [执行 owner](../crates/auction-storage/src/database/migrations)

当前只执行 PostgreSQL `000001`–`000039`。迁移只追加，已发布 SQL 和 checksum 不可修改或删除；
退役后端原件仅作历史保留，不注册、不嵌入、不执行。
表与写入归属见[对象契约](../docs/contracts/schema.md)，业务及历史修正见其[迁移章节](../docs/contracts/schema.md#追加迁移与历史事实)。

## 执行边界

- 分体：受保护的 `auctionctl migrate`，或 `deployment start/restart` 严格停机后迁移。
- 合并升级：停止 `auction-server` 后同账号执行 `migrate-managed --config-dir <绝对目录>`，共享锁证明已停止。
- 普通启动不隐式迁移；网页明确确认后的首次空库初始化不用于旧库接管。
- 普通迁移与首次初始化复用[历史 checksum 兼容](postgres/README.md#历史-checksum-兼容)，在原迁移锁内执行。失败保留现场，不改历史身份。

权限、备份、精确确认及恢复步骤见[部署手册](../ops/deploy/README.md)和[CLI 手册](../apps/auctionctl/README.md)。
默认仅静态检查，不执行迁移或行为测试；运行证据另记[验证状态](../docs/status.md)。

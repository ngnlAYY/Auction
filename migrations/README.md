# 数据库迁移导航

[文档导航](../docs/README.md) · [PostgreSQL 迁移](postgres/README.md) · [SQLite 迁移](sqlite/README.md) · [迁移执行 owner](../crates/auction-storage/src/database/migrations)

迁移只追加，已发布文件及 checksum 不可修改；两个后端共享领域语义，不要求序号或能力对等。

## 完整档与 Lite

| 目录 | 当前范围 |
| --- | --- |
| [postgres](postgres/README.md) | `000001`–`000039`，完整生产 schema |
| [sqlite](sqlite/README.md) | `000001`–`000013`，明确声明的 Lite 子集 |

共有对象保持同名同义，新增能力先登记[架构档位](../docs/architecture.md#部署档位与能力)和[对象 owner](../docs/contracts/schema.md)。
SQL、连接池、事务与执行器由 `auction-storage` 拥有；外部 schema 不得未经审计整体复制。
各目录保留文件索引，逐项业务语义统一见[迁移与历史事实契约](../docs/contracts/schema.md#追加迁移与历史事实)。

## 迁移执行边界

- 分体：受保护的 `auctionctl migrate` 独立迁移，或 `deployment start/restart` 在严格停机门禁后迁移再启动。
- 合并升级：停止 `auction-server`，同账号执行 `migrate-managed --config-dir <绝对目录>`，使用 supervisor/子进程共享锁证明已停止。
- 普通角色和合并启动不隐式迁移；网页明确确认后，合并入口才可进入专用首次空库初始化，不用于旧库接管。

PostgreSQL 普通迁移与首次初始化复用受限[历史 checksum 兼容](postgres/README.md#历史-checksum-兼容)，
在原迁移锁下执行；不能直接编辑旧 SQL，失败保留现场，不自动回滚或更改历史身份。
参数、权限、备份及恢复见[部署手册](../ops/deploy/README.md)和[CLI 手册](../apps/auctionctl/README.md)。

## 验证边界

默认只做文档、结构、编译和静态检查，不运行迁移或行为测试；schema 存在不能证明产品流程已验收。
授权后的运行证据仍须单独登记在[验证状态](../docs/status.md)，SQLite 验证不能替代 PostgreSQL 隔离与行锁语义。

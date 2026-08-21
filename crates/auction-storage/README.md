# auction-storage

持久化适配器边界：PostgreSQL 完整后端与 SQLite Lite 后端。

`database_url` 的 scheme 决定实际后端，也决定部署档位：

| scheme | 档位 | 能力 |
| --- | --- | --- |
| `postgres://` / `postgresql://` | 完整生产档 | 完整持久化能力、行锁、并发写入、多进程 |
| `sqlite:` | Lite 档 | 单主机、每角色一个实例、Worker 并发为一 |

## Cargo feature

默认不带任何数据库驱动：

- `postgres` → `sqlx/postgres` + rustls
- `sqlite` → `sqlx/sqlite`

发布制品只打开实际使用的后端。CI 分别检查两条路径，并用 `--all-features` 检查组合图。

## 边界约束

SQL 模型、查询、连接池、事务和后端锁策略只放在本 crate；业务决定留在领域/应用层。

不使用 `sqlx::Any` 抹平后端差异——锁、事务、能力缺失和迁移必须显式可审查。PostgreSQL 使用
行锁；SQLite 使用明确的串行写事务，不能假设存在 `FOR UPDATE`。

「轻量」只缩减系统能力，不放宽已声明支持功能的数据正确性：Lite 已提供的竞价写入仍必须保证
`command_id` 幂等、事务原子性、审计追加和凭据安全。

## 迁移

`migrations/postgres/` 维护完整生产 schema，`migrations/sqlite/` 只维护 Lite 能力子集。
两个迁移树不要求一一对应，共有对象必须同名同义。迁移只允许追加，已发布文件禁止修改。

新增持久化能力前必须在 `docs/architecture.md` 标明它属于「完整档」「Lite 档」还是「两者」。

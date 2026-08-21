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

## SQLite 连接选项

文件数据库启用 WAL：读事务不阻塞单个写事务，符合 Lite 档「低并发但仍要可用」的定位。
写入串行由 SQLite 自身的写锁保证，`busy_timeout`（5 秒）决定等锁而不是立刻返回
`SQLITE_BUSY`。文件库连接上限为 5（多读 + 一写）。

`create_if_missing(true)`：Lite 档面向本地与单主机部署，数据库文件不存在时创建而不是启动
失败。SQLx 的默认值是 `false`，因此这一项必须显式设置。

`foreign_keys(true)`：SQLite 的外键约束默认关闭，不显式开启会让 Lite 档静默接受违反引用
完整性的写入。

内存数据库（`:memory:` 或 `mode=memory`）不落盘，WAL 无意义，且**必须保持单连接**——每个新
连接都会得到独立的空库，池化会让数据看起来随机消失。

「轻量」只缩减系统能力，不放宽已声明支持功能的数据正确性：Lite 已提供的竞价写入仍必须保证
`command_id` 幂等、事务原子性、审计追加和凭据安全。

## 迁移

`migrations/postgres/` 维护完整生产 schema，`migrations/sqlite/` 只维护 Lite 能力子集。
两个迁移树不要求一一对应，共有对象必须同名同义。迁移只允许追加，已发布文件禁止修改。

新增持久化能力前必须在 `docs/architecture.md` 标明它属于「完整档」「Lite 档」还是「两者」。

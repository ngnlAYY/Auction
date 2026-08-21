# PostgreSQL 迁移

PostgreSQL 迁移必须保持只追加。第一份结构迁移会在完成 Auction-BF 契约和模块所有权评审
后添加。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
完整生产能力和 [`../../docs/architecture.md`](../../docs/architecture.md) 中的不变量一致，
不受 SQLite Lite 能力子集限制。两边共有的领域对象保持同名同义即可，不要求与
[`../sqlite/`](../sqlite/) 迁移一一对应。已经发布的迁移文件禁止修改；结构修正应通过
新的追加迁移完成。

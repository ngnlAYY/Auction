# PostgreSQL 迁移

PostgreSQL 迁移必须保持只追加。第一份结构迁移会在完成 Auction-BF 契约和模块所有权评审
后添加。

迁移脚本可以使用 PostgreSQL 专属的 SQL、索引和行锁语义，但必须与
[`../sqlite/`](../sqlite/) 目录保持同一套逻辑结构。已经发布的迁移文件禁止修改；结构修
正应通过新的追加迁移完成。

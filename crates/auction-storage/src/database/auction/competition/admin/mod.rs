//! PostgreSQL 管理端拍卖与 lot 事务子模块；事务边界由父模块统一装配。

mod create;
mod lot;
mod row_mapping;
mod schedule;
mod world;

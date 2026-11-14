//! PostgreSQL 管理端拍卖与标的事务子模块装配；事务边界仍由父模块统一装配。

mod create;
mod lot;
mod row_mapping;
mod schedule;
mod world;

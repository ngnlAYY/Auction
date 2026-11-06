//! PostgreSQL 管理端拍卖与标的事务子模块装配；事务边界仍由父模块统一装配。

use super::mapping::*;

#[path = "admin/create.rs"]
mod create;
#[path = "admin/lot/mod.rs"]
mod lot;
#[path = "admin/mapping.rs"]
mod row_mapping;
#[path = "admin/schedule.rs"]
mod schedule;
#[path = "admin/world.rs"]
mod world;

use row_mapping::*;

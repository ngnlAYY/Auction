//! PostgreSQL 管理端拍卖与标的事务子模块装配。
//!
//! 拍卖创建、计划修改、lot 更新、业务世界启停和行映射分别由子模块持有；事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

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

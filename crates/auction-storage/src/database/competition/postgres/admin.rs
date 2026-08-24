//! PostgreSQL 管理端拍卖与标的事务子模块装配。
//!
//! 拍卖创建、lot 更新和行映射分别由子模块持有；事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

#[path = "admin/create.rs"]
mod create;
#[path = "admin/lot.rs"]
mod lot;
#[path = "admin/mapping.rs"]
mod row_mapping;

use row_mapping::*;

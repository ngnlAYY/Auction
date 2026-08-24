//! SQLite Lite Users/Admin 存储实现。
//!
//! 具体职责按 SQLite 写事务、查询读取和共享映射子模块拆分；SQLite 的串行写事务与
//! PostgreSQL 的 Serializable 事务保持各自后端语义，不通过统一抽象隐藏差异。

use super::*;

mod commands;
mod mapping;
mod reads;

pub(super) use commands::{create_sqlite, mutate_sqlite, revert_sqlite};
pub(super) use reads::{find_command_sqlite, list_sqlite};

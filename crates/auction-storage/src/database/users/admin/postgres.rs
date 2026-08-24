//! PostgreSQL Users/Admin 存储实现。
//!
//! 具体职责按 PostgreSQL 事务边界拆分到命令写入、查询读取和共享映射子模块；本文件只负责
//! 子模块装配与稳定端口导出。

use super::*;

mod commands;
mod mapping;
mod reads;

pub(super) use commands::{
    create_postgres, ensure_recovery_blacklist_postgres, mutate_postgres, revert_postgres,
};
pub(super) use reads::{find_command_postgres, list_postgres};

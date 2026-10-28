//! PostgreSQL Users/Admin 存储实现；职责按事务边界拆分到命令写入、查询读取和共享映射子模块。

use super::*;
use crate::database::users::envelope::upsert_login_key_envelope;

mod commands;
mod mapping;
mod reads;

pub(super) use commands::{
    create_postgres, ensure_recovery_blacklist_postgres, mutate_postgres, revert_postgres,
};
pub(super) use reads::{find_command_postgres, list_postgres};

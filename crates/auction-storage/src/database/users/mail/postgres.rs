//! PostgreSQL Users/MoogleMail 适配器装配；命令写入、幂等读取和安全行映射由子模块拥有。

use super::*;

#[path = "postgres/commands.rs"]
mod commands;
#[path = "postgres/mapping.rs"]
mod mapping;
#[path = "postgres/reads.rs"]
mod reads;

pub(super) use commands::{rebind_postgres, register_postgres};
pub(super) use reads::{
    find_mail_command_postgres, find_registration_user_id, load_envelope_postgres,
};

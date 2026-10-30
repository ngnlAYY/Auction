//! PostgreSQL Users/MoogleMail 适配器装配；命令写入、幂等读取和安全行映射由子模块拥有。

use super::*;
use crate::database::users::envelope::upsert_login_key_envelope;

mod commands;
mod mapping;
mod reads;

pub(super) use commands::{rebind_postgres, register_postgres};
pub(super) use reads::{
    find_mail_command_postgres, find_registration_user_id, load_envelope_postgres,
};

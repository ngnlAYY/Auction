//! Admin 邮件轮询状态与入站审查端口装配。
//!
//! 状态/入站投影、命令、错误和事务/查询端口分别由子模块拥有；这里保留原有
//! `auction_application::ports::*` 公共导出路径。

mod commands;
mod common;
mod errors;
mod model;
mod ports;

pub use commands::*;
pub use common::{
    MAX_ADMIN_MAIL_DECISION_BYTES, MAX_ADMIN_MAIL_OFFSET, MAX_ADMIN_MAIL_PAGE_SIZE,
    MAX_ADMIN_MAIL_POLLER_COUNT, MAX_ADMIN_MAILBOX_KEY_BYTES, admin_mail_aggregate_id,
};
pub use errors::*;
pub use model::*;
pub use ports::*;

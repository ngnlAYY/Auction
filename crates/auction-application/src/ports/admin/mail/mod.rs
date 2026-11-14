//! Admin 邮件轮询状态与入站审查端口装配。

mod commands;
mod errors;
mod limits;
mod model;
mod ports;

pub use commands::*;
pub use errors::*;
pub use limits::{
    MAX_ADMIN_MAIL_DECISION_BYTES, MAX_ADMIN_MAIL_OFFSET, MAX_ADMIN_MAIL_PAGE_SIZE,
    MAX_ADMIN_MAIL_POLLER_COUNT, MAX_ADMIN_MAILBOX_KEY_BYTES, admin_mail_aggregate_id,
};
pub use model::*;
pub use ports::*;

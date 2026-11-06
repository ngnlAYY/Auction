//! Admin 邮件轮询状态与入站审查页面装配。

mod actions;
mod deliveries;
mod detail;
mod helpers;
mod messages;
mod page;
mod progress;
mod table;

pub(super) use page::page;

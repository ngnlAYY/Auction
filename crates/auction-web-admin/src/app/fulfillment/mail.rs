//! Admin 邮件轮询状态与入站审查页面装配。

#[path = "mail/actions.rs"]
mod actions;
#[path = "mail/detail.rs"]
mod detail;
#[path = "mail/helpers.rs"]
mod helpers;
#[path = "mail/page.rs"]
mod page;
#[path = "mail/table.rs"]
mod table;

pub(super) use page::page;

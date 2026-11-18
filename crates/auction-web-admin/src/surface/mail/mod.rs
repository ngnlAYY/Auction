//! Admin 邮件轮询状态与入站审查页面装配。

mod actions;
mod deliveries;
mod detail;
mod labels;
mod messages;
mod page;
mod progress;
mod table;

pub(super) use page::page;

pub(super) const MAILBOX_ENTITY_PREFIX: &str = "mailbox:";

/// 仪表盘和邮件页的暂停及恢复按同一个邮箱互斥。
pub(super) fn mailbox_command_entity(mailbox_key: &str) -> String {
    format!("{MAILBOX_ENTITY_PREFIX}{mailbox_key}")
}

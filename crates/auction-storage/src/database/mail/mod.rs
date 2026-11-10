//! MoogleMail 收件、投递 outbox、扫描检查点与邮箱维护的 owner 分组。

pub(crate) mod player_notifications;

pub(crate) mod delivery;
pub(crate) mod intake;
pub(crate) mod mailbox_sweep;
pub(crate) mod poll;
pub(crate) mod scan_checkpoint;

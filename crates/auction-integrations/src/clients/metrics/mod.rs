//! 外部客户端指标的同步 guard 与闭集映射；不参与请求控制流。

mod forum;
mod mail;
mod mapping;
mod observation;

pub(crate) use forum::{
    ForumGateWaitGuard, ForumPmOperationGuard, ForumRequestGuard, ForumTopicScanGuard,
    ForumWriteUnknownGuard, forum_signal, topic_scan_discovered,
};
pub(crate) use mail::{MailGateWaitGuard, MailLockWaitGuard, MailRequestGuard, mail_signal};
pub(crate) use mapping::{
    forum_operation, forum_pm_category, forum_pm_request_result, forum_result, forum_world,
    mail_result, mail_world,
};

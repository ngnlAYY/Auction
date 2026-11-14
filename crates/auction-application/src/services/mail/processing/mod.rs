//! MoogleMail 业务处理与拍卖入站用例。
//! 业务处理先恢复已提交的拍卖接受决定；新邮件才进行外部装备读取、安全校验和拍卖录入，
//! 最后为领取/退回/通知安排 delivery outbox。拍卖事实与密钥信封由单独的 owner 事务提交；
//! 重复处理恢复原投稿资产并补齐同一领取意图，不根据变化后的远端页面反向退回。

mod commands;
mod intake;
mod pricing;
mod processor;
mod receipt;

pub use intake::MailAuctionIntakeService;
pub use processor::{MailProcessorConfig, MailProcessorService};
pub use receipt::{
    MAX_SUBMISSION_RECEIPT_BODY_BYTES, SUBMISSION_RECEIPT_SUBJECT, submission_receipt_body,
    submission_receipt_invalid_reason,
};

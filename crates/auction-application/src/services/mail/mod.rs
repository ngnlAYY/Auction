//! mail 上下文。

pub(crate) mod maintenance;
pub(crate) mod processing;
pub(crate) mod user;

pub use maintenance::{MailboxMaintenanceReport, MailboxMaintenanceService};
pub use processing::{
    MAX_SUBMISSION_RECEIPT_BODY_BYTES, MailAuctionIntakeService, MailProcessorConfig,
    MailProcessorService, MailReplyPolicy, MailReplyPolicyHandle, SUBMISSION_RECEIPT_SUBJECT,
    submission_receipt_body, submission_receipt_invalid_reason,
};
pub use user::MailUserService;

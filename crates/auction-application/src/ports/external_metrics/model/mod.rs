//! W0.4/W0.5 外部客户端指标的稳定闭集与事件。

mod event;
mod forum;
mod mail;
mod pm;
mod reply_delay;
mod topic;
mod wait;
mod world;

pub use event::{ExternalMetricsEvent, MailMaintenanceOutcome};
pub use forum::{ForumBudgetPhase, ForumOperation, ForumRequestCategory, ForumSignal};
pub use mail::{
    MailBudget, MailNoticeClass, MailOperation, MailPhase, MailRequestCategory, MailSignal,
};
pub use pm::{ForumPmCategory, ForumPmOutcome, ForumPmPhase};
pub use reply_delay::{
    ReplyDelayBound, ReplyDelayMethod, ReplyDelayPhase, ReplyDelaySampleOutcome,
};
pub use topic::{ForumTopicScanMeasure, ForumTopicScanMode, ForumTopicScanOutcome};
pub use wait::{MailLock, MailLockOutcome, WaitOutcome};
pub use world::{ForumWorld, MailWorld};

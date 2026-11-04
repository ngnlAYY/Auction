//! 外部账号客户端的闭集观测端口；不承载重试、凭据或业务裁决。

mod model;
mod observer;

pub use model::{
    ExternalMetricsEvent, ForumBudgetPhase, ForumOperation, ForumPmCategory, ForumPmOutcome,
    ForumPmPhase, ForumRequestCategory, ForumSignal, ForumTopicScanMeasure, ForumTopicScanMode,
    ForumTopicScanOutcome, ForumWorld, MailBudget, MailLock, MailLockOutcome, MailNoticeClass,
    MailOperation, MailPhase, MailRequestCategory, MailSignal, MailWorld, ReplyDelayBound,
    ReplyDelayMethod, ReplyDelayPhase, ReplyDelaySampleOutcome, WaitOutcome,
};
pub use observer::{ExternalMetricsObserver, noop_external_metrics_observer};

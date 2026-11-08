//! external_metrics 上下文公开 API。

pub use crate::ports::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumBudgetPhase, ForumOperation,
    ForumPmCategory, ForumPmOutcome, ForumPmPhase, ForumRequestCategory, ForumSignal,
    ForumTopicScanMeasure, ForumTopicScanMode, ForumTopicScanOutcome, ForumWorld, MailBudget,
    MailLock, MailLockOutcome, MailMaintenanceOutcome, MailNoticeClass, MailOperation, MailPhase,
    MailRequestCategory, MailSignal, MailWorld, ReplyDelayBound, ReplyDelayMethod, ReplyDelayPhase,
    ReplyDelaySampleOutcome, WaitOutcome, noop_external_metrics_observer,
};

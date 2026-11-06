//! notification_metrics 上下文公开 API。

pub use crate::ports::notification_metrics::{
    NotificationFailure, NotificationKind, NotificationRecorder, NotificationStage,
    noop_notification_recorder,
};

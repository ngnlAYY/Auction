//! 隔离指标 observer 的 panic，避免观测影响请求控制流。
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use auction_application::external_metrics::{ExternalMetricsEvent, ExternalMetricsObserver};

pub(super) fn observe_safely(
    observer: &Arc<dyn ExternalMetricsObserver>,
    event: ExternalMetricsEvent,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| observer.observe(event)));
}

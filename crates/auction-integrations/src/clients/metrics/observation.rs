//! 隔离指标 observer 的 panic，避免观测影响请求控制流。
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use auction_application::external_metrics::{ExternalMetricsEvent, ExternalMetricsObserver};

/// 持有 `Arc<dyn …>` 的观测点：observer panic 时只丢弃该次观测。
pub(super) fn observe_safely(
    observer: &Arc<dyn ExternalMetricsObserver>,
    event: ExternalMetricsEvent,
) {
    observe_dyn_safely(observer.as_ref(), event);
}

/// 已持有 `&dyn` 的观测点（信号转发等）复用同一 panic 隔离。
pub(super) fn observe_dyn_safely(
    observer: &dyn ExternalMetricsObserver,
    event: ExternalMetricsEvent,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| observer.observe(event)));
}

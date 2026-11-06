//! 同步指标观察接口；适配器不得在此执行 I/O 或等待。

use std::{fmt, sync::Arc};

use crate::ports::external_metrics::ExternalMetricsEvent;

/// 外部客户端指标端口；实现必须同步、无 I/O，且不能改变调用结果。
pub trait ExternalMetricsObserver: Send + Sync {
    fn observe(&self, event: ExternalMetricsEvent);
}

struct NoopExternalMetricsObserver;

impl ExternalMetricsObserver for NoopExternalMetricsObserver {
    fn observe(&self, _event: ExternalMetricsEvent) {}
}

impl fmt::Debug for NoopExternalMetricsObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NoopExternalMetricsObserver")
    }
}

/// 关闭指标或非 Worker 角色使用同一 no-op，保持所有调用点与分支不变。
pub fn noop_external_metrics_observer() -> Arc<dyn ExternalMetricsObserver> {
    Arc::new(NoopExternalMetricsObserver)
}

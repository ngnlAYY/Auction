//! HTTP readiness 检查与缓存的低基数记录器。

use std::{sync::Arc, time::Duration};

use prometheus::{Histogram, HistogramOpts, IntCounterVec, Opts, Registry};

use super::types::MetricsError;

/// HTTP 探针请求使用缓存的方式。
#[derive(Clone, Copy)]
pub enum ReadinessCacheOutcome {
    Hit,
    Refresh,
    Coalesced,
    Bypass,
}

impl ReadinessCacheOutcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Refresh => "refresh",
            Self::Coalesced => "coalesced",
            Self::Bypass => "bypass",
        }
    }
}

/// 一轮底层就绪检查的结果；超时与异常都返回未就绪。
#[derive(Clone, Copy)]
pub enum ReadinessCheckResult {
    Ready,
    NotReady,
    Timeout,
}

impl ReadinessCheckResult {
    const fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NotReady => "not_ready",
            Self::Timeout => "timeout",
        }
    }
}

/// 只接受闭集 label 的记录端口；关闭指标时不影响检查或缓存。
pub trait ReadinessRecorder: Send + Sync {
    fn cache_request(&self, outcome: ReadinessCacheOutcome);
    fn check_finished(&self, result: ReadinessCheckResult, elapsed: Duration);
}

struct PrometheusReadinessRecorder {
    requests: IntCounterVec,
    checks: IntCounterVec,
    duration: Histogram,
}

impl ReadinessRecorder for PrometheusReadinessRecorder {
    fn cache_request(&self, outcome: ReadinessCacheOutcome) {
        self.requests.with_label_values(&[outcome.label()]).inc();
    }

    fn check_finished(&self, result: ReadinessCheckResult, elapsed: Duration) {
        self.checks.with_label_values(&[result.label()]).inc();
        self.duration.observe(elapsed.as_secs_f64());
    }
}

pub(super) fn build(registry: &Registry) -> Result<Arc<dyn ReadinessRecorder>, MetricsError> {
    let requests = IntCounterVec::new(
        Opts::new(
            "auction_readiness_cache_requests_total",
            "Total HTTP readiness requests by cache use.",
        ),
        &["outcome"],
    )
    .map_err(MetricsError::Registry)?;
    let checks = IntCounterVec::new(
        Opts::new(
            "auction_readiness_checks_total",
            "Total underlying HTTP readiness checks, excluding cache hits and coalesced waiters.",
        ),
        &["result"],
    )
    .map_err(MetricsError::Registry)?;
    let duration = Histogram::with_opts(
        HistogramOpts::new(
            "auction_readiness_check_duration_seconds",
            "Duration of underlying HTTP readiness checks.",
        )
        .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 0.75, 1.0, 5.0, 15.0]),
    )
    .map_err(MetricsError::Registry)?;
    for outcome in [
        ReadinessCacheOutcome::Hit,
        ReadinessCacheOutcome::Refresh,
        ReadinessCacheOutcome::Coalesced,
        ReadinessCacheOutcome::Bypass,
    ] {
        requests.with_label_values(&[outcome.label()]);
    }
    for result in
        [ReadinessCheckResult::Ready, ReadinessCheckResult::NotReady, ReadinessCheckResult::Timeout]
    {
        checks.with_label_values(&[result.label()]);
    }
    registry.register(Box::new(requests.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(checks.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(duration.clone())).map_err(MetricsError::Registry)?;
    Ok(Arc::new(PrometheusReadinessRecorder { requests, checks, duration }))
}

struct NoopReadinessRecorder;

impl ReadinessRecorder for NoopReadinessRecorder {
    fn cache_request(&self, _outcome: ReadinessCacheOutcome) {}
    fn check_finished(&self, _result: ReadinessCheckResult, _elapsed: Duration) {}
}

pub(super) fn noop() -> Arc<dyn ReadinessRecorder> {
    Arc::new(NoopReadinessRecorder)
}

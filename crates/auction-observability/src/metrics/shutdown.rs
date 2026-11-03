//! 排空指标仅接受编译期闭集；不携带业务身份、凭据或错误原文。
use super::types::MetricsError;
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry};
use std::{sync::Arc, time::Duration};

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        /// 排空观测使用的编译期闭集。
        #[derive(Clone, Copy, Debug)]
        pub enum $name { $($variant),+ }
        impl $name { const ALL: &'static [Self] = &[$(Self::$variant),+]; pub const fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } } }
    };
}
labels!(ShutdownRole { Public => "public", Admin => "admin", Worker => "worker" });
labels!(ShutdownReason { Signal => "signal", RuntimeFailure => "runtime_failure", ServerStopped => "server_stopped" });
labels!(ShutdownOutcome { Completed => "completed", TimedOut => "timed_out", Interrupted => "interrupted", Failed => "failed" });
labels!(ShutdownPhase { Critical => "critical", Drain => "drain" });
labels!(ShutdownCriticalKind { Background => "background", DeliverySend => "delivery_send", DeliveryTake => "delivery_take", DeliveryReturn => "delivery_return", ComposeDiscard => "compose_discard", Forum => "forum", ForumPm => "forum_pm" });

/// 所有 label 均由枚举产生；关闭 exporter 时仍可使用同一端口。
pub trait ShutdownRecorder: Send + Sync {
    fn critical_entered(&self, kind: ShutdownCriticalKind);
    fn critical_released(&self, kind: ShutdownCriticalKind);
    fn finished(&self, role: ShutdownRole, reason: ShutdownReason, outcome: ShutdownOutcome);
    fn phase_finished(&self, role: ShutdownRole, phase: ShutdownPhase, elapsed: Duration);
}
struct Recorder {
    inflight: IntGaugeVec,
    total: IntCounterVec,
    duration: HistogramVec,
}
impl ShutdownRecorder for Recorder {
    fn critical_entered(&self, kind: ShutdownCriticalKind) {
        self.inflight.with_label_values(&[kind.label()]).inc();
    }
    fn critical_released(&self, kind: ShutdownCriticalKind) {
        self.inflight.with_label_values(&[kind.label()]).dec();
    }
    fn finished(&self, role: ShutdownRole, reason: ShutdownReason, outcome: ShutdownOutcome) {
        self.total.with_label_values(&[role.label(), reason.label(), outcome.label()]).inc();
    }
    fn phase_finished(&self, role: ShutdownRole, phase: ShutdownPhase, elapsed: Duration) {
        self.duration
            .with_label_values(&[role.label(), phase.label()])
            .observe(elapsed.as_secs_f64());
    }
}
pub(super) fn build(registry: &Registry) -> Result<Arc<dyn ShutdownRecorder>, MetricsError> {
    let inflight = IntGaugeVec::new(
        Opts::new(
            "auction_shutdown_critical_inflight",
            "Active fence handoffs and external write sections, including result persistence.",
        ),
        &["kind"],
    )
    .map_err(MetricsError::Registry)?;
    let total = IntCounterVec::new(
        Opts::new("auction_shutdown_total", "Shutdown outcomes by role and trigger."),
        &["role", "reason", "outcome"],
    )
    .map_err(MetricsError::Registry)?;
    let duration = HistogramVec::new(
        HistogramOpts::new(
            "auction_shutdown_duration_seconds",
            "Time spent in each bounded shutdown phase.",
        )
        .buckets(vec![0.01, 0.1, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]),
        &["role", "phase"],
    )
    .map_err(MetricsError::Registry)?;
    // 先提供稳定零值，首个关键段或停机发生前也可观察，不依赖第一次事件创建序列。
    for kind in ShutdownCriticalKind::ALL {
        inflight.with_label_values(&[kind.label()]);
    }
    for role in ShutdownRole::ALL {
        for phase in ShutdownPhase::ALL {
            duration.with_label_values(&[role.label(), phase.label()]);
        }
        for reason in ShutdownReason::ALL {
            for outcome in ShutdownOutcome::ALL {
                total.with_label_values(&[role.label(), reason.label(), outcome.label()]);
            }
        }
    }
    registry.register(Box::new(inflight.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(total.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(duration.clone())).map_err(MetricsError::Registry)?;
    Ok(Arc::new(Recorder { inflight, total, duration }))
}
struct Noop;
impl ShutdownRecorder for Noop {
    fn critical_entered(&self, _kind: ShutdownCriticalKind) {}
    fn critical_released(&self, _kind: ShutdownCriticalKind) {}
    fn finished(&self, _role: ShutdownRole, _reason: ShutdownReason, _outcome: ShutdownOutcome) {}
    fn phase_finished(&self, _role: ShutdownRole, _phase: ShutdownPhase, _elapsed: Duration) {}
}
pub(super) fn noop() -> Arc<dyn ShutdownRecorder> {
    Arc::new(Noop)
}

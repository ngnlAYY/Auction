//! tracing exporter 的进程内健康状态。

use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};

use chrono::{DateTime, Utc};

use crate::health::{TracingHealth, TracingHealthReader, TracingStatus};

#[derive(Debug, Clone, Copy)]
pub(crate) enum FailureCode {
    None = 0,
    Transport = 1,
    Shutdown = 2,
}

impl FailureCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Transport => "transport",
            Self::Shutdown => "shutdown",
        }
    }
}

/// tracing exporter 的进程内安全状态；只保存可进入心跳的计数与闭集字段。
#[derive(Debug)]
pub(crate) struct TracingHealthState {
    pub(crate) enabled: bool,
    pub(crate) status: AtomicU8,
    pub(crate) dropped_span_count: AtomicI64,
    pub(crate) export_failure_count: AtomicI64,
    pub(crate) last_success_micros: AtomicI64,
    pub(crate) last_failure_micros: AtomicI64,
    pub(crate) last_failure_code: AtomicU8,
}

impl TracingHealthState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            status: AtomicU8::new(if enabled { 1 } else { 0 }),
            dropped_span_count: AtomicI64::new(0),
            export_failure_count: AtomicI64::new(0),
            last_success_micros: AtomicI64::new(0),
            last_failure_micros: AtomicI64::new(0),
            last_failure_code: AtomicU8::new(FailureCode::None as u8),
        }
    }

    pub(crate) fn record_success(&self) {
        self.last_success_micros.store(Utc::now().timestamp_micros(), Ordering::Relaxed);
        self.last_failure_code.store(FailureCode::None as u8, Ordering::Relaxed);
        self.status.store(1, Ordering::Release);
    }

    pub(crate) fn record_failure(&self, code: FailureCode) {
        self.export_failure_count.fetch_add(1, Ordering::Relaxed);
        self.last_failure_micros.store(Utc::now().timestamp_micros(), Ordering::Relaxed);
        self.last_failure_code.store(code as u8, Ordering::Relaxed);
        self.status.store(2, Ordering::Release);
    }

    pub(crate) fn snapshot(&self) -> TracingHealth {
        let status = match self.status.load(Ordering::Acquire) {
            1 => TracingStatus::Ok,
            2 => TracingStatus::Degraded,
            _ => TracingStatus::Disabled,
        };
        let failure_code = match self.last_failure_code.load(Ordering::Relaxed) {
            value if value == FailureCode::Transport as u8 => FailureCode::Transport,
            value if value == FailureCode::Shutdown as u8 => FailureCode::Shutdown,
            _ => FailureCode::None,
        };
        TracingHealth {
            status,
            dropped_span_count: self.dropped_span_count.load(Ordering::Relaxed),
            export_failure_count: self.export_failure_count.load(Ordering::Relaxed),
            last_success_at: timestamp(self.last_success_micros.load(Ordering::Relaxed)),
            last_failure_at: timestamp(self.last_failure_micros.load(Ordering::Relaxed)),
            last_error_code: if self.enabled {
                failure_code.as_str().to_owned()
            } else {
                String::new()
            },
        }
    }
}

impl TracingHealthReader for TracingHealthState {
    fn snapshot(&self) -> TracingHealth {
        TracingHealthState::snapshot(self)
    }
}

fn timestamp(value: i64) -> Option<DateTime<Utc>> {
    (value != 0).then(|| DateTime::from_timestamp_micros(value)).flatten()
}

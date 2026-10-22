//! tracing exporter 的进程内健康状态。

use std::sync::{
    Mutex,
    atomic::{AtomicI64, Ordering},
};

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
    pub(crate) dropped_span_count: AtomicI64,
    export: Mutex<ExportHealth>,
}

/// 导出结果必须整体发布，避免心跳读到新失败计数与旧状态/时间的混合快照。
/// 锁只覆盖批次结果与快照；逐 span 的队列丢弃计数仍走独立原子路径。
#[derive(Debug)]
struct ExportHealth {
    status: TracingStatus,
    failure_count: i64,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    last_failure_code: FailureCode,
}

impl TracingHealthState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            dropped_span_count: AtomicI64::new(0),
            export: Mutex::new(ExportHealth {
                status: if enabled { TracingStatus::Ok } else { TracingStatus::Disabled },
                failure_count: 0,
                last_success_at: None,
                last_failure_at: None,
                last_failure_code: FailureCode::None,
            }),
        }
    }

    pub(crate) fn record_success(&self) {
        let mut export = self.export.lock().unwrap_or_else(|error| error.into_inner());
        export.last_success_at = Some(Utc::now());
        export.last_failure_code = FailureCode::None;
        export.status = TracingStatus::Ok;
    }

    pub(crate) fn record_failure(&self, code: FailureCode) {
        let mut export = self.export.lock().unwrap_or_else(|error| error.into_inner());
        export.failure_count = export.failure_count.saturating_add(1);
        export.last_failure_at = Some(Utc::now());
        export.last_failure_code = code;
        export.status = TracingStatus::Degraded;
    }

    pub(crate) fn snapshot(&self) -> TracingHealth {
        let export = self.export.lock().unwrap_or_else(|error| error.into_inner());
        TracingHealth {
            status: export.status,
            dropped_span_count: self.dropped_span_count.load(Ordering::Relaxed),
            export_failure_count: export.failure_count,
            last_success_at: export.last_success_at,
            last_failure_at: export.last_failure_at,
            last_error_code: export.last_failure_code.as_str().to_owned(),
        }
    }
}

impl TracingHealthReader for TracingHealthState {
    fn snapshot(&self) -> TracingHealth {
        TracingHealthState::snapshot(self)
    }
}

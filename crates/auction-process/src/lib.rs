//! 原生程序与首次设置服务共用的生命周期原语与本机 readiness 探针。

mod lifecycle;
mod readiness;

pub use lifecycle::{
    DEFAULT_DRAIN_TIMEOUT, DrainActivity, DrainActivityObserver, DrainCoordinator, DrainOutcome,
    TaskGuard, TerminationSignal, TerminationSignals, join_error_kind, task_error_kind,
    wait_for_drain,
};
pub use readiness::{CONTROL_READY_PATH, HTTP_READY_PATH, ReadinessTarget, probe_ready};

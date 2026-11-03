//! 原生程序与首次设置服务共用的生命周期原语。

mod lifecycle;

pub use lifecycle::{
    DEFAULT_DRAIN_TIMEOUT, DrainActivity, DrainActivityObserver, DrainCoordinator, DrainOutcome,
    TaskGuard, TerminationSignal, TerminationSignals, join_error_kind, task_error_kind,
    wait_for_drain,
};

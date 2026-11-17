//! 原生程序与首次设置服务共用的生命周期原语与本机 readiness 探针。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod lifecycle;
mod readiness;

pub use lifecycle::{
    DEFAULT_DRAIN_TIMEOUT, DrainActivity, DrainActivityObserver, DrainCoordinator, DrainOutcome,
    TaskGuard, TerminationSignal, TerminationSignals, join_error_kind, task_error_kind,
    wait_for_drain,
};
pub use readiness::{CONTROL_READY_PATH, HTTP_READY_PATH, ReadinessTarget, probe_ready};

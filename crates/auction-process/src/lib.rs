//! 三个可部署进程共用的生命周期原语。

mod lifecycle;

pub use lifecycle::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignal, TerminationSignals,
    wait_for_drain,
};

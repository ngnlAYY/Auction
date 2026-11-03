//! 邮箱维护的独立调度、完整 Read Mail 纯观察与阶段指标；恢复和外写证明仍由应用层持有。

mod metrics;
mod observation;
mod runner;
mod sweep;

pub(super) use observation::run_read_observation;
pub(crate) use runner::run_loop;
pub(super) use sweep::run_maintenance;

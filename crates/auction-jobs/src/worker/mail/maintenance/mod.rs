//! 邮箱维护的独立调度、完整 Read Mail 纯观察与阶段指标；恢复和外写证明仍由应用层持有。

mod metrics;
pub(in crate::worker::mail) mod observation;
pub(crate) mod quarantine;
mod runner;
pub(in crate::worker::mail) mod sweep;

pub(crate) use runner::run_loop;

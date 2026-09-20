//! Worker 角色的可复用装配入口；调度与生命周期仍由本进程拥有。

mod bootstrap;

pub use bootstrap::{run, run_from_path};

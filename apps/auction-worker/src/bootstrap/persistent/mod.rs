//! PostgreSQL 持久化 Worker 的装配与退出收口。

mod consignment;
mod heartbeat;
mod profile;
mod recovery;
mod run;
pub(super) use run::run_worker;

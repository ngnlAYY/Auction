//! 数据库冲突、连接生命周期与无监听任务归属；关闭时为 no-op，不参与业务判定。

use std::{fmt, sync::Arc};

mod collector;
mod connection;
mod labels;
mod transaction;
mod worker_scope;

pub use connection::DatabaseConnectionGuard;
pub use labels::{
    DatabaseAcquireOutcome, DatabaseConflictSite, DatabaseReservedPurpose,
    DatabaseTransactionOwner, DatabaseTransactionPhase,
};
pub use transaction::{
    database_transaction_attempt, database_transaction_exhausted, database_transaction_phase,
    record_database_conflict,
};
pub use worker_scope::{current_database_worker_loop, scope_database_worker_loop};

/// 与进程注册表共享的数据库记录器；不建立监听、任务或 SQL 查询。
#[derive(Clone, Default)]
pub struct DatabaseRecorder(Option<Arc<collector::DatabaseMetrics>>);

impl fmt::Debug for DatabaseRecorder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("DatabaseRecorder").finish_non_exhaustive()
    }
}

impl DatabaseRecorder {
    pub(super) fn build(
        registry: &prometheus::Registry,
    ) -> Result<Self, super::types::MetricsError> {
        Ok(Self(Some(Arc::new(collector::DatabaseMetrics::build(registry)?))))
    }
}

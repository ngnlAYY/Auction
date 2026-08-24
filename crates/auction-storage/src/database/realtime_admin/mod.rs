//! Admin realtime lag/failure 查询与 replay 持久化适配器。
//!
//! 本模块只负责端口校验与 PostgreSQL/SQLite 子适配器分派；查询投影、失败读取和 replay
//! 事务分别由同目录的具名模块持有，避免把两种后端和多类运维职责堆在单一入口文件中。

use auction_application::{
    ApplicationFuture, RealtimeAdminError, RealtimeAdminPort, RealtimeDashboardSnapshot,
    RealtimeFailureListRequest, RealtimeFailurePage, RealtimeFailureSnapshot,
    RealtimeLagListRequest, RealtimeLagPage, RealtimeReplayCommand, RealtimeReplayResult,
};
use uuid::Uuid;

use super::DatabaseStore;

mod common;
mod failures;
mod lag;
mod replay;
mod snapshot;

use common::admin_error;
#[cfg(feature = "postgres")]
use failures::{get_failure_postgres, list_failures_postgres};
#[cfg(feature = "sqlite")]
use failures::{get_failure_sqlite, list_failures_sqlite};
#[cfg(feature = "postgres")]
use lag::list_lag_postgres;
#[cfg(feature = "sqlite")]
use lag::list_lag_sqlite;
#[cfg(feature = "postgres")]
use replay::replay_postgres;
#[cfg(feature = "sqlite")]
use replay::replay_sqlite;
#[cfg(feature = "postgres")]
use snapshot::dashboard_snapshot_postgres;
#[cfg(feature = "sqlite")]
use snapshot::dashboard_snapshot_sqlite;

impl RealtimeAdminPort for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, RealtimeDashboardSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => dashboard_snapshot_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => dashboard_snapshot_sqlite(store).await,
            }
        })
    }

    fn list_lag(&self, request: RealtimeLagListRequest) -> ApplicationFuture<'_, RealtimeLagPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_lag_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_lag_sqlite(store, request).await,
            }
        })
    }

    fn list_failures(
        &self,
        request: RealtimeFailureListRequest,
    ) -> ApplicationFuture<'_, RealtimeFailurePage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_failures_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_failures_sqlite(store, request).await,
            }
        })
    }

    fn get_failure(
        &self,
        failure_id: Uuid,
    ) -> ApplicationFuture<'_, Option<RealtimeFailureSnapshot>> {
        Box::pin(async move {
            if failure_id.is_nil() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => get_failure_postgres(store, failure_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => get_failure_sqlite(store, failure_id).await,
            }
        })
    }

    fn replay(
        &self,
        command: RealtimeReplayCommand,
    ) -> ApplicationFuture<'_, RealtimeReplayResult> {
        Box::pin(async move {
            if !command.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => replay_postgres(store, command).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => replay_sqlite(store, command).await,
            }
        })
    }
}

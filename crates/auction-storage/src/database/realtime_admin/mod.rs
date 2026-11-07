//! Admin realtime 端口校验与 PostgreSQL 分派；事件、lag、失败和重放各由子模块拥有。

use auction_application::auction::ApplicationFuture;
use auction_application::realtime::{
    RealtimeAdminError, RealtimeAdminPort, RealtimeDashboardSnapshot, RealtimeEventListRequest,
    RealtimeEventPage, RealtimeFailureListRequest, RealtimeFailurePage, RealtimeFailureSnapshot,
    RealtimeLagListRequest, RealtimeLagPage, RealtimeReplayCommand, RealtimeReplayResult,
};
use uuid::Uuid;

use super::DatabaseStore;

mod common;
mod events;
mod failures;
mod lag;
mod replay;
mod snapshot;

use common::admin_error;
#[cfg(feature = "postgres")]
use events::list_events_postgres;
#[cfg(feature = "postgres")]
use failures::{get_failure_postgres, list_failures_postgres};
#[cfg(feature = "postgres")]
use lag::list_lag_postgres;
#[cfg(feature = "postgres")]
use replay::replay_postgres;
#[cfg(feature = "postgres")]
use snapshot::dashboard_snapshot_postgres;

impl RealtimeAdminPort for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, RealtimeDashboardSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => dashboard_snapshot_postgres(store).await,
            }
        })
    }

    fn list_events(
        &self,
        request: RealtimeEventListRequest,
    ) -> ApplicationFuture<'_, RealtimeEventPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_events_postgres(store, request).await,
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
            }
        })
    }
}

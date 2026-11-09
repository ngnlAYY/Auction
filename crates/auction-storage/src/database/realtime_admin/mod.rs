//! Admin realtime 端口校验与 PostgreSQL 分派；事件、lag、失败和重放各由子模块拥有。

use auction_application::auction::ApplicationFuture;
use auction_application::realtime::{
    RealtimeAdminError, RealtimeAdminPort, RealtimeDashboardSnapshot, RealtimeEventListRequest,
    RealtimeEventPage, RealtimeFailureListRequest, RealtimeFailurePage, RealtimeLagListRequest,
    RealtimeLagPage, RealtimeReplayCommand, RealtimeReplayResult,
};

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
use failures::list_failures_postgres;
#[cfg(feature = "postgres")]
use lag::list_lag_postgres;
#[cfg(feature = "postgres")]
use replay::replay_postgres;
#[cfg(feature = "postgres")]
use snapshot::dashboard_snapshot_postgres;

impl RealtimeAdminPort for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, RealtimeDashboardSnapshot> {
        Box::pin(async move { dashboard_snapshot_postgres(self).await })
    }

    fn list_events(
        &self,
        request: RealtimeEventListRequest,
    ) -> ApplicationFuture<'_, RealtimeEventPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            list_events_postgres(self, request).await
        })
    }

    fn list_lag(&self, request: RealtimeLagListRequest) -> ApplicationFuture<'_, RealtimeLagPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(RealtimeAdminError::InvalidCommand));
            }
            list_lag_postgres(self, request).await
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
            list_failures_postgres(self, request).await
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
            replay_postgres(self, command).await
        })
    }
}

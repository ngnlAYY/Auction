//! 持久化 Worker 调度适配器装配。
//!
//! due claim 与租约领取位于 `claim.rs`，完成/失败收口位于 `resolution.rs`；共享阶段命令、
//! UUID、时间和错误映射位于 `mapping.rs`。interval 只负责唤醒 Worker，不能成为状态权威。

use auction_application::{ApplicationError, ScheduledRoundClaim, ScheduledRoundStore};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

mod claim;
mod mapping;
mod resolution;

impl ScheduledRoundStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<ScheduledRoundClaim>> {
        Box::pin(async move {
            if worker_id.is_empty() || worker_id != worker_id.trim() || limit == 0 {
                return Err(mapping::repository_error("Worker 调度领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim::postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    claim::sqlite(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn complete(
        &self,
        claim: ScheduledRoundClaim,
        completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    resolution::complete_postgres(store.pool(), &claim, completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    resolution::complete_sqlite(store.pool(), &claim, completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn fail(
        &self,
        claim: ScheduledRoundClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = mapping::bounded_text(failure_code, 128);
            let safe_detail = mapping::bounded_text(safe_detail, 512);
            if failure_code.is_empty() || safe_detail.is_empty() {
                return Err(mapping::repository_error("Worker 失败记录不能为空"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => resolution::fail_postgres(
                    store.pool(),
                    &claim,
                    retry_at,
                    &failure_code,
                    &safe_detail,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => resolution::fail_sqlite(
                    store.pool(),
                    &claim,
                    retry_at,
                    &failure_code,
                    &safe_detail,
                )
                .await
                .map_err(ApplicationError::from),
            }
        })
    }
}

//! 持久化 Worker 调度适配器装配；claim/租约在 `claim.rs`，完成/失败收口在 `resolution.rs`，
//! 共享阶段命令、UUID、时间和错误映射在 `mapping.rs`。interval 只唤醒 Worker，不是状态权威。

use auction_application::core::ApplicationError;
use auction_application::scheduling::{ScheduledRoundClaim, ScheduledRoundStore};
use auction_time::Timestamp;

use crate::database::DatabaseStore;
use crate::database::storage_error;

mod bootstrap;
mod claim;
mod lookup;
mod mapping;
mod resolution;
mod wakeup;

impl ScheduledRoundStore for DatabaseStore {
    fn database_now(&self) -> auction_application::core::ApplicationFuture<'_, Timestamp> {
        Box::pin(async move {
            bootstrap::database_now_postgres(self.pool()).await.map_err(ApplicationError::from)
        })
    }

    fn find_current_round(
        &self,
        world_id: String,
        local_week: jiff::civil::Date,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        Option<auction_application::scheduling::ScheduledRoundSnapshot>,
    > {
        Box::pin(async move {
            lookup::postgres(self.pool(), &world_id, local_week)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn find_slot_round(
        &self,
        world_id: String,
        slot_key: String,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        Option<auction_application::scheduling::ScheduledRoundSnapshot>,
    > {
        Box::pin(async move {
            lookup::slot_postgres(self.pool(), &world_id, &slot_key)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn ensure_current_round(
        &self,
        request: auction_application::scheduling::ScheduledRoundBootstrap,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        auction_application::scheduling::ScheduledRoundSnapshot,
    > {
        Box::pin(async move {
            bootstrap::ensure_postgres(self.pool(), request).await.map_err(ApplicationError::from)
        })
    }

    fn claim_due(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
        limit: u16,
    ) -> auction_application::core::ApplicationFuture<'_, Vec<ScheduledRoundClaim>> {
        Box::pin(async move {
            if worker_id.is_empty() || worker_id != worker_id.trim() || limit == 0 {
                return Err(storage_error("Worker 调度领取参数无效"));
            }
            claim::postgres(self.pool(), &worker_id, now, lease_until, limit)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn next_claimable_at(
        &self,
        now: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, Option<Timestamp>> {
        Box::pin(async move {
            wakeup::next_claimable_postgres(self.pool(), now).await.map_err(ApplicationError::from)
        })
    }

    fn complete(
        &self,
        claim: ScheduledRoundClaim,
        completed_at: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            resolution::complete_postgres(self.pool(), &claim, completed_at)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn fail(
        &self,
        claim: ScheduledRoundClaim,
        retry_at: Timestamp,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::core::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = mapping::bounded_text(failure_code, 128);
            let safe_detail = mapping::bounded_text(safe_detail, 512);
            if failure_code.is_empty() || safe_detail.is_empty() {
                return Err(storage_error("Worker 失败记录不能为空"));
            }
            resolution::fail_postgres(self.pool(), &claim, retry_at, &failure_code, &safe_detail)
                .await
                .map_err(ApplicationError::from)
        })
    }
}

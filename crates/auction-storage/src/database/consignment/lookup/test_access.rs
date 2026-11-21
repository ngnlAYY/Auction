//! 仅为 PostgreSQL 集成测试提供事务内原语入口。

use auction_application::consignment::{
    LookupFence, LookupLeaseUpdate, LookupPermitStatus, LookupRetryAfter, LookupTransientReason,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::control::{lock_control, read_admission};
use super::work::{
    LookupFenceCheck, LookupResolution, MatchedLookupLease, cancel_lookup, check_lookup_fence,
    check_lookup_fence_at, enqueue_lookup, finish_lookup,
};
use crate::database::DatabaseStore;

impl DatabaseStore {
    /// 在测试事务内锁定单例并返回准入代次。
    pub async fn test_lookup_lock(conn: &mut PgConnection) -> Result<i64, sqlx::Error> {
        Ok(lock_control(conn).await?.enabled_epoch)
    }

    /// 测试无锁的准入和 Worker 心跳投影。
    pub async fn test_lookup_admission(
        conn: &mut PgConnection,
    ) -> Result<(LookupPermitStatus, bool), sqlx::Error> {
        let admission = read_admission(conn).await?;
        Ok((admission.permit, admission.worker_ready))
    }

    /// 在测试事务内调用登记入队原语。
    pub async fn test_lookup_enqueue(
        conn: &mut PgConnection,
        listing_id: Uuid,
        work_id: Uuid,
        registered_at: Timestamp,
    ) -> Result<(), sqlx::Error> {
        enqueue_lookup(conn, listing_id, work_id, registered_at).await
    }

    /// 在测试事务内调用撤销原语。
    pub async fn test_lookup_cancel(
        conn: &mut PgConnection,
        listing_id: Uuid,
    ) -> Result<i64, sqlx::Error> {
        cancel_lookup(conn, listing_id).await
    }

    /// 核对实际工作及许可。夹具可提供与租期完全相等的数据库时刻来验证边界。
    /// `None` 为工作或代次不符；内层结果区分活租约与租约丢失。
    pub async fn test_lookup_fence(
        conn: &mut PgConnection,
        fence: LookupFence,
        as_of: Option<Timestamp>,
    ) -> Result<Option<LookupLeaseUpdate<i16>>, sqlx::Error> {
        Ok(match check(conn, fence, as_of).await? {
            LookupFenceCheck::Matched(lease) => {
                Some(LookupLeaseUpdate::Applied(lease.attempt_count()))
            }
            LookupFenceCheck::LeaseLost => Some(LookupLeaseUpdate::Lost),
            LookupFenceCheck::MissingOrWrongGeneration => None,
        })
    }

    /// 以真实或指定墙钟先核验租约，再以实际数据库墙钟尝试完成或重排。
    pub async fn test_lookup_finish(
        conn: &mut PgConnection,
        fence: LookupFence,
        as_of: Option<Timestamp>,
        failure: Option<(LookupTransientReason, Option<LookupRetryAfter>)>,
    ) -> Result<LookupLeaseUpdate<Option<Timestamp>>, sqlx::Error> {
        let lease: MatchedLookupLease = match check(conn, fence, as_of).await? {
            LookupFenceCheck::Matched(lease) => lease,
            LookupFenceCheck::LeaseLost | LookupFenceCheck::MissingOrWrongGeneration => {
                return Ok(LookupLeaseUpdate::Lost);
            }
        };
        let resolution = match failure {
            Some((reason, retry_after)) => LookupResolution::Transient { reason, retry_after },
            None => LookupResolution::Done,
        };
        finish_lookup(conn, lease, resolution).await
    }
}

async fn check(
    conn: &mut PgConnection,
    fence: LookupFence,
    as_of: Option<Timestamp>,
) -> Result<LookupFenceCheck, sqlx::Error> {
    match as_of {
        Some(at) => check_lookup_fence_at(conn, fence, Some(at)).await,
        None => check_lookup_fence(conn, fence).await,
    }
}

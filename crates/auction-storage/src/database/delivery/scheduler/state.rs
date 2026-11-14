//! delivery claimed/sending 状态的启动、释放与续租。

use super::mapping::*;

use crate::database::delivery::audit::{
    DeliveryAudit, RELEASE, append as append_delivery_audit, worker_state_metadata,
};
use auction_application::delivery::DeliveryWorkClaim;
use chrono::{DateTime, Utc};
use sqlx::Postgres;
use uuid::Uuid;

pub(super) async fn start_sending_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    intervals: &auction_application::delivery::DeliverySendIntervals,
) -> Result<(), auction_application::core::RepositoryError> {
    super::cash::run_legacy(pool, claim, started_at, intervals).await
}

pub(super) async fn release_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    released_at: DateTime<Utc>,
) -> Result<(), auction_application::core::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    lock_delivery_lease(&mut transaction, claim.delivery_id).await?;
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'pending',
               attempt_count = GREATEST(attempt_count - 1, 0),
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'claimed'
             AND lease_token = $3
             AND fencing_version = $4
             AND lease_expires_at > $2
             AND lease_expires_at > clock_timestamp()"#,
    )
    .bind(claim.delivery_id)
    .bind(released_at)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("delivery 释放时 lease 已失效"));
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: released_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: RELEASE,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "pending",
                claim.fencing_version,
                claim.attempt_count.saturating_sub(1),
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

pub(super) async fn renew_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    renewed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<(), auction_application::core::RepositoryError> {
    // 准备和发送共享同一 token/fencing；数据库排队后也必须仍有效，不能用旧调用时间复活过期租约。
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    lock_delivery_lease(&mut transaction, claim.delivery_id).await?;
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET lease_expires_at = GREATEST(lease_expires_at, $3),
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status IN ('claimed', 'sending')
             AND lease_token = $4
             AND fencing_version = $5
             AND lease_expires_at > $2
             AND lease_expires_at > clock_timestamp()
             AND $3 > $2
             AND $3 > clock_timestamp()"#,
    )
    .bind(claim.delivery_id)
    .bind(renewed_at)
    .bind(lease_until)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("delivery 续租时 lease 已失效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

/// 先取得行锁，再由后续 UPDATE 检查实时有效期，避免锁等待前求值的时间条件放过过期租约。
/// sending 调用方保持 lot → delivery 锁序；续租/释放不再取 lot 锁。
async fn lock_delivery_lease(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(), auction_application::core::RepositoryError> {
    sqlx::query("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
        .bind(delivery_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    Ok(())
}

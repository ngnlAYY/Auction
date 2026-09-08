//! delivery claimed/sending 状态的启动、释放与续租。

use super::mapping::*;

use super::*;

pub(super) async fn start_sending_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let result = sqlx::query(concat!(
        r#"UPDATE public.deliveries AS deliveries
           SET status = 'sending', sending_at = $2, updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'claimed'
             AND lease_token = $3
             AND fencing_version = $4
             AND lease_expires_at > $2
             AND "#,
        delivery_completion_gate!()
    ))
    .bind(claim.delivery_id)
    .bind(started_at)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 开始发送时 lease 已失效"));
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: started_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: START_SENDING,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "sending",
                claim.fencing_version,
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn release_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    released_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'pending',
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'claimed'
             AND lease_token = $3
             AND fencing_version = $4
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(released_at)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 释放时 lease 已失效"));
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
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn renew_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    renewed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET lease_expires_at = $3,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $4
             AND fencing_version = $5
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(renewed_at)
    .bind(lease_until)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 续租时 lease 已失效"));
    }
    Ok(())
}

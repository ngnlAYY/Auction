//! 过期sending逐条隔离，早锁持久资金账户；不能凭TTL释放占用。

use super::outcome::OutcomeError;
use super::*;
use crate::database::settlement_funding as funding;

pub(super) async fn reconcile(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    grace_micros: i64,
) -> Result<(), auction_application::RepositoryError> {
    // 候选读取不持业务锁；每条短事务内重新锁定并核验有效期。
    let candidates = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM public.deliveries WHERE status='sending'
         AND lease_expires_at+($2::bigint*interval '1 microsecond')<=$1
         ORDER BY lease_expires_at,id LIMIT $3",
    )
    .bind(now)
    .bind(grace_micros)
    .bind(EXPIRED_SENDING_RECONCILE_LIMIT)
    .fetch_all(pool)
    .await
    .map_err(mapping::map_storage_error)?;
    for id in candidates {
        for attempt in 0..3 {
            match expire_one(pool, id, now, grace_micros).await {
                Ok(()) => break,
                Err(error) if error.retryable() && attempt < 2 => continue,
                Err(_) => {
                    return Err(mapping::repository_error_owned("过期delivery资金隔离未确定完成"));
                }
            }
        }
    }
    Ok(())
}

async fn expire_one(
    pool: &crate::database::TrackedPool<Postgres>,
    id: Uuid,
    now: DateTime<Utc>,
    grace_micros: i64,
) -> Result<(), OutcomeError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let account = funding::lock_funded_delivery_account(&mut tx, id).await?;
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    // 过期只改变本次结果与 CAS fence，不能重置自动维护的跨尝试重排预算。
    let row=sqlx::query(
        "UPDATE public.deliveries SET status='manual_reconcile',lease_token=NULL,lease_expires_at=NULL,
         completed_at=$2,external_reference=NULL,failure_code='lease_expired_outcome_unknown',
         result_metadata='{\"outcome_unknown\":true,\"reason\":\"sending_lease_expired\"}'::jsonb
             || CASE WHEN result_metadata ? 'sweep_retries'
                 THEN jsonb_build_object('sweep_retries',result_metadata->'sweep_retries')
                 ELSE '{}'::jsonb END,
         fencing_version=fencing_version+1,updated_at=clock_timestamp()
         WHERE id=$1 AND status='sending'
         AND lease_expires_at+($3::bigint*interval '1 microsecond')<=$2
         AND lease_expires_at+($3::bigint*interval '1 microsecond')<=clock_timestamp()
         RETURNING command_id,fencing_version,attempt_count",
    ).bind(id).bind(now).bind(grace_micros).fetch_optional(&mut *tx).await?;
    if let Some(row) = row {
        if let Some(account) = account {
            funding::finish_funded_delivery(
                &mut tx,
                &account,
                id,
                auction_application::FundingDeliveryOutcome::Unknown,
            )
            .await?;
        }
        let command_id: Uuid = row.try_get("command_id")?;
        let request_id = command_id.to_string();
        append_delivery_audit(&mut tx,DeliveryAudit {
            occurred_at:now,actor_type:"worker",actor_id:"delivery-worker",action:RECONCILE_EXPIRED_SENDING,
            delivery_id:id,request_id:Some(&request_id),source:"worker",
            metadata:serde_json::json!({"status":"manual_reconcile","failure_code":"lease_expired_outcome_unknown",
                "fencing_version":row.try_get::<i64,_>("fencing_version")?,"attempt_count":row.try_get::<i32,_>("attempt_count")?}),
        }).await?;
    }
    tx.commit().await?;
    Ok(())
}

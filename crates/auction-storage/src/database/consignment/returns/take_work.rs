//! 退件 Take 工作的持久领取与当前领取重读（给 Worker 的退件 Take executor 适配器用）。
//!
//! 只领退件接管建立的唯一 Take 工作（从 `consignment_return_intakes` 出发连接工作与投递），期 4 的
//! 处置领取反向排除这些工作，两条路径互不重叠。领取写法与期 4 共用 `work_claim`。
//!
//! 保护不变量 4、5（D9 §4.1 “Take 工作尚未起写 / 过租、重启或结果未知”）：
//! - 只领尚未起写的工作：已有 `mail_disposition_write_starts` 的工作只由恢复路径按原 attempt 只读核对，
//!   这里不再推进 fence、不再登记 attempt（新 fence 本就拿不到 POST 许可，重复领取只会无界追加尝试行）；
//! - 已消费的接管不再领取；
//! - 重读只在租约仍有效、attempt / fence / token 都是当前值时返回，否则本轮不起写。

use auction_application::consignment::{ConsignmentFuture, DispositionAction};
use auction_application::mail::{
    DispositionWorkClaim, ReturnTakeClaim, ReturnTakeLease, ReturnTakeWorkStore,
};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::disposition::manifest_from_payload;
use crate::database::mail::originals::invariant_decoding;
use crate::database::mail::originals::work_claim::{
    claim_lease_micros, claim_selected_in_tx, disposition_claim_columns, disposition_claimable,
};

impl ReturnTakeWorkStore for DatabaseStore {
    fn claim_due_return_take(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<ReturnTakeClaim>> {
        Box::pin(async move {
            claim_due_return_take(self, &worker_id, now, lease_until)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn load_current_return_take(
        &self,
        lease: ReturnTakeLease,
    ) -> ConsignmentFuture<'_, Option<ReturnTakeClaim>> {
        Box::pin(async move {
            load_current_return_take(self, lease).await.map_err(WriteError::into_application)
        })
    }
}

async fn claim_due_return_take(
    store: &DatabaseStore,
    worker_id: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<Option<ReturnTakeClaim>, WriteError> {
    let lease_micros = claim_lease_micros(worker_id, now, lease_until)?;
    let mut tx = store.pool().begin().await?;
    let row = sqlx::query(concat!(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT intake.id AS intake_id, ",
        disposition_claim_columns!(),
        "
         FROM public.consignment_return_intakes AS intake
         JOIN public.mail_disposition_work AS w
           ON w.id = intake.take_work_id AND w.delivery_id = intake.take_delivery_id
          AND w.decision_id = intake.decision_id
         JOIN public.deliveries AS d ON d.id = w.delivery_id
         WHERE w.action = 'take'
           AND NOT EXISTS (SELECT 1 FROM public.mail_disposition_write_starts AS started
                           WHERE started.work_id = w.id)
           AND NOT EXISTS (SELECT 1 FROM public.consignment_return_consumptions AS consumed
                           WHERE consumed.intake_id = intake.id)
           AND ",
        disposition_claimable!(),
        "
         ORDER BY d.created_at, w.id
         LIMIT 1
         FOR UPDATE OF d SKIP LOCKED",
    ))
    .bind(lease_micros)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let intake_id: Uuid = row.try_get("intake_id")?;
    let work = claim_selected_in_tx(&mut tx, &row).await?;
    tx.commit().await?;
    Ok(Some(ReturnTakeClaim { intake_id, work }))
}

async fn load_current_return_take(
    store: &DatabaseStore,
    lease: ReturnTakeLease,
) -> Result<Option<ReturnTakeClaim>, WriteError> {
    let row = sqlx::query(
        "SELECT w.id, w.decision_id, w.delivery_id, w.manifest_digest, d.payload,
                d.attempt_count, d.lease_expires_at
         FROM public.consignment_return_intakes AS intake
         JOIN public.mail_disposition_work AS w
           ON w.id = intake.take_work_id AND w.delivery_id = intake.take_delivery_id
          AND w.decision_id = intake.decision_id
         JOIN public.deliveries AS d ON d.id = w.delivery_id
         JOIN public.mail_disposition_attempts AS attempt
           ON attempt.delivery_id = d.id AND attempt.work_id = w.id
          AND attempt.fence = d.fencing_version
         WHERE intake.id = $1 AND attempt.id = $2 AND d.fencing_version = $3
           AND d.lease_token = $4 AND d.status = 'claimed'
           AND d.lease_expires_at > clock_timestamp()
           AND w.action = 'take'",
    )
    .bind(lease.intake_id)
    .bind(lease.attempt_id)
    .bind(i64::try_from(lease.fence)?)
    .bind(lease.claim_token)
    .fetch_optional(store.pool())
    .await?;
    let Some(row) = row else { return Ok(None) };
    let manifest_digest: &[u8] = row.try_get("manifest_digest")?;
    let manifest_digest = <[u8; 32]>::try_from(manifest_digest)
        .map_err(invariant_decoding("consignment_work_manifest_digest"))?;
    let payload: serde_json::Value = row.try_get("payload")?;
    let work = DispositionWorkClaim {
        work_id: row.try_get("id")?,
        final_decision_id: row.try_get("decision_id")?,
        delivery_id: row.try_get("delivery_id")?,
        action: DispositionAction::Take,
        attempt_id: lease.attempt_id,
        fence: lease.fence,
        claim_token: lease.claim_token,
        attempt_count: u32::try_from(row.try_get::<i32, _>("attempt_count")?)
            .map_err(invariant_decoding("consignment_work_attempt"))?,
        lease_until: row.try_get("lease_expires_at")?,
        manifest: manifest_from_payload(&payload, manifest_digest)?,
    };
    Ok(Some(ReturnTakeClaim { intake_id: lease.intake_id, work }))
}

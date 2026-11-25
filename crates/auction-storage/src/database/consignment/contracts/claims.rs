//! 寄售义务的专用领取：与拍卖领取同机制（行锁跳过已锁、登记 claim 与 fence、先提交释放），
//! 但只认寄售来源；来源损坏的义务转人工对账、不发送。
//!
//! 保护不变量 2、5、6：领取与 fence 持久化在履约行，内存不决定到期或完成；只领 `ready` 且前置已
//! `satisfied` 的寄售义务；通用拍卖领取以 `contract_id IS NULL` 排除寄售行，两条领取路径互不越界。
//! 领取事务对合同、sale 与 position 只读不加锁（R5-C8），只锁义务与履约行；最终事务的重载加锁归第 6 期。

use auction_application::consignment::{
    ClaimIsolation, ConsignmentError, ConsignmentFulfillmentClaims, ConsignmentFuture,
    ConsignmentObligationClaim,
};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::source::{SourceLoadError, load_source_for_claim};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::settlement::fulfillment::manual_reconcile_in_transaction;

/// 可领取的履约行谓词（参数 $1 为当前时刻）：未被占用或租约已过期，且前置已满足。
macro_rules! claimable {
    () => {
        "o.contract_id IS NOT NULL
    AND o.business_state = 'ready'
    AND f.execution_state IN ('pending', 'prepared', 'enqueued')
    AND ((f.claim_token IS NULL AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1))
         OR f.claim_expires_at <= $1)
    AND (o.prerequisite_obligation_id IS NULL OR prerequisite.execution_state = 'satisfied')"
    };
}

impl ConsignmentFulfillmentClaims for DatabaseStore {
    fn claim_consignment_obligations(
        &self,
        now: Timestamp,
        _worker_id: &str,
        lease_until: Timestamp,
        limit: u16,
    ) -> ConsignmentFuture<'_, Vec<ConsignmentObligationClaim>> {
        Box::pin(async move {
            claim_in_transaction(self, now, lease_until, limit)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn release_consignment_claim(
        &self,
        obligation_id: auction_domain::SettlementId,
        claim_token: Uuid,
        claim_fencing: i64,
        now: Timestamp,
    ) -> ConsignmentFuture<'_, ()> {
        Box::pin(async move {
            // 只释放尚未起写的领取（pending）；prepared / enqueued 已有投递事实，不能无条件放回。
            let released = sqlx::query(
                "UPDATE public.settlement_obligation_fulfillments AS f
                 SET claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
                     state_version = f.state_version + 1, updated_at = statement_timestamp()
                 FROM public.settlement_obligations AS o
                 WHERE f.obligation_id = $1 AND o.id = f.obligation_id
                   AND o.contract_id IS NOT NULL
                   AND f.execution_state = 'pending'
                   AND f.claim_token = $2 AND f.claim_fencing = $3
                   AND f.claim_expires_at > $4",
            )
            .bind(obligation_id.as_uuid())
            .bind(claim_token)
            .bind(claim_fencing)
            .bind(now)
            .execute(self.pool())
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
            if released.rows_affected() == 1 { Ok(()) } else { Err(ConsignmentError::LeaseLost) }
        })
    }
}

async fn claim_in_transaction(
    store: &DatabaseStore,
    now: Timestamp,
    lease_until: Timestamp,
    limit: u16,
) -> Result<Vec<ConsignmentObligationClaim>, WriteError> {
    let mut transaction = store.pool().begin().await?;
    // 先按创建顺序截取有界候选，再按 UUID 锁义务；被他人锁住的候选直接跳过留待下一批。
    let obligation_ids = sqlx::query_scalar::<_, Uuid>(concat!(
        "WITH candidates AS MATERIALIZED (
             SELECT o.id
             FROM public.settlement_obligations AS o
             JOIN public.settlement_obligation_fulfillments AS f ON f.obligation_id = o.id
             LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
                 ON prerequisite.obligation_id = o.prerequisite_obligation_id
             WHERE ",
        claimable!(),
        "
             ORDER BY o.created_at, o.id
             LIMIT $2
         )
         SELECT o.id FROM public.settlement_obligations AS o
         JOIN candidates ON candidates.id = o.id
         WHERE o.business_state = 'ready' AND o.contract_id IS NOT NULL
         ORDER BY o.id FOR UPDATE OF o SKIP LOCKED"
    ))
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await?;

    // 再锁履约行（先义务后履约，与拍卖领取同锁序）；锁后用同一谓词复核。
    let rows = sqlx::query(concat!(
        "SELECT f.obligation_id, f.execution_state, f.claim_fencing, f.retry_count, f.state_version
         FROM public.settlement_obligation_fulfillments AS f
         JOIN public.settlement_obligations AS o ON o.id = f.obligation_id
         LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
             ON prerequisite.obligation_id = o.prerequisite_obligation_id
         WHERE o.id = ANY($3) AND ",
        claimable!(),
        "
         ORDER BY f.obligation_id
         LIMIT $2
         FOR UPDATE OF f SKIP LOCKED"
    ))
    .bind(now)
    .bind(i64::from(limit))
    .bind(&obligation_ids)
    .fetch_all(&mut *transaction)
    .await?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let obligation_id: Uuid = row.try_get("obligation_id")?;
        let execution_state: String = row.try_get("execution_state")?;
        let old_fencing: i64 = row.try_get("claim_fencing")?;
        let retry_count: i32 = row.try_get("retry_count")?;
        let state_version: i64 = row.try_get("state_version")?;
        let (Some(claim_fencing), Ok(retry_count), Some(next_state_version)) = (
            old_fencing.checked_add(1),
            u32::try_from(retry_count),
            state_version.checked_add(1).and_then(|value| u64::try_from(value).ok()),
        ) else {
            return Err(WriteError::invariant("consignment_fulfillment_counters"));
        };
        let claim_token = Uuid::new_v4();
        let updated = sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET claim_token = $2, claimed_at = $3, claim_expires_at = $4, claim_fencing = $5,
                 state_version = state_version + 1, next_attempt_at = NULL,
                 updated_at = statement_timestamp()
             WHERE obligation_id = $1
               AND execution_state IN ('pending', 'prepared', 'enqueued')
               AND ((claim_token IS NULL AND (next_attempt_at IS NULL OR next_attempt_at <= $3))
                    OR claim_expires_at <= $3)",
        )
        .bind(obligation_id)
        .bind(claim_token)
        .bind(now)
        .bind(lease_until)
        .bind(claim_fencing)
        .execute(&mut *transaction)
        .await?;
        // 持锁后行条件已变（被并发领走）：放弃这一条，不是错误。
        if updated.rows_affected() != 1 {
            continue;
        }

        match load_source_for_claim(&mut transaction, obligation_id).await {
            Ok(source) => claims.push(ConsignmentObligationClaim {
                obligation: source.obligation,
                contract: source.contract,
                dispatch_world: source.dispatch_world,
                equipment_name: source.equipment_name,
                external_equipment_id: source.external_equipment_id,
                claim_token,
                claim_fencing,
                claimed_at: now,
                lease_until,
                retry_count,
            }),
            Err(SourceLoadError::Execution(error)) => return Err(error.into()),
            Err(SourceLoadError::Context(isolation)) => {
                // 来源损坏：转人工对账、不发送、不自动重试。
                manual_reconcile_in_transaction(
                    &mut transaction,
                    obligation_id,
                    &execution_state,
                    next_state_version,
                    claim_token,
                    claim_fencing,
                    isolation_code(isolation),
                )
                .await
                .map_err(|error| {
                    tracing::warn!(%error, "寄售来源损坏转人工失败");
                    ConsignmentError::Unavailable
                })?;
            }
        }
    }
    transaction.commit().await?;
    Ok(claims)
}

/// 写入履约行 `last_error_code` 的固定机器码。
const fn isolation_code(isolation: ClaimIsolation) -> &'static str {
    match isolation {
        ClaimIsolation::SourceMissing => "consignment_source_missing",
        ClaimIsolation::WorldMismatch => "consignment_world_mismatch",
        ClaimIsolation::AmountMismatch => "consignment_amount_mismatch",
        ClaimIsolation::RecipientMismatch => "consignment_recipient_mismatch",
        ClaimIsolation::Other(_) => "consignment_source_invalid",
    }
}

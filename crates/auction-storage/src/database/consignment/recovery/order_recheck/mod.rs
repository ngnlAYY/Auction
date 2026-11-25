//! 订单复核工作（方案 A）的存储实现：持久领取与按租约完成。
//!
//! 领取以 `SKIP LOCKED` 选一条未完成、租约空闲或已过期的工作，换新 token、fence 加一，并在同一事务冻结
//! 该订单此刻的待核对项快照；完成按 token / fence 核对当前租约后，由快照与此刻的证据推导结果并只追加一条。
//! 保护不变量 1、4、5、6：
//! - 工作、租约 / fence、快照、完成时刻与结果都持久，内存定时器只用于唤醒；
//! - 同一租约重复完成返回原结果，旧 token、旧 fence 或过租确定拒绝，已完成的工作不可再领取；
//! - 结果不接受调用方声明，`bound` 只在快照每一项都已有终局证据时成立，数据库守卫再核一次；
//! - 领取与完成只写复核自己的两张表，不改投递、起写或证据事实，不产生任何起写或外写许可。
//!
//! 复核结果不进入订单阶段投影（投影不读这两张表），完成不改变订单阶段，因此不追加阶段事件；
//! 补记的证据经既有写口写入时，由写口在自己的事务里经统一入口追加阶段事件。

mod pending;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, OrderRecheckClaim, OrderRecheckLease, OrderRecheckOutcome,
    OrderRecheckWorkStore,
};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use self::pending::{pending_dispatch_attempts, pending_return_takes, snapshot_has_open_items};
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::work_claim::claim_lease_micros;

impl OrderRecheckWorkStore for DatabaseStore {
    fn claim_due_order_recheck(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<OrderRecheckClaim>> {
        Box::pin(async move {
            claim(self, &worker_id, now, lease_until).await.map_err(WriteError::into_application)
        })
    }

    fn complete_order_recheck(
        &self,
        lease: OrderRecheckLease,
    ) -> ConsignmentFuture<'_, OrderRecheckOutcome> {
        Box::pin(async move {
            match complete(self, lease).await.map_err(WriteError::into_application)? {
                Completion::Done(outcome) => Ok(outcome),
                Completion::LeaseLost => Err(ConsignmentError::LeaseLost),
            }
        })
    }
}

async fn claim(
    store: &DatabaseStore,
    worker_id: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<Option<OrderRecheckClaim>, WriteError> {
    let lease_micros = claim_lease_micros(worker_id, now, lease_until)?;
    let mut tx = store.pool().begin().await?;
    // 租约到期以数据库时钟为准；过租的工作可被重新领取，新领取的 fence 使旧租约的完成失效。
    let row = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT work.work_id, work.contract_id, work.contract_version, work.claim_fence,
                (SELECT at FROM sql_clock) + ($1 * interval '1 microsecond') AS lease_until
         FROM public.consignment_order_rechecks AS work
         WHERE NOT EXISTS (SELECT 1 FROM public.consignment_order_recheck_results AS result
                           WHERE result.work_id = work.work_id)
           AND (work.lease_expires_at IS NULL
                OR work.lease_expires_at <= (SELECT at FROM sql_clock))
         ORDER BY work.requested_at, work.work_id
         LIMIT 1
         FOR UPDATE OF work SKIP LOCKED",
    )
    .bind(lease_micros)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let work_id: Uuid = row.try_get("work_id")?;
    let contract_id: Uuid = row.try_get("contract_id")?;
    let fence: i64 = row.try_get("claim_fence")?;
    let lease_until: Timestamp = row.try_get("lease_until")?;
    let dispatch_attempts = pending_dispatch_attempts(&mut tx, contract_id).await?;
    let return_takes = pending_return_takes(&mut tx, contract_id).await?;
    let attempt_ids: Vec<Uuid> = dispatch_attempts.iter().map(|item| item.attempt_id).collect();
    let take_works: Vec<Uuid> = return_takes.iter().map(|item| item.work_id).collect();
    let claim_token = Uuid::new_v4();
    let next_fence =
        fence.checked_add(1).ok_or_else(|| WriteError::invariant("consignment_recheck_fence"))?;
    let updated = sqlx::query(
        "UPDATE public.consignment_order_rechecks
         SET claim_token = $2, claim_fence = $3, lease_expires_at = $4,
             pending_dispatch_attempts = $5, pending_take_works = $6
         WHERE work_id = $1 AND claim_fence = $7",
    )
    .bind(work_id)
    .bind(claim_token)
    .bind(next_fence)
    .bind(lease_until)
    .bind(&attempt_ids)
    .bind(&take_works)
    .bind(fence)
    .execute(&mut *tx)
    .await?;
    // 行锁已在本事务内持有，fence 不可能被他人推进；不是恰好一行说明事实被改坏。
    if updated.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_recheck_claim_cas"));
    }
    tx.commit().await?;
    Ok(Some(OrderRecheckClaim {
        lease: OrderRecheckLease { work_id, claim_token, fence: u64::try_from(next_fence)? },
        contract_id,
        contract_version: u64::try_from(row.try_get::<i64, _>("contract_version")?)?,
        lease_until,
        dispatch_attempts,
        return_takes,
    }))
}

/// 完成的事务内结果：写下（或重放）的结果，或租约已不是当前租约。
enum Completion {
    Done(OrderRecheckOutcome),
    LeaseLost,
}

async fn complete(
    store: &DatabaseStore,
    lease: OrderRecheckLease,
) -> Result<Completion, WriteError> {
    let fence = i64::try_from(lease.fence)?;
    let mut tx = store.pool().begin().await?;
    let Some(work) = sqlx::query(
        "SELECT claim_token, claim_fence,
                COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_live,
                pending_dispatch_attempts, pending_take_works
         FROM public.consignment_order_rechecks WHERE work_id = $1 FOR UPDATE",
    )
    .bind(lease.work_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(Completion::LeaseLost);
    };
    let current = work.try_get::<Option<Uuid>, _>("claim_token")? == Some(lease.claim_token)
        && work.try_get::<i64, _>("claim_fence")? == fence;
    // 行锁之后另起语句读结果：并发的同租约完成已提交时这里一定看得到它（READ COMMITTED 的新快照）。
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT result FROM public.consignment_order_recheck_results WHERE work_id = $1",
    )
    .bind(lease.work_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(stored) = stored {
        tx.commit().await?;
        // 已完成：同一租约重放原结果；其他租约一律拒绝，不覆盖已写下的结果。
        if !current {
            return Ok(Completion::LeaseLost);
        }
        let outcome = OrderRecheckOutcome::parse(&stored)
            .ok_or_else(|| WriteError::invariant("consignment_recheck_result_code"))?;
        return Ok(Completion::Done(outcome));
    }
    if !current || !work.try_get::<bool, _>("lease_live")? {
        tx.commit().await?;
        return Ok(Completion::LeaseLost);
    }
    let dispatch_attempts: Vec<Uuid> = work.try_get("pending_dispatch_attempts")?;
    let take_works: Vec<Uuid> = work.try_get("pending_take_works")?;
    let outcome = if dispatch_attempts.is_empty() && take_works.is_empty() {
        OrderRecheckOutcome::NothingToCheck
    } else if snapshot_has_open_items(&mut tx, &dispatch_attempts, &take_works).await? {
        OrderRecheckOutcome::StillUnknown
    } else {
        OrderRecheckOutcome::Bound
    };
    sqlx::query(
        "INSERT INTO public.consignment_order_recheck_results
            (work_id, result, claim_fence, completed_at)
         VALUES ($1, $2, $3, clock_timestamp())",
    )
    .bind(lease.work_id)
    .bind(outcome.as_str())
    .bind(fence)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Completion::Done(outcome))
}

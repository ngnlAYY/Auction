//! 未付款 COD 被退回后的重发与达到冻结上限的收口。
//!
//! 未达 recovery 冻结的退回上限：把原 COD 投递重置为 pending 并重新排队履约，沿用原金额与装备，
//! 付款窗口按冻结的恢复策略版本重新起算；
//! 达到冻结上限：为卖家建立 withdrawn_return 义务把装备寄回，并把原 COD 与卖家收款按失败收口；
//! 成交随之撤销，同事务冲正补偿池。
//! 两条路径都以当前退件与 Take 为依据，先按与 Admin 确认同一谓词核对退件能否把原 COD 的每一次
//! 可能外达发送逐一归属；不成立时以阻断码 `send_unattributed` 停下，什么都不收口、不重寄。已阻断的
//! 记录按固定间隔由机器复核：新的持久事实（例如 Sent 观察到的远端 MID）让归属成立时解除阻断并在
//! 同一事务继续推进，仍不成立时只推迟下一次复核。

mod resend;
mod seller;

use auction_application::recovery::{RecoveryCommandError, RecoveryResolutionAdvance};
use auction_domain::RecoveryPolicy;
use auction_time::{Duration, Timestamp};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use self::resend::resend_cod;
use self::seller::{SellerReturnContext, return_to_seller};
use super::binding::{
    SEND_UNATTRIBUTED_CODE, SendAttribution, block_unattributed_recovery,
    defer_unattributed_recheck, release_attributed_recovery, send_attribution,
};
use super::mapping::is_conflict;
use super::mapping::map_sql_error;

pub(in crate::database::recovery::commands) async fn advance_cod_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    // 退回卖家时要锁补偿池，可能与任一世界的结算发生序列化冲突；冲突有界重试。
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || advance_cod_return_postgres_once(pool, now),
    )
    .await
}

async fn advance_cod_return_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

    let Some(candidate) = sqlx::query(
        r#"SELECT recovery.id,
                  recovery.auction_id,
                  recovery.lot_id,
                  recovery.equipment_id,
                  recovery.payment_obligation_id,
                  recovery.seller_payout_obligation_id,
                  recovery.cod_return_count,
                  recovery.cod_return_limit,
                  recovery.policy_version,
                  recovery.state_version,
                  recovery.blocking_code IS NOT NULL AS blocked
           FROM public.settlement_recoveries AS recovery
           WHERE recovery.stage = 'reclaiming'
             AND recovery.world_id = 'main'
             AND (recovery.blocking_code IS NULL
                  OR (recovery.blocking_code = $2
                      AND (recovery.next_attempt_at IS NULL OR recovery.next_attempt_at <= $1)))
             AND recovery.custody_proof_kind IS NOT NULL
             AND recovery.cod_return_count BETWEEN 1 AND recovery.cod_return_limit
             AND EXISTS (
                 SELECT 1
                 FROM public.settlement_recovery_actions AS reclaim
                 WHERE reclaim.recovery_id = recovery.id
                   AND reclaim.kind = 'reclaim_equipment_take'
                   AND reclaim.execution_state = 'satisfied'
             )
             AND (
                 recovery.cod_return_count < recovery.cod_return_limit
                 OR EXISTS (
                     SELECT 1
                     FROM public.settlement_recovery_actions AS blacklist
                     WHERE blacklist.recovery_id = recovery.id
                       AND blacklist.kind = 'buyer_blacklist'
                       AND blacklist.execution_state = 'satisfied'
                 )
             )
           ORDER BY recovery.blocking_code IS NOT NULL, recovery.updated_at, recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .bind(now)
    .bind(SEND_UNATTRIBUTED_CODE)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    else {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(None);
    };

    let recovery_id: Uuid = candidate.try_get("id").map_err(map_sql_error)?;
    let mut state_version: i64 = candidate.try_get("state_version").map_err(map_sql_error)?;
    let blocked: bool = candidate.try_get("blocked").map_err(map_sql_error)?;
    let payment_obligation_id: Uuid =
        candidate.try_get("payment_obligation_id").map_err(map_sql_error)?;
    let seller_payout_obligation_id: Uuid =
        candidate.try_get("seller_payout_obligation_id").map_err(map_sql_error)?;
    let cod_return_count: i32 = candidate.try_get("cod_return_count").map_err(map_sql_error)?;
    let cod_return_limit: i32 = candidate.try_get("cod_return_limit").map_err(map_sql_error)?;
    if !(1..=10).contains(&cod_return_limit) || cod_return_count > cod_return_limit {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let policy_version: i32 = candidate.try_get("policy_version").map_err(map_sql_error)?;
    let auction_id: Uuid = candidate.try_get("auction_id").map_err(map_sql_error)?;
    let lot_id: Uuid = candidate.try_get("lot_id").map_err(map_sql_error)?;
    let equipment_id: Uuid = candidate.try_get("equipment_id").map_err(map_sql_error)?;

    // 重寄与退回卖家都以当前退件与 Take 为依据：原 COD 每一次可能已到达远端的发送都必须由已消费的
    // 系统退件逐一归属；与 Admin 确认同一谓词。不成立时不重寄、不撤销成交，以阻断码停下并提交；
    // 已阻断的记录归属成立时同事务解除阻断后继续，仍不成立时只推迟复核（不变量 2、4、6）。
    match (send_attribution(&mut transaction, payment_obligation_id, recovery_id).await?, blocked) {
        (SendAttribution::Attributed(_), false) => {}
        (SendAttribution::Attributed(rule), true) => {
            state_version = release_attributed_recovery(
                &mut transaction,
                recovery_id,
                state_version,
                rule,
                now,
            )
            .await?;
        }
        (SendAttribution::Blocked(blocker), false) => {
            let state_version = block_unattributed_recovery(
                &mut transaction,
                recovery_id,
                state_version,
                blocker,
                now,
            )
            .await?;
            transaction.commit().await.map_err(map_sql_error)?;
            return Ok(Some(still_reclaiming(recovery_id, state_version)));
        }
        (SendAttribution::Blocked(_), true) => {
            defer_unattributed_recheck(&mut transaction, recovery_id, state_version, now).await?;
            transaction.commit().await.map_err(map_sql_error)?;
            return Ok(Some(still_reclaiming(recovery_id, state_version)));
        }
    }

    let (stage, resolution, next_state_version) = if cod_return_count < cod_return_limit {
        // 冻结版本未知或截止溢出是持久事实无效：在任何写入前失败关闭，不能退化为 StaleState。
        let payment_deadline_at = recovery_payment_timeout(policy_version)
            .and_then(|timeout| now.checked_add_signed(timeout))
            .ok_or(RecoveryCommandError::InvalidStoredResult)?;
        resend_cod(
            &mut transaction,
            now,
            recovery_id,
            payment_obligation_id,
            state_version,
            payment_deadline_at,
        )
        .await?
    } else {
        return_to_seller(
            &mut transaction,
            now,
            SellerReturnContext {
                recovery_id,
                state_version,
                payment_obligation_id,
                seller_payout_obligation_id,
                auction_id,
                lot_id,
                equipment_id,
                cod_return_count,
                cod_return_limit,
            },
        )
        .await?
    };

    transaction.commit().await.map_err(map_sql_error)?;
    Ok(Some(RecoveryResolutionAdvance {
        recovery_id: recovery_id.into(),
        stage: stage.clone(),
        resolution,
        relisted_auction_id: None,
        relisted_lot_id: None,
        state_version: next_state_version,
        completed: stage == "completed",
    }))
}

/// 本轮停在 `reclaiming` 的推进结果（阻断或复核后仍阻断）。
fn still_reclaiming(recovery_id: Uuid, state_version: i64) -> RecoveryResolutionAdvance {
    RecoveryResolutionAdvance {
        recovery_id: recovery_id.into(),
        stage: "reclaiming".to_owned(),
        resolution: None,
        relisted_auction_id: None,
        relisted_lot_id: None,
        state_version,
        completed: false,
    }
}

/// recovery 冻结的策略版本对应的付款窗口，版本表由领域持有；未知或非正版本返回 None，由调用方
/// 失败关闭。
fn recovery_payment_timeout(policy_version: i32) -> Option<Duration> {
    RecoveryPolicy::payment_timeout_for_version(u32::try_from(policy_version).ok()?)
}

#[cfg(test)]
mod tests {
    use auction_time::Duration;

    use super::recovery_payment_timeout;

    #[test]
    fn payment_window_follows_the_frozen_recovery_policy_version() {
        assert_eq!(recovery_payment_timeout(1), Some(Duration::days(7)));
        assert_eq!(recovery_payment_timeout(2), Some(Duration::hours(24)));
        for unknown in [-1, 0, 3] {
            assert_eq!(recovery_payment_timeout(unknown), None, "未知版本 {unknown} 必须失败关闭");
        }
    }
}

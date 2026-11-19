//! 按领域守卫的决定改写投递行：以预期状态与当前 fencing version 为条件，推进 fence 并清除租约。

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminGuardDecision,
    DeliveryAdminSnapshot,
};

use super::super::transaction::{DeliveryAdminTransactionError, PostgresDeliveryAdminTransaction};

pub(super) async fn apply_delivery_action(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command: &DeliveryAdminCommand,
    current: &DeliveryAdminSnapshot,
    decision: &DeliveryAdminGuardDecision,
) -> Result<&'static str, DeliveryAdminTransactionError> {
    let next_status = decision.transition.next_status;
    let external_reference = match command.action {
        DeliveryAdminAction::ReconcileSucceeded => command.external_reference.as_deref(),
        _ => None,
    };
    let failure_code = match command.action {
        DeliveryAdminAction::ReconcileFailed => command.failure_code.as_deref(),
        _ => None,
    };
    let result = sqlx::query(
        r#"WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
           UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = CASE WHEN $2 = 'pending' THEN NULL ELSE claimed_at END,
               sending_at = CASE WHEN $2 = 'pending' THEN NULL ELSE sending_at END,
               completed_at = CASE
                   WHEN $2 IN ('succeeded', 'failed') THEN (SELECT at FROM sql_clock)
                   ELSE NULL
               END,
               external_reference = $3,
               failure_code = $4,
               result_metadata = $5,
               fencing_version = fencing_version + 1,
               updated_at = (SELECT at FROM sql_clock)
           WHERE id = $1
             AND status = $6
             AND fencing_version = $7"#,
    )
    .bind(command.delivery_id)
    .bind(next_status)
    .bind(external_reference)
    .bind(failure_code)
    .bind(&decision.result_metadata)
    .bind(decision.transition.expected_status)
    .bind(current.fencing_version)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if result.rows_affected() != 1 {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    Ok(next_status)
}

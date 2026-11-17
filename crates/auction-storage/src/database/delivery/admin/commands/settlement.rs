//! delivery Admin 命令对结算履约行的联动写入，以及人工对账成功证明的请求摘要。

use auction_application::core::RequestHash;
use auction_application::delivery::DeliveryAdminError;
use auction_time::Timestamp;
use serde::Serialize;
use uuid::Uuid;

use super::super::transaction::{DeliveryAdminTransactionError, PostgresDeliveryAdminTransaction};

pub(super) async fn reset_settlement_fulfillment(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
) -> Result<(), DeliveryAdminTransactionError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               last_error_code = NULL,
               completed_at = NULL,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE delivery_id = $1
             AND execution_state IN ('failed', 'manual_reconcile')"#,
    )
    .bind(delivery_id)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

pub(super) async fn update_settlement_fulfillment(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
    next_state: &str,
    failure_code: Option<&str>,
    proof_reference: Option<&str>,
    proof_digest: Option<RequestHash>,
) -> Result<(), DeliveryAdminTransactionError> {
    let completed_at = Timestamp::now();
    let proof_digest = proof_digest.map(RequestHash::as_bytes);
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = $2,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               last_error_code = $3,
               proof_kind = CASE WHEN $2 = 'satisfied' THEN 'admin_reconcile' ELSE NULL END,
               proof_reference = $4,
               proof_digest = $5,
               proof_observed_at = CASE WHEN $2 = 'satisfied' THEN $6 ELSE NULL END,
               completed_at = $6,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE delivery_id = $1
             AND execution_state = 'manual_reconcile'"#,
    )
    .bind(delivery_id)
    .bind(next_state)
    .bind(failure_code)
    .bind(proof_reference)
    .bind(proof_digest.as_ref().map(<[u8; 32]>::as_slice))
    .bind(completed_at)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

async fn ensure_settlement_update_or_non_settlement(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
    affected: u64,
) -> Result<(), DeliveryAdminTransactionError> {
    let is_settlement: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments WHERE delivery_id = $1)",
    )
    .bind(delivery_id)
    .fetch_one(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if is_settlement && affected != 1 {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    Ok(())
}

#[derive(Serialize)]
struct AdminProof<'a> {
    external_reference: &'a str,
}

pub(super) fn admin_proof_digest(
    delivery_id: Uuid,
    external_reference: &str,
) -> Result<RequestHash, DeliveryAdminError> {
    RequestHash::from_request(
        "admin_reconcile_delivery_proof",
        delivery_id,
        &AdminProof { external_reference },
    )
    .map_err(|_| DeliveryAdminError::Unavailable)
}

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
    locked_state: Option<&str>,
) -> Result<(), DeliveryAdminTransactionError> {
    let Some(locked_state) = locked_state else { return Ok(()) };
    // 准入用守卫判定的已锁状态；更新仍匹配该状态，失败时整条投递重排回滚。
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
             AND execution_state = $2"#,
    )
    .bind(delivery_id)
    .bind(locked_state)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    require_updated_fulfillment(updated.rows_affected())
}

pub(super) async fn update_settlement_fulfillment(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
    locked_state: Option<&str>,
    next_state: &str,
    failure_code: Option<&str>,
    proof_reference: Option<&str>,
    proof_digest: Option<RequestHash>,
) -> Result<(), DeliveryAdminTransactionError> {
    let Some(locked_state) = locked_state else { return Ok(()) };
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
             AND execution_state = $7"#,
    )
    .bind(delivery_id)
    .bind(next_state)
    .bind(failure_code)
    .bind(proof_reference)
    .bind(proof_digest.as_ref().map(<[u8; 32]>::as_slice))
    .bind(completed_at)
    .bind(locked_state)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    require_updated_fulfillment(updated.rows_affected())
}

fn require_updated_fulfillment(affected: u64) -> Result<(), DeliveryAdminTransactionError> {
    if affected != 1 {
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

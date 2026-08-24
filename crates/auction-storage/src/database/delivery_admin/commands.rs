//! PostgreSQL delivery Admin 人工命令事务与结算履约联动。

use auction_application::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminResult, DeliveryAdminSnapshot,
    RequestHash,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::snapshot;
use super::{map_storage_error, repository_error_owned};
use crate::database::TrackedPool;
use crate::database::delivery_audit::{
    ADMIN_RECONCILE, ADMIN_RELEASE, ADMIN_RETRY_FAILED, DeliveryAudit,
    append as append_delivery_audit,
};

pub(crate) async fn execute(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error_owned("delivery 不存在"))?;
    let current = snapshot(&row)?;
    if current.fencing_version != command.expected_fencing_version {
        return Err(repository_error_owned("delivery 状态或 fencing version 已变化"));
    }

    let next_status = apply_delivery_action(&mut transaction, command, &current).await?;
    if command.action == DeliveryAdminAction::RetryFailed
        || command.action == DeliveryAdminAction::ReconcileRetry
    {
        reset_settlement_fulfillment(&mut transaction, command.delivery_id).await?;
    } else if command.action == DeliveryAdminAction::ReconcileSucceeded {
        let external_reference = command
            .external_reference
            .as_deref()
            .ok_or_else(|| repository_error_owned("人工成功确认缺少外部引用"))?;
        let proof_digest = admin_proof_digest(command.delivery_id, external_reference)?;
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            "satisfied",
            None,
            Some(external_reference),
            Some(proof_digest),
        )
        .await?;
    } else if command.action == DeliveryAdminAction::ReconcileFailed {
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            "failed",
            command.failure_code.as_deref(),
            None,
            None,
        )
        .await?;
    }

    let updated_row = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let snapshot = snapshot(&updated_row)?;
    if snapshot.status != next_status {
        return Err(repository_error_owned("delivery Admin 动作结果状态不一致"));
    }
    let result = DeliveryAdminResult { command_id: command.command_id, snapshot };
    let payload = serde_json::to_value(&result)
        .map_err(|_| repository_error_owned("delivery Admin 结果编码失败"))?;
    let inserted = sqlx::query(
        r#"INSERT INTO public.delivery_admin_commands
               (command_id, command_kind, request_digest, delivery_id, result_payload)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(command.command_id)
    .bind(command.action.command_kind())
    .bind(command.request_hash.as_bytes().to_vec())
    .bind(command.delivery_id)
    .bind(payload)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if inserted.rows_affected() != 1 {
        return Err(repository_error_owned("delivery Admin 命令并发冲突"));
    }
    let (audit_action, admin_action) = match command.action {
        DeliveryAdminAction::Release => (ADMIN_RELEASE, "release"),
        DeliveryAdminAction::RetryFailed => (ADMIN_RETRY_FAILED, "retry_failed"),
        DeliveryAdminAction::ReconcileRetry => (ADMIN_RECONCILE, "reconcile_retry"),
        DeliveryAdminAction::ReconcileSucceeded => (ADMIN_RECONCILE, "reconcile_succeeded"),
        DeliveryAdminAction::ReconcileFailed => (ADMIN_RECONCILE, "reconcile_failed"),
    };
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: result.snapshot.updated_at,
            actor_type: "admin",
            actor_id: &command.actor_id,
            action: audit_action,
            delivery_id: command.delivery_id,
            request_id: command.request_id.as_deref(),
            source: "admin",
            metadata: serde_json::json!({
                "command_id": command.command_id,
                "admin_action": admin_action,
                "status": result.snapshot.status,
                "fencing_version": result.snapshot.fencing_version,
            }),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

async fn existing_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<Option<DeliveryAdminResult>, auction_application::RepositoryError> {
    let Some(row) = sqlx::query(
        "SELECT command_kind, request_digest, delivery_id, result_payload
         FROM public.delivery_admin_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    else {
        return Ok(None);
    };
    let kind: String = row.try_get("command_kind").map_err(map_storage_error)?;
    let digest: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    let delivery_id: Uuid = row.try_get("delivery_id").map_err(map_storage_error)?;
    if kind != command.action.command_kind()
        || digest != command.request_hash.as_bytes()
        || delivery_id != command.delivery_id
    {
        return Err(repository_error_owned("delivery Admin 命令摘要冲突"));
    }
    let payload: Value = row.try_get("result_payload").map_err(map_storage_error)?;
    serde_json::from_value(payload)
        .map(Some)
        .map_err(|_| repository_error_owned("delivery Admin 命令结果损坏"))
}

async fn apply_delivery_action(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &DeliveryAdminCommand,
    current: &DeliveryAdminSnapshot,
) -> Result<&'static str, auction_application::RepositoryError> {
    let (expected_status, next_status) = match command.action {
        DeliveryAdminAction::Release => ("claimed", "pending"),
        DeliveryAdminAction::RetryFailed => ("failed", "pending"),
        DeliveryAdminAction::ReconcileRetry => ("manual_reconcile", "pending"),
        DeliveryAdminAction::ReconcileSucceeded => ("manual_reconcile", "succeeded"),
        DeliveryAdminAction::ReconcileFailed => ("manual_reconcile", "failed"),
    };
    if current.status != expected_status {
        return Err(repository_error_owned("delivery 当前状态不允许该 Admin 动作"));
    }
    let (external_reference, failure_code, metadata, completed_at) = match command.action {
        DeliveryAdminAction::ReconcileSucceeded => (
            command.external_reference.as_deref(),
            None,
            serde_json::json!({"resolution":"admin_reconciled_succeeded"}),
            Some(Utc::now()),
        ),
        DeliveryAdminAction::ReconcileFailed => (
            None,
            command.failure_code.as_deref(),
            serde_json::json!({"resolution":"admin_reconciled_failed"}),
            Some(Utc::now()),
        ),
        _ => (None, None, serde_json::json!({}), None),
    };
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = CASE WHEN $2 = 'pending' THEN NULL ELSE claimed_at END,
               sending_at = CASE WHEN $2 = 'pending' THEN NULL ELSE sending_at END,
               completed_at = $3,
               external_reference = $4,
               failure_code = $5,
               result_metadata = $6,
               fencing_version = fencing_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = $7
             AND fencing_version = $8"#,
    )
    .bind(command.delivery_id)
    .bind(next_status)
    .bind(completed_at)
    .bind(external_reference)
    .bind(failure_code)
    .bind(metadata)
    .bind(expected_status)
    .bind(command.expected_fencing_version)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery Admin 动作的 lease/fence 已变化"));
    }
    Ok(next_status)
}

async fn reset_settlement_fulfillment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    delivery_id: Uuid,
) -> Result<(), auction_application::RepositoryError> {
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
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

async fn update_settlement_fulfillment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    delivery_id: Uuid,
    next_state: &str,
    failure_code: Option<&str>,
    proof_reference: Option<&str>,
    proof_digest: Option<RequestHash>,
) -> Result<(), auction_application::RepositoryError> {
    let completed_at = Utc::now();
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
    .bind(proof_digest.map(|value| value.as_bytes().to_vec()))
    .bind(completed_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

async fn ensure_settlement_update_or_non_settlement(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    delivery_id: Uuid,
    affected: u64,
) -> Result<(), auction_application::RepositoryError> {
    let is_settlement: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments WHERE delivery_id = $1)",
    )
    .bind(delivery_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if is_settlement && affected != 1 {
        return Err(repository_error_owned("结算履约状态不允许该 Admin 动作"));
    }
    Ok(())
}

#[derive(Serialize)]
struct AdminProof<'a> {
    external_reference: &'a str,
}

fn admin_proof_digest(
    delivery_id: Uuid,
    external_reference: &str,
) -> Result<RequestHash, auction_application::RepositoryError> {
    RequestHash::from_request(
        "admin_reconcile_delivery_proof",
        delivery_id,
        &AdminProof { external_reference },
    )
    .map_err(|_| repository_error_owned("人工对账证明摘要生成失败"))
}

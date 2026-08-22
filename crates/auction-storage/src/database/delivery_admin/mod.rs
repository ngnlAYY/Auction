//! PostgreSQL delivery Admin 查询和人工状态命令适配器。
//!
//! Admin 命令使用独立的 `delivery_admin_commands` 账本，并以 fencing version 作为版本锁。
//! payload、lease token 和原始 result metadata 永远不会被投影到 HTTP；结算履约的对应状态
//! 变更与 delivery 状态在同一事务内提交，避免人工重试后两张表分叉。

use auction_application::{
    ApplicationError, DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminListRequest,
    DeliveryAdminPage, DeliveryAdminPort, DeliveryAdminResult,
};

#[cfg(feature = "postgres")]
use auction_application::{DeliveryAdminSnapshot, RequestHash};
#[cfg(feature = "postgres")]
use chrono::Utc;
#[cfg(feature = "postgres")]
use serde::Serialize;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, Row, postgres::PgRow};

impl DeliveryAdminPort for DatabaseStore {
    fn list(
        &self,
        request: DeliveryAdminListRequest,
    ) -> auction_application::ApplicationFuture<'_, DeliveryAdminPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(repository_error("delivery Admin 列表参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    list_postgres(store.pool(), &request).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery Admin 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn execute(
        &self,
        command: DeliveryAdminCommand,
    ) -> auction_application::ApplicationFuture<'_, DeliveryAdminResult> {
        Box::pin(async move {
            validate_command(&command)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    execute_postgres(store.pool(), &command).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery Admin 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    pool: &PgPool,
    request: &DeliveryAdminListRequest,
) -> Result<DeliveryAdminPage, auction_application::RepositoryError> {
    let status = request.status.as_deref();
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM public.deliveries
         WHERE $1::text IS NULL OR status = $1::text",
    )
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, status,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries
           WHERE $1::text IS NULL OR status = $1::text
           ORDER BY created_at, id
           LIMIT $2 OFFSET $3"#,
    )
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(pool)
    .await
    .map_err(map_storage_error)?;
    let entries = rows.iter().map(map_snapshot).collect::<Result<Vec<_>, _>>()?;
    Ok(DeliveryAdminPage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "postgres")]
async fn execute_postgres(
    pool: &PgPool,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, status,
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
    let current = map_snapshot(&row)?;
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
        r#"SELECT id, origin_kind, origin_id, recipient, status,
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
    let snapshot = map_snapshot(&updated_row)?;
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
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
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
    let payload: serde_json::Value = row.try_get("result_payload").map_err(map_storage_error)?;
    serde_json::from_value(payload)
        .map(Some)
        .map_err(|_| repository_error_owned("delivery Admin 命令结果损坏"))
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct AdminProof<'a> {
    external_reference: &'a str,
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
fn map_snapshot(
    row: &PgRow,
) -> Result<DeliveryAdminSnapshot, auction_application::RepositoryError> {
    let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|_| repository_error_owned("delivery 尝试次数无效"))?;
    let snapshot = DeliveryAdminSnapshot {
        id: row.try_get("id").map_err(map_storage_error)?,
        origin_kind: row.try_get("origin_kind").map_err(map_storage_error)?,
        origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        status: row.try_get("status").map_err(map_storage_error)?,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_storage_error)?,
        fencing_version: row.try_get("fencing_version").map_err(map_storage_error)?,
        attempt_count,
        claimed_at: row.try_get("claimed_at").map_err(map_storage_error)?,
        sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
        completed_at: row.try_get("completed_at").map_err(map_storage_error)?,
        external_reference: row.try_get("external_reference").map_err(map_storage_error)?,
        failure_code: row.try_get("failure_code").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    if snapshot.id.is_nil()
        || snapshot.origin_kind.is_empty()
        || snapshot.origin_id != snapshot.origin_id.trim()
        || snapshot.origin_id.is_empty()
        || snapshot.recipient != snapshot.recipient.trim()
        || snapshot.recipient.is_empty()
        || snapshot.fencing_version < 0
        || snapshot.updated_at < snapshot.created_at
    {
        return Err(repository_error_owned("delivery Admin 投影不满足不变量"));
    }
    Ok(snapshot)
}

fn validate_command(command: &DeliveryAdminCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.delivery_id.is_nil()
        || command.expected_fencing_version <= 0
        || command.actor_id.is_empty()
        || command.actor_id != command.actor_id.trim()
        || command
            .request_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value != value.trim())
        || command.request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error("delivery Admin 命令参数无效"));
    }
    match command.action {
        DeliveryAdminAction::Release
        | DeliveryAdminAction::RetryFailed
        | DeliveryAdminAction::ReconcileRetry
            if command.external_reference.is_none() && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileSucceeded
            if command
                .external_reference
                .as_ref()
                .is_some_and(|value| !value.is_empty() && value == value.trim())
                && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileFailed
            if command.external_reference.is_none()
                && command
                    .failure_code
                    .as_ref()
                    .is_some_and(|value| !value.is_empty() && value == value.trim()) => {}
        _ => return Err(repository_error("delivery Admin 命令结果字段不匹配")),
    }
    Ok(())
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "delivery Admin 记录不存在".to_owned(),
        _ => "delivery Admin 数据库操作失败".to_owned(),
    })
}

//! 操作者确认“历史失败已处置”的单事务命令。
//!
//! 该命令不改变投递状态、资金、履约或资产事实，只追加一条处置事实与同事务审计，让读侧
//! 停止把没有资金或资产影响的历史失败当作待处理项。资格由最终事务重新读取，不能把
//! 形状校验或已提交预览当作处置依据。

use auction_application::delivery::{
    DeliveryAcknowledgementCommand, DeliveryAcknowledgementResult, DeliveryDispositionCommandError,
};
use auction_domain::{
    DeliveryAcknowledgementFacts, DeliveryDispositionKind, DeliveryDispositionSource,
    validate_acknowledgement,
};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::delivery_audit::{self, DeliveryAudit};
use crate::database::{
    SERIALIZABLE_ATTEMPTS, TrackedPool, TrackedTransaction, observed_serialization_conflict,
    retry_conflicts,
};

pub(super) async fn acknowledge(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAcknowledgementCommand,
) -> Result<DeliveryAcknowledgementResult, DeliveryDispositionCommandError> {
    command.validate()?;
    retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::DeliveryDisposition,
        SERIALIZABLE_ATTEMPTS,
        |error: &TransactionError| error.is_retryable(),
        || async move { acknowledge_transaction(pool, command).await },
    )
    .await
    .map_err(TransactionError::into_command_error)
}

/// 事务内部错误：原始 SQLx 错误必须保留到最外层重试分类。
enum TransactionError {
    Sqlx(sqlx::Error),
    Business(DeliveryDispositionCommandError),
}

impl TransactionError {
    /// 只有明确的序列化冲突或死锁允许换新事务重试。
    fn is_retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => observed_serialization_conflict(error),
            Self::Business(_) => false,
        }
    }

    fn into_command_error(self) -> DeliveryDispositionCommandError {
        match self {
            Self::Sqlx(_) => DeliveryDispositionCommandError::Unavailable,
            Self::Business(error) => error,
        }
    }
}

impl From<sqlx::Error> for TransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<DeliveryDispositionCommandError> for TransactionError {
    fn from(error: DeliveryDispositionCommandError) -> Self {
        Self::Business(error)
    }
}

impl From<auction_domain::DeliveryDispositionError> for TransactionError {
    fn from(error: auction_domain::DeliveryDispositionError) -> Self {
        Self::Business(DeliveryDispositionCommandError::from(error))
    }
}

async fn acknowledge_transaction(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAcknowledgementCommand,
) -> Result<DeliveryAcknowledgementResult, TransactionError> {
    let mut transaction = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    // 同一命令 ID 的并发入口统一为 advisory lock；与其它命令 owner 使用同一键空间。
    crate::database::lock_command_id(&mut transaction, command.command_id).await?;
    if let Some(result) = replay(&mut transaction, command).await? {
        transaction.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"SELECT origin_kind, status, fencing_version,
                  CASE WHEN payload ? 'credits'
                       THEN CASE WHEN payload ->> 'credits' ~ '^-?[0-9]{1,18}$'
                                 THEN (payload ->> 'credits')::bigint END
                       ELSE 0 END AS credits,
                  CASE WHEN payload ? 'cod'
                       THEN CASE WHEN payload ->> 'cod' ~ '^-?[0-9]{1,18}$'
                                 THEN (payload ->> 'cod')::bigint END
                       ELSE 0 END AS cod,
                  CASE WHEN payload ? 'attachment_ids'
                       THEN CASE WHEN jsonb_typeof(payload -> 'attachment_ids') = 'array'
                                 THEN jsonb_array_length(payload -> 'attachment_ids') END
                       ELSE 0 END AS equipment_count,
                  EXISTS (
                      SELECT 1 FROM public.settlement_obligation_fulfillments AS fulfillment
                      WHERE fulfillment.delivery_id = deliveries.id
                  ) AS has_fulfillment_obligation
           FROM public.deliveries AS deliveries
           WHERE deliveries.id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(DeliveryDispositionCommandError::NotFound)?;
    let fencing_version: i64 = row.try_get("fencing_version")?;
    if fencing_version != command.expected_fencing_version {
        return Err(DeliveryDispositionCommandError::Stale.into());
    }
    let origin_kind: String = row.try_get("origin_kind")?;
    let status: String = row.try_get("status")?;
    let acknowledged = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM public.delivery_dispositions
             WHERE delivery_id = $1 AND fencing_version = $2 AND resolves_alert
         )",
    )
    .bind(command.delivery_id)
    .bind(fencing_version)
    .fetch_one(&mut *transaction)
    .await?;
    if acknowledged {
        return Err(DeliveryDispositionCommandError::AlreadyDisposed.into());
    }
    validate_acknowledgement(DeliveryAcknowledgementFacts {
        status: &status,
        origin_kind: &origin_kind,
        credits: row.try_get("credits")?,
        cod: row.try_get("cod")?,
        equipment_count: row
            .try_get::<Option<i64>, _>("equipment_count")?
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(DeliveryDispositionCommandError::NotAcknowledgeable)?,
        has_fulfillment_obligation: row.try_get("has_fulfillment_obligation")?,
    })?;

    // 同一事务内的审计是处置事实的来源引用；任一步失败都回滚整条命令。
    let audit_id = delivery_audit::append(
        &mut transaction,
        DeliveryAudit {
            occurred_at: chrono::Utc::now(),
            actor_type: "admin",
            actor_id: &command.actor_id,
            action: delivery_audit::OPERATOR_ACKNOWLEDGED,
            delivery_id: command.delivery_id,
            request_id: command.request_id.as_deref(),
            source: command.actor_source.as_str(),
            metadata: auction_application::core::cli_audit_metadata(
                command.operator_identity.as_ref(),
                json!({
                    "command_id": command.command_id,
                    "kind": DeliveryDispositionKind::OperatorAcknowledged.as_str(),
                    "fencing_version": fencing_version,
                    "status": status,
                    "fund_impact": false,
                    "asset_impact": false,
                    "executor": "auction-cli",
                    "reason": &command.reason,
                }),
            ),
        },
    )
    .await?;

    super::append(
        &mut transaction,
        super::DeliveryDispositionFact {
            delivery_id: command.delivery_id,
            fencing_version,
            kind: DeliveryDispositionKind::OperatorAcknowledged,
            source: DeliveryDispositionSource { audit_id: Some(audit_id), ..Default::default() },
            actor_source: command.actor_source,
            actor_id: &command.actor_id,
            reason: Some(&command.reason),
            reason_code: None,
            detail: Some(&json!({
                "status": status,
                "fund_impact": false,
                "asset_impact": false,
                "origin_kind": origin_kind,
            })),
            command_id: Some(command.command_id),
            request_digest: Some(command.request_hash.as_bytes()),
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(DeliveryAcknowledgementResult {
        command_id: command.command_id,
        delivery_id: command.delivery_id,
        fencing_version,
        kind: DeliveryDispositionKind::OperatorAcknowledged.as_str().to_owned(),
        replayed: false,
    })
}

/// 已提交的同 ID 命令只按原摘要与目标重放；异参一律冲突，不重新判断资格。
async fn replay(
    transaction: &mut TrackedTransaction<'_, Postgres>,
    command: &DeliveryAcknowledgementCommand,
) -> Result<Option<DeliveryAcknowledgementResult>, TransactionError> {
    let row = sqlx::query(
        "SELECT delivery_id, fencing_version, request_digest, kind
         FROM public.delivery_dispositions WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let kind: String = row.try_get("kind")?;
    let delivery_id: Uuid = row.try_get("delivery_id")?;
    let fencing_version: i64 = row.try_get("fencing_version")?;
    let digest: Option<Vec<u8>> = row.try_get("request_digest")?;
    let expected_digest = command.request_hash.as_bytes();
    if kind != DeliveryDispositionKind::OperatorAcknowledged.as_str()
        || delivery_id != command.delivery_id
        || fencing_version != command.expected_fencing_version
        || digest.as_deref() != Some(expected_digest.as_slice())
    {
        return Err(DeliveryDispositionCommandError::CommandConflict.into());
    }
    Ok(Some(DeliveryAcknowledgementResult {
        command_id: command.command_id,
        delivery_id,
        fencing_version,
        kind,
        replayed: true,
    }))
}

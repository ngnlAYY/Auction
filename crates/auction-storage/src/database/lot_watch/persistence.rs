//! 标记持久错误分类、追加审计和通知资格失效；不修改 delivery 历史结果。

use auction_application::CompetitionTransactionError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) fn sql_error(error: sqlx::Error) -> CompetitionTransactionError {
    if crate::database::is_serialization_conflict(&error) {
        CompetitionTransactionError::SerializationConflict
    } else {
        CompetitionTransactionError::Failed("装备标记持久化失败".to_owned())
    }
}

/// 只追加安全状态，不保存代理上限、装备链接或邮件正文。
pub(super) async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
    action: &str,
    metadata: serde_json::Value,
) -> Result<(), CompetitionTransactionError> {
    use crate::database::audit_health::AuditWriteResult as _;
    sqlx::query(
        "INSERT INTO public.audit_records
        (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
        VALUES ($1, 'system', $2, $3, 'lot', $4, 'success', 'system', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id.to_string())
    .bind(action)
    .bind(lot_id.to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(sql_error)?;
    Ok(())
}

/// 取消和重新领先永久撤销旧资格；已经 sending 的结果仍由 delivery owner 追加保留。
pub(super) async fn invalidate(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
    reason: &str,
) -> Result<(), CompetitionTransactionError> {
    let count = sqlx::query(
        "UPDATE public.lot_watch_notifications
        SET invalidated_at = clock_timestamp(), invalidation_reason = $3
        WHERE user_id = $1 AND lot_id = $2 AND invalidated_at IS NULL",
    )
    .bind(user_id)
    .bind(lot_id)
    .bind(reason)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?
    .rows_affected();
    if count > 0 {
        audit(
            transaction,
            user_id,
            lot_id,
            "lot_watch.invalidate",
            serde_json::json!({"reason": reason, "count": count}),
        )
        .await?;
    }
    Ok(())
}

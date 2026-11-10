//! 标记持久错误分类、追加审计和通知资格失效；不修改 delivery 历史结果。

use crate::database::TrackedTransaction as Transaction;
use auction_application::core::CompetitionTransactionError;
use sqlx::Postgres;
use uuid::Uuid;

pub(super) fn sql_error(error: sqlx::Error) -> CompetitionTransactionError {
    if crate::database::observed_serialization_conflict(&error) {
        CompetitionTransactionError::SerializationConflict
    } else {
        CompetitionTransactionError::Failed(crate::database::sql_error_context(
            &error,
            "装备标记持久化失败",
        ))
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
    use crate::database::operations::audit_health::AuditWriteResult as _;
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
    // 慢代理待定提醒也必须随取消/重新领先退役；只清普通通知会让旧 pending 行在用户
    // 重新 watch 后被 resolve_pending 复活。
    let pending_reason = if reason == "unwatched" { "watch_removed" } else { "leading_again" };
    let pending_count = sqlx::query(
        "UPDATE public.lot_watch_pending_notifications
         SET status = 'retired', resolution_reason = $3, resolved_at = clock_timestamp()
         WHERE user_id = $1 AND lot_id = $2 AND status = 'pending'",
    )
    .bind(user_id)
    .bind(lot_id)
    .bind(pending_reason)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?
    .rows_affected();
    if count > 0 || pending_count > 0 {
        audit(
            transaction,
            user_id,
            lot_id,
            "lot_watch.invalidate",
            serde_json::json!({"reason": reason, "count": count, "pending_count": pending_count}),
        )
        .await?;
    }
    Ok(())
}

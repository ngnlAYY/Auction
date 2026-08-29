//! SQLite 竞价事务的追加式审计写入。

use super::super::mapping::*;
use super::super::*;

/// SQLite Lite 同样写入每个外部 Public 竞价命令唯一的一条旧版审计。
pub(in crate::database::sqlite::competition) async fn insert_audit_for_public_command(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
) -> Result<(), CompetitionTransactionError> {
    let record =
        crate::database::competition_audit::competition_command_audit_record(plan, context)?;
    let metadata = serde_json::to_string(&record.metadata).map_err(|_| invalid_record())?;
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'success', NULL, ?, ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(record.occurred_at))
    .bind(record.actor_type)
    .bind(record.actor_id)
    .bind(record.action)
    .bind(record.entity_type)
    .bind(record.entity_id)
    .bind(record.source)
    .bind(metadata)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

/// SQLite Lite 同样写入旧版慢速代理 Worker 审计，并显式保存数据库执行时间。
pub(in crate::database::sqlite::competition) async fn insert_audit_for_slow_proxy(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
    context: SlowProxyExecutionContext,
) -> Result<(), CompetitionTransactionError> {
    context.validate()?;
    let mut metadata = json!({
        "lot_id": plan.final_lot.lot_id.as_uuid(),
        "authorization_id": context.authorization_id.as_uuid(),
        "generation": context.generation,
        "due_at": context.due_at,
        "price_changed": !plan.public_bids.is_empty(),
    });
    if let Some(next_due_at) = plan.final_lot.slow_due_at {
        metadata["next_due_at"] = json!(next_due_at);
    }
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES (?, ?, 'system', 'auction-worker', 'lot.slow_proxy_tick', 'lot', ?,
                 'success', NULL, 'worker', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(context.executed_at))
    .bind(plan.final_lot.lot_id.as_uuid().to_string())
    .bind(serde_json::to_string(&metadata).map_err(|_| invalid_record())?)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

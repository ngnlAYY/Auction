//! PostgreSQL 竞价事务的追加式审计写入。

use super::super::mapping::*;
use crate::database::audit_health::AuditWriteResult;
use crate::database::competition::PostgresCompetitionTransaction;
use auction_application::{
    CompetitionCommandAuditContext, CompetitionTransactionError, SlowProxyExecutionContext,
};
use auction_domain::{AcceptedBid, CompetitionPlan};
use serde_json::json;
use uuid::Uuid;

/// 写入 Auction-BF 每个外部 Public 竞价命令唯一的一条审计。
pub(in crate::database::competition) async fn insert_audit_for_public_command(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
) -> Result<(), CompetitionTransactionError> {
    let record =
        crate::database::competition_audit::competition_command_audit_record(plan, context)?;
    let database = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'success', NULL, $8, $9)"#,
    )
    .bind(Uuid::new_v4())
    .bind(record.occurred_at)
    .bind(record.actor_type)
    .bind(record.actor_id)
    .bind(record.action)
    .bind(record.entity_type)
    .bind(record.entity_id)
    .bind(record.source)
    .bind(record.metadata)
    .execute(&mut **database)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

pub(in crate::database::competition) async fn insert_audit_for_bid(
    transaction: &mut PostgresCompetitionTransaction,
    bid: &AcceptedBid,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "bid_id": bid.id.as_uuid(),
        "command_id": bid.command_id.as_uuid(),
        "amount": bid.amount.value(),
        "mechanism": bid.mechanism.as_str(),
        "state_version": bid.state_version,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'public_user', $2, 'place_bid', 'lot', $3, 'success', $4, 'public', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(bid.bidder_id.as_uuid().to_string())
    .bind(bid.lot_id.as_uuid().to_string())
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

pub(in crate::database::competition) async fn insert_audit_for_proxy_changes(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
    command_id: auction_domain::CommandId,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    for transition in &plan.proxy_transitions {
        let metadata = json!({
            "command_id": command_id.as_uuid(),
            "authorization_id": transition.authorization_id.as_uuid(),
            "transition_kind": transition.kind.as_str(),
            "state_version": transition.state_version,
        });
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES ($1, 'system', 'auction-application', 'proxy_transition',
                     'proxy_authorization', $2, 'success', $3, 'system', $4)"#,
        )
        .bind(Uuid::new_v4())
        .bind(transition.authorization_id.as_uuid().to_string())
        .bind(request_id)
        .bind(metadata)
        .execute(&mut **transaction)
        .await
        .record_audit_failure()
        .map_err(map_sql_error)?;
    }
    Ok(())
}

/// 写入 Auction-BF 已冻结的慢速代理 Worker 审计；不把它伪装成 Public 用户出价。
pub(in crate::database::competition) async fn insert_audit_for_slow_proxy(
    transaction: &mut PostgresCompetitionTransaction,
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
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'system', 'auction-worker', 'lot.slow_proxy_tick', 'lot', $3,
                 'success', NULL, 'worker', $4)"#,
    )
    .bind(Uuid::new_v4())
    .bind(context.executed_at)
    .bind(plan.final_lot.lot_id.as_uuid().to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

//! SQLite 竞价事务的追加式审计写入。

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::sqlite::competition) async fn insert_audit_for_bid(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "bid_id": bid.id.as_uuid(),
        "command_id": bid.command_id.as_uuid(),
        "amount": bid.amount.value(),
        "mechanism": bid_mechanism(bid.mechanism),
        "state_version": bid.state_version,
    });
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES (?, 'public_user', ?, 'place_bid', 'lot', ?, 'success', ?, 'public', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(bid.bidder_id.as_uuid().to_string())
    .bind(bid.lot_id.as_uuid().to_string())
    .bind(request_id)
    .bind(serde_json::to_string(&metadata).map_err(|_| invalid_record())?)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

pub(in crate::database::sqlite::competition) async fn insert_audit_for_proxy_changes(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
    command_id: auction_domain::CommandId,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    for transition in &plan.proxy_transitions {
        let metadata = json!({
            "command_id": command_id.as_uuid(),
            "authorization_id": transition.authorization_id.as_uuid(),
            "transition_kind": proxy_transition_kind(transition.kind),
            "state_version": transition.state_version,
        });
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES (?, 'system', 'auction-application', 'proxy_transition',
                     'proxy_authorization', ?, 'success', ?, 'system', ?)",
        )
        .bind(uuid_blob(Uuid::new_v4()))
        .bind(transition.authorization_id.as_uuid().to_string())
        .bind(request_id)
        .bind(serde_json::to_string(&metadata).map_err(|_| invalid_record())?)
        .execute(&mut *connection)
        .await
        .record_audit_failure()
        .map_err(map_sql_error)?;
    }
    Ok(())
}

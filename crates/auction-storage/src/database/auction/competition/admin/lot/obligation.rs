//! 撤拍装备的单件零金额寄回义务写入。

use auction_application::admin::{AdminLotUpdateCommand, AdminLotUpdateSnapshot};
use auction_application::core::CompetitionTransactionError;
use auction_domain::deterministic_uuid;
use chrono::{DateTime, Utc};

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::mapping::invalid_state;
use crate::database::auction::competition::mapping::map_sql_error;

pub(super) async fn insert_withdrawn_return_obligation_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    snapshot: &AdminLotUpdateSnapshot,
    now: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let seller = snapshot.lot.seller.as_deref().ok_or(CompetitionTransactionError::InvalidState)?;
    if !matches!(snapshot.world_id.as_str(), "main" | "isekai") {
        return Err(invalid_state("withdrawn-return 世界不在允许集合内"));
    }
    let obligation_id = deterministic_uuid(
        command.command_id,
        &format!("settlement-obligation:{}:withdrawn_return", snapshot.lot.id),
        1,
    );
    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, state_version, recipient_user_id,
                 recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
                 prerequisite_obligation_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn_return', 'ready', 1, NULL,
                     NULLIF($7, ''), 0, 0, $8, $9, NULL, $10, $10)",
        )
        .bind(obligation_id)
        .bind(snapshot.lot.auction_id.as_uuid())
        .bind(snapshot.lot.id.as_uuid())
        .bind(&snapshot.world_id)
        .bind(command.command_id.as_uuid())
        .bind(snapshot.policy_version)
        .bind(seller)
        .bind(snapshot.lot.equipment_id.as_uuid())
        .bind(snapshot.lot.submission_id.map(auction_domain::SubmissionId::as_uuid))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation)
             VALUES ($1, 'pending', $2, 'send')",
        )
        .bind(obligation_id)
        .bind(&snapshot.world_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

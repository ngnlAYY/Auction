//! PostgreSQL 管理端开拍前计划时间修改及其审计/实时记录。

use crate::database::audit_health::AuditWriteResult;
use crate::database::competition::PostgresCompetitionTransaction;
use crate::database::competition::admin::{invalid_record, invalid_state, map_sql_error};
use auction_application::admin::{
    AdminAuctionScheduleSnapshot, AdminAuctionScheduleTransaction, AdminAuctionSummary,
    AdminUpdateAuctionScheduleCommand,
};
use auction_application::core::CompetitionTransactionError;
use auction_application::settlement::SettlementAuctionSnapshot;
use auction_domain::AuctionId;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

#[allow(clippy::manual_async_fn)]
impl AdminAuctionScheduleTransaction for PostgresCompetitionTransaction {
    fn load_admin_auction_schedule(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<
        Output = Result<Option<AdminAuctionScheduleSnapshot>, CompetitionTransactionError>,
    > + Send {
        async move { load_admin_auction_schedule_inner(self, auction_id).await }
    }

    fn update_admin_auction_schedule(
        &mut self,
        command: AdminUpdateAuctionScheduleCommand,
        snapshot: AdminAuctionScheduleSnapshot,
        updated_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send {
        async move { update_admin_auction_schedule_inner(self, &command, snapshot, updated_at).await }
    }

    fn append_admin_auction_schedule_record(
        &mut self,
        command: AdminUpdateAuctionScheduleCommand,
        previous_preview_scheduled_at: DateTime<Utc>,
        previous_scheduled_open_at: DateTime<Utc>,
        previous_scheduled_end_at: DateTime<Utc>,
        result: AdminAuctionSummary,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move {
            append_admin_auction_schedule_record_inner(
                self,
                &command,
                previous_preview_scheduled_at,
                previous_scheduled_open_at,
                previous_scheduled_end_at,
                &result,
                changed,
            )
            .await
        }
    }
}

async fn load_admin_auction_schedule_inner(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AdminAuctionScheduleSnapshot>, CompetitionTransactionError> {
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"WITH target_auction AS MATERIALIZED (
                   SELECT auction.id, auction.world_id
                   FROM public.auctions AS auction
                   WHERE auction.id = $1
               ),
               locked_world AS MATERIALIZED (
                   SELECT world.id
                   FROM public.worlds AS world
                   INNER JOIN target_auction ON target_auction.world_id = world.id
                   FOR UPDATE OF world
               ),
               locked_round AS MATERIALIZED (
                   SELECT round.id, round.state_version, round.preview_due_at,
                          round.open_due_at, round.scheduled_end_at,
                          round.preview_completed_at, round.open_completed_at,
                          round.final_completed_at, round.lease_owner, round.lease_until,
                          round.schedule_snapshot
                   FROM public.scheduled_rounds AS round
                   INNER JOIN target_auction ON target_auction.id = round.auction_id
                   INNER JOIN locked_world ON locked_world.id = target_auction.world_id
                   WHERE round.status = 'active'
                   FOR UPDATE OF round
               )
             SELECT
                a.id, a.world_id, a.sequence, a.status, a.preview_scheduled_at,
                a.scheduled_open_at, a.preview_opened_at, a.preview_manual_open,
                a.preview_revision, a.opened_at, a.final_phase_at, a.settled_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by, a.state_version,
                a.scheduled_end_at, a.effective_end_at, a.created_at, a.updated_at,
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
                    AS policy_scheduled_end_at,
                locked_round.id AS round_id,
                locked_round.state_version AS round_state_version,
                locked_round.preview_due_at AS round_preview_due_at,
                locked_round.open_due_at AS round_open_due_at,
                locked_round.scheduled_end_at AS round_scheduled_end_at,
                locked_round.preview_completed_at,
                locked_round.open_completed_at,
                locked_round.final_completed_at,
                locked_round.lease_owner,
                locked_round.lease_until,
                locked_round.schedule_snapshot,
                (SELECT COUNT(*)::bigint FROM public.lots AS lot WHERE lot.auction_id = a.id)
                    AS lot_count
             FROM public.auctions AS a
             INNER JOIN target_auction ON target_auction.id = a.id
             INNER JOIN locked_world ON locked_world.id = a.world_id
             INNER JOIN locked_round ON TRUE
             FOR UPDATE OF a"#,
        )
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let policy_scheduled_end_at: DateTime<Utc> =
        row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?;
    let summary = map_schedule_summary(&row, Some(policy_scheduled_end_at))?;
    let schedule_source = row
        .try_get::<Option<Value>, _>("schedule_snapshot")
        .map_err(map_sql_error)?
        .map(serde_json::from_value::<auction_domain::AuctionScheduleSource>)
        .transpose()
        .map_err(|_| invalid_state("Admin 拍卖排期来源无效"))?;
    let snapshot = AdminAuctionScheduleSnapshot {
        schedule_source,
        summary,
        policy_scheduled_end_at,
        round_id: row.try_get("round_id").map_err(map_sql_error)?,
        round_state_version: row.try_get("round_state_version").map_err(map_sql_error)?,
        round_preview_due_at: row.try_get("round_preview_due_at").map_err(map_sql_error)?,
        round_open_due_at: row.try_get("round_open_due_at").map_err(map_sql_error)?,
        round_scheduled_end_at: row.try_get("round_scheduled_end_at").map_err(map_sql_error)?,
        preview_completed_at: row.try_get("preview_completed_at").map_err(map_sql_error)?,
        open_completed_at: row.try_get("open_completed_at").map_err(map_sql_error)?,
        final_completed_at: row.try_get("final_completed_at").map_err(map_sql_error)?,
        lease_owner: row.try_get("lease_owner").map_err(map_sql_error)?,
        lease_until: row.try_get("lease_until").map_err(map_sql_error)?,
    };
    if !snapshot.validate() {
        return Err(invalid_state("Admin 拍卖计划锁内快照不一致"));
    }
    Ok(Some(snapshot))
}

async fn update_admin_auction_schedule_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminUpdateAuctionScheduleCommand,
    snapshot: AdminAuctionScheduleSnapshot,
    updated_at: DateTime<Utc>,
) -> Result<AdminAuctionSummary, CompetitionTransactionError> {
    let previous_state_version = snapshot.summary.auction.state_version;
    let previous_round_state_version = snapshot.round_state_version;
    let previous_preview_scheduled_at = snapshot.summary.auction.preview_scheduled_at;
    let previous_scheduled_open_at = snapshot.summary.auction.scheduled_open_at;
    let previous_scheduled_end_at = snapshot.policy_scheduled_end_at;
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"UPDATE public.auctions
               SET preview_scheduled_at = $2,
                   scheduled_open_at = $3,
                   policy_snapshot = jsonb_set(
                       policy_snapshot,
                       '{scheduled_end_at}',
                       to_jsonb($4::timestamptz),
                       false
                   ),
                   state_version = state_version + 1,
                   updated_at = $5
               WHERE id = $1
                 AND state_version = $6
                 AND status IN ('draft', 'preview')
                 AND scheduled_end_at IS NULL
                 AND effective_end_at IS NULL
                 AND preview_scheduled_at = $7
                 AND scheduled_open_at = $8
                 AND (policy_snapshot ->> 'scheduled_end_at')::timestamptz = $9
               RETURNING state_version, updated_at"#,
        )
        .bind(command.auction_id.as_uuid())
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(command.scheduled_end_at)
        .bind(updated_at)
        .bind(previous_state_version)
        .bind(previous_preview_scheduled_at)
        .bind(previous_scheduled_open_at)
        .bind(previous_scheduled_end_at)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Err(CompetitionTransactionError::StaleState);
    };
    let state_version: i64 = row.try_get("state_version").map_err(map_sql_error)?;
    let stored_updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(map_sql_error)?;

    let round_row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"UPDATE public.scheduled_rounds
               SET preview_due_at = $2,
                   open_due_at = $3,
                   scheduled_end_at = $4,
                   schedule_snapshot = $15,
                   lease_owner = NULL,
                   lease_until = NULL,
                   state_version = state_version + 1,
                   updated_at = $5
               WHERE id = $1
                 AND status = 'active'
                 AND state_version = $6
                 AND preview_due_at = $7
                 AND open_due_at = $8
                 AND scheduled_end_at = $9
                 AND preview_completed_at IS NOT DISTINCT FROM $10
                 AND open_completed_at IS NOT DISTINCT FROM $11
                 AND final_completed_at IS NOT DISTINCT FROM $12
                 AND lease_owner IS NOT DISTINCT FROM $13
                 AND lease_until IS NOT DISTINCT FROM $14
               RETURNING state_version"#,
        )
        .bind(snapshot.round_id)
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(command.scheduled_end_at)
        .bind(updated_at)
        .bind(snapshot.round_state_version)
        .bind(snapshot.round_preview_due_at)
        .bind(snapshot.round_open_due_at)
        .bind(snapshot.round_scheduled_end_at)
        .bind(snapshot.preview_completed_at)
        .bind(snapshot.open_completed_at)
        .bind(snapshot.final_completed_at)
        .bind(snapshot.lease_owner.as_deref())
        .bind(snapshot.lease_until)
        .bind(
            snapshot
                .schedule_source
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| invalid_record())?,
        )
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(round_row) = round_row else { return Err(CompetitionTransactionError::StaleState) };
    let round_state_version: i64 = round_row.try_get("state_version").map_err(map_sql_error)?;

    let mut result = snapshot.summary;
    result.auction.state_version = state_version;
    result.auction.updated_at = stored_updated_at;
    result.auction.preview_scheduled_at = command.preview_scheduled_at;
    result.auction.scheduled_open_at = command.scheduled_open_at;
    result.scheduled_ends_at = Some(command.scheduled_end_at);
    let expected_state_version = previous_state_version
        .checked_add(1)
        .ok_or_else(|| invalid_state("Admin 拍卖状态版本溢出"))?;
    let expected_round_state_version = previous_round_state_version
        .checked_add(1)
        .ok_or_else(|| invalid_state("Admin 拍卖调度版本溢出"))?;
    if state_version != expected_state_version
        || round_state_version != expected_round_state_version
        || !result.validate()
    {
        return Err(invalid_state("Admin 拍卖计划修改后的投影无效"));
    }
    Ok(result)
}

async fn append_admin_auction_schedule_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminUpdateAuctionScheduleCommand,
    previous_preview_scheduled_at: DateTime<Utc>,
    previous_scheduled_open_at: DateTime<Utc>,
    previous_scheduled_end_at: DateTime<Utc>,
    result: &AdminAuctionSummary,
    changed: bool,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "previous_preview_scheduled_at": previous_preview_scheduled_at,
        "preview_scheduled_at": command.preview_scheduled_at,
        "previous_scheduled_open_at": previous_scheduled_open_at,
        "scheduled_open_at": command.scheduled_open_at,
        "previous_scheduled_end_at": previous_scheduled_end_at,
        "scheduled_end_at": command.scheduled_end_at,
        "changed": changed,
        "state_version": result.auction.state_version,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, 'auction.schedule.update', 'auction', $3, 'success', $4, 'admin', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(result.auction.id.as_uuid().to_string())
    .bind(command.request_id.as_deref())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    if !changed {
        return Ok(());
    }
    let stream_key = format!("auction:{}", result.auction.id.as_uuid());
    let payload = json!({
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "scheduled_end_at": command.scheduled_end_at,
        "state_version": result.auction.state_version,
    });
    let sequence =
        crate::database::realtime::allocate_stream_sequence(transaction, &stream_key, None)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.schedule_updated', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(result.auction.state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

fn map_schedule_summary(
    row: &PgRow,
    scheduled_ends_at: Option<DateTime<Utc>>,
) -> Result<AdminAuctionSummary, CompetitionTransactionError> {
    Ok(AdminAuctionSummary {
        auction: SettlementAuctionSnapshot {
            id: AuctionId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
            world_id: row.try_get("world_id").map_err(map_sql_error)?,
            sequence: row.try_get("sequence").map_err(map_sql_error)?,
            status: row.try_get("status").map_err(map_sql_error)?,
            preview_scheduled_at: row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
            scheduled_open_at: row.try_get("scheduled_open_at").map_err(map_sql_error)?,
            preview_opened_at: row.try_get("preview_opened_at").map_err(map_sql_error)?,
            preview_manual_open: row.try_get("preview_manual_open").map_err(map_sql_error)?,
            preview_revision: row.try_get("preview_revision").map_err(map_sql_error)?,
            opened_at: row.try_get("opened_at").map_err(map_sql_error)?,
            final_phase_at: row.try_get("final_phase_at").map_err(map_sql_error)?,
            settled_at: row.try_get("settled_at").map_err(map_sql_error)?,
            cancelled_at: row.try_get("cancelled_at").map_err(map_sql_error)?,
            cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
            cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
            state_version: row.try_get("state_version").map_err(map_sql_error)?,
            scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_sql_error)?,
            effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
            created_at: row.try_get("created_at").map_err(map_sql_error)?,
            updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        },
        scheduled_ends_at,
        lot_count: row.try_get("lot_count").map_err(map_sql_error)?,
    })
}

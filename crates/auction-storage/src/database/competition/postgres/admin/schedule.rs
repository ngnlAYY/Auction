//! PostgreSQL 管理端开拍前计划结束时间修改及其审计/实时记录。

use super::*;

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
        previous_scheduled_end_at: DateTime<Utc>,
        result: AdminAuctionSummary,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move {
            append_admin_auction_schedule_record_inner(
                self,
                &command,
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
                   SELECT round.scheduled_end_at
                   FROM public.scheduled_rounds AS round
                   INNER JOIN target_auction ON target_auction.id = round.auction_id
                   INNER JOIN locked_world ON locked_world.id = target_auction.world_id
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
                locked_round.scheduled_end_at AS round_scheduled_end_at,
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
    let snapshot = AdminAuctionScheduleSnapshot {
        summary,
        policy_scheduled_end_at,
        round_scheduled_end_at: row.try_get("round_scheduled_end_at").map_err(map_sql_error)?,
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
    let previous_scheduled_end_at = snapshot.policy_scheduled_end_at;
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"UPDATE public.auctions
               SET policy_snapshot = jsonb_set(
                       policy_snapshot,
                       '{scheduled_end_at}',
                       to_jsonb($2::timestamptz),
                       false
                   ),
                   state_version = state_version + 1,
                   updated_at = $3
               WHERE id = $1
                 AND state_version = $4
                 AND status IN ('draft', 'preview')
                 AND scheduled_end_at IS NULL
                 AND effective_end_at IS NULL
                 AND (policy_snapshot ->> 'scheduled_end_at')::timestamptz = $5
               RETURNING state_version, updated_at"#,
        )
        .bind(command.auction_id.as_uuid())
        .bind(command.scheduled_end_at)
        .bind(updated_at)
        .bind(previous_state_version)
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

    let round_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"UPDATE public.scheduled_rounds
               SET scheduled_end_at = $2,
                   state_version = state_version + 1,
                   updated_at = $3
               WHERE auction_id = $1
                 AND status = 'active'
                 AND scheduled_end_at = $4"#,
        )
        .bind(command.auction_id.as_uuid())
        .bind(command.scheduled_end_at)
        .bind(updated_at)
        .bind(previous_scheduled_end_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if round_rows != 1 {
        return Err(CompetitionTransactionError::StaleState);
    }

    let mut result = snapshot.summary;
    result.auction.state_version = state_version;
    result.auction.updated_at = stored_updated_at;
    result.scheduled_ends_at = Some(command.scheduled_end_at);
    if state_version != previous_state_version.saturating_add(1) || !result.validate() {
        return Err(invalid_state("Admin 拍卖计划修改后的投影无效"));
    }
    Ok(result)
}

async fn append_admin_auction_schedule_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminUpdateAuctionScheduleCommand,
    previous_scheduled_end_at: DateTime<Utc>,
    result: &AdminAuctionSummary,
    changed: bool,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
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
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
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

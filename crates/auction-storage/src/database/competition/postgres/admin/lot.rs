//! PostgreSQL 管理端 lot 更新、退回义务和相关审计/实时记录。

use super::*;

#[allow(clippy::manual_async_fn)]
impl AdminLotUpdateTransaction for PostgresCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.insert_command_inner(command).await }
    }

    fn load_admin_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError>> + Send
    {
        async move { load_admin_lot_update_inner(self, lot_id).await }
    }

    fn update_admin_lot(
        &mut self,
        command: &AdminLotUpdateCommand,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminLotSummary, CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { update_admin_lot_inner(self, &command, now).await }
    }

    fn advance_admin_preview_revision(
        &mut self,
        auction_id: AuctionId,
        expected_revision: i64,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<i64, CompetitionTransactionError>> + Send {
        async move {
            let next_revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| invalid_state("Admin lot 预览修订溢出"))?;
            let row = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "UPDATE public.auctions
                     SET preview_revision = $1,
                         state_version = state_version + 1,
                         updated_at = $2
                     WHERE id = $3 AND status = 'draft' AND preview_revision = $4
                     RETURNING state_version",
                )
                .bind(next_revision)
                .bind(now)
                .bind(auction_id.as_uuid())
                .bind(expected_revision)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            let Some(row) = row else {
                return Err(CompetitionTransactionError::StaleState);
            };
            row.try_get("state_version").map_err(map_sql_error)
        }
    }

    fn insert_withdrawn_return_obligation(
        &mut self,
        command: &AdminLotUpdateCommand,
        snapshot: &AdminLotUpdateSnapshot,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        let snapshot = snapshot.clone();
        async move { insert_withdrawn_return_obligation_inner(self, &command, &snapshot, now).await }
    }

    fn append_admin_lot_record(
        &mut self,
        command: &AdminLotUpdateCommand,
        result: &AdminLotSummary,
        changed: bool,
        auction_state_version: i64,
        preview_revision: i64,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        let result = result.clone();
        async move {
            append_admin_lot_record_inner(
                self,
                &command,
                &result,
                changed,
                auction_state_version,
                preview_revision,
            )
            .await
        }
    }
}

async fn load_admin_lot_update_inner(
    transaction: &mut PostgresCompetitionTransaction,
    lot_id: LotId,
) -> Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError> {
    let scope = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT a.id AS auction_id, a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             WHERE l.id = $1",
        )
        .bind(lot_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(scope) = scope else {
        return Ok(None);
    };
    let auction_id: Uuid = scope.try_get("auction_id").map_err(map_sql_error)?;
    let world_id: String = scope.try_get("world_id").map_err(map_sql_error)?;

    {
        let transaction = transaction.transaction_mut()?;
        let world_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
                .bind(&world_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if world_exists.is_none() {
            return Ok(None);
        }
        let auction_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
                .bind(auction_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if auction_exists.is_none() {
            return Ok(None);
        }
    }

    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT
                a.status AS auction_status, a.state_version AS auction_state_version,
                a.preview_revision,
                COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                    AS policy_version,
                l.id, l.auction_id, l.equipment_id, l.submission_id, l.lot_number, l.status,
                l.start_price, COALESCE(l.current_price, l.start_price) AS current_price,
                l.leader_user_id, l.effective_end_at, l.slow_due_at, l.slow_generation,
                l.slow_authorization_id, l.state_version, l.created_at, l.updated_at,
                e.external_equipment_id, e.name AS equipment_name, e.category AS equipment_category,
                e.attributes AS equipment_attributes, e.collected_at AS equipment_collected_at,
                s.seller, s.source_kind AS submission_source_kind,
                s.source_reference AS submission_source_reference, s.mailbox_key,
                s.external_mail_id, s.received_at AS submission_received_at
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             INNER JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.id = $1 AND a.id = $2
             FOR UPDATE OF l",
        )
        .bind(lot_id.as_uuid())
        .bind(auction_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let auction_status: String = row.try_get("auction_status").map_err(map_sql_error)?;
    let auction_state_version: i64 = row.try_get("auction_state_version").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let policy_version: i32 = row.try_get("policy_version").map_err(map_sql_error)?;
    let lot = map_admin_lot_row(row)?;
    let snapshot = AdminLotUpdateSnapshot {
        lot,
        auction_status,
        world_id,
        auction_state_version,
        preview_revision,
        policy_version,
    };
    if !snapshot.validate() {
        return Err(invalid_state("Admin lot 锁内投影校验失败"));
    }
    Ok(Some(snapshot))
}

async fn update_admin_lot_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    now: DateTime<Utc>,
) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let start_price = command.start_price;
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.lots
             SET status = CASE WHEN $1 = 'withdraw' THEN 'withdrawn' ELSE 'pending' END,
                 start_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE start_price END,
                 current_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE current_price END,
                 state_version = state_version + 1,
                 updated_at = $3
             WHERE id = $4 AND auction_id = (
                 SELECT auction_id FROM public.lots WHERE id = $4
             ) AND status = 'pending' AND state_version = $5
             RETURNING id",
        )
        .bind(command.action.as_str())
        .bind(start_price)
        .bind(now)
        .bind(command.lot_id.as_uuid())
        .bind(command.expected_state_version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.is_none() {
        return Err(CompetitionTransactionError::StaleState);
    }
    load_admin_lot_update_inner(transaction, command.lot_id)
        .await?
        .map(|snapshot| snapshot.lot)
        .ok_or(CompetitionTransactionError::NotFound)
}

async fn insert_withdrawn_return_obligation_inner(
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
        .bind(snapshot.lot.submission_id.map(|id| id.as_uuid()))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation)
             VALUES ($1, 'pending', $2, 'return')",
        )
        .bind(obligation_id)
        .bind(&snapshot.world_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn append_admin_lot_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    result: &AdminLotSummary,
    changed: bool,
    auction_state_version: i64,
    preview_revision: i64,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.as_str(),
        "lot_id": result.id.as_uuid(),
        "state_version": result.state_version,
        "changed": changed,
        "preview_revision": preview_revision,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, $3, 'lot', $4, 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(format!("lot.{}", command.action.as_str()))
    .bind(result.id.as_uuid().to_string())
    .bind(&command.request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    if !changed {
        return Ok(());
    }

    let event_data = sqlx::query(
        "SELECT a.world_id, a.preview_manual_open,
                (SELECT COUNT(*)::bigint FROM public.auction_preview_items AS item
                 WHERE item.auction_id = a.id) AS item_count
         FROM public.auctions AS a WHERE a.id = $1",
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let item_count: i64 = event_data.try_get("item_count").map_err(map_sql_error)?;
    let manual_open: bool = event_data.try_get("preview_manual_open").map_err(map_sql_error)?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
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
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.preview_updated', 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({
        "auction_id": result.auction_id.as_uuid(),
        "preview_revision": preview_revision,
        "item_count": item_count,
        "manual_open": manual_open,
        "state_version": auction_state_version,
    }))
    .bind(auction_state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

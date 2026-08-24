//! PostgreSQL 管理端拍卖与标的事务适配。
//!
//! 该文件只承载 PostgreSQL 竞价事务的单一职责，事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

#[allow(clippy::manual_async_fn)]
impl AdminAuctionCreateTransaction for PostgresCompetitionTransaction {
    fn create_admin_auction(
        &mut self,
        command: AdminCreateAuctionCommand,
        title: String,
        policy: AuctionPolicyTemplate,
        created_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send {
        async move { create_admin_auction_inner(self, &command, &title, &policy, created_at).await }
    }

    fn append_admin_auction_created_record(
        &mut self,
        command: AdminCreateAuctionCommand,
        result: AdminAuctionSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_admin_auction_created_record_inner(self, &command, &result).await }
    }
}

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

fn map_admin_lot_row(row: PgRow) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let attributes: sqlx::types::Json<Value> =
        row.try_get("equipment_attributes").map_err(map_sql_error)?;
    Ok(AdminLotSummary {
        id: LotId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        auction_id: AuctionId::from_uuid(row.try_get("auction_id").map_err(map_sql_error)?),
        equipment_id: auction_domain::EquipmentId::from_uuid(
            row.try_get("equipment_id").map_err(map_sql_error)?,
        ),
        submission_id: row
            .try_get::<Option<Uuid>, _>("submission_id")
            .map_err(map_sql_error)?
            .map(auction_domain::SubmissionId::from_uuid),
        lot_number: row.try_get("lot_number").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        start_price: row.try_get("start_price").map_err(map_sql_error)?,
        current_price: row.try_get("current_price").map_err(map_sql_error)?,
        leader_user_id: row
            .try_get::<Option<Uuid>, _>("leader_user_id")
            .map_err(map_sql_error)?
            .map(UserId::from_uuid),
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        slow_due_at: row.try_get("slow_due_at").map_err(map_sql_error)?,
        slow_generation: row.try_get("slow_generation").map_err(map_sql_error)?,
        slow_authorization_id: row
            .try_get::<Option<Uuid>, _>("slow_authorization_id")
            .map_err(map_sql_error)?
            .map(ProxyAuthorizationId::from_uuid),
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_sql_error)?,
        equipment_name: row.try_get("equipment_name").map_err(map_sql_error)?,
        equipment_category: row.try_get("equipment_category").map_err(map_sql_error)?,
        equipment_attributes: attributes.0,
        equipment_collected_at: row.try_get("equipment_collected_at").map_err(map_sql_error)?,
        seller: row.try_get("seller").map_err(map_sql_error)?,
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_sql_error)?,
        submission_source_reference: row
            .try_get("submission_source_reference")
            .map_err(map_sql_error)?,
        mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
        external_mail_id: row.try_get("external_mail_id").map_err(map_sql_error)?,
        submission_received_at: row.try_get("submission_received_at").map_err(map_sql_error)?,
    })
}

async fn create_admin_auction_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminCreateAuctionCommand,
    title: &str,
    policy: &AuctionPolicyTemplate,
    created_at: DateTime<Utc>,
) -> Result<AdminAuctionSummary, CompetitionTransactionError> {
    let rules = policy
        .auction_rules()
        .map_err(|error| invalid_state(&format!("拍卖策略规则无效：{error}")))?;
    let policy_snapshot = policy.snapshot(command.scheduled_end_at);
    let world_enabled = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&command.world_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
    };
    let Some(world_enabled) = world_enabled else {
        return Err(CompetitionTransactionError::NotFound);
    };
    if !world_enabled {
        return Err(CompetitionTransactionError::Conflict);
    }

    let existing_target = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT auction_id FROM public.auction_intake_targets WHERE world_id = $1 FOR UPDATE",
        )
        .bind(&command.world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if existing_target.is_some() {
        return Err(CompetitionTransactionError::Conflict);
    }

    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.auctions
                (id, world_id, sequence, title, status, preview_scheduled_at,
                 scheduled_open_at, default_start_price, soft_close_window_seconds,
                 soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'draft', $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
             RETURNING state_version, created_at, updated_at"#,
        )
        .bind(command.auction_id.as_uuid())
        .bind(&command.world_id)
        .bind(command.sequence)
        .bind(title)
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(rules.default_start_price.value())
        .bind(rules.soft_close_window.num_seconds())
        .bind(rules.soft_close_extension.num_seconds())
        .bind(rules.cancel_cutoff.num_seconds())
        .bind(rules.proxy_slow_interval.num_seconds())
        .bind(policy_snapshot)
        .bind(created_at)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let state_version: i64 = row.try_get("state_version").map_err(map_sql_error)?;
    if state_version <= 0 {
        return Err(invalid_state("后台拍卖创建返回的状态版本无效"));
    }
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_sql_error)?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(map_sql_error)?;

    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.auction_intake_targets
                (world_id, auction_id, target_status, state_version, updated_at)
             VALUES ($1, $2, 'draft', $3, $4)",
        )
        .bind(&command.world_id)
        .bind(command.auction_id.as_uuid())
        .bind(state_version)
        .bind(updated_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        let local_week = command.preview_scheduled_at.date_naive()
            - Duration::days(i64::from(
                command.preview_scheduled_at.weekday().num_days_from_monday(),
            ));
        let round_id = deterministic_uuid(command.command_id, "scheduled-round", 1);
        sqlx::query(
            "INSERT INTO public.scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $9)",
        )
        .bind(round_id)
        .bind(&command.world_id)
        .bind(local_week)
        .bind(command.auction_id.as_uuid())
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(command.scheduled_end_at)
        .bind(command.command_id.as_uuid())
        .bind(created_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    // 旧 A03 创建接口把计划结束时间冻结在 policy_snapshot，拍卖生命周期列在真正开放前保持空值。
    let result = AdminAuctionSummary {
        auction: SettlementAuctionSnapshot {
            id: command.auction_id,
            world_id: command.world_id.clone(),
            sequence: command.sequence,
            status: "draft".to_owned(),
            preview_scheduled_at: command.preview_scheduled_at,
            scheduled_open_at: command.scheduled_open_at,
            preview_opened_at: None,
            preview_manual_open: false,
            preview_revision: 0,
            opened_at: None,
            final_phase_at: None,
            settled_at: None,
            cancelled_at: None,
            cancelled_reason: None,
            cancelled_by: None,
            state_version,
            scheduled_end_at: None,
            effective_end_at: None,
            created_at,
            updated_at,
        },
        scheduled_ends_at: None,
        lot_count: 0,
    };
    if !result.validate() {
        return Err(invalid_state("后台拍卖创建结果不满足生命周期不变量"));
    }
    Ok(result)
}

async fn append_admin_auction_created_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminCreateAuctionCommand,
    result: &AdminAuctionSummary,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "sequence": result.auction.sequence,
        "state_version": result.auction.state_version,
    });
    let payload = json!({
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "sequence": result.auction.sequence,
        "state_version": result.auction.state_version,
    });
    let stream_key = format!("auction:{}", result.auction.id.as_uuid());
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, 'auction.create', 'auction', $3, 'success', $4, 'admin', $5)"#,
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
         VALUES ($1, $2, $3, 'auction.created', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(&payload)
    .bind(result.auction.state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    // 旧版 relay 会把创建事件同时投递到公开发现范围。这里在同一事务内追加一个持久化
    // 派生事件，使跨进程重启、HTTP catch-up 和 WebSocket 初始读取都能从权威数据库恢复。
    // 派生事件使用独立流序号和事件 ID，但保留同一命令、状态版本和公开负载；它不是第二份
    // 拍卖事实，也不能被当作拍卖专属流中的额外生命周期事件。
    let discovery_stream_key = "auction:discovery";
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(discovery_stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let discovery_sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(discovery_stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(discovery_stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.created', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(discovery_stream_key)
    .bind(discovery_sequence)
    .bind(&payload)
    .bind(result.auction.state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

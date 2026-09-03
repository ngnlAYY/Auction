//! PostgreSQL 管理端拍卖创建及其审计/实时记录。

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
        scheduled_ends_at: Some(command.scheduled_end_at),
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

    Ok(())
}

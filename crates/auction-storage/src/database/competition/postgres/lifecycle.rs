//! PostgreSQL 拍卖生命周期事务适配。
//!
//! 该文件只承载 PostgreSQL 竞价事务的单一职责，事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

#[allow(clippy::manual_async_fn)]
impl AuctionLifecycleTransaction for PostgresCompetitionTransaction {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send
    {
        async move { load_lifecycle_context_postgres(self, auction_id).await }
    }

    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { apply_lifecycle_plan_postgres(self, &command, plan).await }
    }

    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send {
        async move {
            let rows = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "SELECT id FROM public.lots
                     WHERE auction_id = $1 AND status = 'live'
                     ORDER BY lot_number
                     FOR UPDATE",
                )
                .bind(auction_id.as_uuid())
                .fetch_all(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            rows.into_iter()
                .map(|row| row.try_get("id").map(LotId::from_uuid).map_err(map_sql_error))
                .collect()
        }
    }

    fn proxy_user_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move {
            let status = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM public.users WHERE id = $1 FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("活动代理用户不存在"))?
            };
            let blacklist_active = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, bool>(
                    "SELECT revoked_at IS NULL
                     FROM public.user_blacklist
                     WHERE user_id = $1
                     FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .unwrap_or(false)
            };
            Ok(status == "active" && !blacklist_active)
        }
    }

    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        result: &AuctionLifecycleResult,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_lifecycle_record_inner(self, command, result).await }
    }
}

async fn load_lifecycle_context_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_manual_open,
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
                    AS policy_scheduled_end_at,
                (a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint
                    AS policy_final_phase_lead_seconds,
                (SELECT round.local_week
                 FROM public.scheduled_rounds AS round
                 WHERE round.auction_id = a.id
                 LIMIT 1) AS scheduled_round_local_week
             FROM public.auctions AS a
             WHERE a.id = $1
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
    let state_version =
        positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "拍卖状态版本无效")?;
    let auction = Auction {
        id: AuctionId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        title: row.try_get("title").map_err(map_sql_error)?,
        status: parse_auction_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?,
        state_version,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_sql_error)?,
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        cancelled_at: row.try_get("cancelled_at").map_err(map_sql_error)?,
        cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
        cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
    };
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("最终阶段领先时间无效"));
    }
    Ok(Some(AuctionLifecycleContext {
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at: row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        scheduled_open_at: row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        policy_scheduled_end_at: row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        preview_manual_open: row.try_get("preview_manual_open").map_err(map_sql_error)?,
        scheduled_round_local_week: row
            .try_get::<Option<NaiveDate>, _>("scheduled_round_local_week")
            .map_err(map_sql_error)?,
    }))
}

async fn apply_lifecycle_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<(), CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual, successor } => {
            apply_preview_plan_postgres(transaction, command, &auction, manual, &successor).await
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            apply_open_plan_postgres(transaction, command, &auction, manual_override).await
        }
        AuctionLifecycleEffect::Cancel => {
            apply_cancel_plan_postgres(transaction, command, &auction).await
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans } => {
            apply_final_plan_postgres(transaction, command, &auction, proxy_plans).await
        }
    }
}

async fn apply_preview_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = previous_lifecycle_version(auction)?;
    let next_version = i64_version(auction.state_version, "预览状态版本")?;
    let scope = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT a.world_id, a.predecessor_auction_id, world.enabled
             FROM public.auctions AS a
             INNER JOIN public.worlds AS world ON world.id = a.world_id
             WHERE a.id = $1
             FOR UPDATE OF a, world",
        )
        .bind(auction.id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(scope) = scope else {
        return Err(CompetitionTransactionError::NotFound);
    };
    let world_id: String = scope.try_get("world_id").map_err(map_sql_error)?;
    if !scope.try_get::<bool, _>("enabled").map_err(map_sql_error)? {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if let Some(predecessor_id) =
        scope.try_get::<Option<Uuid>, _>("predecessor_auction_id").map_err(map_sql_error)?
    {
        let predecessor = {
            let transaction = transaction.transaction_mut()?;
            sqlx::query("SELECT world_id, status FROM public.auctions WHERE id = $1 FOR UPDATE")
                .bind(predecessor_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
        };
        let Some(predecessor) = predecessor else {
            return Err(invalid_state("预览拍卖的前驱记录不存在"));
        };
        let predecessor_world: String = predecessor.try_get("world_id").map_err(map_sql_error)?;
        let predecessor_status: String = predecessor.try_get("status").map_err(map_sql_error)?;
        if predecessor_world != world_id
            || !matches!(predecessor_status.as_str(), "settled" | "cancelled")
        {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }

    let intake_target = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT auction_id, state_version
             FROM public.auction_intake_targets
             WHERE world_id = $1
             FOR UPDATE",
        )
        .bind(&world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(intake_target) = intake_target else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    let target_auction_id: Uuid = intake_target.try_get("auction_id").map_err(map_sql_error)?;
    let target_version: i64 = intake_target.try_get("state_version").map_err(map_sql_error)?;
    if target_auction_id != auction.id.as_uuid() || target_version <= 0 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    let preview_revision = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE public.auctions
             SET status = 'preview',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_manual_open = preview_manual_open OR $2,
                 preview_revision = preview_revision + 1,
                 state_version = $3,
                 updated_at = $1
             WHERE id = $4 AND status = 'draft' AND state_version = $5
               AND ($2 OR preview_scheduled_at <= $1)
             RETURNING preview_revision",
        )
        .bind(auction.updated_at)
        .bind(manual)
        .bind(next_version)
        .bind(auction.id.as_uuid())
        .bind(previous_version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(preview_revision) = preview_revision else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    if preview_revision <= 0 {
        return Err(invalid_state("预览修订版本无效"));
    }

    let materialized = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.auction_preview_items
                (auction_id, equipment_id, display_order, start_price, source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    auction.preview_revision, equipment.collected_at
             FROM public.lots AS lot
             INNER JOIN public.auctions AS auction ON auction.id = lot.auction_id
             INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.auction_id = $1 AND lot.status = 'pending' AND auction.status = 'preview'
             ORDER BY lot.lot_number",
        )
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    let pending_count: i64 = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM public.lots
             WHERE auction_id = $1 AND status = 'pending'",
        )
        .bind(auction.id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("预览物化条目数与 pending lot 不一致"));
    }

    let successor_row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.auctions
                (id, world_id, sequence, title, status, preview_scheduled_at,
                 scheduled_open_at, default_start_price, soft_close_window_seconds,
                 soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, predecessor_auction_id,
                 created_at, updated_at)
             SELECT $1, current.world_id, $2, $3, 'draft', $4, $5,
                    current.default_start_price, current.soft_close_window_seconds,
                    current.soft_close_extension_seconds, current.cancel_cutoff_seconds,
                    current.proxy_slow_interval_seconds,
                    jsonb_set(current.policy_snapshot, '{scheduled_end_at}',
                              to_jsonb($6::timestamptz), true),
                    current.id, $7, $7
             FROM public.auctions AS current
             WHERE current.id = $8 AND current.status = 'preview'
             RETURNING world_id, state_version"#,
        )
        .bind(successor.auction_id.as_uuid())
        .bind(successor.sequence)
        .bind(&successor.title)
        .bind(successor.preview_due_at)
        .bind(successor.open_due_at)
        .bind(successor.scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(successor_row) = successor_row else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    let successor_world: String = successor_row.try_get("world_id").map_err(map_sql_error)?;
    let successor_version: i64 = successor_row.try_get("state_version").map_err(map_sql_error)?;
    if successor_world != world_id || successor_version != 1 {
        return Err(invalid_state("下一期草稿不满足初始状态不变量"));
    }

    transaction
        .insert_command_inner(StoredCommand {
            command_id: successor.create_command_id,
            command_kind: "scheduled_auction_create".to_owned(),
            aggregate_id: successor.auction_id.as_uuid(),
            request_hash: successor.create_request_hash,
            result_code: "accepted".to_owned(),
            result_payload: serde_json::to_vec(&json!({
                "command_id": successor.create_command_id.as_uuid(),
                "auction_id": successor.auction_id.as_uuid(),
                "status": "draft",
                "state_version": successor_version,
            }))
            .map_err(|_| invalid_record())?,
        })
        .await?;

    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $9)",
        )
        .bind(successor.round_id)
        .bind(&world_id)
        .bind(successor.local_week)
        .bind(successor.auction_id.as_uuid())
        .bind(successor.preview_due_at)
        .bind(successor.open_due_at)
        .bind(successor.scheduled_end_at)
        .bind(successor.create_command_id.as_uuid())
        .bind(auction.updated_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        let moved = sqlx::query(
            "UPDATE public.auction_intake_targets
             SET auction_id = $1, state_version = state_version + 1, updated_at = $2
             WHERE world_id = $3 AND auction_id = $4 AND state_version = $5",
        )
        .bind(successor.auction_id.as_uuid())
        .bind(auction.updated_at)
        .bind(&world_id)
        .bind(auction.id.as_uuid())
        .bind(target_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if moved.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }

        if manual {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET preview_command_id = $1, preview_completed_at = $2,
                     state_version = state_version + 1, updated_at = $2
                 WHERE auction_id = $3 AND status = 'active'
                   AND preview_completed_at IS NULL",
            )
            .bind(command.command_id.as_uuid())
            .bind(auction.updated_at)
            .bind(auction.id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }
    }
    Ok(())
}

async fn apply_open_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("开拍计划的名义与有效结束时间不一致"));
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT id, status, submission_id
             FROM public.lots
             WHERE auction_id = $1
             ORDER BY lot_number
             FOR UPDATE",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if lot_rows.is_empty()
        || lot_rows.iter().any(|row| {
            row.try_get::<String, _>("status").ok().as_deref() != Some("pending")
                || row.try_get::<Option<Uuid>, _>("submission_id").ok().flatten().is_none()
        })
    {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let activated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.lots
             SET status = 'live', current_price = start_price, effective_end_at = $1,
                 slow_due_at = NULL, slow_authorization_id = NULL, slow_generation = 0,
                 last_proxy_sequence = 0, state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'pending'",
        )
        .bind(scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if usize::try_from(activated).ok() != Some(lot_rows.len()) {
        return Err(CompetitionTransactionError::InvalidState);
    }

    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'live',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_scheduled_at = CASE
                     WHEN $2 AND preview_scheduled_at > $1 THEN $1 ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN $2 THEN $1 ELSE scheduled_open_at END,
                 opened_at = $1, scheduled_end_at = $3, effective_end_at = $3,
                 state_version = $4, updated_at = $1
             WHERE id = $5 AND status = 'preview' AND state_version = $6
               AND (preview_scheduled_at <= $1 OR $2)
               AND (scheduled_open_at <= $1 OR $2)",
        )
        .bind(auction.updated_at)
        .bind(manual_override)
        .bind(scheduled_end_at)
        .bind(i64_version(auction.state_version, "开拍状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET open_command_id = $1, open_completed_at = $2,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn apply_cancel_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
) -> Result<(), CompetitionTransactionError> {
    let previous_status: String = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1 FOR UPDATE")
            .bind(auction.id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT l.id, l.status, l.state_version, l.equipment_id, l.submission_id,
                    s.seller,
                    COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                        AS policy_version,
                    a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.auction_id = $1
             ORDER BY l.lot_number
             FOR UPDATE OF l",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    for lot in lot_rows {
        let status: String = lot.try_get("status").map_err(map_sql_error)?;
        if status == "withdrawn" {
            // 已由单 lot 撤回命令生成过返还义务，取消命令不能重复建立第二份责任。
            continue;
        }
        if status != "pending" {
            return Err(invalid_state("取消拍卖包含非 pending/withdrawn 标的"));
        }
        let lot_id: Uuid = lot.try_get("id").map_err(map_sql_error)?;
        let equipment_id: Uuid = lot.try_get("equipment_id").map_err(map_sql_error)?;
        let submission_id: Option<Uuid> = lot.try_get("submission_id").map_err(map_sql_error)?;
        let seller: Option<String> = lot.try_get("seller").map_err(map_sql_error)?;
        let seller = seller
            .filter(|value| !value.trim().is_empty() && value == value.trim())
            .ok_or_else(|| invalid_state("取消返还义务缺少卖方游戏身份"))?;
        let world_id: String = lot.try_get("world_id").map_err(map_sql_error)?;
        if !matches!(world_id.as_str(), "main" | "isekai") {
            return Err(invalid_state("取消返还义务的世界不在允许集合内"));
        }
        let policy_version: i32 = lot.try_get("policy_version").map_err(map_sql_error)?;
        if policy_version < 0 {
            return Err(invalid_state("取消返还义务的策略版本无效"));
        }
        let lot_state_version: i64 = lot.try_get("state_version").map_err(map_sql_error)?;
        if lot_state_version <= 0 {
            return Err(invalid_state("取消标的状态版本无效"));
        }

        let withdrawn = {
            let transaction = transaction.transaction_mut()?;
            sqlx::query(
                "UPDATE public.lots
                 SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
                     state_version = state_version + 1, updated_at = $1
                 WHERE id = $2 AND auction_id = $3 AND status = 'pending' AND state_version = $4",
            )
            .bind(auction.updated_at)
            .bind(lot_id)
            .bind(auction.id.as_uuid())
            .bind(lot_state_version)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if withdrawn.rows_affected() != 1 {
            return Err(CompetitionTransactionError::StaleState);
        }
        insert_cancellation_return_obligation_postgres(
            transaction,
            command,
            auction,
            lot_id,
            equipment_id,
            submission_id,
            &world_id,
            &seller,
            policy_version,
        )
        .await?;
    }

    {
        let transaction = transaction.transaction_mut()?;
        if previous_status == "draft" {
            sqlx::query(
                "DELETE FROM public.auction_intake_targets
                 WHERE auction_id = $1",
            )
            .bind(auction.id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET status = 'missed', auction_id = NULL,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $1
             WHERE auction_id = $2 AND status = 'active' AND open_completed_at IS NULL",
        )
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        let updated = sqlx::query(
            "UPDATE public.auctions
             SET status = 'cancelled', state_version = $1, updated_at = $2,
                 cancelled_at = $3, cancelled_reason = $4, cancelled_by = $5
             WHERE id = $6 AND status = $7 AND state_version = $8",
        )
        .bind(i64_version(auction.state_version, "取消状态版本")?)
        .bind(auction.updated_at)
        .bind(auction.cancelled_at)
        .bind(&auction.cancelled_reason)
        .bind(&auction.cancelled_by)
        .bind(auction.id.as_uuid())
        .bind(&previous_status)
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if updated.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_cancellation_return_obligation_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    lot_id: Uuid,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    world_id: &str,
    seller: &str,
    policy_version: i32,
) -> Result<(), CompetitionTransactionError> {
    let obligation_id = deterministic_uuid(
        command.command_id,
        &format!("settlement-obligation:{lot_id}:withdrawn_return"),
        1,
    );
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
    .bind(auction.id.as_uuid())
    .bind(lot_id)
    .bind(world_id)
    .bind(command.command_id.as_uuid())
    .bind(policy_version)
    .bind(seller)
    .bind(equipment_id)
    .bind(submission_id)
    .bind(auction.updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation)
         VALUES ($1, 'pending', $2, 'return')",
    )
    .bind(obligation_id)
    .bind(world_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn apply_final_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'final', final_phase_at = $1, state_version = $2, updated_at = $1
             WHERE id = $3 AND status = 'live' AND state_version = $4",
        )
        .bind(auction.updated_at)
        .bind(i64_version(auction.state_version, "最终阶段状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET final_command_id = $1, final_completed_at = $2,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

fn previous_lifecycle_version(auction: &Auction) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("拍卖状态版本无法回退校验"))
}

#[allow(clippy::manual_async_fn)]
async fn append_lifecycle_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.command_kind(),
        "status": auction_status_name(result.status),
        "state_version": result.state_version,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, $2, $3, $4, 'auction', $5, 'success', $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(command.action.command_kind())
    .bind(result.auction_id.as_uuid().to_string())
    .bind(command.request_id.as_deref())
    .bind(command.actor.actor_type.database_name())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let (event_kind, payload) = lifecycle_event_payload(transaction, command, result).await?;
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
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(event_kind)
    .bind(payload)
    .bind(i64_version(result.state_version, "生命周期事件状态版本")?)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn lifecycle_event_payload(
    transaction: &mut PgTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(&'static str, Value), CompetitionTransactionError> {
    let row = sqlx::query(
        r#"SELECT a.world_id,
                  a.preview_revision,
                  a.preview_manual_open,
                  (SELECT COUNT(*)::bigint
                   FROM public.auction_preview_items AS item
                   WHERE item.auction_id = a.id) AS item_count
           FROM public.auctions AS a
           WHERE a.id = $1"#,
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let manual_open: bool = row.try_get("preview_manual_open").map_err(map_sql_error)?;
    let item_count: i64 = row.try_get("item_count").map_err(map_sql_error)?;
    let state_version = result.state_version;
    let event = match command.action {
        auction_application::AuctionLifecycleAction::Preview => (
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": preview_revision,
                "item_count": item_count,
                "manual_open": manual_open,
                "state_version": state_version,
            }),
        ),
        auction_application::AuctionLifecycleAction::Open => (
            "auction.opened",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "world_id": world_id,
                "state_version": state_version,
                "preview_revision": preview_revision,
            }),
        ),
        auction_application::AuctionLifecycleAction::Cancel => (
            "auction.cancelled",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
        auction_application::AuctionLifecycleAction::FinalPhase => (
            "auction.final_phase_entered",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
    };
    Ok(event)
}

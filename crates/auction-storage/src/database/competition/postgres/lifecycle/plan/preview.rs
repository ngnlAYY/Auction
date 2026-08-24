//! PostgreSQL 预览阶段计划写入。

use super::super::record::previous_lifecycle_version;
use super::super::*;

pub(super) async fn apply_preview_plan_postgres(
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

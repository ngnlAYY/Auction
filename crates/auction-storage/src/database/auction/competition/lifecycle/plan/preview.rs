//! PostgreSQL 预览阶段计划写入。

use auction_application::auction::AuctionLifecycleCommand;
use auction_application::core::CompetitionTransactionError;
use auction_domain::Auction;
use sqlx::Row;
use uuid::Uuid;

use super::super::record::previous_lifecycle_version;
use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::lifecycle::map_sql_error;
use crate::database::auction::competition::mapping::i64_version;
use crate::database::auction::competition::mapping::invalid_state;

pub(super) async fn apply_preview_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
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
            "SELECT auction_id, target_status, state_version
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
    let target_status: String = intake_target.try_get("target_status").map_err(map_sql_error)?;
    let target_version: i64 = intake_target.try_get("state_version").map_err(map_sql_error)?;
    if target_auction_id != auction.id.as_uuid() || target_status != "draft" || target_version <= 0
    {
        return Err(CompetitionTransactionError::InvalidState);
    }

    // Preview 仍接收当期装备；空目录依然要在状态写入前拒绝，避免生成无法开放的活跃场次。
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
    if pending_count <= 0 {
        return Err(invalid_state("预览至少需要一个 pending lot"));
    }

    let preview_revision = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE public.auctions
             SET status = 'preview',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_manual_open = preview_manual_open OR $2,
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
    if preview_revision < 0 {
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
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("预览物化条目数与 pending lot 不一致"));
    }

    {
        let transaction = transaction.transaction_mut()?;
        let retained = sqlx::query(
            "UPDATE public.auction_intake_targets
             SET target_status = 'preview', state_version = state_version + 1, updated_at = $1
             WHERE world_id = $2 AND auction_id = $3 AND target_status = 'draft'
               AND state_version = $4",
        )
        .bind(auction.updated_at)
        .bind(&world_id)
        .bind(auction.id.as_uuid())
        .bind(target_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if retained.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }
    super::complete_schedule_phase_postgres(
        transaction,
        command,
        auction,
        auction_application::scheduling::ScheduledRoundPhase::Preview,
    )
    .await?;
    Ok(())
}

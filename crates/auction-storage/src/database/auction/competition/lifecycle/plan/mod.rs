//! PostgreSQL 拍卖生命周期计划的原子写入装配。

use auction_application::auction::{
    AuctionLifecycleCommand, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleWriteSummary,
};
use auction_application::core::CompetitionTransactionError;
use auction_domain::Auction;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::lifecycle::map_sql_error;
use crate::database::auction::competition::mapping::invalid_state;

mod cancel;
mod early_settlement;
mod final_phase;
mod open;
mod preview;
mod successor;

/// 在同一 PostgreSQL 事务内应用已经由应用层计算并校验的生命周期计划。
pub(super) async fn apply_lifecycle_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: &AuctionLifecyclePlan,
) -> Result<AuctionLifecycleWriteSummary, CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("生命周期命令与动作计划不一致"));
    }
    if command.actor.actor_type == auction_application::auction::AuctionLifecycleActorType::Admin {
        // 上下文已按 world → round → auction 锁定；人工动作不能夺走仍在执行的 Worker 租约。
        let leased = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM public.scheduled_rounds
             WHERE auction_id = $1 AND status = 'active'
               AND lease_owner IS NOT NULL AND lease_until > clock_timestamp())",
        )
        .bind(command.auction_id.as_uuid())
        .fetch_one(&mut **transaction.transaction_mut()?)
        .await
        .map_err(map_sql_error)?;
        if leased {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual } => {
            preview::apply_preview_plan_postgres(transaction, command, auction, *manual).await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Open { manual_override, successor } => {
            open::apply_open_plan_postgres(
                transaction,
                command,
                auction,
                *manual_override,
                successor,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Cancel { successor } => {
            let return_obligation_count = cancel::apply_cancel_plan_postgres(
                transaction,
                command,
                auction,
                successor.as_ref(),
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary { return_obligation_count })
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans, effective_end_at } => {
            final_phase::apply_final_plan_postgres(
                transaction,
                command,
                auction,
                proxy_plans,
                *effective_end_at,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::EarlySettlement { proxy_plans, effective_end_at, reason: _ } => {
            early_settlement::apply_early_settlement_plan_postgres(
                transaction,
                command,
                auction,
                proxy_plans,
                *effective_end_at,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
    }
}

/// 阶段事实与轮次完成原子提交；无轮次的历史场次仍可迁移，有轮次则严格检查阶段顺序。
async fn complete_schedule_phase_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    phase: auction_application::scheduling::ScheduledRoundPhase,
) -> Result<(), CompetitionTransactionError> {
    use auction_application::scheduling::ScheduledRoundPhase;

    let transaction = transaction.transaction_mut()?;
    let row = sqlx::query(
        "SELECT id, status, preview_completed_at, open_completed_at, final_completed_at
         FROM public.scheduled_rounds WHERE auction_id = $1 FOR UPDATE",
    )
    .bind(auction.id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let Some(row) = row else { return Ok(()) };
    let status: String = row.try_get("status").map_err(map_sql_error)?;
    let preview: Option<DateTime<Utc>> =
        row.try_get("preview_completed_at").map_err(map_sql_error)?;
    let open: Option<DateTime<Utc>> = row.try_get("open_completed_at").map_err(map_sql_error)?;
    let final_at: Option<DateTime<Utc>> =
        row.try_get("final_completed_at").map_err(map_sql_error)?;
    if status != "active" {
        return Err(CompetitionTransactionError::InvalidState);
    }
    // 已进入 Final 后的人工提前结算不改写原阶段完成证据。
    if matches!(phase, ScheduledRoundPhase::Final)
        && command.action == auction_application::auction::AuctionLifecycleAction::EarlySettlement
        && preview.is_some()
        && open.is_some()
        && final_at.is_some()
    {
        return Ok(());
    }
    let sql = match phase {
        ScheduledRoundPhase::Preview if preview.is_none() && open.is_none() && final_at.is_none() => {
            "UPDATE public.scheduled_rounds SET preview_command_id = $1, preview_completed_at = $2,
             lease_owner = NULL, lease_until = NULL, state_version = state_version + 1, updated_at = $2
             WHERE id = $3 AND status = 'active' AND preview_completed_at IS NULL"
        }
        ScheduledRoundPhase::Open if preview.is_some() && open.is_none() && final_at.is_none() => {
            "UPDATE public.scheduled_rounds SET open_command_id = $1, open_completed_at = $2,
             lease_owner = NULL, lease_until = NULL, state_version = state_version + 1, updated_at = $2
             WHERE id = $3 AND status = 'active' AND open_completed_at IS NULL"
        }
        ScheduledRoundPhase::Final if preview.is_some() && open.is_some() && final_at.is_none() => {
            "UPDATE public.scheduled_rounds SET final_command_id = $1, final_completed_at = $2,
             lease_owner = NULL, lease_until = NULL, state_version = state_version + 1, updated_at = $2
             WHERE id = $3 AND status = 'active' AND final_completed_at IS NULL"
        }
        _ => return Err(CompetitionTransactionError::InvalidState),
    };
    let updated = sqlx::query(sql)
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(row.try_get::<Uuid, _>("id").map_err(map_sql_error)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    Ok(())
}

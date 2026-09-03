//! PostgreSQL 拍卖生命周期计划的原子写入装配。

use super::*;

#[path = "plan/cancel.rs"]
mod cancel;
#[path = "plan/early_settlement.rs"]
mod early_settlement;
#[path = "plan/final_phase.rs"]
mod final_phase;
#[path = "plan/open.rs"]
mod open;
#[path = "plan/preview.rs"]
mod preview;
#[path = "plan/successor.rs"]
mod successor;

/// 在同一 PostgreSQL 事务内应用已经由应用层计算并校验的生命周期计划。
pub(super) async fn apply_lifecycle_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<AuctionLifecycleWriteSummary, CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual } => {
            preview::apply_preview_plan_postgres(transaction, command, &auction, manual).await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Open { manual_override, successor } => {
            open::apply_open_plan_postgres(
                transaction,
                command,
                &auction,
                manual_override,
                &successor,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Cancel { successor } => {
            let return_obligation_count = cancel::apply_cancel_plan_postgres(
                transaction,
                command,
                &auction,
                successor.as_ref(),
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary { return_obligation_count })
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans, effective_end_at } => {
            final_phase::apply_final_plan_postgres(
                transaction,
                command,
                &auction,
                proxy_plans,
                effective_end_at,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::EarlySettlement { proxy_plans, effective_end_at, reason: _ } => {
            early_settlement::apply_early_settlement_plan_postgres(
                transaction,
                command,
                &auction,
                proxy_plans,
                effective_end_at,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
    }
}

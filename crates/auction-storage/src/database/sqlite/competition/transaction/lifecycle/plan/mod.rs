//! SQLite 生命周期计划的装配与共享状态版本校验。
//!
//! preview、open、cancel 和 final phase 各自拥有独立的 SQL 写入流程；本模块只负责按
//! 领域效果分派，并保留所有动作共用的状态版本回退校验。

use super::super::mapping::*;
use super::super::*;

mod cancel;
mod final_phase;
mod open;
mod preview;
mod successor;

pub(super) async fn apply_lifecycle_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<AuctionLifecycleWriteSummary, CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("Lite 生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual } => {
            preview::apply_preview_plan_sqlite(transaction, command, &auction, manual).await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Open { manual_override, successor } => {
            open::apply_open_plan_sqlite(
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
            let return_obligation_count =
                cancel::apply_cancel_plan_sqlite(transaction, &auction, successor.as_ref()).await?;
            Ok(AuctionLifecycleWriteSummary { return_obligation_count })
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans, effective_end_at } => {
            final_phase::apply_final_plan_sqlite(
                transaction,
                command,
                &auction,
                proxy_plans,
                effective_end_at,
            )
            .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        // SQLite Lite 没有 settlement Worker；不能只停止竞价却留下永远无法结算的拍卖。
        AuctionLifecycleEffect::EarlySettlement { .. } => {
            Err(CompetitionTransactionError::InvalidState)
        }
    }
}

fn previous_lifecycle_version_sqlite(
    auction: &Auction,
) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("Lite 拍卖状态版本无法回退校验"))
}

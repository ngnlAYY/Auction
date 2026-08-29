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
        AuctionLifecycleEffect::Preview { manual, successor } => {
            preview::apply_preview_plan_sqlite(transaction, command, &auction, manual, &successor)
                .await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            open::apply_open_plan_sqlite(transaction, command, &auction, manual_override).await?;
            Ok(AuctionLifecycleWriteSummary::default())
        }
        AuctionLifecycleEffect::Cancel => {
            let return_obligation_count =
                cancel::apply_cancel_plan_sqlite(transaction, &auction).await?;
            Ok(AuctionLifecycleWriteSummary { return_obligation_count })
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans } => {
            final_phase::apply_final_plan_sqlite(transaction, command, &auction, proxy_plans)
                .await?;
            Ok(AuctionLifecycleWriteSummary::default())
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

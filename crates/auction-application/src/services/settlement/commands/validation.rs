//! 结算到期世代与活动代理关闭计划的校验。

use auction_domain::{CompetitionPlan, CompetitionTrigger};

use crate::SettlementSnapshot;

use super::SettleAuctionCommand;

/// 在写入任何金额事实前验证 scheduler 冻结的 due generation，防止软截止延期后过早结算。
pub(super) fn validate_settlement_due(
    command: SettleAuctionCommand,
    snapshot: &SettlementSnapshot,
) -> Result<(), auction_domain::DomainError> {
    let latest_end = snapshot
        .facts
        .iter()
        .map(|fact| fact.effective_end_at)
        .max()
        .ok_or(auction_domain::DomainError::InvalidSettlementFacts("结算不能缺少 lot 事实"))?;
    if command.due_at.timestamp_nanos_opt().is_none()
        || command.now.timestamp_nanos_opt().is_none()
        || command.now < command.due_at
        || latest_end != command.due_at
        || snapshot.facts.iter().any(|fact| fact.effective_end_at > command.now)
    {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算到期世代与锁内 lot 结束时间不一致",
        ));
    }
    Ok(())
}

/// 用同一结算命令为每个活动代理生成关闭计划；存储层只能应用计划，不能自行发明代理终态。
pub(super) fn plan_proxy_closures(
    command: SettleAuctionCommand,
    snapshot: &SettlementSnapshot,
) -> Result<Vec<CompetitionPlan>, auction_domain::DomainError> {
    if snapshot.lot_competitions.len() != snapshot.facts.len()
        || snapshot.facts.iter().any(|fact| {
            snapshot
                .lot_competitions
                .iter()
                .filter(|competition| competition.state.lot_id == fact.lot_id)
                .count()
                != 1
        })
    {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算 lot 与竞价关闭快照不一致",
        ));
    }

    let mut plans = Vec::new();
    for competition in &snapshot.lot_competitions {
        let Some(active_proxy) = competition.active_proxy else {
            if competition.state.slow_authorization_id.is_some()
                || competition.state.slow_due_at.is_some()
            {
                return Err(auction_domain::DomainError::InvalidBidState(
                    "结算 lot 的慢速代理调度缺少活动授权",
                ));
            }
            continue;
        };
        plans.push(CompetitionPlan::plan(
            competition.rules,
            competition.state,
            Some(active_proxy),
            CompetitionTrigger::CloseProxy {
                command_id: command.command_id,
                authorization_id: active_proxy.id,
                now: command.now,
            },
        )?);
    }
    Ok(plans)
}

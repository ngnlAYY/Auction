//! 结算到期世代与活动代理关闭计划的校验。

use std::collections::HashSet;

use auction_domain::{
    CommandId, CompetitionPlan, CompetitionTrigger, SettlementAmount, SettlementPlan,
};

use crate::SettlementSnapshot;

use super::SettleAuctionCommand;
use super::obligations::append_obligations;

/// 校验新计划和命令账本重放的完整 lot、金额与义务矩阵。
///
/// 只校验每个元素本身不足以防止重复 lot、缺失义务或被篡改的统计总额；这里使用锁内事实
/// 重建义务集合，并要求自然键、顺序、金额和前置关系逐项相同，任何损坏结果都失败关闭。
pub(super) fn validate_settlement_plan(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    plan: &SettlementPlan,
) -> Result<(), auction_domain::DomainError> {
    plan.validate(&snapshot.policy)?;
    if plan.lines.len() != snapshot.facts.len() || snapshot.facts.is_empty() {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算计划与锁内 lot 数量不一致",
        ));
    }

    let mut fact_ids = HashSet::with_capacity(snapshot.facts.len());
    if snapshot.facts.iter().any(|fact| !fact_ids.insert(fact.lot_id)) {
        return Err(auction_domain::DomainError::InvalidSettlementFacts("结算事实包含重复 lot"));
    }

    let mut line_ids = HashSet::with_capacity(plan.lines.len());
    let mut tax_total = SettlementAmount::zero();
    let mut discount_demand = SettlementAmount::zero();
    let mut compensation_total = SettlementAmount::zero();
    for line in &plan.lines {
        if !line_ids.insert(line.lot_id) {
            return Err(auction_domain::DomainError::InvalidSettlementFacts(
                "结算明细包含重复 lot",
            ));
        }
        let fact =
            snapshot.facts.iter().find(|fact| fact.lot_id == line.lot_id).ok_or(
                auction_domain::DomainError::InvalidSettlementFacts("结算明细包含未知 lot"),
            )?;
        if line.end_price != fact.end_price.unwrap_or_else(SettlementAmount::zero) {
            return Err(auction_domain::DomainError::InvalidSettlementFacts(
                "结算明细成交价与锁内事实不一致",
            ));
        }
        tax_total = tax_total.checked_add(line.tax_amount)?;
        discount_demand =
            discount_demand.checked_add(line.end_price.checked_sub(line.listed_price)?)?;
        compensation_total = compensation_total.checked_add(line.compensation_amount)?;
    }
    if tax_total != plan.tax_total
        || discount_demand != plan.discount_demand
        || compensation_total != plan.compensation_total
    {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算计划金额总计与明细不一致",
        ));
    }

    let mut expected = plan.clone();
    expected.obligations.clear();
    append_obligations(command_id, snapshot, &mut expected)?;
    if expected.obligations != plan.obligations {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算义务与锁内事实或自然键不一致",
        ));
    }
    Ok(())
}

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

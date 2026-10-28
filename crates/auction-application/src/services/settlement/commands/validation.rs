//! 结算到期世代与活动代理关闭计划的校验。

use std::collections::HashMap;

use auction_domain::{
    CommandId, CompetitionPlan, CompetitionTrigger, SettlementAmount, SettlementPlan,
};

use crate::{ApplicationError, CommandLedgerError, SettlementSnapshot, SettlementTransactionError};

use super::obligations::{build_obligations, validate_obligations};
use super::{
    SettleAuctionCommand, settlement_claim_command_id, settlement_complete_command_id,
    settlement_complete_request_hash, settlement_workflow_id,
};

/// 校验所有结算账本世代共用的身份与时间形状。
pub(super) fn validate_command_identity(
    command: SettleAuctionCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.claim_command_id.is_nil()
        || command.workflow_id.is_nil()
        || command.auction_id.is_nil()
        || command.command_id == command.claim_command_id
        || command.workflow_id == command.command_id.as_uuid()
        || command.workflow_id == command.claim_command_id.as_uuid()
        || command.due_at.timestamp_nanos_opt().is_none()
        || command.now.timestamp_nanos_opt().is_none()
        || command.now < command.due_at
    {
        return Err(
            SettlementTransactionError::Failed("结算命令身份或时间世代无效".to_owned()).into()
        );
    }
    Ok(())
}

/// 新版完成命令必须与锁内 workflow、领取命令、截止世代和原稳定摘要一致。
/// 旧版 settlement_complete 使用独立防腐重放，不经过这个门禁。
pub(super) fn validate_current_settlement_identity(
    command: SettleAuctionCommand,
    snapshot: &SettlementSnapshot,
) -> Result<(), ApplicationError> {
    validate_command_identity(command)?;
    let auction_id = command.auction_id.as_uuid();
    if snapshot.auction_id != command.auction_id
        || snapshot.workflow_id != Some(command.workflow_id)
        || snapshot.claim_command_id != Some(command.claim_command_id)
        || snapshot.due_at != Some(command.due_at)
        || settlement_workflow_id(auction_id, command.due_at) != Some(command.workflow_id)
        || settlement_claim_command_id(auction_id, command.due_at) != Some(command.claim_command_id)
        || settlement_complete_command_id(auction_id, command.due_at) != Some(command.command_id)
    {
        return Err(SettlementTransactionError::Conflict.into());
    }
    if settlement_complete_request_hash(auction_id, command.workflow_id, command.due_at)?
        != command.request_hash
    {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}

/// 从锁内 lot、冻结策略和首次结算资金池重建当前命令结果，拒绝只满足内部守恒但不符合
/// 当前版本算法的账本 payload。结算后 `snapshot.opening_pool` 已是最新池余额，不能拿它代替
/// 摘要中冻结的首次 opening pool。
pub(super) fn validate_replayed_settlement_plan(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    replayed: &SettlementPlan,
) -> Result<(), auction_domain::DomainError> {
    validate_settlement_plan(command_id, snapshot, replayed)?;
    let persisted = snapshot.persisted_settlement.as_ref().ok_or(
        auction_domain::DomainError::InvalidSettlementFacts("结算命令重放缺少权威结算事实"),
    )?;
    let mut expected = SettlementPlan::calculate(
        &snapshot.policy,
        snapshot.scheduled_open_at,
        snapshot.scheduled_end_at,
        persisted.opening_pool,
        &snapshot.facts,
    )?;
    expected.obligations = build_obligations(command_id, snapshot, &expected)?;
    validate_settlement_plan(command_id, snapshot, &expected)?;
    if persisted.command_id != command_id
        || persisted.policy_version != expected.policy_version
        || persisted.policy_mode != expected.policy_mode
        || persisted.pool_enabled != expected.compensation_pool_enabled
        || persisted.opening_pool != expected.opening_pool
        || persisted.closing_pool != expected.closing_pool
        || persisted.tax_total != expected.tax_total
        || persisted.discount_demand != expected.discount_demand
        || persisted.compensation_total != expected.compensation_total
        || persisted.lines != expected.lines
        || expected != *replayed
    {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算命令结果或持久化摘要无法从权威 lot 与策略重建",
        ));
    }
    Ok(())
}

/// 校验新计划和命令账本重放的完整 lot、金额与义务矩阵。
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

    let mut facts_by_id = HashMap::with_capacity(snapshot.facts.len());
    for fact in &snapshot.facts {
        if facts_by_id.insert(fact.lot_id, Some(fact)).is_some() {
            return Err(auction_domain::DomainError::InvalidSettlementFacts(
                "结算事实包含重复 lot",
            ));
        }
    }

    let mut tax_total = SettlementAmount::zero();
    let mut discount_demand = SettlementAmount::zero();
    let mut compensation_total = SettlementAmount::zero();
    for line in &plan.lines {
        // 同一索引保留已消费的键：None 仍能区分重复 lot 与未知 lot，无需另建判重集合。
        let fact = facts_by_id
            .get_mut(&line.lot_id)
            .ok_or(auction_domain::DomainError::InvalidSettlementFacts("结算明细包含未知 lot"))?
            .take()
            .ok_or(auction_domain::DomainError::InvalidSettlementFacts("结算明细包含重复 lot"))?;
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

    validate_obligations(command_id, snapshot, plan)
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
    // latest_end == due_at <= now 已覆盖全场，不再逐件重复扫描截止时间。
    if command.due_at.timestamp_nanos_opt().is_none()
        || command.now.timestamp_nanos_opt().is_none()
        || command.now < command.due_at
        || latest_end != command.due_at
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
    let mut competition_counts = HashMap::with_capacity(snapshot.lot_competitions.len());
    for competition in &snapshot.lot_competitions {
        *competition_counts.entry(competition.state.lot_id).or_insert(0_usize) += 1;
    }
    if snapshot.lot_competitions.len() != snapshot.facts.len()
        || snapshot.facts.iter().any(|fact| competition_counts.get(&fact.lot_id) != Some(&1))
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

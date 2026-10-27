//! 竞价规划结果和输入状态的领域校验。

use std::collections::{HashMap, HashSet};

use crate::{
    BidMechanism, CommandId, DomainError, LotBidState, ProxyAuthorization, ProxyAuthorizationId,
    ProxyMode, ProxyStatus, ProxyTransition, ProxyTransitionKind,
};

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;

impl CompetitionPlan {
    /// 校验事实顺序、版本、代理转换链及其与最终快照的一致性。
    /// 历史重放只携带局部快照；不要求未变更授权出现，也不对已接受金额重套新输入上限。
    pub fn validate(&self) -> Result<(), DomainError> {
        self.final_lot.validate()?;
        let command_id = self.validate_public_bids()?;
        self.validate_proxy_changes(command_id)?;
        if self.soft_close_applied && self.public_bids.is_empty() {
            return Err(DomainError::InvalidBidState("无竞价事实不能触发软截止"));
        }
        Ok(())
    }

    fn validate_public_bids(&self) -> Result<Option<CommandId>, DomainError> {
        let mut previous_version = self
            .final_lot
            .state_version
            .checked_sub(self.public_bids.len() as u64)
            .ok_or(DomainError::InvalidBidState("公开竞价事实超过最终状态版本"))?;
        let mut command_id = None;
        // 新竞价最多两条事实；更长的历史或异常结果仍使用集合，避免二次扫描。
        let mut seen_bid_ids =
            (self.public_bids.len() > 2).then(|| HashSet::with_capacity(self.public_bids.len()));
        for (index, bid) in self.public_bids.iter().enumerate() {
            bid.validate()?;
            let expected_fact_index = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or(DomainError::InvalidBidState("公开竞价事实序号溢出"))?;
            let expected_state_version =
                previous_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
            let duplicate_id = seen_bid_ids.as_mut().map_or_else(
                || self.public_bids[..index].iter().any(|previous| previous.id == bid.id),
                |seen| !seen.insert(bid.id),
            );
            if bid.fact_index != expected_fact_index
                || bid.lot_id != self.final_lot.lot_id
                || bid.state_version != expected_state_version
                || bid.effective_end_at != self.final_lot.effective_end_at
                || duplicate_id
            {
                return Err(DomainError::InvalidBidState("公开竞价事实顺序或版本无效"));
            }
            // 不重新计算历史竞价，只验证事实链自身不倒退；代理同价事实保留旧版优先级。
            if bid.amount < self.final_lot.start_price
                || index > 0
                    && (bid.amount < self.public_bids[index - 1].amount
                        || (bid.amount == self.public_bids[index - 1].amount
                            && bid.mechanism != BidMechanism::Proxy))
            {
                return Err(DomainError::InvalidBidState("公开竞价事实金额发生倒退或非法同价"));
            }
            if let Some(expected) = command_id {
                if expected != bid.command_id {
                    return Err(DomainError::InvalidBidState("同一规划结果包含多个命令"));
                }
            } else {
                command_id = Some(bid.command_id);
            }
            previous_version = bid.state_version;
        }

        if let Some(last) = self.public_bids.last()
            && (self.final_lot.current_price != Some(last.amount)
                || self.final_lot.leader_id != Some(last.bidder_id)
                || self.final_lot.leader_proxy_last != (last.mechanism == BidMechanism::Proxy))
        {
            return Err(DomainError::InvalidBidState("最终 lot 快照与最后竞价事实不一致"));
        }
        Ok(command_id)
    }

    fn validate_proxy_changes(&self, mut command_id: Option<CommandId>) -> Result<(), DomainError> {
        let mut states = HashMap::with_capacity(self.proxy_states.len());
        for state in &self.proxy_states {
            state.validate()?;
            if state.lot_id != self.final_lot.lot_id || states.insert(state.id, state).is_some() {
                return Err(DomainError::InvalidBidState("代理快照重复或 lot 不一致"));
            }
        }

        let mut last_transitions: HashMap<ProxyAuthorizationId, &ProxyTransition> = HashMap::new();
        for transition in &self.proxy_transitions {
            transition.validate()?;
            if let Some(expected) = command_id {
                if expected != transition.command_id {
                    return Err(DomainError::InvalidBidState("代理转换与竞价事实命令不一致"));
                }
            } else {
                command_id = Some(transition.command_id);
            }

            if let Some(previous) = last_transitions.insert(transition.authorization_id, transition)
            {
                let expected_version = previous
                    .state_version
                    .checked_add(1)
                    .ok_or(DomainError::StateVersionOverflow)?;
                if transition.state_version != expected_version
                    || transition.old_maximum != Some(previous.new_maximum)
                    || transition.old_mode != Some(previous.new_mode)
                    || transition.old_status != Some(previous.new_status)
                {
                    return Err(DomainError::InvalidBidState("代理转换版本或前后状态不连续"));
                }
            }
            // Created 只能是该授权的首条转换；上方连续性校验也会拒绝重复创建。
            if transition.kind == ProxyTransitionKind::Created
                && states
                    .get(&transition.authorization_id)
                    .is_some_and(|state| state.creation_command_id != transition.command_id)
            {
                return Err(DomainError::InvalidBidState("代理创建命令与最终快照不一致"));
            }
        }

        for (authorization_id, state) in &states {
            if let Some(last) = last_transitions.get(authorization_id)
                && (last.state_version != state.state_version
                    || last.new_maximum != state.maximum_amount
                    || last.new_mode != state.mode
                    || last.new_status != state.status)
            {
                return Err(DomainError::InvalidBidState("代理快照与转换链最终状态不一致"));
            }
            if is_active_proxy_status(state.status) != (self.active_proxy_id == Some(state.id)) {
                return Err(DomainError::InvalidBidState("活动代理标识与最终快照不一致"));
            }
        }
        if last_transitions.keys().any(|authorization_id| !states.contains_key(authorization_id)) {
            return Err(DomainError::InvalidBidState("代理转换缺少对应的最终快照"));
        }
        if self.active_proxy_id.is_some_and(ProxyAuthorizationId::is_nil) {
            return Err(DomainError::InvalidBidState("活动代理标识不能是 nil UUID"));
        }
        if let Some(slow_id) = self.final_lot.slow_authorization_id
            && self.active_proxy_id != Some(slow_id)
        {
            return Err(DomainError::InvalidBidState("慢速代理调度与活动授权不一致"));
        }
        if self.final_lot.slow_authorization_id.is_none() && self.final_lot.slow_due_at.is_some() {
            return Err(DomainError::InvalidBidState("慢速代理到期时间缺少授权"));
        }

        Ok(())
    }
}

pub(super) fn validate_trigger_basics(trigger: &CompetitionTrigger) -> Result<(), DomainError> {
    if trigger.command_id().is_nil() || trigger.now().timestamp_nanos_opt().is_none() {
        return Err(DomainError::InvalidBidState("竞价命令缺少有效标识或时间"));
    }

    let bid_ids = trigger.bid_ids();
    // 常见的零、一或两条事实不需要堆集合，较长输入继续线性时间判重。
    let mut seen = (bid_ids.len() > 2).then(|| HashSet::with_capacity(bid_ids.len()));
    for (index, bid_id) in bid_ids.iter().enumerate() {
        let duplicate_id = seen
            .as_mut()
            .map_or_else(|| bid_ids[..index].contains(bid_id), |seen| !seen.insert(*bid_id));
        if bid_id.is_nil() || duplicate_id {
            return Err(DomainError::InvalidBidState("公开竞价事实标识重复或无效"));
        }
    }
    match trigger {
        CompetitionTrigger::OrdinaryBid { amount, .. } if !amount.is_valid_bid_input() => {
            return Err(DomainError::InvalidAmount { value: amount.value() });
        }
        CompetitionTrigger::CreateProxy { maximum_amount, previous_maximum, .. }
            if !maximum_amount.is_valid_bid_input()
                || previous_maximum.is_some_and(|amount| !amount.is_credit_grid_aligned()) =>
        {
            return Err(DomainError::InvalidProxyMaximum);
        }
        CompetitionTrigger::LowerProxy { new_maximum, .. } if !new_maximum.is_valid_bid_input() => {
            return Err(DomainError::InvalidProxyMaximum);
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_active_authorization(
    state: LotBidState,
    active: Option<&ProxyAuthorization>,
) -> Result<(), DomainError> {
    let Some(active) = active else {
        // `leader_proxy_last` 只记录最后一笔接受事实的机制。代理因失格等原因终止后，
        // 该历史标记仍应保留；只有尚存慢速调度引用才要求同时存在活动授权。
        if state.slow_authorization_id.is_some() {
            return Err(DomainError::InvalidBidState("竞价状态引用了不存在的活动代理"));
        }
        return Ok(());
    };

    active.validate()?;
    if !is_active_proxy_status(active.status)
        || active.lot_id != state.lot_id
        || active.authorization_sequence == 0
        || active.authorization_sequence > state.last_proxy_sequence
        || active.maximum_amount < state.current_price.unwrap_or(state.start_price)
    {
        return Err(DomainError::InvalidBidState("活动代理快照与 lot 状态不一致"));
    }

    if state.slow_authorization_id == Some(active.id) {
        if active.mode != ProxyMode::Slow
            || state.leader_id == Some(active.user_id)
            || state.slow_due_at.is_none()
        {
            return Err(DomainError::InvalidBidState("慢速代理调度状态无效"));
        }
    } else if state.slow_authorization_id.is_some()
        || (state.leader_id.is_some() && state.leader_id != Some(active.user_id))
    {
        return Err(DomainError::InvalidBidState("活动代理与领先者状态不一致"));
    }
    Ok(())
}

fn is_active_proxy_status(status: ProxyStatus) -> bool {
    matches!(status, ProxyStatus::Active | ProxyStatus::FinalFast)
}

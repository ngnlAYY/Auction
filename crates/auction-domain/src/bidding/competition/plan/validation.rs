//! 竞价规划结果和输入状态的领域校验。

use std::collections::{HashMap, HashSet};

use crate::{
    DomainError, LotBidState, ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus,
    ProxyTransitionKind,
};

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;

impl CompetitionPlan {
    /// 校验规划结果的事实顺序、版本推进和代理转换链。
    pub fn validate(&self) -> Result<(), DomainError> {
        self.final_lot.validate()?;

        let mut previous_version = self
            .final_lot
            .state_version
            .checked_sub(self.public_bids.len() as u64)
            .ok_or(DomainError::InvalidBidState("公开竞价事实超过最终状态版本"))?;
        let mut command_id = None;
        let mut seen_bid_ids = HashSet::with_capacity(self.public_bids.len());
        for (index, bid) in self.public_bids.iter().enumerate() {
            bid.validate()?;
            let expected_fact_index = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or(DomainError::InvalidBidState("公开竞价事实序号溢出"))?;
            let expected_state_version =
                previous_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
            if bid.fact_index != expected_fact_index
                || bid.lot_id != self.final_lot.lot_id
                || bid.state_version != expected_state_version
                || bid.effective_end_at != self.final_lot.effective_end_at
                || !seen_bid_ids.insert(bid.id)
            {
                return Err(DomainError::InvalidBidState("公开竞价事实顺序或版本无效"));
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

        let mut states = HashMap::with_capacity(self.proxy_states.len());
        for state in &self.proxy_states {
            state.validate()?;
            if state.lot_id != self.final_lot.lot_id || states.insert(state.id, *state).is_some() {
                return Err(DomainError::InvalidBidState("代理快照重复或 lot 不一致"));
            }
        }

        let mut transition_versions: HashMap<ProxyAuthorizationId, u64> = HashMap::new();
        for transition in &self.proxy_transitions {
            transition.validate()?;
            if let Some(expected) = command_id {
                if expected != transition.command_id {
                    return Err(DomainError::InvalidBidState("代理转换与竞价事实命令不一致"));
                }
            } else {
                command_id = Some(transition.command_id);
            }

            let expected_version = match transition.kind {
                ProxyTransitionKind::Created => 1,
                _ => {
                    let previous_version = transition_versions
                        .get(&transition.authorization_id)
                        .copied()
                        .or_else(|| transition.state_version.checked_sub(1))
                        .ok_or(DomainError::StateVersionOverflow)?;
                    previous_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?
                }
            };
            if transition.state_version != expected_version {
                return Err(DomainError::InvalidBidState("代理转换版本没有连续推进"));
            }
            transition_versions.insert(transition.authorization_id, transition.state_version);
        }

        for (authorization_id, state) in &states {
            if let Some(version) = transition_versions.get(authorization_id)
                && *version != state.state_version
            {
                return Err(DomainError::InvalidBidState("代理快照与转换链版本不一致"));
            }
        }

        if let Some(active_id) = self.active_proxy_id
            && states.get(&active_id).is_some_and(|state| !is_active_proxy_status(state.status))
        {
            return Err(DomainError::InvalidBidState("活动代理标识指向终态快照"));
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

    let mut seen = HashSet::with_capacity(trigger.bid_ids().len());
    for bid_id in trigger.bid_ids() {
        if bid_id.is_nil() || !seen.insert(*bid_id) {
            return Err(DomainError::InvalidBidState("公开竞价事实标识重复或无效"));
        }
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

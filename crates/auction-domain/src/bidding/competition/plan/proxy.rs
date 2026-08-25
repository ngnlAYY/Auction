//! 代理授权及其慢速、最终阶段生命周期规划。

use crate::{
    AuctionRules, BidMechanism, DomainError, LotBidState, ProxyAuthorization, ProxyMode,
    ProxyStatus, ProxyTerminalReason,
};

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;
use super::state::{BidDraft, ProxyPlanOutput, clear_slow_schedule, exhaust_proxy, finalize_plan};

pub(super) fn plan_create_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::CreateProxy {
        command_id,
        actor_id,
        authorization_id,
        maximum_amount,
        mode,
        authorization_sequence,
        usage_number,
        bid_ids,
        now,
    } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    if !rules.proxy_enabled {
        return Err(DomainError::ProxyDisabled);
    }
    if rules.proxy_max_outbid_uses > 0 && usage_number > u64::from(rules.proxy_max_outbid_uses) {
        return Err(DomainError::ProxyOutbidLimit);
    }
    if mode == ProxyMode::Slow {
        let final_phase_start = state
            .effective_end_at
            .checked_sub_signed(rules.final_phase_lead)
            .ok_or(DomainError::InvalidSchedule("最终阶段窗口超出时间范围"))?;
        if now >= final_phase_start {
            return Err(DomainError::ProxySlowFinal);
        }
    }
    let expected_sequence =
        state.last_proxy_sequence.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    if actor_id.is_nil()
        || authorization_id.is_nil()
        || authorization_sequence != expected_sequence
        || state.leader_id == Some(actor_id)
        || active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id)
    {
        return Err(DomainError::InvalidBidState("代理授权输入与 lot 状态不一致"));
    }

    let minimum = rules.next_minimum_bid(
        Some(state.start_price),
        state.current_price,
        state.material,
        false,
    )?;
    if maximum_amount < minimum {
        return Err(DomainError::BidTooLow { minimum });
    }

    let created = ProxyAuthorization::new(
        authorization_id,
        state.lot_id,
        actor_id,
        command_id,
        authorization_sequence,
        usage_number,
        maximum_amount,
        mode,
    )?;
    let creation_transition = created.creation_transition()?;
    let mut final_state = state;
    final_state.last_proxy_sequence = authorization_sequence;

    let (transitions, proxy_states, active_proxy_id, amount, winner_user_id) =
        if let Some(active) = active {
            let candidate_wins = maximum_amount > active.maximum_amount
                || (maximum_amount == active.maximum_amount
                    && authorization_sequence < active.authorization_sequence);
            let (winner_user_id, winner_proxy_id, winner_maximum, loser) = if candidate_wins {
                (created.user_id, created.id, created.maximum_amount, active)
            } else {
                (active.user_id, active.id, active.maximum_amount, created)
            };
            let mut price = rules.next_minimum_bid(
                Some(final_state.start_price),
                Some(loser.maximum_amount),
                final_state.material,
                false,
            )?;
            price = price.min(winner_maximum);
            if let Some(current) = final_state.current_price {
                price = price.max(current);
            }
            if price <= final_state.current_price.unwrap_or(final_state.start_price) {
                return Err(DomainError::InvalidBidState("新代理无法产生有效竞价"));
            }
            final_state = clear_slow_schedule(final_state)?;

            let (loser, loser_transition) =
                exhaust_proxy(loser, command_id, ProxyTerminalReason::RankedBelow)?;
            let transitions = vec![creation_transition, loser_transition];
            let mut proxy_states = vec![loser];
            if candidate_wins {
                proxy_states.push(created);
            }
            (transitions, proxy_states, Some(winner_proxy_id), price, winner_user_id)
        } else {
            final_state = clear_slow_schedule(final_state)?;
            (vec![creation_transition], vec![created], Some(created.id), minimum, created.user_id)
        };

    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("代理创建事实数量不正确"));
    }
    let drafts = vec![BidDraft {
        bid_id: bid_ids[0],
        bidder_id: winner_user_id,
        amount,
        mechanism: BidMechanism::Proxy,
    }];
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput { transitions, states: proxy_states, active_proxy_id },
    )
}

pub(super) fn plan_lower_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::LowerProxy { command_id, actor_id, authorization_id, new_maximum, now } =
        trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidProxyMaximum);
    };
    if actor_id != active.user_id || authorization_id != active.id {
        return Err(DomainError::InvalidProxyMaximum);
    }
    let binding = if state.slow_authorization_id.is_some() {
        rules.next_minimum_bid(
            Some(state.start_price),
            state.current_price,
            state.material,
            false,
        )?
    } else {
        state.current_price.unwrap_or(state.start_price)
    };
    if new_maximum < binding {
        return Err(DomainError::InvalidProxyMaximum);
    }
    let mut after = active;
    let transition = after.lower(command_id, new_maximum)?;
    finalize_plan(
        rules,
        state,
        command_id,
        now,
        Vec::new(),
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: Some(after.id),
        },
    )
}

pub(super) fn plan_slow_due(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::SlowDue {
        command_id,
        authorization_id,
        generation,
        due_at,
        bid_ids,
        now,
    } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidBidState("慢速代理已不存在"));
    };
    if active.mode != ProxyMode::Slow
        || authorization_id != active.id
        || state.slow_authorization_id != Some(active.id)
        || state.slow_generation != generation
        || state.slow_due_at != Some(due_at)
        || now < due_at
    {
        return Err(DomainError::InvalidBidState("慢速代理调度已经过期"));
    }
    let current = state.current_price.ok_or(DomainError::InvalidBidState("慢速代理缺少当前价"))?;
    let amount = rules
        .next_minimum_bid(Some(state.start_price), Some(current), state.material, false)?
        .min(active.maximum_amount);
    if amount <= current {
        return Err(DomainError::InvalidBidState("慢速代理无法产生更高出价"));
    }
    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("慢速代理事实数量不正确"));
    }
    let final_state = clear_slow_schedule(state)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        vec![BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
        }],
        ProxyPlanOutput {
            transitions: Vec::new(),
            states: Vec::new(),
            active_proxy_id: Some(active.id),
        },
    )
}

pub(super) fn plan_final_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::FinalProxy { command_id, authorization_id, bid_ids, now } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidBidState("慢速代理已不存在"));
    };
    if active.mode != ProxyMode::Slow || authorization_id != active.id {
        return Err(DomainError::InvalidBidState("只有慢速代理可以进入 final fast"));
    }
    let mut after = active;
    let transition = after.promote_to_final_fast(command_id)?;
    let mut final_state = state;
    let mut drafts = Vec::new();
    if final_state.slow_authorization_id == Some(active.id) {
        let current = final_state
            .current_price
            .ok_or(DomainError::InvalidBidState("最终阶段代理缺少当前价"))?;
        let amount = rules
            .next_minimum_bid(
                Some(final_state.start_price),
                Some(current),
                final_state.material,
                false,
            )?
            .min(active.maximum_amount);
        if amount <= current {
            return Err(DomainError::InvalidBidState("最终阶段代理无法产生更高出价"));
        }
        if bid_ids.len() != 1 {
            return Err(DomainError::InvalidBidState("最终阶段代理事实数量不正确"));
        }
        final_state = clear_slow_schedule(final_state)?;
        drafts.push(BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
        });
    } else if !bid_ids.is_empty() {
        return Err(DomainError::InvalidBidState("无慢速调度时不应产生最终阶段事实"));
    }
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: Some(after.id),
        },
    )
}

pub(super) fn plan_terminal_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
    next_status: ProxyStatus,
) -> Result<CompetitionPlan, DomainError> {
    let (command_id, authorization_id, now) = match trigger {
        CompetitionTrigger::FreezeProxy { command_id, authorization_id, now }
        | CompetitionTrigger::CloseProxy { command_id, authorization_id, now } => {
            (command_id, authorization_id, now)
        }
        _ => unreachable!("代理终态触发器种类已在外层匹配"),
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidProxyTransition { from: next_status, to: next_status });
    };
    if authorization_id != active.id {
        return Err(DomainError::InvalidProxyTransition { from: active.status, to: next_status });
    }
    let mut after = active;
    let transition = match next_status {
        ProxyStatus::Frozen => after.freeze(command_id, ProxyTerminalReason::EligibilityLost)?,
        ProxyStatus::Closed => after.close(command_id)?,
        _ => {
            return Err(DomainError::InvalidProxyTransition {
                from: active.status,
                to: next_status,
            });
        }
    };
    let final_state = clear_slow_schedule(state)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        Vec::new(),
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: None,
        },
    )
}

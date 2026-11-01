//! 代理授权及其慢速、最终阶段生命周期规划。

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;
use super::state::{
    BidDraft, ProxyPlanOutput, clear_slow_schedule, exhaust_proxy, finalize_plan,
    proxy_response_amount,
};
use crate::{
    AuctionRules, AuctionStatus, BidAmount, BidMechanism, DomainError, LotBidState,
    ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason, UserId,
};

pub(super) fn validate_create_proxy_policy(
    rules: AuctionRules,
    auction_status: AuctionStatus,
    mode: ProxyMode,
) -> Result<(), DomainError> {
    if !rules.proxy_enabled {
        return Err(DomainError::ProxyDisabled);
    }
    if mode == ProxyMode::Slow && auction_status == AuctionStatus::Final {
        return Err(DomainError::ProxySlowFinal);
    }
    Ok(())
}

pub(super) fn validate_create_proxy_history(
    rules: AuctionRules,
    usage_number: u64,
    previous_maximum: Option<BidAmount>,
    previous_status: Option<ProxyStatus>,
    maximum_amount: BidAmount,
) -> Result<(), DomainError> {
    if rules.proxy_max_outbid_uses > 0 && usage_number > u64::from(rules.proxy_max_outbid_uses) {
        return Err(DomainError::ProxyOutbidLimit);
    }
    if usage_number == 1 && (previous_maximum.is_some() || previous_status.is_some())
        || usage_number > 1 && (previous_maximum.is_none() || previous_status.is_none())
    {
        return Err(DomainError::InvalidBidState("代理重入上下文与使用次数不一致"));
    }
    match previous_status {
        Some(ProxyStatus::Active | ProxyStatus::FinalFast) => {
            return Err(DomainError::ActiveProxyExists);
        }
        Some(ProxyStatus::Closed) => return Err(DomainError::AuctionClosed),
        Some(ProxyStatus::Exhausted | ProxyStatus::Frozen) | None => {}
    }
    if previous_maximum.is_some_and(|previous| maximum_amount <= previous) {
        return Err(DomainError::ProxyReentryMaximumNotHigher);
    }
    Ok(())
}

pub(super) fn validate_lower_proxy_target(
    active: Option<ProxyAuthorization>,
    actor_id: UserId,
    authorization_id: ProxyAuthorizationId,
) -> Result<ProxyAuthorization, DomainError> {
    let Some(active) = active else {
        return Err(DomainError::ProxyAuthorizationNotActive);
    };
    if actor_id != active.user_id
        || authorization_id != active.id
        || !matches!(active.status, ProxyStatus::Active | ProxyStatus::FinalFast)
    {
        return Err(DomainError::ProxyAuthorizationNotActive);
    }
    Ok(active)
}

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
        auction_status,
        authorization_sequence,
        usage_number,
        previous_maximum,
        previous_status,
        bid_ids,
        now,
    } = trigger
    else {
        return Err(DomainError::InvalidBidState("竞价触发器与规划器不一致"));
    };
    validate_create_proxy_policy(rules, auction_status, mode)?;
    let expected_sequence =
        state.last_proxy_sequence.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    if actor_id.is_nil() || authorization_id.is_nil() || authorization_sequence != expected_sequence
    {
        return Err(DomainError::InvalidBidState("代理授权输入与 lot 状态不一致"));
    }
    validate_create_proxy_history(
        rules,
        usage_number,
        previous_maximum,
        previous_status,
        maximum_amount,
    )?;
    if state.leader_id == Some(actor_id) {
        return Err(DomainError::AlreadyLeading);
    }
    if active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id) {
        return Err(DomainError::ActiveProxyExists);
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

    let (transitions, proxy_states, active_proxy_id, amount, winner_user_id, winner_maximum) =
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
                true,
            )?;
            price = price.min(winner_maximum).bid_ceiling();
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
            (
                transitions,
                proxy_states,
                Some(winner_proxy_id),
                price,
                winner_user_id,
                winner_maximum,
            )
        } else {
            final_state = clear_slow_schedule(final_state)?;
            (
                vec![creation_transition],
                vec![created],
                Some(created.id),
                minimum,
                created.user_id,
                created.maximum_amount,
            )
        };

    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("代理创建事实数量不正确"));
    }
    let drafts = [BidDraft {
        bid_id: bid_ids[0],
        bidder_id: winner_user_id,
        amount,
        mechanism: BidMechanism::Proxy,
        proxy_maximum: Some(winner_maximum),
    }];
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        &drafts,
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
        return Err(DomainError::InvalidBidState("竞价触发器与规划器不一致"));
    };
    let active = validate_lower_proxy_target(active, actor_id, authorization_id)?;
    if new_maximum >= active.maximum_amount {
        return Err(DomainError::ProxyMaximumNotLower);
    }
    let binding = if state.slow_authorization_id.is_some() {
        rules.next_minimum_bid(
            Some(state.start_price),
            state.current_price,
            state.material,
            true,
        )?
    } else {
        state.current_price.unwrap_or(state.start_price)
    };
    if new_maximum < binding {
        return Err(DomainError::ProxyMaximumBelowBinding);
    }
    let mut after = active;
    let transition = after.lower(command_id, new_maximum)?;
    finalize_plan(
        rules,
        state,
        command_id,
        now,
        &[],
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
        return Err(DomainError::InvalidBidState("竞价触发器与规划器不一致"));
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
    let Some(amount) = proxy_response_amount(rules, state, current, active.maximum_amount)? else {
        return exhaust_waiting_proxy(rules, state, active, command_id, now);
    };
    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("慢速代理事实数量不正确"));
    }
    let final_state = clear_slow_schedule(state)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        &[BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
            proxy_maximum: Some(active.maximum_amount),
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
        return Err(DomainError::InvalidBidState("竞价触发器与规划器不一致"));
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidBidState("慢速代理已不存在"));
    };
    if active.mode != ProxyMode::Slow || authorization_id != active.id {
        return Err(DomainError::InvalidBidState("只有慢速代理可以进入 final fast"));
    }
    // 先确认最低价或上限价能严格反超，再推进 Final；否则只追加耗尽事实。
    let mut final_state = state;
    let mut draft = None;
    if final_state.slow_authorization_id == Some(active.id) {
        let current = final_state
            .current_price
            .ok_or(DomainError::InvalidBidState("最终阶段代理缺少当前价"))?;
        let Some(amount) =
            proxy_response_amount(rules, final_state, current, active.maximum_amount)?
        else {
            return exhaust_waiting_proxy(rules, final_state, active, command_id, now);
        };
        if bid_ids.len() != 1 {
            return Err(DomainError::InvalidBidState("最终阶段代理事实数量不正确"));
        }
        final_state = clear_slow_schedule(final_state)?;
        draft = Some(BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
            proxy_maximum: Some(active.maximum_amount),
        });
    } else if !bid_ids.is_empty() {
        return Err(DomainError::InvalidBidState("无慢速调度时不应产生最终阶段事实"));
    }
    let mut after = active;
    let transition = after.promote_to_final_fast(command_id)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        draft.as_slice(),
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
        _ => return Err(DomainError::InvalidBidState("代理终态触发器与规划器不一致")),
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
        &[],
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: None,
        },
    )
}

fn exhaust_waiting_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: ProxyAuthorization,
    command_id: crate::CommandId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<CompetitionPlan, DomainError> {
    let (after, transition) =
        exhaust_proxy(active, command_id, ProxyTerminalReason::StrictlyDefeated)?;
    finalize_plan(
        rules,
        clear_slow_schedule(state)?,
        command_id,
        now,
        &[],
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: None,
        },
    )
}

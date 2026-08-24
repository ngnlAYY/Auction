//! 普通竞价规划。

use crate::{
    AuctionRules, BidMechanism, DomainError, LotBidState, ProxyAuthorization, ProxyMode,
    ProxyTerminalReason,
};

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;
use super::state::{
    BidDraft, ProxyPlanOutput, clear_slow_schedule, exhaust_proxy, finalize_plan,
    schedule_slow_proxy,
};

pub(super) fn plan_ordinary_bid(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::OrdinaryBid { command_id, actor_id, amount, bid_ids, now } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    if actor_id.is_nil() || state.leader_id == Some(actor_id) {
        return Err(DomainError::InvalidBidState("领先者不能重复提交普通出价"));
    }
    if active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id) {
        return Err(DomainError::InvalidBidState("活动代理持有者不能提交普通出价"));
    }

    let minimum =
        rules.next_minimum_bid(Some(state.start_price), state.current_price, state.material, false);
    if amount < minimum {
        return Err(DomainError::BidTooLow { minimum });
    }

    let mut final_state = state;
    let mut transitions = Vec::new();
    let mut proxy_states = Vec::new();
    let mut active_proxy_id = active.as_ref().map(|proxy| proxy.id);
    let mut drafts = vec![BidDraft {
        bid_id: *bid_ids.first().ok_or(DomainError::InvalidBidState("普通出价缺少事实标识"))?,
        bidder_id: actor_id,
        amount,
        mechanism: BidMechanism::Ordinary,
    }];

    if let Some(active) = active {
        if amount > active.maximum_amount {
            let (after, transition) =
                exhaust_proxy(active, command_id, ProxyTerminalReason::StrictlyDefeated)?;
            final_state = clear_slow_schedule(final_state)?;
            transitions.push(transition);
            proxy_states.push(after);
            active_proxy_id = None;
        } else if amount == active.maximum_amount {
            final_state = clear_slow_schedule(final_state)?;
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("代理反超缺少第二条事实标识"))?;
            drafts.push(BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount,
                mechanism: BidMechanism::Proxy,
            });
        } else if active.mode == ProxyMode::Fast {
            let response = rules.next_minimum_bid(
                Some(final_state.start_price),
                Some(amount),
                final_state.material,
                false,
            );
            let response = response.min(active.maximum_amount);
            if response <= amount {
                return Err(DomainError::InvalidBidState("快速代理无法产生更高出价"));
            }
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("快速代理缺少第二条事实标识"))?;
            drafts.push(BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount: response,
                mechanism: BidMechanism::Proxy,
            });
        } else {
            if final_state.slow_authorization_id.is_none() {
                final_state = schedule_slow_proxy(rules, final_state, active.id, now)?;
            }
        }
    } else if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("普通出价事实数量不正确"));
    }
    if bid_ids.len() != drafts.len() {
        return Err(DomainError::InvalidBidState("普通出价事实数量不正确"));
    }

    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput { transitions, states: proxy_states, active_proxy_id },
    )
}

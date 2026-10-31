//! 普通竞价规划。

use crate::{
    AuctionRules, BidMechanism, DomainError, LotBidState, ProxyAuthorization, ProxyMode,
    ProxyTerminalReason,
};

use super::super::trigger::CompetitionTrigger;
use super::CompetitionPlan;
use super::state::{
    BidDraft, ProxyPlanOutput, clear_slow_schedule, exhaust_proxy, finalize_plan,
    proxy_response_amount, schedule_slow_proxy,
};

pub(super) fn plan_ordinary_bid(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::OrdinaryBid { command_id, actor_id, amount, bid_ids, now } = trigger
    else {
        return Err(DomainError::InvalidBidState("竞价触发器与规划器不一致"));
    };
    if actor_id.is_nil() {
        return Err(DomainError::InvalidBidState("普通出价缺少有效参与者"));
    }
    if state.leader_id == Some(actor_id) {
        return Err(DomainError::AlreadyLeading);
    }
    if active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id) {
        return Err(DomainError::ActiveProxyExists);
    }

    // 普通出价沿用完整经济增量；固定 +1000 只用于代理自动反超，不能由历史领先机制放宽。
    let minimum = rules.next_minimum_bid(
        Some(state.start_price),
        state.current_price,
        state.material,
        false,
    )?;
    if amount < minimum {
        return Err(DomainError::BidTooLow { minimum });
    }

    let mut final_state = state;
    let mut transitions = Vec::new();
    let mut proxy_states = Vec::new();
    let mut active_proxy_id = active.as_ref().map(|proxy| proxy.id);
    // 一次普通出价至多追加一次代理反超；固定两格避免为临时草稿分配或扩容。
    let mut drafts = [BidDraft {
        bid_id: *bid_ids.first().ok_or(DomainError::InvalidBidState("普通出价缺少事实标识"))?,
        bidder_id: actor_id,
        amount,
        mechanism: BidMechanism::Ordinary,
    }; 2];
    let mut draft_count = 1;

    if let Some(active) = active {
        // 历史授权仍保留原上限；普通出价触及有效上限时，Slow 也立即同价守位。
        let maximum = active.maximum_amount.bid_ceiling();
        if amount > maximum {
            let (after, transition) =
                exhaust_proxy(active, command_id, ProxyTerminalReason::StrictlyDefeated)?;
            final_state = clear_slow_schedule(final_state)?;
            transitions.push(transition);
            proxy_states.push(after);
            active_proxy_id = None;
        } else if amount == maximum {
            final_state = clear_slow_schedule(final_state)?;
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("代理反超缺少第二条事实标识"))?;
            drafts[1] = BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount,
                mechanism: BidMechanism::Proxy,
            };
            draft_count = 2;
        } else if active.mode == ProxyMode::Fast {
            let response = proxy_response_amount(rules, final_state, amount, maximum)?;
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("快速代理缺少第二条事实标识"))?;
            drafts[1] = BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount: response,
                mechanism: BidMechanism::Proxy,
            };
            draft_count = 2;
        } else {
            if final_state.slow_authorization_id.is_none() {
                final_state = schedule_slow_proxy(rules, final_state, active.id, now)?;
            }
        }
    }
    if bid_ids.len() != draft_count {
        return Err(DomainError::InvalidBidState("普通出价事实数量不正确"));
    }

    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        &drafts[..draft_count],
        ProxyPlanOutput { transitions, states: proxy_states, active_proxy_id },
    )
}

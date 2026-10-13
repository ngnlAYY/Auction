//! 竞价规划结果的状态推进、软截止和代理调度辅助。

use chrono::{DateTime, Utc};

use crate::{
    AcceptedBid, AuctionRules, BidAmount, BidMechanism, CommandId, DomainError, LotBidState,
    MAX_BID_AMOUNT, ProxyAuthorization, ProxyAuthorizationId, ProxyTerminalReason, ProxyTransition,
};

use super::CompetitionPlan;

#[derive(Debug, Clone, Copy)]
pub(super) struct BidDraft {
    pub(super) bid_id: crate::BidId,
    pub(super) bidder_id: crate::UserId,
    pub(super) amount: BidAmount,
    pub(super) mechanism: BidMechanism,
}

pub(super) struct ProxyPlanOutput {
    pub(super) transitions: Vec<ProxyTransition>,
    pub(super) states: Vec<ProxyAuthorization>,
    pub(super) active_proxy_id: Option<ProxyAuthorizationId>,
}

pub(super) fn finalize_plan(
    rules: AuctionRules,
    mut state: LotBidState,
    command_id: CommandId,
    now: DateTime<Utc>,
    drafts: Vec<BidDraft>,
    proxy_output: ProxyPlanOutput,
) -> Result<CompetitionPlan, DomainError> {
    if !drafts.is_empty() && now > state.effective_end_at {
        return Err(DomainError::AuctionClosed);
    }
    let expected_end = if drafts.is_empty() {
        state.effective_end_at
    } else {
        let soft_close_start = state
            .effective_end_at
            .checked_sub_signed(rules.soft_close_window)
            .ok_or(DomainError::InvalidSchedule("软截止窗口超出时间范围"))?;
        if now >= soft_close_start {
            let candidate = now
                .checked_add_signed(rules.soft_close_extension)
                .ok_or(DomainError::InvalidSchedule("软截止延长超出时间范围"))?;
            candidate.max(state.effective_end_at)
        } else {
            state.effective_end_at
        }
    };
    let soft_close_applied = expected_end > state.effective_end_at;
    state.effective_end_at = expected_end;

    let mut public_bids = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.into_iter().enumerate() {
        // 仅约束将要写入的新事实；历史读取和幂等重放仍逐字保留原金额。
        if draft.amount.value() > MAX_BID_AMOUNT {
            return Err(DomainError::InvalidAmount { value: draft.amount.value() });
        }
        let fact_index = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or(DomainError::InvalidBidState("公开竞价事实序号溢出"))?;
        let previous = state.current_price.unwrap_or(state.start_price);
        // 没有历史竞价时，起拍价就是允许写入的第一口；已有当前价后，只有代理内部
        // 竞争产生的同价事实可以维持旧版顺序，普通竞价仍必须严格抬价。
        if draft.amount < previous
            || (draft.amount == previous
                && state.current_price.is_some()
                && draft.mechanism != BidMechanism::Proxy)
        {
            return Err(DomainError::BidTooLow { minimum: previous });
        }
        state.state_version =
            state.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        state.current_price = Some(draft.amount);
        state.leader_id = Some(draft.bidder_id);
        state.leader_proxy_last = draft.mechanism == BidMechanism::Proxy;
        let bid = AcceptedBid {
            id: draft.bid_id,
            command_id,
            lot_id: state.lot_id,
            bidder_id: draft.bidder_id,
            fact_index,
            amount: draft.amount,
            effective_end_at: expected_end,
            state_version: state.state_version,
            mechanism: draft.mechanism,
        };
        bid.validate()?;
        public_bids.push(bid);
    }

    Ok(CompetitionPlan {
        public_bids,
        proxy_transitions: proxy_output.transitions,
        proxy_states: proxy_output.states,
        active_proxy_id: proxy_output.active_proxy_id,
        final_lot: state,
        soft_close_applied,
    })
}

/// Fast、SlowDue 和 Final 共用反超计算，保留历史金额余数与 1B 同价收口规则。
pub(super) fn proxy_response_amount(
    rules: AuctionRules,
    state: LotBidState,
    current: BidAmount,
    maximum: BidAmount,
) -> Result<BidAmount, DomainError> {
    let response = rules
        .next_minimum_bid(Some(state.start_price), Some(current), state.material, false)?
        .min(maximum)
        .bid_ceiling();
    // 历史授权可高于现行上限；不能继续抬价，也不能让代理在上限以下无故同价反超。
    if response < current || (response == current && current.value() != MAX_BID_AMOUNT) {
        return Err(DomainError::InvalidBidState("代理无法产生更高出价"));
    }
    Ok(response)
}

pub(super) fn schedule_slow_proxy(
    rules: AuctionRules,
    mut state: LotBidState,
    authorization_id: ProxyAuthorizationId,
    now: DateTime<Utc>,
) -> Result<LotBidState, DomainError> {
    let generation =
        state.slow_generation.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    let due_at = now
        .checked_add_signed(rules.proxy_slow_interval)
        .ok_or(DomainError::InvalidSchedule("慢速代理到期时间超出范围"))?;
    state.slow_generation = generation;
    state.slow_authorization_id = Some(authorization_id);
    state.slow_due_at = Some(due_at);
    Ok(state)
}

pub(super) fn clear_slow_schedule(mut state: LotBidState) -> Result<LotBidState, DomainError> {
    if state.slow_authorization_id.is_none() {
        return Ok(state);
    }
    state.slow_generation =
        state.slow_generation.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    state.slow_authorization_id = None;
    state.slow_due_at = None;
    Ok(state)
}

pub(super) fn exhaust_proxy(
    mut authorization: ProxyAuthorization,
    command_id: CommandId,
    reason: ProxyTerminalReason,
) -> Result<(ProxyAuthorization, ProxyTransition), DomainError> {
    let transition = authorization.exhaust(command_id, reason)?;
    authorization.validate()?;
    Ok((authorization, transition))
}

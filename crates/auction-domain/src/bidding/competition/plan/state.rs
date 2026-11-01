//! 竞价规划结果的状态推进、软截止和代理调度辅助。

use chrono::{DateTime, Duration, Utc};

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
    /// 仅供锁内核验上限出价，不进入公开事实或命令结果。
    pub(super) proxy_maximum: Option<BidAmount>,
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
    drafts: &[BidDraft],
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
        // 显式零窗口在截止等值时也不延期；非零窗口继续沿用冻结规则取较晚截止。
        if rules.soft_close_window > Duration::zero() && now >= soft_close_start {
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
    for (index, draft) in drafts.iter().enumerate() {
        // 仅约束将要写入的新事实；历史读取和幂等重放仍逐字保留原金额。
        if draft.amount.value() > MAX_BID_AMOUNT {
            return Err(DomainError::InvalidAmount { value: draft.amount.value() });
        }
        let proxy_maximum = match (draft.mechanism, draft.proxy_maximum) {
            (BidMechanism::Ordinary, None) => None,
            (BidMechanism::Proxy, Some(maximum)) if draft.amount <= maximum.bid_ceiling() => {
                Some(maximum.bid_ceiling())
            }
            _ => return Err(DomainError::InvalidBidState("竞价草稿与代理上限不一致")),
        };
        let fact_index = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or(DomainError::InvalidBidState("公开竞价事实序号溢出"))?;
        // v2 逐笔核验完整最低价；仅自动代理恰好用尽授权上限时允许不足完整增量。
        if rules.bidding_version == 2 {
            let minimum = rules.next_minimum_bid(
                Some(state.start_price),
                state.current_price,
                state.material,
                false,
            )?;
            let capped_proxy = state.current_price.is_some() && proxy_maximum == Some(draft.amount);
            if draft.amount < minimum && !capped_proxy {
                return Err(DomainError::BidTooLow { minimum });
            }
        }
        let previous = state.current_price.unwrap_or(state.start_price);
        // 上限出价仍须严格反超；同价事实只保留给冻结的 v1 规则。
        if draft.amount < previous
            || (draft.amount == previous
                && state.current_price.is_some()
                && (rules.bidding_version == 2 || draft.mechanism != BidMechanism::Proxy))
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

/// 按冻结策略计算反击并截到授权上限；v2 只有无法严格反超时才返回耗尽信号。
pub(super) fn proxy_response_amount(
    rules: AuctionRules,
    state: LotBidState,
    current: BidAmount,
    maximum: BidAmount,
) -> Result<Option<BidAmount>, DomainError> {
    let minimum =
        rules.next_minimum_bid(Some(state.start_price), Some(current), state.material, true)?;
    let maximum = maximum.bid_ceiling();
    let response = minimum.min(maximum);
    if rules.bidding_version == 2 {
        return Ok((response > current).then_some(response));
    }
    // 历史授权可高于现行上限；不能继续抬价，也不能让代理在上限以下无故同价反超。
    if response < current || (response == current && current.value() != MAX_BID_AMOUNT) {
        return Err(DomainError::InvalidBidState("代理无法产生更高出价"));
    }
    Ok(Some(response))
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

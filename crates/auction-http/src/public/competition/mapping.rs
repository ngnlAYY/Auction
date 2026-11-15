//! Public 代理命令结果到旧 v1 成功响应的映射；事务内冻结的计划与授权不一致时返回 `None`，
//! 由 handler 失败关闭。

use auction_application::auction::{
    CompetitionProxyCommandOutcome, CompetitionProxyTransitionOutcome, CompetitionWireTimestamp,
};
use auction_contracts::{
    PublicOwnedProxyQuotaResponse, PublicOwnedProxyTransitionResponse,
    PublicProxyCommandAuthorizationResponse, PublicProxyCommandBidResponse,
    PublicProxyCommandResponse,
};
use auction_domain::{
    BidAmount, CompetitionPlan, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransitionKind,
    UserId,
};

/// 把事务内冻结的代理命令结果映射成已发布契约的成功响应。
pub(super) fn map_proxy_command_response(
    proxy: CompetitionProxyCommandOutcome,
    plan: &CompetitionPlan,
    bidder_id: UserId,
    effective_end_at: CompetitionWireTimestamp,
) -> Option<PublicProxyCommandResponse> {
    let expected = proxy.authorization;
    let expected_state_version = i64::try_from(expected.state_version).ok()?;
    let transition_count = usize::try_from(expected.state_version).ok()?;
    if effective_end_at.instant() != plan.final_lot.effective_end_at
        || expected.lot_id != plan.final_lot.lot_id
        || proxy.quota_used != expected.usage_number
        || proxy.quota_limit > 0 && proxy.quota_used > u64::from(proxy.quota_limit)
        || proxy.transitions.len() != transition_count
        || proxy.transitions.iter().enumerate().any(|(index, transition)| {
            let number = u64::try_from(index).ok().and_then(|value| value.checked_add(1));
            Some(transition.transition_number) != number
                || transition.resulting_state_version != transition.transition_number
        })
    {
        return None;
    }
    let state_version = i64::try_from(plan.final_lot.state_version).ok()?;
    let bid = match plan.public_bids.as_slice() {
        [] => None,
        [bid] => {
            if bid.lot_id != plan.final_lot.lot_id
                || bid.amount.value() != plan.final_lot.current_price?.value()
                || bid.state_version != plan.final_lot.state_version
            {
                return None;
            }
            Some(PublicProxyCommandBidResponse {
                lot_id: bid.lot_id.to_string(),
                current_price: bid.amount.value(),
                effective_end_at: effective_end_at.wire().to_owned(),
                state_version,
                leading: plan.final_lot.leader_id == Some(bidder_id),
            })
        }
        _ => return None,
    };

    let status = proxy_status_wire(expected.status);
    let mode = proxy_mode_wire(expected.mode);
    let terminal_reason = proxy.terminal_reason.map(proxy_terminal_reason_wire).map(str::to_owned);
    if (status == "active") != terminal_reason.is_none() {
        return None;
    }
    let transitions = proxy
        .transitions
        .into_iter()
        .map(map_proxy_command_transition)
        .collect::<Option<Vec<_>>>()?;
    Some(PublicProxyCommandResponse {
        authorization: PublicProxyCommandAuthorizationResponse {
            authorization_id: expected.id.to_string(),
            lot_id: expected.lot_id.to_string(),
            authorization_sequence: i64::try_from(expected.authorization_sequence).ok()?,
            usage_number: i64::try_from(expected.usage_number).ok()?,
            mode: mode.to_owned(),
            maximum_amount: expected.maximum_amount.value(),
            quota: PublicOwnedProxyQuotaResponse {
                used: i64::try_from(proxy.quota_used).ok()?,
                limit: i64::from(proxy.quota_limit),
            },
            transitions,
            status: status.to_owned(),
            terminal_reason,
            state_version: expected_state_version,
            bid,
        },
    })
}

fn map_proxy_command_transition(
    transition: CompetitionProxyTransitionOutcome,
) -> Option<PublicOwnedProxyTransitionResponse> {
    Some(PublicOwnedProxyTransitionResponse {
        transition_number: i64::try_from(transition.transition_number).ok()?,
        kind: proxy_transition_kind_wire(transition.kind).to_owned(),
        old_maximum_amount: transition.old_maximum_amount.map(BidAmount::value),
        new_maximum_amount: transition.new_maximum_amount.value(),
        old_mode: transition.old_mode.map(proxy_mode_wire).map(str::to_owned),
        new_mode: proxy_mode_wire(transition.new_mode).to_owned(),
        old_status: transition.old_status.map(proxy_status_wire).map(str::to_owned),
        new_status: proxy_status_wire(transition.new_status).to_owned(),
        reason: transition.reason.map(proxy_terminal_reason_wire).map(str::to_owned),
        resulting_state_version: i64::try_from(transition.resulting_state_version).ok()?,
        created_at: transition.created_at.wire().to_owned(),
    })
}

const fn proxy_mode_wire(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

const fn proxy_status_wire(status: ProxyStatus) -> &'static str {
    match status {
        ProxyStatus::Active | ProxyStatus::FinalFast => "active",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    }
}

const fn proxy_terminal_reason_wire(reason: ProxyTerminalReason) -> &'static str {
    match reason {
        ProxyTerminalReason::StrictlyDefeated => "strictly_defeated",
        ProxyTerminalReason::RankedBelow => "ranked_below",
        ProxyTerminalReason::EligibilityLost => "eligibility_lost",
        ProxyTerminalReason::CompetitionClosed => "competition_closed",
    }
}

const fn proxy_transition_kind_wire(kind: ProxyTransitionKind) -> &'static str {
    match kind {
        ProxyTransitionKind::Created => "created",
        ProxyTransitionKind::Lowered => "lowered",
        ProxyTransitionKind::FinalFast => "final_fast",
        ProxyTransitionKind::Exhausted => "exhausted",
        ProxyTransitionKind::Frozen => "frozen",
        ProxyTransitionKind::Closed => "closed",
    }
}

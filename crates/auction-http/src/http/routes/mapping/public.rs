//! Public 拍卖、竞价、代理和价格历史契约映射。

use super::super::*;
use super::*;

pub(crate) fn map_auction_page(
    page: PublicAuctionPage,
    page_number: i64,
    cursor: Option<String>,
) -> AuctionListResponse {
    let auctions = page.entries.into_iter().map(map_auction_summary).collect();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_cursor);
    let has_more = cursor.as_ref().map(|_| page.has_more);
    AuctionListResponse {
        auctions,
        page: cursor.is_none().then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor,
        has_more,
    }
}

pub(crate) fn map_bid_log_page(
    page: PublicBidLogPage,
    page_number: i64,
    cursor: Option<String>,
) -> PublicBidLogListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_bid_log_cursor);
    PublicBidLogListResponse {
        bids: page.entries.into_iter().map(map_bid_log_entry).collect(),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

pub(crate) fn map_bid_log_entry(entry: PublicBidLogEntry) -> PublicBidLogEntryResponse {
    let source_post_url = forum_post_url(&entry);
    PublicBidLogEntryResponse {
        id: entry.id.to_string(),
        auction_id: entry.auction_id.to_string(),
        lot_id: entry.lot_id.to_string(),
        lot_number: entry.lot_number,
        amount: entry.amount,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        bidder_name: entry.bidder_name,
        source: entry.source,
        source_post_number: entry.source_post_number.map(i64::from),
        source_post_url,
        valid: entry.valid,
        accepted_state_version: entry.accepted_state_version,
        fact_index: i64::from(entry.fact_index),
        mechanism: entry.mechanism,
        created_at: public_timestamp(entry.created_at),
    }
}

fn forum_post_url(entry: &PublicBidLogEntry) -> Option<String> {
    if entry.mechanism != "ordinary"
        || entry.source.as_deref() != Some("forum")
        || entry.source_post_number.is_none()
    {
        return None;
    }
    let thread_id = entry.source_thread_id.as_deref().filter(|value| canonical_forum_id(value))?;
    let post_id = entry.source_post_id.as_deref().filter(|value| canonical_forum_id(value))?;
    Some(format!(
        "https://forums.e-hentai.org/index.php?showtopic={thread_id}&p={post_id}#entry{post_id}"
    ))
}

fn canonical_forum_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn map_owned_bid(bid: PublicMyBid) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: i64::from(bid.fact_index),
        mechanism: bid.mechanism,
        created_at: public_timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: bid.equipment_name,
            category: bid.equipment_category,
        },
    }
}

pub(crate) fn map_owned_proxy(proxy: PublicOwnedProxy) -> PublicOwnedProxyResponse {
    let equipment = PublicOwnedEquipmentResponse {
        name: proxy.equipment_name.clone(),
        category: proxy.equipment_category.clone(),
    };
    PublicOwnedProxyResponse {
        authorization_id: proxy.authorization_id.to_string(),
        auction_id: proxy.auction_id.to_string(),
        lot_id: proxy.lot_id.to_string(),
        lot_number: proxy.lot_number,
        authorization_sequence: proxy.authorization_sequence,
        usage_number: proxy.usage_number,
        mode: proxy.mode,
        maximum_amount: proxy.maximum_amount,
        quota: PublicOwnedProxyQuotaResponse { used: proxy.quota_used, limit: proxy.quota_limit },
        transitions: proxy.transitions.into_iter().map(map_proxy_transition).collect(),
        status: proxy.status,
        terminal_reason: proxy.terminal_reason,
        state_version: proxy.state_version,
        created_at: public_timestamp(proxy.created_at),
        updated_at: public_timestamp(proxy.updated_at),
        equipment,
        bid: proxy
            .bid
            .map(|bid| map_proxy_bid(bid, &proxy.equipment_name, &proxy.equipment_category)),
        invalid_reason: proxy.invalid_reason,
        capabilities: PublicOwnedProxyCapabilitiesResponse {
            can_lower: proxy.capabilities.can_lower,
            waiting: proxy.capabilities.waiting,
            at_maximum: proxy.capabilities.at_maximum,
        },
    }
}

/// 把事务内冻结的代理命令结果映射成旧系统要求的成功响应。
pub(crate) fn map_proxy_command_response(
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

pub(crate) fn map_proxy_bid(
    bid: PublicProxyBid,
    equipment_name: &str,
    equipment_category: &str,
) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: i64::from(bid.fact_index),
        mechanism: bid.mechanism,
        created_at: public_timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: equipment_name.to_owned(),
            category: equipment_category.to_owned(),
        },
    }
}

pub(crate) fn map_proxy_transition(
    transition: PublicProxyTransition,
) -> PublicOwnedProxyTransitionResponse {
    PublicOwnedProxyTransitionResponse {
        transition_number: transition.transition_number,
        kind: transition.kind,
        old_maximum_amount: transition.old_maximum_amount,
        new_maximum_amount: transition.new_maximum_amount,
        old_mode: transition.old_mode,
        new_mode: transition.new_mode,
        old_status: transition.old_status,
        new_status: transition.new_status,
        reason: transition.reason,
        resulting_state_version: transition.resulting_state_version,
        created_at: public_timestamp(transition.created_at),
    }
}

pub(crate) fn map_price_history_page(
    page: PublicPriceHistoryPage,
    page_number: i64,
    cursor: Option<String>,
) -> PublicPriceHistoryListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_price_history_cursor);
    PublicPriceHistoryListResponse {
        rows: page.entries.into_iter().map(map_price_history_entry).collect(),
        stats: page.stats.map(map_price_history_stats),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

pub(crate) fn map_price_history_entry(
    entry: PublicPriceHistoryEntry,
) -> PublicPriceHistoryEntryResponse {
    PublicPriceHistoryEntryResponse {
        lot_id: entry.lot_id.to_string(),
        auction_id: entry.auction_id.to_string(),
        world_id: entry.world_id,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.lot_number,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        equipment_level: entry.equipment_level,
        buyer_name: entry.buyer_name,
        seller_name: entry.seller_name,
        price: entry.price,
        closed_at: public_timestamp(entry.closed_at),
    }
}

pub(crate) fn map_price_history_stats(
    stats: PublicPriceHistoryStats,
) -> PublicPriceHistoryStatsResponse {
    PublicPriceHistoryStatsResponse {
        count: stats.count,
        min: stats.minimum,
        max: stats.maximum,
        median: stats.median,
    }
}

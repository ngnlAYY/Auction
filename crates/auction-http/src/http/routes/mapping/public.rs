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
    PublicBidLogEntryResponse {
        id: entry.id.to_string(),
        auction_id: entry.auction_id.to_string(),
        lot_id: entry.lot_id.to_string(),
        lot_number: entry.lot_number,
        amount: entry.amount,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        bidder_name: entry.bidder_name,
        valid: entry.valid,
        accepted_state_version: entry.accepted_state_version,
        fact_index: entry.fact_index,
        mechanism: entry.mechanism,
        created_at: timestamp(entry.created_at),
    }
}

pub(crate) fn map_owned_bid(bid: PublicMyBid) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: bid.fact_index,
        mechanism: bid.mechanism,
        created_at: timestamp(bid.created_at),
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
        created_at: timestamp(proxy.created_at),
        updated_at: timestamp(proxy.updated_at),
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

/// 校验命令结果所需的历史账户投影身份和追加式转换链。
///
/// 当前授权行可能已经被后续命令推进，但按授权 ID 读取的转换历史仍包含原命令版本；命令
/// 映射器会根据领域计划截取到原状态版本，避免把后续状态或后续转换混入重放结果。
pub(crate) fn valid_command_proxy_identity(
    proxy: &PublicOwnedProxy,
    expected: &ProxyAuthorization,
) -> bool {
    let Ok(expected_state_version) = i64::try_from(expected.state_version) else {
        return false;
    };
    proxy.authorization_id == expected.id.as_uuid()
        && proxy.lot_id == expected.lot_id.as_uuid()
        && proxy.user_id == expected.user_id.as_uuid()
        && proxy.quota_limit >= 0
        && proxy.transitions.len() >= usize::try_from(expected_state_version).unwrap_or(usize::MAX)
}

/// 把已提交的代理快照和本次竞价计划组合成旧系统要求的命令响应。
pub(crate) fn map_proxy_command_response(
    proxy: PublicOwnedProxy,
    plan: &CompetitionPlan,
    expected: &ProxyAuthorization,
    bidder_id: UserId,
) -> Option<PublicProxyCommandResponse> {
    let expected_state_version = i64::try_from(expected.state_version).ok()?;
    let transition_count = usize::try_from(expected_state_version).ok()?;
    if proxy.transitions.len() < transition_count {
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
                effective_end_at: timestamp(plan.final_lot.effective_end_at),
                state_version,
                leading: plan.final_lot.leader_id == Some(bidder_id),
            })
        }
        _ => return None,
    };

    let status = match expected.status {
        ProxyStatus::Active | ProxyStatus::FinalFast => "active",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    };
    let mode = match expected.mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    };
    let terminal_reason = if status == "active" {
        None
    } else {
        proxy
            .transitions
            .get(transition_count.saturating_sub(1))
            .and_then(|transition| transition.reason.clone())
    };
    let transitions =
        proxy.transitions.into_iter().take(transition_count).map(map_proxy_transition).collect();
    Some(PublicProxyCommandResponse {
        authorization: PublicProxyCommandAuthorizationResponse {
            authorization_id: expected.id.to_string(),
            lot_id: expected.lot_id.to_string(),
            authorization_sequence: i64::try_from(expected.authorization_sequence).ok()?,
            usage_number: i64::try_from(expected.usage_number).ok()?,
            mode: mode.to_owned(),
            maximum_amount: expected.maximum_amount.value(),
            quota: PublicOwnedProxyQuotaResponse {
                used: i64::try_from(expected.usage_number).ok()?,
                limit: proxy.quota_limit,
            },
            transitions,
            status: status.to_owned(),
            terminal_reason,
            state_version: expected_state_version,
            bid,
        },
    })
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
        fact_index: bid.fact_index,
        mechanism: bid.mechanism,
        created_at: timestamp(bid.created_at),
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
        created_at: timestamp(transition.created_at),
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
        closed_at: timestamp(entry.closed_at),
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

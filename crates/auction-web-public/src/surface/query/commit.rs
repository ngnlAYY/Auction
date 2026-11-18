//! 地址写入是场次和筛选的提交边界，应用状态从成功写入后的地址重建。

use auction_web_shared::{QueryWriteMode, valid_uuid, write_query_pairs};
use dioxus::prelude::{Signal, WritableExt};

use super::auction::{AuctionParameter, AuctionSelection};
use super::{PublicBoardQuery, public_query_pairs, read_public_query};
use crate::surface::{query_values, read_page_pairs, read_page_query, write_page_pairs};

/// 本 crate 在地址栏管理的全部公开 query 键；含迁移期间曾写出的 `bidlog_page` 别名，
/// 写回时该别名被清除（序列化只产出 canonical `page`）。
const PUBLIC_QUERY_KEYS: &[&str] = &[
    "world",
    "auction",
    "type",
    "equipment_group",
    "max_level",
    "include_unleveled",
    "status",
    "name",
    "from",
    "to",
    "min_price",
    "max_price",
    "buyer",
    "seller",
    "history_sort",
    "history_group",
    "lot",
    "page",
    "page_size",
    "movement_page",
    "movement_page_size",
    "bidlog_page",
    "bid_page",
    "proxy_page",
];

/// 写入成功时返回实际地址解析出的查询。自动纠偏遇到非法场次时返回当前查询。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QueryCommit {
    Applied(PublicBoardQuery),
    Unchanged(PublicBoardQuery),
    InvalidAuction(PublicBoardQuery),
    InvalidSelection,
    Rejected,
}

/// 提交完整场次选择，并在同一次写入中应用相关的世界、筛选和页码。
/// `keys` 限定本次随场次一起提交的键；看板可传 `None` 写入整组查询。
/// `update` 只修改场次以外的字段，场次由传入的选择决定。
pub(crate) fn commit_auction_selection(
    state: Signal<PublicBoardQuery>,
    selection: AuctionSelection,
    keys: Option<&[&str]>,
    update: impl FnOnce(&mut PublicBoardQuery),
) -> QueryCommit {
    let current = if keys.is_some() { read_page_query() } else { read_public_query() };
    let mut next = current.clone();
    update(&mut next);
    let mode = selection.mode();
    let result = commit_query(current, next, Some(selection), mode, keys);
    publish(state, &result);
    result
}

/// 应用场次以外的筛选更新，并从提交后的地址同步页面信号。
pub(crate) fn update_public_query_state(
    state: Signal<PublicBoardQuery>,
    update: impl FnOnce(&mut PublicBoardQuery),
    mode: QueryWriteMode,
) -> bool {
    let current = read_public_query();
    let mut next = current.clone();
    update(&mut next);
    let result = commit_query(current, next, None, mode, None);
    publish(state, &result);
    matches!(result, QueryCommit::Applied(_))
}

fn commit_query(
    current: PublicBoardQuery,
    mut next: PublicBoardQuery,
    selection: Option<AuctionSelection>,
    mode: QueryWriteMode,
    keys: Option<&[&str]>,
) -> QueryCommit {
    let has_selection = selection.is_some();
    let auction = match selection {
        Some(AuctionSelection::Automatic(_))
            if matches!(current.auction_parameter(), AuctionParameter::Invalid(_)) =>
        {
            return QueryCommit::InvalidAuction(current);
        }
        Some(AuctionSelection::Explicit(id) | AuctionSelection::Automatic(id)) => match id {
            None => AuctionParameter::Unspecified,
            Some(id) if valid_uuid(&id) => AuctionParameter::Selected(id),
            Some(_) => return QueryCommit::InvalidSelection,
        },
        None => current.auction_parameter().clone(),
    };
    let keep_auction = current.auction_parameter() == &auction;
    next.set_auction_parameter(auction);
    if let Some(keys) = keys {
        return commit_scoped_query(current, &next, keys, mode);
    }
    if (mode == QueryWriteMode::Push || has_selection) && current == next {
        return QueryCommit::Unchanged(current);
    }

    // 场次未变时连原有编码一起保留。变更时整组替换，避免保留同名非法值。
    let keys: Vec<_> = PUBLIC_QUERY_KEYS
        .iter()
        .copied()
        .filter(|key| *key != "auction" || !keep_auction)
        .collect();
    let pairs: Vec<_> = public_query_pairs(&next)
        .into_iter()
        .filter(|(key, _)| keys.contains(&key.as_str()))
        .collect();
    if !write_query_pairs(&keys, &pairs, mode) {
        return QueryCommit::Rejected;
    }
    QueryCommit::Applied(read_public_query())
}

fn commit_scoped_query(
    current: PublicBoardQuery,
    next: &PublicBoardQuery,
    keys: &[&str],
    mode: QueryWriteMode,
) -> QueryCommit {
    let mut keys = keys.to_vec();
    if !keys.contains(&"auction") {
        keys.push("auction");
    }
    // 只替换本次操作拥有的键，并比较原始值，避免把重复值或旧页码别名误认为无变化。
    let pairs = public_query_pairs(next);
    let current_pairs = read_page_pairs();
    if keys.iter().all(|key| query_values(&current_pairs, key).eq(query_values(&pairs, key))) {
        return QueryCommit::Unchanged(current);
    }
    let pairs: Vec<_> = pairs.into_iter().filter(|(key, _)| keys.contains(&key.as_str())).collect();
    if !write_page_pairs(&keys, &pairs, mode) {
        return QueryCommit::Rejected;
    }
    QueryCommit::Applied(read_page_query())
}

fn publish(mut state: Signal<PublicBoardQuery>, result: &QueryCommit) {
    // 只发布地址写入后解析的快照，拒写时调用方保留原有查询和草稿。
    if let QueryCommit::Applied(applied) = result {
        state.set(applied.clone());
    }
}

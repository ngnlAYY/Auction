//! 公开目录路由（世界、拍卖目录、竞价日志、价格历史、规则、黑名单、补偿池）的 data 校验。

use auction_contracts::{
    AuctionListResponse, AuctionSummaryV2Response, CurrentAuctionResponse,
    PublicBidLogListResponse, PublicBlacklistResponse, PublicCompensationLotPageResponse,
    PublicCompensationResponse, PublicPriceHistoryListResponse, PublicRulesResponse,
    WorldsResponse,
};
use auction_time::Timestamp;
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_keys_only;
use gloo_net::http::Response;
use serde_json::{Map, Value};

use super::{
    account_nonempty, bool_value, decode_success, forum_post_url, game_url, int, nonempty,
    nonnegative, object, optional_nonnegative, optional_time, positive, positive_page_size,
    proxy_mode, raw_nonempty_str, revision, string, time,
};
/// 解码世界目录；世界 ID 是动态 key，但每个 value 的字段闭集固定。
pub(crate) async fn decode_worlds(response: Response) -> Result<WorldsResponse, String> {
    decode_success(response, valid_worlds).await
}

/// 解码指定世界的当前拍卖，并把响应中的 world 与请求目标绑定。请求总是带
/// `include=proxy_default_mode`，因此条目必须含冻结策略的代理默认模式。
pub(crate) async fn decode_current_auction(
    response: Response,
    world: &str,
) -> Result<CurrentAuctionResponse, String> {
    decode_success(response, |value| valid_current_auction_response(value, world)).await
}

/// 解码拍卖目录编号页，并恢复旧浏览器对 status/world 查询条件的响应绑定。
pub(crate) async fn decode_auction_list(
    response: Response,
    world: Option<&str>,
    status: &str,
) -> Result<AuctionListResponse, String> {
    decode_success(response, |value| valid_auction_list(value, world, status, false)).await
}

/// v2 独立扩展摘要字段闭集；v1 仍拒绝论坛地址字段。
pub(crate) async fn decode_auction_list_v2(
    response: Response,
    world: Option<&str>,
    status: &str,
) -> Result<AuctionListResponse<AuctionSummaryV2Response>, String> {
    decode_success(response, |value| valid_auction_list(value, world, status, true)).await
}

/// 解码按编号分页且顺序稳定的公开竞价事实。
pub(crate) async fn decode_bid_log_page(
    response: Response,
) -> Result<PublicBidLogListResponse, String> {
    decode_success(response, valid_bid_log_page).await
}

/// 解码价格历史编号页；cursor 形状不能混入当前 WebUI 的编号页请求。
pub(crate) async fn decode_price_history_page(
    response: Response,
) -> Result<PublicPriceHistoryListResponse, String> {
    decode_success(response, valid_price_history_page).await
}

/// 解码已发布规则文档。
pub(crate) async fn decode_rules(response: Response) -> Result<PublicRulesResponse, String> {
    decode_success(response, valid_rules).await
}

/// 解码公开黑名单编号页。
pub(crate) async fn decode_blacklist(
    response: Response,
) -> Result<PublicBlacklistResponse, String> {
    decode_success(response, valid_blacklist_page).await
}

/// 解码补偿池批次编号页。
pub(crate) async fn decode_compensation_pool(
    response: Response,
) -> Result<PublicCompensationResponse, String> {
    decode_success(response, valid_compensation_pool).await
}

/// 解码单场逐件结算明细编号页。
pub(crate) async fn decode_compensation_lots(
    response: Response,
) -> Result<PublicCompensationLotPageResponse, String> {
    decode_success(response, valid_compensation_lots).await
}

fn valid_worlds(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    if !json_keys_only(value, &["worlds"], &[]) {
        return false;
    }
    value.get("worlds").and_then(object).is_some_and(|worlds| {
        worlds.iter().all(|(name, world)| {
            raw_nonempty_str(name)
                && object(world).is_some_and(|world| {
                    json_keys_only(world, &["enabled"], &[])
                        && bool_value(world.get("enabled")).is_some()
                })
        })
    })
}

fn valid_current_auction_response(value: &Value, world: &str) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["auction"], &[])
        && match value.get("auction") {
            Some(Value::Null) => true,
            Some(auction) => valid_current_auction(auction, world),
            None => false,
        }
}

fn valid_current_auction(value: &Value, world: &str) -> bool {
    let Some(value) = object(value) else { return false };
    if !json_keys_only(
        value,
        &[
            "id",
            "world",
            "number",
            "status",
            "opens_at",
            "scheduled_ends_at",
            "presentation_mode",
            "bidding_open",
            "preview_revision",
            "proxy_default_mode",
        ],
        &["final_restriction"],
    ) || !valid_uuid(string(value.get("id")).unwrap_or_default())
        || string(value.get("world")) != Some(world)
        || !positive(value.get("number"))
        || !time(value.get("opens_at"))
        || !(value.get("scheduled_ends_at") == Some(&Value::Null)
            || time(value.get("scheduled_ends_at")))
        || bool_value(value.get("bidding_open")).is_none()
        || !revision(value.get("preview_revision"))
        // 冻结策略的代理默认模式决定代理新建表单的初始选项；缺失或闭集外的值整条拒绝。
        || !proxy_mode(value.get("proxy_default_mode"))
    {
        return false;
    }
    match string(value.get("presentation_mode")) {
        Some("preview" | "opening") => {
            string(value.get("status")) == Some("preview")
                && bool_value(value.get("bidding_open")) == Some(false)
                && value.get("scheduled_ends_at") == Some(&Value::Null)
        }
        Some("live") => {
            matches!(string(value.get("status")), Some("live" | "final"))
                || (string(value.get("status")) == Some("settling")
                    && bool_value(value.get("bidding_open")) == Some(false))
        }
        _ => false,
    }
}

fn valid_auction_list(
    value: &Value,
    world: Option<&str>,
    status: &str,
    with_forum_url: bool,
) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["auctions", "page", "page_size", "total"], &[])
        && valid_numbered_page(value)
        && value.get("auctions").and_then(Value::as_array).is_some_and(|auctions| {
            auctions
                .iter()
                .all(|auction| valid_auction_summary(auction, world, status, with_forum_url))
        })
}

fn valid_auction_summary(
    value: &Value,
    world: Option<&str>,
    status: &str,
    with_forum_url: bool,
) -> bool {
    let Some(value) = object(value) else { return false };
    if !json_keys_only(
        value,
        &[
            "id",
            "world_id",
            "sequence",
            "status",
            "scheduled_open_at",
            "lot_count",
            "sold_count",
            "unsold_count",
            "sold_amount",
            "state_version",
        ],
        if with_forum_url {
            &["opened_at", "final_phase_at", "effective_end_at", "settled_at", "forum_thread_url"]
        } else {
            &["opened_at", "final_phase_at", "effective_end_at", "settled_at"]
        },
    ) || !valid_uuid(string(value.get("id")).unwrap_or_default())
        || !account_nonempty(value.get("world_id"))
        || string(value.get("status")) != Some(status)
        || world.is_some_and(|world| string(value.get("world_id")) != Some(world))
        || !positive(value.get("sequence"))
        || !time(value.get("scheduled_open_at"))
        || !optional_time(value, "opened_at")
        || !optional_time(value, "final_phase_at")
        || !optional_time(value, "effective_end_at")
        || !optional_time(value, "settled_at")
        || !nonnegative(value.get("lot_count"))
        || !nonnegative(value.get("sold_count"))
        || !nonnegative(value.get("unsold_count"))
        || !nonnegative(value.get("sold_amount"))
        || !positive(value.get("state_version"))
        || value.get("forum_thread_url").is_some_and(|value| {
            !string(Some(value)).is_some_and(super::super::forum::valid_forum_url)
        })
    {
        return false;
    }
    int(value.get("sold_count"))
        .zip(int(value.get("unsold_count")))
        .and_then(|(sold, unsold)| sold.checked_add(unsold))
        .zip(int(value.get("lot_count")))
        .is_some_and(|(closed, total)| closed <= total)
}

fn valid_bid_log_page(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["bids", "page", "page_size", "total"], &[])
        && valid_numbered_page(value)
        && value
            .get("bids")
            .and_then(Value::as_array)
            .is_some_and(|bids| valid_ordered_bid_log(bids))
}

fn valid_ordered_bid_log(values: &[Value]) -> bool {
    let mut previous = None;
    for value in values {
        let Some(current) = bid_log_order_key(value) else { return false };
        if previous.is_some_and(|previous| previous <= current) {
            return false;
        }
        previous = Some(current);
    }
    true
}

fn bid_log_order_key(value: &Value) -> Option<(Timestamp, &str, i64, i64, &str)> {
    let value = object(value)?;
    let bid_id = string(value.get("id"))?;
    let lot_id = string(value.get("lot_id"))?;
    let state_version = int(value.get("accepted_state_version"))?;
    let fact_index = int(value.get("fact_index"))?;
    if !json_keys_only(
        value,
        &[
            "id",
            "auction_id",
            "lot_id",
            "lot_number",
            "amount",
            "equipment_name",
            "equipment_category",
            "bidder_name",
            "valid",
            "accepted_state_version",
            "fact_index",
            "mechanism",
            "created_at",
        ],
        &["source", "source_post_number", "source_post_url", "defeated_proxy"],
    ) || !valid_uuid(bid_id)
        || !valid_uuid(string(value.get("auction_id")).unwrap_or_default())
        || !valid_uuid(lot_id)
        || !positive(value.get("lot_number"))
        || !positive(value.get("amount"))
        || !nonempty(value.get("equipment_name"))
        || !nonempty(value.get("equipment_category"))
        || !nonempty(value.get("bidder_name"))
        || bool_value(value.get("valid")).is_none()
        || state_version <= 0
        || fact_index <= 0
        || !matches!(string(value.get("mechanism")), Some("ordinary" | "proxy"))
        || !valid_bid_log_source(value)
        || !valid_bid_log_defeated_proxy(value)
    {
        return None;
    }
    // Public 时间省略小数尾零，排序须使用保留纳秒精度的时刻。
    let created_at = Timestamp::parse_from_rfc3339(string(value.get("created_at"))?).ok()?;
    Some((created_at, lot_id, state_version, fact_index, bid_id))
}

/// 落败代理只随代理竞争的那一条胜者代理事实下发，且不会是胜者本人。
fn valid_bid_log_defeated_proxy(value: &Map<String, Value>) -> bool {
    let Some(defeated) = value.get("defeated_proxy") else { return true };
    let Some(defeated) = object(defeated) else { return false };
    json_keys_only(defeated, &["bidder_name", "maximum_amount"], &[])
        && nonempty(defeated.get("bidder_name"))
        && positive(defeated.get("maximum_amount"))
        && string(value.get("mechanism")) == Some("proxy")
        && int(value.get("fact_index")) == Some(1)
        && string(defeated.get("bidder_name")) != string(value.get("bidder_name"))
}

fn valid_bid_log_source(value: &Map<String, Value>) -> bool {
    match (
        string(value.get("mechanism")),
        string(value.get("source")),
        int(value.get("source_post_number")),
        string(value.get("source_post_url")),
    ) {
        (Some("proxy"), Some("proxy"), None, None) => true,
        (Some("ordinary"), Some("forum"), Some(post_number), Some(url)) => {
            post_number >= 3 && forum_post_url(url)
        }
        // 兼容尚未增加链接字段的同版本旧响应；新服务会交付可点击的真实帖子 URL。
        (Some("ordinary"), Some("forum"), Some(post_number), None) => post_number >= 3,
        (Some("ordinary"), Some("forum"), None, None) | (Some("ordinary"), None, None, None) => {
            true
        }
        _ => false,
    }
}

fn valid_price_history_page(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["page", "page_size", "rows", "stats", "total"], &[])
        && valid_numbered_page(value)
        && value
            .get("rows")
            .and_then(Value::as_array)
            .is_some_and(|rows| rows.iter().all(valid_price_history_entry))
        && valid_price_history_stats(value.get("stats").unwrap_or(&Value::Null))
}

fn valid_price_history_entry(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(
        value,
        &[
            "lot_id",
            "auction_id",
            "world_id",
            "auction_sequence",
            "lot_number",
            "equipment_name",
            "equipment_category",
            "price",
            "buyer_name",
            "seller_name",
            "closed_at",
        ],
        &["equipment_level"],
    ) && valid_uuid(string(value.get("lot_id")).unwrap_or_default())
        && valid_uuid(string(value.get("auction_id")).unwrap_or_default())
        && account_nonempty(value.get("world_id"))
        && positive(value.get("auction_sequence"))
        && positive(value.get("lot_number"))
        && account_nonempty(value.get("equipment_name"))
        && account_nonempty(value.get("equipment_category"))
        && optional_nonnegative(value, "equipment_level")
        && positive(value.get("price"))
        && account_nonempty(value.get("buyer_name"))
        && account_nonempty(value.get("seller_name"))
        && time(value.get("closed_at"))
}

fn valid_price_history_stats(value: &Value) -> bool {
    object(value).is_some_and(|value| {
        json_keys_only(value, &["count", "min", "max", "median"], &["median_exact"])
            && ["count", "min", "max", "median"].iter().all(|key| nonnegative(value.get(*key)))
            && valid_exact_history_median(value)
    })
}

// 精确字段只接受规范整数或 .5；以二倍整数核对范围，并保留旧 median 的 ties-to-even 口径。
fn valid_exact_history_median(value: &Map<String, Value>) -> bool {
    let exact = match value.get("median_exact") {
        None | Some(Value::Null) => return true,
        Some(Value::String(exact)) => exact,
        _ => return false,
    };
    let (whole, half) = match exact.split_once('.') {
        Some((whole, "5")) => (whole, true),
        Some(_) => return false,
        None => (exact.as_str(), false),
    };
    if whole.is_empty()
        || whole.len() > 1 && whole.starts_with('0')
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let (Ok(whole), Some(minimum), Some(maximum), Some(median)) = (
        whole.parse::<i128>(),
        int(value.get("min")),
        int(value.get("max")),
        int(value.get("median")),
    ) else {
        return false;
    };
    let Some(doubled) = whole.checked_mul(2).and_then(|value| value.checked_add(i128::from(half)))
    else {
        return false;
    };
    let rounded = whole + i128::from(half && whole % 2 == 1);
    doubled <= 9_007_199_254_740_991_i128 * 2
        && doubled >= i128::from(minimum) * 2
        && doubled <= i128::from(maximum) * 2
        && rounded == i128::from(median)
}

fn valid_rules(value: &Value) -> bool {
    object(value).is_some_and(|value| {
        account_nonempty(value.get("locale"))
            && account_nonempty(value.get("title"))
            && string(value.get("markdown")).is_some()
            && positive(value.get("revision"))
            && time(value.get("published_at"))
    })
}

fn valid_blacklist_page(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["users", "total", "page", "page_size"], &[])
        && valid_numbered_page(value)
        && value.get("users").and_then(Value::as_array).is_some_and(|users| {
            users.iter().all(|entry| {
                object(entry).is_some_and(|entry| {
                    json_keys_only(entry, &["username", "reason", "blacklisted_at"], &[])
                        && account_nonempty(entry.get("username"))
                        && string(entry.get("reason")).is_some()
                        && time(entry.get("blacklisted_at"))
                })
            })
        })
}

fn valid_compensation_pool(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    // 两个世界共用一个补偿池，单行始终存在，因此余额与更新时间都必须给出。
    valid_numbered_page(value)
        && nonnegative(value.get("balance"))
        && time(value.get("updated_at"))
        && value
            .get("rounds")
            .and_then(Value::as_array)
            .is_some_and(|rounds| rounds.iter().all(valid_compensation_round))
}

fn valid_compensation_lots(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    valid_numbered_page(value)
        && valid_compensation_round(value.get("round").unwrap_or(&Value::Null))
        && value
            .get("lots")
            .and_then(Value::as_array)
            .is_some_and(|lots| lots.iter().all(valid_compensation_lot))
}

fn valid_compensation_round(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    valid_uuid(string(value.get("auction_id")).unwrap_or_default())
        && account_nonempty(value.get("world_id"))
        && positive(value.get("auction_sequence"))
        && matches!(
            string(value.get("policy_mode")),
            Some("legacy_zero_v0" | "hold_curve_v1" | "hold_curve_v2")
        )
        && nonnegative(value.get("opening_balance"))
        && nonnegative(value.get("tax_income"))
        && nonnegative(value.get("discount_demand"))
        && nonnegative(value.get("compensation_out"))
        && nonnegative(value.get("closing_balance"))
        && nonnegative(value.get("movement_count"))
        && nonnegative(value.get("reversal_count"))
        && int(value.get("reversal_amount")).is_some()
        && time(value.get("settled_at"))
}

fn valid_compensation_lot(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    valid_uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("lot_number"))
        && account_nonempty(value.get("equipment_name"))
        && account_nonempty(value.get("equipment_category"))
        // 明细返回完整 ID/key 链接，复用目录校验，但不接受目录兼容的空串。
        && nonempty(value.get("game_equipment_url"))
        && game_url(value.get("game_equipment_url"))
        && positive(value.get("end_price"))
        && int(value.get("hold_ppm")).is_some_and(|value| (0..=1_000_000).contains(&value))
        && int(value.get("adjustment_ppm"))
            .is_some_and(|value| (-1_000_000..=1_000_000).contains(&value))
        && nonnegative(value.get("leader_held_nanos"))
        && nonnegative(value.get("tax_amount"))
        && nonnegative(value.get("discount_amount"))
        && nonnegative(value.get("compensation_amount"))
        && nonnegative(value.get("buyer_amount"))
        && nonnegative(value.get("seller_amount"))
        && account_nonempty(value.get("buyer_name"))
        && account_nonempty(value.get("seller_name"))
        && value.get("voided").is_some_and(Value::is_boolean)
        && value.get("reversal").is_none_or(|reversal| {
            object(reversal).is_some_and(|reversal| {
                int(reversal.get("amount")).is_some()
                    && nonnegative(reversal.get("tax_shortfall"))
                    && nonnegative(reversal.get("balance_after"))
                    && time(reversal.get("reversed_at"))
            })
        })
}

fn valid_numbered_page(value: &Map<String, Value>) -> bool {
    positive(value.get("page"))
        && positive_page_size(value.get("page_size"))
        && nonnegative(value.get("total"))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        bid_log_order_key, valid_current_auction_response, valid_ordered_bid_log, valid_worlds,
    };

    #[test]
    fn world_directory_requires_the_closed_projection() {
        assert!(valid_worlds(&json!({
            "worlds": { "main": { "enabled": true }, "isekai": { "enabled": false } }
        })));
        for data in [
            json!({"worlds": {"main": {"enabled": true, "extra": true}}}),
            json!({"worlds": {"main": {"enabled": "true"}}}),
            json!({"worlds": {}, "extra": true}),
        ] {
            assert!(!valid_worlds(&data));
        }
    }

    fn current(proxy_default_mode: Option<Value>) -> Value {
        let mut auction = json!({
            "id": "6f1c2a5e-1b2c-4d3e-8f4a-000000000002",
            "world": "main",
            "number": 12,
            "status": "live",
            "opens_at": "2026-11-01T00:00:00Z",
            "scheduled_ends_at": "2026-11-08T00:00:00Z",
            "presentation_mode": "live",
            "bidding_open": true,
            "preview_revision": 1,
        });
        if let Some(mode) = proxy_default_mode {
            auction["proxy_default_mode"] = mode;
        }
        json!({ "auction": auction })
    }

    /// 请求显式选择了代理默认模式，当前场次必须带该键，只接受 fast 与 slow；缺失、null、闭集外
    /// 或非字符串的值整条拒绝。没有当前场次时仍是 `auction: null`。
    #[test]
    fn current_auction_requires_a_frozen_proxy_default_mode() {
        for mode in ["fast", "slow"] {
            assert!(valid_current_auction_response(&current(Some(json!(mode))), "main"), "{mode}");
        }
        for invalid in [
            None,
            Some(Value::Null),
            Some(json!("")),
            Some(json!("Fast")),
            Some(json!("final_fast")),
            Some(json!(1)),
        ] {
            let rejected = current(invalid.clone());
            assert!(!valid_current_auction_response(&rejected, "main"), "{invalid:?}");
        }
        assert!(valid_current_auction_response(&json!({ "auction": null }), "main"));
    }

    fn proxy_bid(defeated_proxy: Option<Value>) -> Value {
        let mut bid = json!({
            "id": "6f1c2a5e-1b2c-4d3e-8f4a-000000000010",
            "auction_id": "6f1c2a5e-1b2c-4d3e-8f4a-000000000002",
            "lot_id": "6f1c2a5e-1b2c-4d3e-8f4a-000000000003",
            "lot_number": 1,
            "amount": 15_750_000,
            "equipment_name": "Legendary Staff",
            "equipment_category": "Staff",
            "bidder_name": "winner",
            "source": "proxy",
            "valid": true,
            "accepted_state_version": 11,
            "fact_index": 1,
            "mechanism": "proxy",
            "created_at": "2026-11-14T12:57:20Z",
        });
        if let Some(defeated) = defeated_proxy {
            bid["defeated_proxy"] = defeated;
        }
        bid
    }

    #[test]
    fn bid_log_orders_fractional_times_by_instant() {
        for (later, earlier) in
            [("00.11Z", "00.1Z"), ("05.5Z", "05Z"), ("10.000000002Z", "10.000000001Z")]
        {
            let mut later_bid = proxy_bid(None);
            later_bid["created_at"] = json!(format!("2026-11-14T12:57:{later}"));
            let mut earlier_bid = proxy_bid(None);
            earlier_bid["created_at"] = json!(format!("2026-11-14T12:57:{earlier}"));
            earlier_bid["accepted_state_version"] = json!(12);

            assert!(valid_ordered_bid_log(&[later_bid.clone(), earlier_bid.clone()]), "{later}");
            assert!(!valid_ordered_bid_log(&[earlier_bid, later_bid]), "{earlier}");
        }
    }

    #[test]
    fn equal_bid_log_instants_use_the_remaining_sort_keys() {
        let mut short_time = proxy_bid(None);
        short_time["created_at"] = json!("2026-11-14T12:57:00.1Z");
        let mut long_time = proxy_bid(None);
        long_time["created_at"] = json!("2026-11-14T12:57:00.10Z");

        for (key, higher) in [
            ("lot_id", json!("6f1c2a5e-1b2c-4d3e-8f4a-000000000004")),
            ("accepted_state_version", json!(12)),
            ("fact_index", json!(2)),
            ("id", json!("6f1c2a5e-1b2c-4d3e-8f4a-000000000011")),
        ] {
            let mut higher_bid = long_time.clone();
            higher_bid[key] = higher;
            assert!(valid_ordered_bid_log(&[higher_bid.clone(), short_time.clone()]), "{key}");
            assert!(!valid_ordered_bid_log(&[short_time.clone(), higher_bid]), "{key}");
        }
        assert!(!valid_ordered_bid_log(&[short_time, long_time]));
    }

    #[test]
    fn bid_log_rejects_an_unparseable_timestamp_even_on_a_single_row() {
        let mut bid = proxy_bid(None);
        bid["created_at"] = json!("2026-11-31T12:57:00Z");
        assert!(!valid_ordered_bid_log(&[bid]));
    }

    /// 落败代理只有出价人与正的上限两个键，不带模式；只能挂在新建代理命令的第 1 条代理事实上，
    /// 且不是胜者本人。任一不符整条拒绝。
    #[test]
    fn bid_log_entry_accepts_a_defeated_proxy_only_on_the_winning_proxy_fact() {
        let defeated = json!({ "bidder_name": "challenger", "maximum_amount": 15_000_000 });
        assert!(bid_log_order_key(&proxy_bid(None)).is_some());
        assert!(bid_log_order_key(&proxy_bid(Some(defeated.clone()))).is_some());
        for invalid in [
            Value::Null,
            json!({ "bidder_name": "challenger" }),
            json!({ "bidder_name": "", "maximum_amount": 15_000_000 }),
            json!({ "bidder_name": "challenger", "maximum_amount": 0 }),
            json!({ "bidder_name": "challenger", "maximum_amount": 15_000_000, "mode": "slow" }),
            json!({ "bidder_name": "winner", "maximum_amount": 15_000_000 }),
        ] {
            assert!(bid_log_order_key(&proxy_bid(Some(invalid.clone()))).is_none(), "{invalid}");
        }
        let mut second_fact = proxy_bid(Some(defeated.clone()));
        second_fact["fact_index"] = json!(2);
        assert!(bid_log_order_key(&second_fact).is_none(), "只随第 1 条事实返回");
        let mut ordinary = proxy_bid(Some(defeated));
        ordinary["mechanism"] = json!("ordinary");
        if let Some(fields) = ordinary.as_object_mut() {
            fields.remove("source");
        }
        assert!(bid_log_order_key(&ordinary).is_none(), "普通出价不带落败代理");
    }
}

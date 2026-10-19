//! 公开目录路由（世界、拍卖目录、竞价日志、价格历史、规则、黑名单、补偿池）的 data 校验。

use super::*;
use auction_contracts::{
    AuctionListResponse, CurrentAuctionResponse, PublicBidLogListResponse, PublicBlacklistResponse,
    PublicCompensationMovementPageResponse, PublicCompensationResponse,
    PublicPriceHistoryListResponse, PublicRulesResponse, WorldsResponse,
};
use gloo_net::http::Response;
use serde_json::{Map, Value};
/// 解码世界目录；世界 ID 是动态 key，但每个 value 的字段闭集固定。
pub(crate) async fn decode_worlds(response: Response) -> Result<WorldsResponse, String> {
    decode_success(response, valid_worlds).await
}

/// 解码指定世界的当前拍卖，并把响应中的 world 与请求目标绑定。
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
    decode_success(response, |value| valid_auction_list(value, world, status)).await
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

/// 解码单场补偿池 movement 编号页。
pub(crate) async fn decode_compensation_movements(
    response: Response,
) -> Result<PublicCompensationMovementPageResponse, String> {
    decode_success(response, valid_compensation_movements).await
}

fn valid_worlds(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(value, &["worlds"], &[]) {
        return false;
    }
    value.get("worlds").and_then(object).is_some_and(|worlds| {
        worlds.iter().all(|(name, world)| {
            raw_nonempty_str(name)
                && object(world).is_some_and(|world| {
                    only(world, &["enabled"], &[]) && bool_value(world.get("enabled")).is_some()
                })
        })
    })
}

fn valid_current_auction_response(value: &Value, world: &str) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["auction"], &[])
        && match value.get("auction") {
            Some(Value::Null) => true,
            Some(auction) => valid_current_auction(auction, world),
            None => false,
        }
}

fn valid_current_auction(value: &Value, world: &str) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(
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
        ],
        &["final_restriction"],
    ) || !uuid(string(value.get("id")).unwrap_or_default())
        || string(value.get("world")) != Some(world)
        || !positive(value.get("number"))
        || !time(value.get("opens_at"))
        || !(value.get("scheduled_ends_at") == Some(&Value::Null)
            || time(value.get("scheduled_ends_at")))
        || bool_value(value.get("bidding_open")).is_none()
        || !revision(value.get("preview_revision"))
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

fn valid_auction_list(value: &Value, world: Option<&str>, status: &str) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["auctions", "page", "page_size", "total"], &[])
        && valid_numbered_page(value)
        && value.get("auctions").and_then(Value::as_array).is_some_and(|auctions| {
            auctions.iter().all(|auction| valid_auction_summary(auction, world, status))
        })
}

fn valid_auction_summary(value: &Value, world: Option<&str>, status: &str) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(
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
        &["opened_at", "final_phase_at", "effective_end_at", "settled_at"],
    ) || !uuid(string(value.get("id")).unwrap_or_default())
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
    only(value, &["bids", "page", "page_size", "total"], &[])
        && valid_numbered_page(value)
        && value
            .get("bids")
            .and_then(Value::as_array)
            .is_some_and(|bids| valid_ordered_bid_log(bids))
}

fn valid_ordered_bid_log(values: &[Value]) -> bool {
    if !values.iter().all(valid_bid_log_entry) {
        return false;
    }
    values.windows(2).all(|pair| {
        let Some(previous) = object(&pair[0]) else { return false };
        let Some(current) = object(&pair[1]) else { return false };
        for key in ["created_at", "lot_id"] {
            let previous_value = string(previous.get(key)).unwrap_or_default();
            let current_value = string(current.get(key)).unwrap_or_default();
            if previous_value != current_value {
                return previous_value > current_value;
            }
        }
        for key in ["accepted_state_version", "fact_index"] {
            let previous_value = int(previous.get(key)).unwrap_or_default();
            let current_value = int(current.get(key)).unwrap_or_default();
            if previous_value != current_value {
                return previous_value > current_value;
            }
        }
        string(previous.get("id")).unwrap_or_default()
            > string(current.get("id")).unwrap_or_default()
    })
}

fn valid_bid_log_entry(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(
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
        &["source", "source_post_number", "source_post_url"],
    ) && uuid(string(value.get("id")).unwrap_or_default())
        && uuid(string(value.get("auction_id")).unwrap_or_default())
        && uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("lot_number"))
        && positive(value.get("amount"))
        && nonempty(value.get("equipment_name"))
        && nonempty(value.get("equipment_category"))
        && nonempty(value.get("bidder_name"))
        && bool_value(value.get("valid")).is_some()
        && positive(value.get("accepted_state_version"))
        && positive(value.get("fact_index"))
        && matches!(string(value.get("mechanism")), Some("ordinary" | "proxy"))
        && valid_bid_log_source(value)
        && time(value.get("created_at"))
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
    only(value, &["page", "page_size", "rows", "stats", "total"], &[])
        && valid_numbered_page(value)
        && value
            .get("rows")
            .and_then(Value::as_array)
            .is_some_and(|rows| rows.iter().all(valid_price_history_entry))
        && valid_price_history_stats(value.get("stats").unwrap_or(&Value::Null))
}

fn valid_price_history_entry(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(
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
    ) && uuid(string(value.get("lot_id")).unwrap_or_default())
        && uuid(string(value.get("auction_id")).unwrap_or_default())
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
        only(value, &["count", "min", "max", "median"], &[])
            && ["count", "min", "max", "median"].iter().all(|key| nonnegative(value.get(*key)))
    })
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
    only(value, &["users", "total", "page", "page_size"], &[])
        && valid_numbered_page(value)
        && value.get("users").and_then(Value::as_array).is_some_and(|users| {
            users.iter().all(|entry| {
                object(entry).is_some_and(|entry| {
                    only(entry, &["username", "reason", "blacklisted_at"], &[])
                        && account_nonempty(entry.get("username"))
                        && string(entry.get("reason")).is_some()
                        && time(entry.get("blacklisted_at"))
                })
            })
        })
}

fn valid_compensation_pool(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    if !valid_numbered_page(value)
        || !nonnegative(value.get("balance"))
        || !value
            .get("rounds")
            .and_then(Value::as_array)
            .is_some_and(|rounds| rounds.iter().all(valid_compensation_round))
    {
        return false;
    }
    match int(value.get("total")) {
        Some(0) => value.get("updated_at") == Some(&Value::Null),
        Some(_) => time(value.get("updated_at")),
        None => false,
    }
}

fn valid_compensation_movements(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    valid_numbered_page(value)
        && valid_compensation_round(value.get("round").unwrap_or(&Value::Null))
        && value
            .get("movements")
            .and_then(Value::as_array)
            .is_some_and(|movements| movements.iter().all(valid_compensation_movement))
}

fn valid_compensation_round(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    uuid(string(value.get("auction_id")).unwrap_or_default())
        && account_nonempty(value.get("world_id"))
        && positive(value.get("auction_sequence"))
        && nonnegative(value.get("opening_balance"))
        && nonnegative(value.get("tax_income"))
        && nonnegative(value.get("discount_demand"))
        && nonnegative(value.get("compensation_out"))
        && nonnegative(value.get("closing_balance"))
        && nonnegative(value.get("movement_count"))
        && time(value.get("settled_at"))
}

fn valid_compensation_movement(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    positive(value.get("sequence"))
        && optional_uuid(value, "lot_id")
        && optional_positive(value, "lot_number")
        && optional_account_nonempty(value, "equipment_name")
        && optional_account_nonempty(value, "equipment_category")
        // 流水返回完整 ID/key 链接，复用目录校验；字段可省略，但不接受目录兼容的空串。
        && value
            .get("game_equipment_url")
            .is_none_or(|url| nonempty(Some(url)) && game_url(Some(url)))
        && value.get("hold_ppm").is_none_or(|amount| {
            int(Some(amount)).is_some_and(|value| (0..=1_000_000).contains(&value))
        })
        && value.get("adjustment_ppm").is_none_or(|amount| {
            int(Some(amount)).is_some_and(|value| (-1_000_000..=1_000_000).contains(&value))
        })
        && optional_nonnegative(value, "leader_held_nanos")
        && matches!(string(value.get("kind")), Some("opening" | "tax" | "compensation" | "closing"))
        && int(value.get("amount")).is_some()
        && nonnegative(value.get("balance_after"))
        && time(value.get("created_at"))
}

fn valid_numbered_page(value: &Map<String, Value>) -> bool {
    positive(value.get("page"))
        && positive_page_size(value.get("page_size"))
        && nonnegative(value.get("total"))
}

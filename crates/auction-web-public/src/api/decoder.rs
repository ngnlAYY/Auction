//! Public 成功响应的 route-specific decoder。
//!
//! 旧版 Public 浏览器在共享 envelope 之后，仍对每条 route 的 `data` 使用 `only(...)` 和
//! 跨字段谓词校验。本模块保留该边界：先验证 envelope 和原始 JSON，再反序列化为 contracts
//! DTO，避免 `serde` 的默认宽松行为把缺字段、未知字段或自相矛盾的投影送进组件。

use auction_contracts::{
    AuctionListResponse, CurrentAuctionResponse, PlaceBidResponse,
    PlaceBidResultResponse as BidResult, PublicBidLogListResponse, PublicBlacklistResponse,
    PublicCompensationMovementPageResponse, PublicCompensationResponse, PublicMyBidsResponse,
    PublicMyProxyResponse, PublicPriceHistoryListResponse, PublicProxyCommandResponse,
    PublicRulesResponse, WorldsResponse,
};
#[cfg(target_arch = "wasm32")]
use auction_contracts::{
    PublicRealtimeDiscoveryResponse, PublicRealtimeEventResponse, PublicRealtimeResponse,
};
use gloo_net::http::Response;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use uuid::{Uuid, Variant};

#[cfg(not(target_arch = "wasm32"))]
use chrono::DateTime;

use super::{LotEnvelope, ProxyCommandAuthorization};

const MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// 解析成功 envelope，并在反序列化前执行 route-specific data 校验。
pub(crate) async fn decode_success<T, F>(response: Response, validate: F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnOnce(&Value) -> bool,
{
    let payload = response.json::<Value>().await.map_err(|_| "invalid_response".to_owned())?;
    decode_success_value(payload, validate)
}

/// 解析旧共享 client 接受的错误 envelope；顶层扩展允许存在，但三项基础字段必须显式出现。
pub(crate) async fn decode_error(response: Response) -> Result<(String, Value), String> {
    let payload = response.json::<Value>().await.map_err(|_| "invalid_response".to_owned())?;
    let envelope = object(&payload).ok_or_else(|| "invalid_response".to_owned())?;
    let code =
        string(envelope.get("code")).ok_or_else(|| "invalid_response".to_owned())?.to_owned();
    string(envelope.get("message")).ok_or_else(|| "invalid_response".to_owned())?;
    let data = envelope.get("data").ok_or_else(|| "invalid_response".to_owned())?.clone();
    Ok((code, data))
}

/// 错误 envelope 无效时统一返回旧 client 的 `invalid_response`。
pub(crate) async fn decode_error_code(response: Response) -> String {
    decode_error(response).await.map(|(code, _)| code).unwrap_or_else(|code| code)
}

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

/// 解码旧版单拍卖 catch-up 页；共享 envelope 允许扩展，但 data/event/payload 必须精确同形。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn decode_auction_realtime_page(
    response: Response,
    auction_id: &str,
) -> Result<PublicRealtimeResponse, String> {
    decode_success(response, |value| valid_auction_realtime_page(value, auction_id)).await
}

/// 解码显式发现流页；它使用独立 data 形状，不能混入旧版单拍卖目标。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn decode_discovery_realtime_page(
    response: Response,
) -> Result<PublicRealtimeDiscoveryResponse, String> {
    decode_success(response, valid_discovery_realtime_page).await
}

/// 解码 WebSocket 的单事件文本帧，并复用 catch-up 页的严格事件防腐层。
#[cfg(target_arch = "wasm32")]
pub(crate) fn decode_realtime_event(
    message: &str,
    auction_id: Option<&str>,
    world: Option<&str>,
) -> Result<PublicRealtimeEventResponse, String> {
    let mut value =
        serde_json::from_str::<Value>(message).map_err(|_| "invalid_response".to_owned())?;
    if !valid_realtime_event(&value, auction_id, auction_id.is_none())
        || !valid_realtime_event_world(&value, world)
    {
        return Err("invalid_response".to_owned());
    }
    normalize_legacy_integer_numbers(&mut value);
    serde_json::from_value(value).map_err(|_| "invalid_response".to_owned())
}

fn decode_success_value<T, F>(payload: Value, validate: F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnOnce(&Value) -> bool,
{
    let envelope = object(&payload).ok_or_else(|| "invalid_response".to_owned())?;
    if !string(envelope.get("code")).is_some()
        || !string(envelope.get("message")).is_some()
        || !envelope.contains_key("data")
    {
        return Err("invalid_response".to_owned());
    }
    let data = envelope.get("data").ok_or_else(|| "invalid_response".to_owned())?;
    if !validate(data) {
        return Err("invalid_response".to_owned());
    }
    let mut data = data.clone();
    normalize_legacy_integer_numbers(&mut data);
    serde_json::from_value(data).map_err(|_| "invalid_response".to_owned())
}

pub(crate) async fn decode_lot_list(
    response: Response,
    auction_id: &str,
    category: Option<&str>,
) -> Result<LotEnvelope, String> {
    decode_success(response, |value| valid_lot_list(value, auction_id, category)).await
}

pub(crate) async fn decode_lot_detail(
    response: Response,
    auction_id: Option<&str>,
    lot_id: &str,
) -> Result<LotEnvelope, String> {
    decode_success(response, |value| valid_lot_detail(value, auction_id, lot_id)).await
}

pub(crate) async fn decode_my_bids(response: Response) -> Result<PublicMyBidsResponse, String> {
    decode_success(response, valid_my_bids).await
}

pub(crate) async fn decode_my_proxy(
    response: Response,
    lot_id: &str,
) -> Result<PublicMyProxyResponse, String> {
    decode_success(response, |value| valid_my_proxy(value, lot_id)).await
}

pub(crate) async fn decode_bid_command(response: Response) -> Result<BidResult, String> {
    decode_success::<PlaceBidResponse, _>(response, valid_bid_command)
        .await
        .map(|response| response.bid)
}

pub(crate) async fn decode_create_proxy_command(
    response: Response,
    lot_id: &str,
    maximum_amount: i64,
    mode: &str,
) -> Result<ProxyCommandAuthorization, String> {
    decode_success::<PublicProxyCommandResponse, _>(response, |value| {
        valid_proxy_command(value, lot_id, Some(maximum_amount), Some(mode), false)
    })
    .await
    .map(|response| response.authorization)
}

pub(crate) async fn decode_lower_proxy_command(
    response: Response,
    lot_id: &str,
    authorization_id: &str,
    maximum_amount: i64,
) -> Result<ProxyCommandAuthorization, String> {
    decode_success::<PublicProxyCommandResponse, _>(response, |value| {
        valid_proxy_command(value, lot_id, Some(maximum_amount), None, true)
            && object(value)
                .and_then(|value| value.get("authorization"))
                .and_then(object)
                .and_then(|value| string(value.get("authorization_id")))
                == Some(authorization_id)
    })
    .await
    .map(|response| response.authorization)
}

#[cfg(target_arch = "wasm32")]
fn valid_auction_realtime_page(value: &Value, auction_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["auction_id", "events", "next_after", "has_more", "refetch_required"], &[])
        && string(value.get("auction_id")) == Some(auction_id)
        && uuid(auction_id)
        && valid_realtime_events(value.get("events"), Some(auction_id), false)
        && nonnegative(value.get("next_after"))
        && bool_value(value.get("has_more")).is_some()
        && bool_value(value.get("refetch_required")).is_some()
}

#[cfg(target_arch = "wasm32")]
fn valid_discovery_realtime_page(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["scope", "events", "next_after", "has_more", "refetch_required"], &[])
        && string(value.get("scope")) == Some("auctions")
        && valid_realtime_events(value.get("events"), None, true)
        && nonnegative(value.get("next_after"))
        && bool_value(value.get("has_more")).is_some()
        && bool_value(value.get("refetch_required")).is_some()
}

#[cfg(target_arch = "wasm32")]
fn valid_realtime_events(value: Option<&Value>, auction_id: Option<&str>, discovery: bool) -> bool {
    value.and_then(Value::as_array).is_some_and(|events| {
        events.iter().all(|event| valid_realtime_event(event, auction_id, discovery))
    })
}

#[cfg(target_arch = "wasm32")]
fn valid_realtime_event(value: &Value, auction_id: Option<&str>, discovery: bool) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(
        value,
        &["event_id", "stream_sequence", "kind", "schema_version", "payload", "created_at"],
        &[],
    ) || !uuid(string(value.get("event_id")).unwrap_or_default())
        || !positive(value.get("stream_sequence"))
        || !time(value.get("created_at"))
    {
        return false;
    }
    let Some(payload) = object(value.get("payload").unwrap_or(&Value::Null)) else {
        return false;
    };
    let kind = string(value.get("kind"));
    let schema_version = int(value.get("schema_version"));
    if discovery && kind != Some("auction.created") {
        return false;
    }
    if auction_id.is_some_and(|expected| string(payload.get("auction_id")) != Some(expected)) {
        return false;
    }
    match (kind, schema_version) {
        (Some("auction.created"), Some(1)) => {
            only(payload, &["auction_id", "world_id", "sequence", "state_version"], &[])
                && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && world(payload.get("world_id"))
                && positive(payload.get("sequence"))
                && positive(payload.get("state_version"))
        }
        (Some("auction.preview_updated"), Some(1)) => {
            only(
                payload,
                &["auction_id", "preview_revision", "item_count", "manual_open", "state_version"],
                &[],
            ) && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && revision(payload.get("preview_revision"))
                && nonnegative(payload.get("item_count"))
                && bool_value(payload.get("manual_open")).is_some()
                && positive(payload.get("state_version"))
        }
        (Some("preview_catalog_changed"), Some(1)) => {
            only(payload, &["world", "auction_id", "preview_revision"], &[])
                && world(payload.get("world"))
                && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && positive(payload.get("preview_revision"))
        }
        (Some("auction.opened"), Some(1)) => {
            only(payload, &["auction_id", "world_id", "state_version", "preview_revision"], &[])
                && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && world(payload.get("world_id"))
                && positive(payload.get("state_version"))
                && positive(payload.get("preview_revision"))
        }
        (
            Some(
                "auction.final_phase_entered"
                | "auction.early_settlement_requested"
                | "auction.cancelled"
                | "auction.settling",
            ),
            Some(1),
        ) => {
            only(payload, &["auction_id", "state_version"], &[])
                && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && positive(payload.get("state_version"))
        }
        (Some("auction.settled"), Some(1)) => {
            only(payload, &["auction_id", "sold_count", "unsold_count", "state_version"], &[])
                && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && nonnegative(payload.get("sold_count"))
                && nonnegative(payload.get("unsold_count"))
                && positive(payload.get("state_version"))
        }
        (Some("auction.settled"), Some(2)) => {
            only(
                payload,
                &[
                    "auction_id",
                    "state_version",
                    "policy_version",
                    "sold_count",
                    "unsold_count",
                    "obligation_count",
                ],
                &[],
            ) && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && positive(payload.get("state_version"))
                && nonnegative(payload.get("policy_version"))
                && nonnegative(payload.get("sold_count"))
                && nonnegative(payload.get("unsold_count"))
                && int(payload.get("sold_count"))
                    .zip(int(payload.get("unsold_count")))
                    .and_then(|(sold, unsold)| sold.checked_add(unsold))
                    .is_some_and(|total| total > 0)
                && positive(payload.get("obligation_count"))
        }
        (Some("lot.bid_accepted"), Some(2)) => {
            only(
                payload,
                &[
                    "auction_id",
                    "lot_id",
                    "bid_id",
                    "amount",
                    "effective_end",
                    "state_version",
                    "fact_index",
                    "mechanism",
                ],
                &[],
            ) && uuid(string(payload.get("auction_id")).unwrap_or_default())
                && uuid(string(payload.get("lot_id")).unwrap_or_default())
                && uuid(string(payload.get("bid_id")).unwrap_or_default())
                && positive(payload.get("amount"))
                && time(payload.get("effective_end"))
                && positive(payload.get("state_version"))
                && positive(payload.get("fact_index"))
                && matches!(string(payload.get("mechanism")), Some("ordinary" | "proxy"))
        }
        _ => false,
    }
}

/// 旧版 auction-specific WebSocket 还会把带 world 的事件与当前页面目标核对；HTTP catch-up
/// 只冻结 auction ID，因此该校验只由单事件帧入口调用。
#[cfg(target_arch = "wasm32")]
fn valid_realtime_event_world(value: &Value, expected_world: Option<&str>) -> bool {
    let Some(expected_world) = expected_world else { return true };
    let Some(event) = object(value) else { return false };
    let Some(payload) = event.get("payload").and_then(object) else { return false };
    match string(event.get("kind")) {
        Some("preview_catalog_changed") => string(payload.get("world")) == Some(expected_world),
        Some("auction.created" | "auction.opened") => {
            string(payload.get("world_id")) == Some(expected_world)
        }
        _ => true,
    }
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
        &["source", "source_post_number"],
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
    ) {
        (Some("proxy"), Some("proxy"), None) => true,
        (Some("ordinary"), Some("forum"), Some(post_number)) => post_number >= 3,
        (Some("ordinary"), Some("forum"), None) | (Some("ordinary"), None, None) => true,
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
        && matches!(string(value.get("kind")), Some("opening" | "tax" | "compensation" | "closing"))
        && int(value.get("amount")).is_some()
        && nonnegative(value.get("balance_after"))
        && time(value.get("created_at"))
}

fn valid_numbered_page(value: &Map<String, Value>) -> bool {
    positive(value.get("page"))
        && positive(value.get("page_size"))
        && nonnegative(value.get("total"))
}

fn valid_lot_list(value: &Value, auction_id: &str, category: Option<&str>) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(
        value,
        &[
            "auction_id",
            "auction_status",
            "projection_kind",
            "presentation_mode",
            "bidding_open",
            "opens_at",
            "preview_revision",
            "lots",
            "total",
            "page",
            "page_size",
            "category_counts",
        ],
        &["final_restriction"],
    ) || !valid_projection(value, auction_id)
        || !positive(value.get("page"))
        || !positive(value.get("page_size"))
        || !nonnegative(value.get("total"))
        || !valid_category_counts(value.get("category_counts"))
    {
        return false;
    }
    let category_key = category.unwrap_or("all");
    let Some(counts) = object(value.get("category_counts").unwrap_or(&Value::Null)) else {
        return false;
    };
    if !matches!(category_key, "all" | "one" | "two" | "sta" | "shd" | "clo" | "lig" | "hea")
        || int(counts.get(category_key)) != int(value.get("total"))
    {
        return false;
    }
    let Some(lots) = value.get("lots").and_then(Value::as_array) else { return false };
    lots.iter().all(|lot| {
        if value.get("projection_kind") == Some(&Value::String("preview".to_owned())) {
            valid_preview_lot(lot, auction_id, None, false)
        } else {
            valid_live_lot(lot, auction_id, None)
                && (value.get("auction_status") != Some(&Value::String("settled".to_owned()))
                    || object(lot).and_then(|lot| string(lot.get("status"))) != Some("live"))
        }
    })
}

fn valid_lot_detail(value: &Value, auction_id: Option<&str>, lot_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    let Some(response_auction_id) = string(value.get("auction_id")) else { return false };
    if auction_id.is_some_and(|expected| expected != response_auction_id)
        || !only(
            value,
            &[
                "auction_id",
                "auction_status",
                "projection_kind",
                "presentation_mode",
                "bidding_open",
                "opens_at",
                "preview_revision",
                "lot",
            ],
            &["final_restriction"],
        )
        || !valid_projection(value, response_auction_id)
    {
        return false;
    }
    if value.get("projection_kind") == Some(&Value::String("preview".to_owned())) {
        valid_preview_lot(
            value.get("lot").unwrap_or(&Value::Null),
            response_auction_id,
            Some(lot_id),
            true,
        )
    } else {
        valid_live_lot(value.get("lot").unwrap_or(&Value::Null), response_auction_id, Some(lot_id))
            && (value.get("auction_status") != Some(&Value::String("settled".to_owned()))
                || object(value.get("lot").unwrap_or(&Value::Null))
                    .and_then(|lot| string(lot.get("status")))
                    != Some("live"))
    }
}

fn valid_projection(value: &Map<String, Value>, auction_id: &str) -> bool {
    string(value.get("auction_id")) == Some(auction_id)
        && uuid(auction_id)
        && matches!(
            string(value.get("auction_status")),
            Some("final" | "live" | "preview" | "settled" | "settling")
        )
        && matches!(string(value.get("projection_kind")), Some("live" | "preview"))
        && matches!(string(value.get("presentation_mode")), Some("live" | "opening" | "preview"))
        && bool_value(value.get("bidding_open")).is_some()
        && time(value.get("opens_at"))
        && revision(value.get("preview_revision"))
        && valid_final_restriction(value)
        && match string(value.get("projection_kind")) {
            Some("preview") => {
                string(value.get("auction_status")) == Some("preview")
                    && bool_value(value.get("bidding_open")) == Some(false)
                    && matches!(string(value.get("presentation_mode")), Some("opening" | "preview"))
                    && value.get("final_restriction").is_none()
            }
            Some("live") => {
                string(value.get("presentation_mode")) == Some("live")
                    && matches!(string(value.get("auction_status")), Some("live" | "final"))
                    || (matches!(string(value.get("auction_status")), Some("settling" | "settled"))
                        && bool_value(value.get("bidding_open")) == Some(false)
                        && string(value.get("presentation_mode")) == Some("live"))
            }
            _ => false,
        }
}

fn valid_final_restriction(value: &Map<String, Value>) -> bool {
    match string(value.get("auction_status")) {
        Some("final") => object(value.get("final_restriction").unwrap_or(&Value::Null))
            .is_some_and(|restriction| {
                only(restriction, &["new_slow_proxy_allowed", "existing_slow_proxy_converted"], &[])
                    && bool_value(restriction.get("new_slow_proxy_allowed")) == Some(false)
                    && bool_value(restriction.get("existing_slow_proxy_converted")) == Some(true)
            }),
        _ => value.get("final_restriction").is_none(),
    }
}

fn valid_live_lot(value: &Value, auction_id: &str, lot_id: Option<&str>) -> bool {
    let Some(value) = object(value) else { return false };
    let has_leader = bool_value(value.get("has_leader"));
    let leader_present = value.contains_key("leader_name");
    only(
        value,
        &[
            "id",
            "auction_id",
            "lot_number",
            "status",
            "start_price",
            "current_price",
            "next_bid",
            "has_leader",
            "seller_name",
            "state_version",
            "equipment",
        ],
        &["effective_end_at", "leader_name"],
    ) && uuid(string(value.get("id")).unwrap_or_default())
        && lot_id.is_none_or(|expected| string(value.get("id")) == Some(expected))
        && string(value.get("auction_id")) == Some(auction_id)
        && positive(value.get("lot_number"))
        && matches!(string(value.get("status")), Some("live" | "sold" | "unsold"))
        && positive(value.get("start_price"))
        && positive(value.get("current_price"))
        && int(value.get("current_price")).is_some_and(|current| {
            int(value.get("start_price")).is_some_and(|start| current >= start)
        })
        && positive(value.get("next_bid"))
        && has_leader.is_some()
        && match has_leader {
            Some(true) => int(value.get("next_bid")).is_some_and(|next| {
                int(value.get("current_price")).is_some_and(|current| next > current)
            }),
            Some(false) => {
                int(value.get("current_price")) == int(value.get("start_price"))
                    && int(value.get("next_bid")) == int(value.get("start_price"))
            }
            None => false,
        }
        && nonempty(value.get("seller_name"))
        && (!leader_present || nonempty(value.get("leader_name")))
        && has_leader == Some(leader_present)
        && match string(value.get("status")) {
            Some("live" | "sold") => time(value.get("effective_end_at")),
            Some("unsold") => {
                value.get("effective_end_at").is_none() || time(value.get("effective_end_at"))
            }
            _ => false,
        }
        && positive(value.get("state_version"))
        && valid_equipment(value.get("equipment"), true)
}

fn valid_preview_lot(value: &Value, auction_id: &str, lot_id: Option<&str>, detail: bool) -> bool {
    let Some(value) = object(value) else { return false };
    let optional = if detail {
        &["equipment_category", "equipment_level", "avg_roll", "equipment"][..]
    } else {
        &["equipment_category", "equipment_level", "avg_roll"][..]
    };
    only(
        value,
        &[
            "id",
            "auction_id",
            "code",
            "status",
            "seller_name",
            "start_price",
            "equipment_name",
            "game_equipment_url",
        ],
        optional,
    ) && uuid(string(value.get("id")).unwrap_or_default())
        && lot_id.is_none_or(|expected| string(value.get("id")) == Some(expected))
        && string(value.get("auction_id")) == Some(auction_id)
        && nonempty(value.get("code"))
        && string(value.get("status")) == Some("pending")
        && nonempty(value.get("seller_name"))
        && positive(value.get("start_price"))
        && nonempty(value.get("equipment_name"))
        && optional_nonempty(value, "equipment_category")
        && optional_nonnegative(value, "equipment_level")
        && optional_finite(value, "avg_roll")
        && game_url(value.get("game_equipment_url"))
        && (!detail || valid_preview_detail_equipment(value))
}

fn valid_preview_detail_equipment(summary: &Map<String, Value>) -> bool {
    let Some(equipment) = object(summary.get("equipment").unwrap_or(&Value::Null)) else {
        return false;
    };
    if !valid_preview_equipment(equipment) {
        return false;
    }
    string(equipment.get("name")) == string(summary.get("equipment_name"))
        && optional_string(equipment, "category") == optional_string(summary, "equipment_category")
        && optional_int(equipment, "level") == optional_int(summary, "equipment_level")
        && string(equipment.get("game_equipment_url")) == string(summary.get("game_equipment_url"))
}

fn valid_preview_equipment(value: &Map<String, Value>) -> bool {
    only(
        value,
        &["name", "game_equipment_url"],
        &["category", "level", "info_summary", "stats", "avg_roll"],
    ) && nonempty(value.get("name"))
        && optional_nonempty(value, "category")
        && optional_nonnegative(value, "level")
        && optional_nonempty(value, "info_summary")
        && optional_stats(value, "stats")
        && optional_finite(value, "avg_roll")
        && game_url(value.get("game_equipment_url"))
}

fn valid_equipment(value: Option<&Value>, full: bool) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    let required = ["name", "category", "game_equipment_url"];
    let optional = ["attributes", "level", "info_summary", "stats", "avg_roll"];
    only(value, &required, &optional)
        && nonempty(value.get("name"))
        && nonempty(value.get("category"))
        && optional_nonnegative(value, "level")
        && optional_nonempty(value, "info_summary")
        && optional_stats(value, "stats")
        && optional_finite(value, "avg_roll")
        && game_url(value.get("game_equipment_url"))
        && (!full || optional_attributes(value, "attributes"))
        && (!full || value.get("attributes").is_none() || valid_attributes(value.get("attributes")))
}

fn valid_attributes(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    only(value, &["quality"], &["type", "prefix", "suffix", "level", "tradeable", "stats"])
        && nonempty(value.get("quality"))
        && optional_nonempty(value, "type")
        && optional_nonempty(value, "prefix")
        && optional_nonempty(value, "suffix")
        && optional_nonnegative(value, "level")
        && optional_bool(value, "tradeable")
        && optional_attribute_stats(value, "stats")
}

fn valid_attribute_stats(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    value.iter().all(|(name, stat)| {
        trimmed_nonempty_str(name) && {
            let Some(stat) = object(stat) else { return false };
            only(stat, &["value"], &["base"])
                && nonempty(stat.get("value"))
                && optional_finite(stat, "base")
        }
    })
}

fn valid_stats(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    value.iter().all(|(name, stat)| {
        raw_nonempty_str(name) && {
            let Some(stat) = object(stat) else { return false };
            only(stat, &["value"], &["base", "roll"])
                && nonempty(stat.get("value"))
                && optional_finite(stat, "base")
                && optional_finite(stat, "roll")
        }
    })
}

fn valid_my_bids(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(
        value,
        &[
            "bids",
            "proxies",
            "bid_total",
            "proxy_total",
            "page",
            "bid_page",
            "proxy_page",
            "page_size",
        ],
        &[],
    ) && positive(value.get("page"))
        && positive(value.get("bid_page"))
        && positive(value.get("proxy_page"))
        && positive(value.get("page_size"))
        && value
            .get("bids")
            .and_then(Value::as_array)
            .is_some_and(|bids| bids.iter().all(valid_account_bid))
        && value.get("proxies").and_then(Value::as_array).is_some_and(|proxies| {
            proxies.iter().all(|proxy| valid_owned_proxy(proxy, None, false))
        })
        && nonnegative(value.get("bid_total"))
        && nonnegative(value.get("proxy_total"))
}

fn valid_my_proxy(value: &Value, lot_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["authorization"], &[])
        && (value.get("authorization") == Some(&Value::Null)
            || valid_owned_proxy(
                value.get("authorization").unwrap_or(&Value::Null),
                Some(lot_id),
                true,
            ))
}

fn valid_owned_bid(value: &Value) -> bool {
    valid_owned_bid_with_equipment(value, true)
}

fn valid_account_bid(value: &Value) -> bool {
    valid_owned_bid_with_equipment(value, false)
}

fn valid_owned_bid_with_equipment(value: &Value, trimmed_equipment: bool) -> bool {
    let Some(value) = object(value) else { return false };
    only(
        value,
        &[
            "id",
            "auction_id",
            "lot_id",
            "lot_number",
            "amount",
            "accepted_state_version",
            "fact_index",
            "mechanism",
            "created_at",
            "leading",
            "equipment",
        ],
        &[],
    ) && uuid(string(value.get("id")).unwrap_or_default())
        && uuid(string(value.get("auction_id")).unwrap_or_default())
        && uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("lot_number"))
        && positive(value.get("amount"))
        && positive(value.get("accepted_state_version"))
        && positive(value.get("fact_index"))
        && matches!(string(value.get("mechanism")), Some("ordinary" | "proxy"))
        && time(value.get("created_at"))
        && bool_value(value.get("leading")).is_some()
        && valid_owned_equipment(value.get("equipment"), trimmed_equipment)
}

fn valid_owned_proxy(value: &Value, lot_id: Option<&str>, trimmed_equipment: bool) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(
        value,
        &[
            "authorization_id",
            "auction_id",
            "lot_id",
            "lot_number",
            "authorization_sequence",
            "usage_number",
            "mode",
            "maximum_amount",
            "quota",
            "transitions",
            "status",
            "state_version",
            "created_at",
            "updated_at",
            "equipment",
            "capabilities",
        ],
        &["bid", "invalid_reason", "terminal_reason"],
    ) || !uuid(string(value.get("authorization_id")).unwrap_or_default())
        || !uuid(string(value.get("auction_id")).unwrap_or_default())
        || lot_id.is_some_and(|expected| string(value.get("lot_id")) != Some(expected))
        || !uuid(string(value.get("lot_id")).unwrap_or_default())
        || !positive(value.get("lot_number"))
        || !positive(value.get("authorization_sequence"))
        || !positive(value.get("usage_number"))
        || !proxy_mode(value.get("mode"))
        || !positive(value.get("maximum_amount"))
        || !valid_quota(value.get("quota"), int(value.get("usage_number")))
        || !positive(value.get("state_version"))
        || !valid_owned_lifecycle(value)
        || !valid_capabilities(value.get("capabilities"))
        || !valid_transitions(value.get("transitions"), value)
        || !time(value.get("created_at"))
        || !time(value.get("updated_at"))
        || !valid_owned_equipment(value.get("equipment"), trimmed_equipment)
        || (value.get("bid").is_some()
            && !valid_owned_bid(value.get("bid").unwrap_or(&Value::Null)))
        || (value.get("bid").is_some()
            && (object(value.get("bid").unwrap_or(&Value::Null))
                .and_then(|bid| string(bid.get("auction_id")))
                != string(value.get("auction_id"))
                || object(value.get("bid").unwrap_or(&Value::Null))
                    .and_then(|bid| string(bid.get("lot_id")))
                    != string(value.get("lot_id"))
                || object(value.get("bid").unwrap_or(&Value::Null))
                    .and_then(|bid| int(bid.get("lot_number")))
                    != int(value.get("lot_number"))))
    {
        return false;
    }
    let status = string(value.get("status"));
    let capabilities = object(value.get("capabilities").unwrap_or(&Value::Null));
    if status != Some("active") {
        return capabilities.is_some_and(|capabilities| {
            bool_value(capabilities.get("can_lower")) == Some(false)
                && bool_value(capabilities.get("waiting")) == Some(false)
                && bool_value(capabilities.get("at_maximum")) == Some(false)
        });
    }
    let Some(bid) = object(value.get("bid").unwrap_or(&Value::Null)) else { return false };
    capabilities.is_some_and(|capabilities| {
        bool_value(bid.get("leading")) != bool_value(capabilities.get("waiting"))
            && (bool_value(capabilities.get("waiting")) != Some(true)
                || string(value.get("mode")) == Some("slow"))
    })
}

fn valid_bid_command(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["bid"], &[]) && valid_bid_result(value.get("bid").unwrap_or(&Value::Null))
}

fn valid_bid_result(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["lot_id", "current_price", "effective_end_at", "state_version", "leading"], &[])
        && uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("current_price"))
        && time(value.get("effective_end_at"))
        && positive(value.get("state_version"))
        && bool_value(value.get("leading")).is_some()
}

fn normalize_legacy_integer_numbers(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_legacy_integer_numbers),
        Value::Object(values) => {
            values.values_mut().for_each(normalize_legacy_integer_numbers);
        }
        Value::Number(number) if number.as_i64().is_none() && number.as_u64().is_none() => {
            let Some(raw) = number.as_f64() else { return };
            if raw.is_finite() && raw.fract() == 0.0 && raw.abs() <= MAX_SAFE_INTEGER_F64 {
                *value = if raw.is_sign_negative() {
                    Value::Number(serde_json::Number::from(raw as i64))
                } else {
                    Value::Number(serde_json::Number::from(raw as u64))
                };
            }
        }
        _ => {}
    }
}

fn valid_proxy_command(
    value: &Value,
    lot_id: &str,
    expected_maximum: Option<i64>,
    expected_mode: Option<&str>,
    lowered: bool,
) -> bool {
    let Some(value) = object(value) else { return false };
    if !only(value, &["authorization"], &[])
        || !valid_command_authorization(value.get("authorization").unwrap_or(&Value::Null))
    {
        return false;
    }
    let Some(authorization) = object(value.get("authorization").unwrap_or(&Value::Null)) else {
        return false;
    };
    string(authorization.get("lot_id")) == Some(lot_id)
        && uuid(lot_id)
        && expected_maximum
            .is_none_or(|expected| int(authorization.get("maximum_amount")) == Some(expected))
        && expected_mode.is_none_or(|expected| string(authorization.get("mode")) == Some(expected))
        && if lowered {
            string(authorization.get("status")) == Some("active")
                && authorization.get("bid").is_none()
        } else {
            matches!(string(authorization.get("status")), Some("active" | "exhausted"))
                && authorization.get("bid").is_some_and(|bid| {
                    object(bid).and_then(|bid| string(bid.get("lot_id"))) == Some(lot_id)
                        && bool_value(object(bid).and_then(|bid| bid.get("leading")))
                            == Some(string(authorization.get("status")) == Some("active"))
                })
        }
}

fn valid_command_authorization(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(
        value,
        &[
            "authorization_id",
            "lot_id",
            "authorization_sequence",
            "usage_number",
            "mode",
            "maximum_amount",
            "quota",
            "transitions",
            "status",
            "state_version",
        ],
        &["terminal_reason", "bid"],
    ) && uuid(string(value.get("authorization_id")).unwrap_or_default())
        && uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("authorization_sequence"))
        && positive(value.get("usage_number"))
        && proxy_mode(value.get("mode"))
        && positive(value.get("maximum_amount"))
        && valid_quota(value.get("quota"), int(value.get("usage_number")))
        && positive(value.get("state_version"))
        && valid_lifecycle(value)
        && valid_transitions(value.get("transitions"), value)
        && value.get("bid").is_none_or(valid_bid_result_without_route)
}

fn valid_bid_result_without_route(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    only(value, &["lot_id", "current_price", "effective_end_at", "state_version", "leading"], &[])
        && uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("current_price"))
        && time(value.get("effective_end_at"))
        && positive(value.get("state_version"))
        && bool_value(value.get("leading")).is_some()
}

fn valid_transitions(value: Option<&Value>, authorization: &Map<String, Value>) -> bool {
    let Some(transitions) = value.and_then(Value::as_array) else { return false };
    let Some(state_version) = int(authorization.get("state_version")) else { return false };
    if transitions.len() != usize::try_from(state_version).unwrap_or(usize::MAX)
        || transitions.is_empty()
    {
        return false;
    }
    let mut previous: Option<&Map<String, Value>> = None;
    for (index, transition) in transitions.iter().enumerate() {
        let Some(transition) = object(transition) else { return false };
        if !valid_transition(transition, i64::try_from(index + 1).unwrap_or(i64::MAX)) {
            return false;
        }
        if let Some(previous) = previous
            && (optional_int(transition, "old_maximum_amount")
                != optional_int(previous, "new_maximum_amount")
                || optional_string(transition, "old_mode") != optional_string(previous, "new_mode")
                || optional_string(transition, "old_status")
                    != optional_string(previous, "new_status")
                || time_value(transition.get("created_at"))
                    .zip(time_value(previous.get("created_at")))
                    .is_some_and(|(current, previous)| current < previous))
        {
            return false;
        }
        previous = Some(transition);
    }
    let Some(last) = previous else { return false };
    optional_int(last, "new_maximum_amount") == int(authorization.get("maximum_amount"))
        && optional_string(last, "new_mode") == string(authorization.get("mode"))
        && optional_string(last, "new_status") == string(authorization.get("status"))
        && optional_string(last, "reason") == optional_string(authorization, "terminal_reason")
        && int(last.get("resulting_state_version")) == Some(state_version)
}

fn valid_transition(value: &Map<String, Value>, number: i64) -> bool {
    if !only(
        value,
        &[
            "transition_number",
            "kind",
            "old_maximum_amount",
            "new_maximum_amount",
            "old_mode",
            "new_mode",
            "old_status",
            "new_status",
            "reason",
            "resulting_state_version",
            "created_at",
        ],
        &[],
    ) || int(value.get("transition_number")) != Some(number)
        || int(value.get("resulting_state_version")) != Some(number)
        || !positive(value.get("new_maximum_amount"))
        || !proxy_mode(value.get("new_mode"))
        || !proxy_status(value.get("new_status"))
        || !time(value.get("created_at"))
    {
        return false;
    }
    let old_state = positive(value.get("old_maximum_amount"))
        && proxy_mode(value.get("old_mode"))
        && proxy_status(value.get("old_status"));
    match string(value.get("kind")) {
        Some("created") => {
            number == 1
                && value.get("old_maximum_amount") == Some(&Value::Null)
                && value.get("old_mode") == Some(&Value::Null)
                && value.get("old_status") == Some(&Value::Null)
                && string(value.get("new_status")) == Some("active")
                && value.get("reason") == Some(&Value::Null)
        }
        Some("lowered") => {
            old_state
                && int(value.get("new_maximum_amount")) < optional_int(value, "old_maximum_amount")
                && string(value.get("old_mode")) == string(value.get("new_mode"))
                && string(value.get("old_status")) == Some("active")
                && string(value.get("new_status")) == Some("active")
                && value.get("reason") == Some(&Value::Null)
        }
        Some("final_fast") => {
            old_state
                && int(value.get("new_maximum_amount")) == optional_int(value, "old_maximum_amount")
                && string(value.get("old_mode")) == Some("slow")
                && string(value.get("new_mode")) == Some("fast")
                && string(value.get("old_status")) == Some("active")
                && string(value.get("new_status")) == Some("active")
                && value.get("reason") == Some(&Value::Null)
        }
        Some("exhausted") => {
            unchanged_active(value)
                && string(value.get("new_status")) == Some("exhausted")
                && matches!(string(value.get("reason")), Some("strictly_defeated" | "ranked_below"))
        }
        Some("frozen") => {
            unchanged_active(value)
                && string(value.get("new_status")) == Some("frozen")
                && string(value.get("reason")) == Some("eligibility_lost")
        }
        Some("closed") => {
            unchanged_active(value)
                && string(value.get("new_status")) == Some("closed")
                && string(value.get("reason")) == Some("competition_closed")
        }
        _ => false,
    }
}

fn unchanged_active(value: &Map<String, Value>) -> bool {
    positive(value.get("old_maximum_amount"))
        && int(value.get("new_maximum_amount")) == optional_int(value, "old_maximum_amount")
        && proxy_mode(value.get("old_mode"))
        && string(value.get("old_mode")) == string(value.get("new_mode"))
        && string(value.get("old_status")) == Some("active")
}

fn valid_owned_lifecycle(value: &Map<String, Value>) -> bool {
    match string(value.get("status")) {
        Some("active") => {
            value.get("terminal_reason").is_none() && value.get("invalid_reason").is_none()
        }
        Some("exhausted") => {
            matches!(
                string(value.get("terminal_reason")),
                Some("strictly_defeated" | "ranked_below")
            ) && string(value.get("invalid_reason")) == Some("outbid")
        }
        Some("frozen") => {
            string(value.get("terminal_reason")) == Some("eligibility_lost")
                && string(value.get("invalid_reason")) == Some("frozen")
        }
        Some("closed") => {
            string(value.get("terminal_reason")) == Some("competition_closed")
                && string(value.get("invalid_reason")) == Some("closed")
        }
        _ => false,
    }
}

fn valid_lifecycle(value: &Map<String, Value>) -> bool {
    match string(value.get("status")) {
        Some("active") => value.get("terminal_reason").is_none(),
        Some("exhausted") => matches!(
            string(value.get("terminal_reason")),
            Some("strictly_defeated" | "ranked_below")
        ),
        Some("frozen") => string(value.get("terminal_reason")) == Some("eligibility_lost"),
        Some("closed") => string(value.get("terminal_reason")) == Some("competition_closed"),
        _ => false,
    }
}

fn valid_owned_equipment(value: Option<&Value>, trimmed: bool) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    only(value, &["name", "category"], &[])
        && nonempty_mode(value.get("name"), trimmed)
        && nonempty_mode(value.get("category"), trimmed)
}

fn valid_quota(value: Option<&Value>, usage_number: Option<i64>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    only(value, &["used", "limit"], &[])
        && positive(value.get("used"))
        && nonnegative(value.get("limit"))
        && (int(value.get("limit")) == Some(0) || int(value.get("used")) <= int(value.get("limit")))
        && usage_number.is_none_or(|usage| int(value.get("used")).is_some_and(|used| used >= usage))
}

fn valid_capabilities(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    only(value, &["can_lower", "waiting", "at_maximum"], &[])
        && bool_value(value.get("can_lower")).is_some()
        && bool_value(value.get("waiting")).is_some()
        && bool_value(value.get("at_maximum")).is_some()
        && !(bool_value(value.get("at_maximum")) == Some(true)
            && (bool_value(value.get("can_lower")) == Some(true)
                || bool_value(value.get("waiting")) == Some(true)))
}

fn valid_category_counts(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    let keys = ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];
    only(value, &keys, &[])
        && keys.iter().all(|key| nonnegative(value.get(*key)))
        && keys[1..].iter().filter_map(|key| int(value.get(*key))).sum::<i64>()
            <= int(value.get("all")).unwrap_or(-1)
}

fn optional_attributes(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || valid_attributes(value.get(key))
}

fn optional_stats(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || valid_stats(value.get(key))
}

fn optional_attribute_stats(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || valid_attribute_stats(value.get(key))
}

fn optional_nonempty(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || nonempty(value.get(key))
}

fn optional_nonnegative(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || nonnegative(value.get(key))
}

fn optional_positive(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || positive(value.get(key))
}

fn optional_uuid(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || uuid(string(value.get(key)).unwrap_or_default())
}

fn optional_account_nonempty(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || account_nonempty(value.get(key))
}

fn optional_time(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || time(value.get(key))
}

fn optional_finite(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || finite(value.get(key))
}

fn optional_bool(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || bool_value(value.get(key)).is_some()
}

fn optional_string<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|value| string(Some(value)))
}

fn optional_int(value: &Map<String, Value>, key: &str) -> Option<i64> {
    value.get(key).and_then(|value| int(Some(value)))
}

fn only(value: &Map<String, Value>, required: &[&str], optional: &[&str]) -> bool {
    required.iter().all(|key| value.contains_key(*key))
        && value
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn string(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn int(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(|value| {
            let number = value.as_number()?;
            if let Some(integer) = number.as_i64() {
                return Some(integer);
            }
            let float = number.as_f64()?;
            (float.is_finite()
                && float.fract() == 0.0
                && (-9_007_199_254_740_991.0..=9_007_199_254_740_991.0).contains(&float))
            .then_some(float as i64)
        })
        .filter(|value| (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(value))
}

fn positive(value: Option<&Value>) -> bool {
    int(value).is_some_and(|value| value > 0)
}

fn nonnegative(value: Option<&Value>) -> bool {
    int(value).is_some_and(|value| value >= 0)
}

fn revision(value: Option<&Value>) -> bool {
    nonnegative(value)
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

fn finite(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64).is_some_and(f64::is_finite)
}

fn nonempty(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty() && value == value.trim())
}

fn account_nonempty(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty())
}

fn nonempty_mode(value: Option<&Value>, trimmed: bool) -> bool {
    if trimmed { nonempty(value) } else { account_nonempty(value) }
}

fn trimmed_nonempty_str(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn raw_nonempty_str(value: &str) -> bool {
    !value.is_empty()
}

#[cfg(target_arch = "wasm32")]
fn time(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty() && !js_sys::Date::parse(value).is_nan())
}

#[cfg(not(target_arch = "wasm32"))]
fn time(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok())
}

#[cfg(target_arch = "wasm32")]
fn time_value(value: Option<&Value>) -> Option<f64> {
    string(value).and_then(|value| {
        let timestamp = js_sys::Date::parse(value);
        (!timestamp.is_nan()).then_some(timestamp)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn time_value(value: Option<&Value>) -> Option<f64> {
    string(value)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis() as f64)
}

fn uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.to_string() == value
            && !id.is_nil()
            && (1..=5).contains(&id.get_version_num())
            && id.get_variant() == Variant::RFC4122
    })
}

#[cfg(target_arch = "wasm32")]
fn world(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value == value.trim()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    })
}

fn proxy_mode(value: Option<&Value>) -> bool {
    matches!(string(value), Some("fast" | "slow"))
}

fn proxy_status(value: Option<&Value>) -> bool {
    matches!(string(value), Some("active" | "exhausted" | "frozen" | "closed"))
}

/// PostgreSQL 必须交付真实 ID/key URL；空字符串只表示 SQLite Lite 明确没有该能力。
fn game_url(value: Option<&Value>) -> bool {
    let Some(value) = string(value) else { return false };
    if value.is_empty() {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else { return false };
    let Some(path) = value
        .strip_prefix("https://hentaiverse.org/equip/")
        .or_else(|| value.strip_prefix("https://hentaiverse.org/isekai/equip/"))
    else {
        return false;
    };
    let mut parts = path.split('/');
    let equipment_id = parts.next().unwrap_or_default();
    let key = parts.next().unwrap_or_default();
    url.scheme() == "https"
        && url.host_str() == Some("hentaiverse.org")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !equipment_id.is_empty()
        && equipment_id.len() <= 32
        && equipment_id.bytes().all(|byte| byte.is_ascii_digit())
        && key.len() == 10
        && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        && parts.next().is_none()
}

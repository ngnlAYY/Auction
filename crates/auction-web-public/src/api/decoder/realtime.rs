//! 实时路由（`/api/v1/events` catch-up 页与 WebSocket 单事件帧）的 data 校验；整个模块只服务
//! WASM 浏览器构建。

use super::*;
use auction_contracts::{
    PublicRealtimeDiscoveryResponse, PublicRealtimeEventResponse, PublicRealtimeResponse,
};
use gloo_net::http::Response;
use serde_json::Value;
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

#[cfg(target_arch = "wasm32")]
fn valid_auction_realtime_page(value: &Value, auction_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(
        value,
        &["auction_id", "events", "next_after", "has_more", "refetch_required"],
        &[],
    ) && string(value.get("auction_id")) == Some(auction_id)
        && valid_uuid(auction_id)
        && valid_realtime_events(value.get("events"), Some(auction_id), false)
        && valid_realtime_page_metadata(value)
}

#[cfg(target_arch = "wasm32")]
fn valid_discovery_realtime_page(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["scope", "events", "next_after", "has_more", "refetch_required"], &[])
        && string(value.get("scope")) == Some("auctions")
        && valid_realtime_events(value.get("events"), None, true)
        && valid_realtime_page_metadata(value)
}

#[cfg(target_arch = "wasm32")]
fn valid_realtime_page_metadata(value: &serde_json::Map<String, Value>) -> bool {
    nonnegative(value.get("next_after"))
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
    if !json_keys_only(
        value,
        &["event_id", "stream_sequence", "kind", "schema_version", "payload", "created_at"],
        &[],
    ) || !valid_uuid(string(value.get("event_id")).unwrap_or_default())
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
            json_keys_only(payload, &["auction_id", "world_id", "sequence", "state_version"], &[])
                && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && world(payload.get("world_id"))
                && positive(payload.get("sequence"))
                && positive(payload.get("state_version"))
        }
        (Some("auction.preview_updated"), Some(1)) => {
            json_keys_only(
                payload,
                &["auction_id", "preview_revision", "item_count", "manual_open", "state_version"],
                &[],
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && revision(payload.get("preview_revision"))
                && nonnegative(payload.get("item_count"))
                && bool_value(payload.get("manual_open")).is_some()
                && positive(payload.get("state_version"))
        }
        (Some("preview_catalog_changed"), Some(1)) => {
            json_keys_only(payload, &["world", "auction_id", "preview_revision"], &[])
                && world(payload.get("world"))
                && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && positive(payload.get("preview_revision"))
        }
        (Some("auction.opened"), Some(1)) => {
            json_keys_only(
                payload,
                &["auction_id", "world_id", "state_version", "preview_revision"],
                &[],
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && world(payload.get("world_id"))
                && positive(payload.get("state_version"))
                && positive(payload.get("preview_revision"))
        }
        (Some("auction.schedule_updated"), Some(1)) => {
            json_keys_only(
                payload,
                &["auction_id", "world_id", "scheduled_end_at", "state_version"],
                &[],
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && world(payload.get("world_id"))
                && time(payload.get("scheduled_end_at"))
                && positive(payload.get("state_version"))
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
            json_keys_only(payload, &["auction_id", "state_version"], &[])
                && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && positive(payload.get("state_version"))
        }
        (Some("auction.settled"), Some(1)) => {
            json_keys_only(
                payload,
                &["auction_id", "sold_count", "unsold_count", "state_version"],
                &[],
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && nonnegative(payload.get("sold_count"))
                && nonnegative(payload.get("unsold_count"))
                && positive(payload.get("state_version"))
        }
        (Some("auction.settled"), Some(2)) => {
            json_keys_only(
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
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
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
            json_keys_only(
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
            ) && valid_uuid(string(payload.get("auction_id")).unwrap_or_default())
                && valid_uuid(string(payload.get("lot_id")).unwrap_or_default())
                && valid_uuid(string(payload.get("bid_id")).unwrap_or_default())
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
        Some("auction.created" | "auction.opened" | "auction.schedule_updated") => {
            string(payload.get("world_id")) == Some(expected_world)
        }
        _ => true,
    }
}

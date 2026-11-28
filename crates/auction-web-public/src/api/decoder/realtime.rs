//! 实时路由（`/api/v1/events` catch-up 页与 WebSocket 单事件帧）的 data 校验；整个模块只服务
//! WASM 浏览器构建。

use auction_contracts::PublicRealtimeEventResponse;
#[cfg(target_arch = "wasm32")]
use auction_contracts::PublicRealtimeResponse;
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_keys_only;
#[cfg(target_arch = "wasm32")]
use gloo_net::http::Response;
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
use super::decode_success;
use super::{
    bool_value, int, nonnegative, normalize_legacy_integer_numbers, object, positive, revision,
    string, time, world,
};
/// 解码旧版单拍卖 catch-up 页；共享 envelope 允许扩展，但 data/event/payload 必须精确同形。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn decode_auction_realtime_page(
    response: Response,
    auction_id: &str,
) -> Result<PublicRealtimeResponse, String> {
    decode_success(response, |value| valid_auction_realtime_page(value, auction_id)).await
}

/// 解码 WebSocket 的单事件文本帧，并复用 catch-up 页的严格事件防腐层。
pub(crate) fn decode_realtime_event(
    message: &str,
    auction_id: &str,
    world: &str,
) -> Result<PublicRealtimeEventResponse, String> {
    let mut value =
        serde_json::from_str::<Value>(message).map_err(|_| "invalid_response".to_owned())?;
    if !valid_realtime_event(&value, auction_id) || !valid_realtime_event_world(&value, world) {
        return Err("invalid_response".to_owned());
    }
    normalize_legacy_integer_numbers(&mut value);
    serde_json::from_value(value).map_err(|_| "invalid_response".to_owned())
}

fn valid_auction_realtime_page(value: &Value, auction_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(
        value,
        &["auction_id", "events", "next_after", "has_more", "refetch_required"],
        &[],
    ) && string(value.get("auction_id")) == Some(auction_id)
        && valid_uuid(auction_id)
        && valid_realtime_events(value.get("events"), auction_id)
        && valid_realtime_page_metadata(value)
}

fn valid_realtime_page_metadata(value: &serde_json::Map<String, Value>) -> bool {
    nonnegative(value.get("next_after"))
        && bool_value(value.get("has_more")).is_some()
        && bool_value(value.get("refetch_required")).is_some()
}

fn valid_realtime_events(value: Option<&Value>, auction_id: &str) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|events| events.iter().all(|event| valid_realtime_event(event, auction_id)))
}

fn valid_realtime_event(value: &Value, auction_id: &str) -> bool {
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
    if string(payload.get("auction_id")) != Some(auction_id) {
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
                && positive_count_sum(payload.get("sold_count"), payload.get("unsold_count"))
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

fn positive_count_sum(sold_count: Option<&Value>, unsold_count: Option<&Value>) -> bool {
    int(sold_count)
        .zip(int(unsold_count))
        .and_then(|(sold, unsold)| sold.checked_add(unsold))
        .is_some_and(|total| total > 0)
}

/// 旧版 auction-specific WebSocket 还会把带 world 的事件与当前页面目标核对；HTTP catch-up
/// 只冻结 auction ID，因此该校验只由单事件帧入口调用。
fn valid_realtime_event_world(value: &Value, expected_world: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use auction_contracts::PublicRealtimeResponse;

    use super::super::decode_success_text;
    use super::valid_auction_realtime_page;

    #[test]
    fn settled_v2_requires_nonzero_counts_and_v1_keeps_zero_count_compatibility() {
        let auction_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
        let event = |schema_version, sold_count, unsold_count, obligation_count| {
            serde_json::json!({
                "event_id": "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c",
                "stream_sequence": 1,
                "kind": "auction.settled",
                "schema_version": schema_version,
                "payload": {
                    "auction_id": auction_id,
                    "state_version": 1,
                    "policy_version": 1,
                    "sold_count": sold_count,
                    "unsold_count": unsold_count,
                    "obligation_count": obligation_count
                },
                "created_at": "2026-11-20T12:00:00Z"
            })
        };
        let v1 = serde_json::json!({
            "event_id": "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c",
            "stream_sequence": 1,
            "kind": "auction.settled",
            "schema_version": 1,
            "payload": {
                "auction_id": auction_id,
                "sold_count": 0,
                "unsold_count": 0,
                "state_version": 1
            },
            "created_at": "2026-11-20T12:00:00Z"
        });
        assert!(super::valid_realtime_event(&v1, auction_id));
        assert!(!super::valid_realtime_event(&event(2, 0, 0, 1), auction_id));
        assert!(!super::valid_realtime_event(&event(2, 1, 0, 0), auction_id));
        assert!(super::valid_realtime_event(&event(2, 1, 0, 1), auction_id));
    }

    #[test]
    fn realtime_pages_decode_backend_contract_fixtures() {
        let auction_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
        for (body, refetch_required) in [
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/realtime-events.json"
                ),
                false,
            ),
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/realtime-events-refetch.json"
                ),
                true,
            ),
        ] {
            let page: PublicRealtimeResponse = decode_success_text(Some(body), |data| {
                valid_auction_realtime_page(data, auction_id)
            })
            .expect("实时回放");
            assert_eq!(page.next_after, 2);
            assert_eq!(page.refetch_required, refetch_required);
            assert_eq!(page.events.is_empty(), refetch_required);
        }
    }

    #[test]
    fn websocket_decodes_backend_event_contract_fixture() {
        let body = include_str!(
            "../../../../auction-contracts/tests/fixtures/public/websocket-event.json"
        );
        let auction_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
        let event = super::decode_realtime_event(body, auction_id, "main").expect("Final 事件");
        assert_eq!(event.kind, "auction.final_phase_entered");
        assert_eq!(event.stream_sequence, 2);
        assert!(
            super::decode_realtime_event(body, "00000000-0000-4000-8000-000000000001", "main")
                .is_err()
        );
    }
}

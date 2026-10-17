//! 账户路由（`/api/v1/me/*` 与代理授权投影）的 data 校验。
//! `valid_quota` / `valid_transitions` 同时服务 lot 路由的命令授权投影，以 `pub(super)`
//! 共享，避免同一代理生命周期谓词出现两份实现。

use super::*;
use auction_contracts::{PublicMyBidsResponse, PublicMyProxyResponse};
use gloo_net::http::Response;
use serde_json::{Map, Value};
pub(crate) async fn decode_my_bids(response: Response) -> Result<PublicMyBidsResponse, String> {
    decode_success(response, valid_my_bids).await
}

pub(crate) async fn decode_my_proxy(
    response: Response,
    lot_id: &str,
) -> Result<PublicMyProxyResponse, String> {
    decode_success(response, |value| valid_my_proxy(value, lot_id)).await
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

pub(super) fn valid_transitions(value: Option<&Value>, authorization: &Map<String, Value>) -> bool {
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

fn valid_owned_equipment(value: Option<&Value>, trimmed: bool) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    only(value, &["name", "category"], &[])
        && nonempty_mode(value.get("name"), trimmed)
        && nonempty_mode(value.get("category"), trimmed)
}

pub(super) fn valid_quota(value: Option<&Value>, usage_number: Option<i64>) -> bool {
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

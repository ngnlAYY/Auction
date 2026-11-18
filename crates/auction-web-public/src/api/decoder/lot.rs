//! lot 路由（目录、详情、竞价与代理授权命令）的 data 校验。
//! 代理授权的配额与转换历史谓词由 `account` 模块共享（`pub(super)`），两处投影必须保持
//! 同一条严格边界。

use auction_contracts::{
    EQUIPMENT_CATEGORY_CODES, PlaceBidResponse, PlaceBidResultResponse as BidResult,
    PublicProxyCommandResponse,
};
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_keys_only;
use gloo_net::http::Response;
use serde_json::{Map, Value};

use super::super::{CatalogLevelFilter, LotEnvelope, ProxyCommandAuthorization};
use super::account::{valid_quota, valid_transitions};
use super::{
    bool_value, decode_success, decode_success_text, game_url, int, nonempty, nonnegative, object,
    optional_bool, optional_finite, optional_int, optional_nonempty, optional_nonnegative,
    optional_primary_attribute_initials, optional_string, positive, positive_page_size, proxy_mode,
    raw_nonempty_str, revision, string, time, trimmed_nonempty_str,
};
pub(crate) async fn decode_lot_list(
    response: Response,
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
) -> Result<LotEnvelope, String> {
    decode_success(response, |value| {
        valid_lot_list(value, auction_id, category, equipment_group, levels)
    })
    .await
}

pub(crate) fn decode_lot_detail(
    body: Option<&str>,
    auction_id: Option<&str>,
    lot_id: &str,
) -> Result<LotEnvelope, String> {
    decode_success_text(body, |value| valid_lot_detail(value, auction_id, lot_id))
}

/// 先完成严格解码，再核对命令目标；错 lot 的成功响应仍属于未知结果。
pub(in crate::api) fn decode_bid_command(
    body: Option<&str>,
    lot_id: &str,
) -> Result<BidResult, String> {
    let result = decode_success_text::<PlaceBidResponse, _>(body, valid_bid_command)?.bid;
    if result.lot_id != lot_id {
        return Err("invalid_response".to_owned());
    }
    Ok(result)
}

pub(in crate::api) fn decode_create_proxy_command(
    body: Option<&str>,
    lot_id: &str,
    maximum_amount: i64,
    mode: &str,
) -> Result<ProxyCommandAuthorization, String> {
    decode_success_text::<PublicProxyCommandResponse, _>(body, |value| {
        valid_proxy_command(value, lot_id, Some(maximum_amount), Some(mode), false)
    })
    .map(|response| response.authorization)
}

pub(in crate::api) fn decode_lower_proxy_command(
    body: Option<&str>,
    lot_id: &str,
    authorization_id: &str,
    maximum_amount: i64,
) -> Result<ProxyCommandAuthorization, String> {
    decode_success_text::<PublicProxyCommandResponse, _>(body, |value| {
        valid_proxy_command(value, lot_id, Some(maximum_amount), None, true)
            && object(value)
                .and_then(|value| value.get("authorization"))
                .and_then(object)
                .and_then(|value| string(value.get("authorization_id")))
                == Some(authorization_id)
    })
    .map(|response| response.authorization)
}

fn valid_lot_list(
    value: &Value,
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
) -> bool {
    let Some(value) = object(value) else { return false };
    if !json_keys_only(
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
        || !positive_page_size(value.get("page_size"))
        || !nonnegative(value.get("total"))
        || !valid_category_counts(value.get("category_counts"))
    {
        return false;
    }
    let category_key = category.unwrap_or("all");
    let Some(counts) = object(value.get("category_counts").unwrap_or(&Value::Null)) else {
        return false;
    };
    if !category_count_keys().contains(&category_key)
        || int(counts.get(category_key)) != int(value.get("total"))
    {
        return false;
    }
    let Some(lots) = value.get("lots").and_then(Value::as_array) else { return false };
    lots.iter().all(|lot| {
        let preview = string(value.get("projection_kind")) == Some("preview");
        let valid = if preview {
            valid_preview_lot(lot, auction_id, None, false)
        } else {
            valid_live_lot(lot, auction_id, None)
                && (string(value.get("auction_status")) != Some("settled")
                    || object(lot).and_then(|lot| string(lot.get("status"))) != Some("live"))
        };
        // 请求分组、等级和每行服务端判定必须一致；只校验，不把漏筛响应变成残缺页。
        let equipment = if preview {
            object(lot)
        } else {
            object(lot).and_then(|lot| lot.get("equipment")).and_then(object)
        };
        let level = equipment.and_then(|equipment| {
            int(equipment.get(if preview { "equipment_level" } else { "level" }))
        });
        valid
            && levels.matches_level(level)
            && equipment_group.is_none_or(|expected| {
                equipment.and_then(|equipment| string(equipment.get("equipment_group")))
                    == Some(expected)
            })
    })
}

fn valid_lot_detail(value: &Value, auction_id: Option<&str>, lot_id: &str) -> bool {
    let Some(value) = object(value) else { return false };
    let Some(response_auction_id) = string(value.get("auction_id")) else { return false };
    if auction_id.is_some_and(|expected| expected != response_auction_id)
        || !json_keys_only(
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
    if string(value.get("projection_kind")) == Some("preview") {
        valid_preview_lot(
            value.get("lot").unwrap_or(&Value::Null),
            response_auction_id,
            Some(lot_id),
            true,
        )
    } else {
        valid_live_lot(value.get("lot").unwrap_or(&Value::Null), response_auction_id, Some(lot_id))
            && (string(value.get("auction_status")) != Some("settled")
                || object(value.get("lot").unwrap_or(&Value::Null))
                    .and_then(|lot| string(lot.get("status")))
                    != Some("live"))
    }
}

fn valid_projection(value: &Map<String, Value>, auction_id: &str) -> bool {
    string(value.get("auction_id")) == Some(auction_id)
        && valid_uuid(auction_id)
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
                json_keys_only(
                    restriction,
                    &["new_slow_proxy_allowed", "existing_slow_proxy_converted"],
                    &[],
                ) && bool_value(restriction.get("new_slow_proxy_allowed")) == Some(false)
                    && bool_value(restriction.get("existing_slow_proxy_converted")) == Some(true)
            }),
        _ => value.get("final_restriction").is_none(),
    }
}

fn valid_live_lot(value: &Value, auction_id: &str, lot_id: Option<&str>) -> bool {
    let Some(value) = object(value) else { return false };
    let has_leader = bool_value(value.get("has_leader"));
    let leader_present = value.contains_key("leader_name");
    json_keys_only(
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
    ) && valid_uuid(string(value.get("id")).unwrap_or_default())
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
        &[
            "equipment_category",
            "equipment_level",
            "primary_attribute_initials",
            "avg_roll",
            "equipment",
        ][..]
    } else {
        &["equipment_category", "equipment_level", "primary_attribute_initials", "avg_roll"][..]
    };
    json_keys_only(
        value,
        &[
            "id",
            "auction_id",
            "code",
            "status",
            "seller_name",
            "start_price",
            "equipment_name",
            "equipment_group",
            "game_equipment_url",
        ],
        optional,
    ) && valid_uuid(string(value.get("id")).unwrap_or_default())
        && lot_id.is_none_or(|expected| string(value.get("id")) == Some(expected))
        && string(value.get("auction_id")) == Some(auction_id)
        && nonempty(value.get("code"))
        && string(value.get("status")) == Some("pending")
        && nonempty(value.get("seller_name"))
        && positive(value.get("start_price"))
        && nonempty(value.get("equipment_name"))
        && valid_equipment_group(value.get("equipment_group"))
        && optional_nonempty(value, "equipment_category")
        && optional_nonnegative(value, "equipment_level")
        && optional_primary_attribute_initials(value, "primary_attribute_initials")
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
        && string(equipment.get("equipment_group")) == string(summary.get("equipment_group"))
        && optional_string(equipment, "category") == optional_string(summary, "equipment_category")
        && optional_int(equipment, "level") == optional_int(summary, "equipment_level")
        && optional_string(equipment, "primary_attribute_initials")
            == optional_string(summary, "primary_attribute_initials")
        && string(equipment.get("game_equipment_url")) == string(summary.get("game_equipment_url"))
}

fn valid_preview_equipment(value: &Map<String, Value>) -> bool {
    json_keys_only(
        value,
        &["name", "equipment_group", "game_equipment_url"],
        &["category", "level", "primary_attribute_initials", "info_summary", "stats", "avg_roll"],
    ) && nonempty(value.get("name"))
        && valid_equipment_group(value.get("equipment_group"))
        && optional_nonempty(value, "category")
        && optional_nonnegative(value, "level")
        && optional_primary_attribute_initials(value, "primary_attribute_initials")
        && optional_nonempty(value, "info_summary")
        && optional_stats(value, "stats")
        && optional_finite(value, "avg_roll")
        && game_url(value.get("game_equipment_url"))
}

fn valid_equipment(value: Option<&Value>, full: bool) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    let required = ["name", "category", "equipment_group", "game_equipment_url"];
    let optional =
        ["attributes", "level", "primary_attribute_initials", "info_summary", "stats", "avg_roll"];
    json_keys_only(value, &required, &optional)
        && nonempty(value.get("name"))
        && valid_equipment_group(value.get("equipment_group"))
        && nonempty(value.get("category"))
        && optional_nonnegative(value, "level")
        && optional_primary_attribute_initials(value, "primary_attribute_initials")
        && optional_nonempty(value, "info_summary")
        && optional_stats(value, "stats")
        && optional_finite(value, "avg_roll")
        && game_url(value.get("game_equipment_url"))
        && (!full || optional_attributes(value, "attributes"))
}

fn valid_equipment_group(value: Option<&Value>) -> bool {
    matches!(string(value), Some("rare" | "fusion"))
}

fn valid_attributes(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    json_keys_only(
        value,
        &["quality"],
        &["type", "prefix", "suffix", "level", "tradeable", "stats"],
    ) && nonempty(value.get("quality"))
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
            json_keys_only(stat, &["value"], &["base"])
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
            json_keys_only(stat, &["value"], &["base", "roll"])
                && nonempty(stat.get("value"))
                && optional_finite(stat, "base")
                && optional_finite(stat, "roll")
        }
    })
}

fn valid_bid_command(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(value, &["bid"], &[])
        && valid_bid_result(value.get("bid").unwrap_or(&Value::Null))
}

fn valid_bid_result(value: &Value) -> bool {
    let Some(value) = object(value) else { return false };
    json_keys_only(
        value,
        &["lot_id", "current_price", "effective_end_at", "state_version", "leading"],
        &[],
    ) && valid_uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("current_price"))
        && time(value.get("effective_end_at"))
        && positive(value.get("state_version"))
        && bool_value(value.get("leading")).is_some()
}

fn valid_proxy_command(
    value: &Value,
    lot_id: &str,
    expected_maximum: Option<i64>,
    expected_mode: Option<&str>,
    lowered: bool,
) -> bool {
    let Some(value) = object(value) else { return false };
    if !json_keys_only(value, &["authorization"], &[])
        || !valid_command_authorization(value.get("authorization").unwrap_or(&Value::Null))
    {
        return false;
    }
    let Some(authorization) = object(value.get("authorization").unwrap_or(&Value::Null)) else {
        return false;
    };
    string(authorization.get("lot_id")) == Some(lot_id)
        && valid_uuid(lot_id)
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
    json_keys_only(
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
    ) && valid_uuid(string(value.get("authorization_id")).unwrap_or_default())
        && valid_uuid(string(value.get("lot_id")).unwrap_or_default())
        && positive(value.get("authorization_sequence"))
        && positive(value.get("usage_number"))
        && proxy_mode(value.get("mode"))
        && positive(value.get("maximum_amount"))
        && valid_quota(value.get("quota"), int(value.get("usage_number")))
        && positive(value.get("state_version"))
        && valid_lifecycle(value)
        && valid_transitions(value.get("transitions"), value)
        && value.get("bid").is_none_or(valid_bid_result)
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

/// 分类统计的完整键集合：`all` 后按契约表顺序排列各大类的公开键。
fn category_count_keys() -> [&'static str; 8] {
    let mut keys = ["all"; 8];
    for (slot, (key, _, _)) in keys[1..].iter_mut().zip(EQUIPMENT_CATEGORY_CODES) {
        *slot = key;
    }
    keys
}

fn valid_category_counts(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(object) else { return false };
    let keys = category_count_keys();
    json_keys_only(value, &keys, &[])
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::decode_success_value;
    use super::valid_lot_list;
    use crate::api::{CatalogLevelFilter, CatalogRow, LotEnvelope, LotEnvelopeRows};

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const LOT_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    fn list_envelope(equipment: Value) -> Value {
        json!({
            "code": "ok",
            "message": "拍卖批次已加载",
            "data": {
                "auction_id": AUCTION_ID,
                "auction_status": "live",
                "projection_kind": "live",
                "presentation_mode": "live",
                "bidding_open": true,
                "opens_at": "2025-11-15T08:00:00Z",
                "preview_revision": 0,
                "lots": [{
                    "id": LOT_ID,
                    "auction_id": AUCTION_ID,
                    "lot_number": 1,
                    "status": "live",
                    "start_price": 1000,
                    "current_price": 1500,
                    "next_bid": 1600,
                    "has_leader": true,
                    "leader_name": "leader",
                    "seller_name": "seller",
                    "effective_end_at": "2025-11-16T08:00:00Z",
                    "state_version": 3,
                    "equipment": equipment,
                }],
                "total": 1,
                "page": 1,
                "page_size": 20,
                "category_counts": {
                    "all": 1, "one": 0, "two": 0, "sta": 0, "shd": 0, "clo": 1, "lig": 0, "hea": 0,
                },
            },
        })
    }

    /// `fields=summary` 的 live 行：只缺属性明细，首字母与 avg_roll 仍由服务端给出。
    fn summary_equipment() -> Value {
        json!({
            "name": "Legendary Cotton Robe of Heimdall",
            "category": "Cloth",
            "equipment_group": "rare",
            "level": 450,
            "primary_attribute_initials": "IW",
            "avg_roll": 70.0,
            "game_equipment_url": "https://hentaiverse.org/equip/123456/0123456789",
        })
    }

    fn catalog_rows(payload: Value) -> Vec<CatalogRow> {
        let mut envelope: LotEnvelope = decode_success_value(payload, |value| {
            valid_lot_list(value, AUCTION_ID, None, None, CatalogLevelFilter::default())
        })
        .expect("valid lot list");
        envelope.take_rows()
    }

    #[test]
    fn summary_projection_decodes_to_the_same_catalog_rows() {
        let mut full = summary_equipment();
        let equipment = full.as_object_mut().expect("equipment object");
        equipment.insert("info_summary".to_owned(), json!("Robe Lv450"));
        equipment.insert(
            "stats".to_owned(),
            json!({
                "Primary Attributes / Intelligence": {"value": "+12.5", "base": 10.0, "roll": 80.0},
                "Primary Attributes / Wisdom": {"value": "+9", "base": 7.0, "roll": 60.0},
                "Holy EDB": {"value": "+20%"},
            }),
        );
        let full_rows = catalog_rows(list_envelope(full));
        let summary_rows = catalog_rows(list_envelope(summary_equipment()));
        assert_eq!(full_rows.len(), 1);
        // 目录行本来就不取属性明细：两种投影得到完全相同的展示行。
        assert_eq!(summary_rows, full_rows);
        assert_eq!(summary_rows[0].primary_attribute_initials.as_deref(), Some("IW"));
        assert_eq!(summary_rows[0].avg_roll, Some(70.0));
    }
}

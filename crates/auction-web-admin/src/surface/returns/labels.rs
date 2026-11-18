//! 结算退件页的闭集取值到译文键的映射与安全细节文本。
//!
//! 每个闭集取值都有固定的 Fluent 键；解码已拒绝闭集外的值，这里只兜底原样显示。键映射与
//! 取译文分开，单元测试据此核对契约闭集逐项有译文、五种语言键集合一致。

use auction_web_shared::t;
use serde_json::Value;

/// 案件状态的译文键。
pub(super) fn status_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "quarantined" => "settlement-return-status-quarantined",
        "recheck_pending" => "settlement-return-status-recheck-pending",
        "awaiting_decision" => "settlement-return-status-awaiting-decision",
        "take_pending" => "settlement-return-status-take-pending",
        "take_unknown" => "settlement-return-status-take-unknown",
        "resend_pending" => "settlement-return-status-resend-pending",
        "held" => "settlement-return-status-held",
        "resent" => "settlement-return-status-resent",
        "inbox_closed" => "settlement-return-status-inbox-closed",
        "reviewed" => "settlement-return-status-reviewed",
        _ => return None,
    })
}

/// 证据分类的译文键。
pub(super) fn match_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "untrusted" => "settlement-return-match-untrusted",
        "unbound" => "settlement-return-match-unbound",
        "fulfillment_terminal" => "settlement-return-match-fulfillment-terminal",
        "manual_reconcile" => "settlement-return-match-manual-reconcile",
        _ => return None,
    })
}

/// 原因码的译文键。
pub(super) fn reason_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "source_unproven" => "settlement-return-reason-source-unproven",
        "evidence_invalid" => "settlement-return-reason-evidence-invalid",
        "obligation_unknown" => "settlement-return-reason-obligation-unknown",
        "subject_kind_mismatch" => "settlement-return-reason-subject-kind-mismatch",
        "delivery_unsent" => "settlement-return-reason-delivery-unsent",
        "world_mismatch" => "settlement-return-reason-world-mismatch",
        "dispatch_unknown" => "settlement-return-reason-dispatch-unknown",
        "dispatch_ambiguous" => "settlement-return-reason-dispatch-ambiguous",
        "dispatch_already_returned" => "settlement-return-reason-dispatch-already-returned",
        "dispatch_superseded" => "settlement-return-reason-dispatch-superseded",
        "body_unproven" => "settlement-return-reason-body-unproven",
        "returned_assets_match" => "settlement-return-reason-returned-assets-match",
        "returned_assets_mismatch" => "settlement-return-reason-returned-assets-mismatch",
        "received_before_send" => "settlement-return-reason-received-before-send",
        _ => return None,
    })
}

/// 处理动作的按钮文案键。
pub(super) fn action_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "reclaim_and_resend" => "settlement-return-action-reclaim-and-resend",
        "reclaim_and_hold" => "settlement-return-action-reclaim-and-hold",
        "resend_from_custody" => "settlement-return-action-resend-from-custody",
        "close_inbox" => "settlement-return-action-close-inbox",
        "acknowledge" => "settlement-return-action-acknowledge",
        "recheck" => "settlement-return-action-recheck",
        "retry_take" => "settlement-return-action-retry-take",
        _ => return None,
    })
}

/// 处理动作在确认框中的后果说明键；说明只描述服务端冻结的后果，不提供可编辑的资产或收件人。
pub(super) fn action_effect_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "reclaim_and_resend" => "settlement-return-effect-reclaim-and-resend",
        "reclaim_and_hold" => "settlement-return-effect-reclaim-and-hold",
        "resend_from_custody" => "settlement-return-effect-resend-from-custody",
        "close_inbox" => "settlement-return-effect-close-inbox",
        "acknowledge" => "settlement-return-effect-acknowledge",
        "recheck" => "settlement-return-effect-recheck",
        "retry_take" => "settlement-return-effect-retry-take",
        _ => return None,
    })
}

/// 阻断原因的译文键。
pub(super) fn blocker_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "untrusted" => "settlement-return-blocker-untrusted",
        "unbound" => "settlement-return-blocker-unbound",
        "assets_mismatch" => "settlement-return-blocker-assets-mismatch",
        "state_conflict" => "settlement-return-blocker-state-conflict",
        "resend_limit" => "settlement-return-blocker-resend-limit",
        "obligation_ineligible" => "settlement-return-blocker-obligation-ineligible",
        "proof_required" => "settlement-return-blocker-proof-required",
        "send_unresolved" => "settlement-return-blocker-send-unresolved",
        "observation_required" => "settlement-return-blocker-observation-required",
        _ => return None,
    })
}

/// 资产位置的译文键；文案明确区分“已寄出”“仍在 Bot 收件箱”“已领回”与“代管”。
pub(super) fn location_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "unverified" => "settlement-return-location-unverified",
        "returned_in_inbox" => "settlement-return-location-returned-in-inbox",
        "take_in_progress" => "settlement-return-location-take-in-progress",
        "location_unknown" => "settlement-return-location-location-unknown",
        "taken_awaiting_resend" => "settlement-return-location-taken-awaiting-resend",
        "taken_in_custody" => "settlement-return-location-taken-in-custody",
        "resent_in_transit" => "settlement-return-location-resent-in-transit",
        _ => return None,
    })
}

/// 补寄案的结案证据只证明寄出；案件不再跟踪后续持有人或领取状态。
pub(super) fn shows_asset_location(status: &str) -> bool {
    status != "resent"
}

/// 处理计划的译文键。
pub(super) fn plan_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "resend" => "settlement-return-plan-resend",
        "hold" => "settlement-return-plan-hold",
        _ => return None,
    })
}

/// 处理事件的译文键。
pub(super) fn event_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "opened" => "settlement-return-event-opened",
        "acknowledged" => "settlement-return-event-acknowledged",
        "recheck_requested" => "settlement-return-event-recheck-requested",
        "recheck_rebound" => "settlement-return-event-recheck-rebound",
        "recheck_unchanged" => "settlement-return-event-recheck-unchanged",
        "take_requested" => "settlement-return-event-take-requested",
        "take_activated" => "settlement-return-event-take-activated",
        "take_proven" => "settlement-return-event-take-proven",
        "take_unknown" => "settlement-return-event-take-unknown",
        "take_observed" => "settlement-return-event-take-observed",
        "take_retry_requested" => "settlement-return-event-take-retry-requested",
        "held" => "settlement-return-event-held",
        "resend_authorized" => "settlement-return-event-resend-authorized",
        "resent" => "settlement-return-event-resent",
        "inbox_closed" => "settlement-return-event-inbox-closed",
        _ => return None,
    })
}

/// 义务种类的译文键。
pub(super) fn kind_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "winner_equipment_cod" => "settlement-return-kind-winner-equipment-cod",
        "winner_payment_carrier_cod" => "settlement-return-kind-winner-payment-carrier-cod",
        "winner_equipment_handoff" => "settlement-return-kind-winner-equipment-handoff",
        "seller_payout" => "settlement-return-kind-seller-payout",
        "buyer_refund" => "settlement-return-kind-buyer-refund",
        "unsold_return" => "settlement-return-kind-unsold-return",
        "withdrawn_return" => "settlement-return-kind-withdrawn-return",
        _ => return None,
    })
}

/// Take 结果未知原因的译文键（原因名称）。
pub(super) fn take_unknown_reason_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "assets_claimable" => "settlement-return-take-unknown-reason-assets-claimable",
        "assets_unclaimable" => "settlement-return-take-unknown-reason-assets-unclaimable",
        "assets_changed" => "settlement-return-take-unknown-reason-assets-changed",
        "mail_missing" => "settlement-return-take-unknown-reason-mail-missing",
        "receipt_unproven" => "settlement-return-take-unknown-reason-receipt-unproven",
        "session_changed" => "settlement-return-take-unknown-reason-session-changed",
        "take_unverifiable" => "settlement-return-take-unknown-reason-take-unverifiable",
        "disposition_conflict" => "settlement-return-take-unknown-reason-disposition-conflict",
        "awaiting_observation" => "settlement-return-take-unknown-reason-awaiting-observation",
        _ => return None,
    })
}

/// Take 结果未知原因的运维指引译文键；与告警通知的指引同一口径。
pub(super) fn take_unknown_guidance_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "assets_claimable" => "settlement-return-take-unknown-guidance-assets-claimable",
        "assets_unclaimable" => "settlement-return-take-unknown-guidance-assets-unclaimable",
        "assets_changed" => "settlement-return-take-unknown-guidance-assets-changed",
        "mail_missing" => "settlement-return-take-unknown-guidance-mail-missing",
        "receipt_unproven" => "settlement-return-take-unknown-guidance-receipt-unproven",
        "session_changed" => "settlement-return-take-unknown-guidance-session-changed",
        "take_unverifiable" => "settlement-return-take-unknown-guidance-take-unverifiable",
        "disposition_conflict" => "settlement-return-take-unknown-guidance-disposition-conflict",
        "awaiting_observation" => "settlement-return-take-unknown-guidance-awaiting-observation",
        _ => return None,
    })
}

/// 持久 Take 执行状态的译文键。
pub(super) fn take_state_key(value: &str) -> Option<&'static str> {
    Some(match value {
        "pending" => "settlement-return-take-state-pending",
        "enqueued" => "settlement-return-take-state-enqueued",
        "satisfied" => "settlement-return-take-state-satisfied",
        "manual_reconcile" => "settlement-return-take-state-manual-reconcile",
        _ => return None,
    })
}

/// 取闭集译文；键缺失（解码已拒绝，理论上不会发生）时原样显示。
pub(super) fn label(key: fn(&str) -> Option<&'static str>, value: &str) -> String {
    key(value).map_or_else(|| value.to_owned(), |key| t!(key))
}

/// 事件安全细节的单行文本：按键名排序，只展示标量；嵌套值以省略号代替，不展开任意结构。
pub(super) fn event_details_text(details: &Value) -> String {
    let Some(object) = details.as_object() else {
        return String::new();
    };
    let mut entries: Vec<(&String, &Value)> = object.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
        .into_iter()
        .map(|(key, value)| {
            let text = match value {
                Value::Null => "-".to_owned(),
                Value::Bool(flag) => flag.to_string(),
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.clone(),
                Value::Array(_) | Value::Object(_) => "…".to_owned(),
            };
            format!("{key}={text}")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 投递来源为结算退件 Take 时，取出其案件 ID（`settlement_return:<案件 ID>`）；只接受规范 UUID。
pub(crate) fn settlement_return_case(origin_kind: &str, origin_id: &str) -> Option<String> {
    if origin_kind != "settlement_return_action" {
        return None;
    }
    origin_id
        .strip_prefix("settlement_return:")
        .filter(|case| auction_web_shared::valid_uuid(case))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use auction_contracts::{
        ADMIN_SETTLEMENT_RETURN_ACTIONS, ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
        ADMIN_SETTLEMENT_RETURN_BLOCKERS, ADMIN_SETTLEMENT_RETURN_EVENT_KINDS,
        ADMIN_SETTLEMENT_RETURN_MATCH_STATES, ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
        ADMIN_SETTLEMENT_RETURN_PLANS, ADMIN_SETTLEMENT_RETURN_REASONS,
        ADMIN_SETTLEMENT_RETURN_STATUSES, ADMIN_SETTLEMENT_RETURN_TAKE_STATES,
        ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS,
    };
    use serde_json::json;

    use super::{
        action_effect_key, action_key, blocker_key, event_details_text, event_key, kind_key,
        location_key, match_key, plan_key, reason_key, settlement_return_case,
        shows_asset_location, status_key, take_state_key, take_unknown_guidance_key,
        take_unknown_reason_key,
    };

    /// 闭集取值到译文键的映射函数。
    type KeyFn = fn(&str) -> Option<&'static str>;

    const LOCALES: [(&str, &str); 5] = [
        ("en", include_str!("../../../i18n/en.ftl")),
        ("zh-CN", include_str!("../../../i18n/zh-CN.ftl")),
        ("zh-TW", include_str!("../../../i18n/zh-TW.ftl")),
        ("ja", include_str!("../../../i18n/ja.ftl")),
        ("ko", include_str!("../../../i18n/ko.ftl")),
    ];

    #[test]
    fn resent_case_does_not_present_historical_custody_as_current() {
        assert!(!shows_asset_location("resent"));
        assert!(shows_asset_location("held"));
        assert!(shows_asset_location("take_unknown"));
    }

    fn ftl_keys(source: &str) -> BTreeSet<&str> {
        source
            .lines()
            .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim_end()))
            .filter(|key| {
                key.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
                    && key.chars().all(|char| char.is_ascii_alphanumeric() || char == '-')
            })
            .collect()
    }

    /// 每个契约闭集取值都映射到唯一的译文键，且五种语言都有该键；闭集增删取值时这里先失败，
    /// 不会让页面显示内部名称。
    #[test]
    fn every_closed_value_has_a_translated_label_in_all_locales() {
        let sets: [(&[&str], KeyFn); 13] = [
            (&ADMIN_SETTLEMENT_RETURN_STATUSES, status_key),
            (&ADMIN_SETTLEMENT_RETURN_MATCH_STATES, match_key),
            (&ADMIN_SETTLEMENT_RETURN_REASONS, reason_key),
            (&ADMIN_SETTLEMENT_RETURN_ACTIONS, action_key),
            (&ADMIN_SETTLEMENT_RETURN_ACTIONS, action_effect_key),
            (&ADMIN_SETTLEMENT_RETURN_BLOCKERS, blocker_key),
            (&ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS, location_key),
            (&ADMIN_SETTLEMENT_RETURN_PLANS, plan_key),
            (&ADMIN_SETTLEMENT_RETURN_EVENT_KINDS, event_key),
            (&ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, kind_key),
            (&ADMIN_SETTLEMENT_RETURN_TAKE_STATES, take_state_key),
            (&ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS, take_unknown_reason_key),
            (&ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS, take_unknown_guidance_key),
        ];
        let locales: Vec<(&str, BTreeSet<&str>)> =
            LOCALES.iter().map(|(name, source)| (*name, ftl_keys(source))).collect();
        let mut seen = HashSet::new();
        for (values, key) in sets {
            for value in values {
                let key = key(value).unwrap_or_else(|| panic!("{value} 缺少译文键"));
                assert!(seen.insert(key), "{key} 被多个取值共用");
                for (locale, keys) in &locales {
                    assert!(keys.contains(key), "{locale} 缺少 {key}");
                }
            }
            assert_eq!(key("not_a_value"), None);
        }
    }

    /// 事件细节按键名排序，只展示标量，嵌套结构不展开。
    #[test]
    fn event_details_render_sorted_scalars_only() {
        let details = json!({
            "reason_code": "returned_assets_match",
            "dispatch_no": 2,
            "backfilled": true,
            "delivery_id": null,
            "nested": {"secret": "x"}
        });
        assert_eq!(
            event_details_text(&details),
            "backfilled=true · delivery_id=- · dispatch_no=2 · nested=… · reason_code=returned_assets_match"
        );
        assert_eq!(event_details_text(&json!("text")), "");
        assert_eq!(event_details_text(&json!({})), "");
    }

    /// 投递台只为结算退件 Take 生成案件链接，且只接受规范 UUID。
    #[test]
    fn delivery_origin_links_only_settlement_return_cases() {
        let case = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(
            settlement_return_case(
                "settlement_return_action",
                &format!("settlement_return:{case}")
            )
            .as_deref(),
            Some(case)
        );
        assert_eq!(
            settlement_return_case("settlement", &format!("settlement_return:{case}")),
            None
        );
        assert_eq!(settlement_return_case("settlement_return_action", case), None);
        assert_eq!(
            settlement_return_case("settlement_return_action", "settlement_return:not-a-uuid"),
            None
        );
    }
}

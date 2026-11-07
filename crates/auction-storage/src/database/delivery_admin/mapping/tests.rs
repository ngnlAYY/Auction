use auction_application::delivery::DELIVERY_ADMIN_RETIREMENT_REASONS;
use serde_json::json;

use super::status::{
    reason_code_filtered, send_category, suppression_reason_for, template_from_payload,
    world_from_payload,
};

#[test]
fn send_category_mirrors_sql_function_order() {
    assert_eq!(
        send_category("lot_watch", &json!({"operation": "send"})).as_deref(),
        Some("outbid")
    );
    assert_eq!(
        send_category("operations_alert", &json!({"operation": "send"})).as_deref(),
        Some("operations")
    );
    assert_eq!(
        send_category("seller_summary", &json!({"operation": "send"})).as_deref(),
        Some("settlement")
    );
    for template in ["register_key", "rekey_key", "rekey_result", "rebind_result"] {
        assert_eq!(
            send_category("mail_intake", &json!({"operation": "send", "template": template}))
                .as_deref(),
            Some("account"),
            "{template} must stay in the account class"
        );
    }
    assert_eq!(
        send_category(
            "mail_intake",
            &json!({"operation": "send", "template": "auction_submission_receipt"})
        )
        .as_deref(),
        Some("intake")
    );
    // 装备资产优先于结算来源归类，与 SQL CASE 的顺序一致。
    assert_eq!(
        send_category("settlement", &json!({"operation": "send", "attachment_ids": ["1"]}))
            .as_deref(),
        Some("equipment")
    );
    assert_eq!(
        send_category("settlement", &json!({"operation": "send"})).as_deref(),
        Some("settlement")
    );
    assert_eq!(send_category("manual", &json!({"operation": "send"})).as_deref(), Some("manual"));
    // Return/Take 不是 Send，不能占用发送类别。
    assert_eq!(send_category("lot_watch", &json!({"operation": "take"})), None);
}

#[test]
fn world_and_template_only_accept_known_shapes() {
    assert_eq!(world_from_payload(&json!({})).as_deref(), Some("main"));
    assert_eq!(world_from_payload(&json!({"world": "isekai"})).as_deref(), Some("isekai"));
    assert_eq!(world_from_payload(&json!({"world": "moon"})), None);
    assert_eq!(
        template_from_payload(&json!({"template": "lot_watch"})).as_deref(),
        Some("lot_watch")
    );
    assert_eq!(template_from_payload(&json!({"template": null})), None);
    assert_eq!(template_from_payload(&json!({"template": ""})), None);
}

#[test]
fn retirement_and_suppression_are_closed_sets() {
    assert_eq!(
        reason_code_filtered(Some("recipient_renamed"), &DELIVERY_ADMIN_RETIREMENT_REASONS)
            .as_deref(),
        Some("recipient_renamed")
    );
    assert_eq!(reason_code_filtered(Some("unknown"), &DELIVERY_ADMIN_RETIREMENT_REASONS), None);
    assert_eq!(
        suppression_reason_for(Some("invalid_payload"), None).as_deref(),
        Some("invalid_payload")
    );
    assert_eq!(
        suppression_reason_for(Some("retry_exhausted"), None).as_deref(),
        Some("retry_exhausted")
    );
    // 普通失败码不是抑制原因。
    assert_eq!(suppression_reason_for(Some("recipient_not_found"), None), None);
    assert_eq!(suppression_reason_for(None, Some("recipient_renamed")), None);
}

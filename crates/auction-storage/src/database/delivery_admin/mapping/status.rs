//! payload 与状态推导出的安全展示投影：类别、世界、模板、COD 及退役/抑制原因。

use serde_json::Value;

pub(super) fn cod_from_payload(payload: &Value) -> Option<i64> {
    if payload.get("schema_version").and_then(Value::as_i64) != Some(1)
        || payload.get("operation").and_then(Value::as_str) != Some("send")
    {
        return None;
    }
    let cod = payload.get("cod").and_then(Value::as_i64)?;
    (0..=auction_application::delivery::MAX_DELIVERY_COD).contains(&cod).then_some(cod)
}

/// 冻结模板键；Return/Take 的 `template` 为 null 时返回 `None`。
pub(super) fn template_from_payload(payload: &Value) -> Option<String> {
    let value = payload.get("template").and_then(Value::as_str)?;
    (!value.is_empty() && !value.chars().any(char::is_control)).then(|| value.to_owned())
}

/// 投递世界；旧载荷缺省 `main`，与 `deliveries_mailbox_sweep_order` 的 COALESCE 一致。
pub(super) fn world_from_payload(payload: &Value) -> Option<String> {
    match payload.get("world").and_then(Value::as_str) {
        None => Some("main".to_owned()),
        Some(world @ ("main" | "isekai")) => Some(world.to_owned()),
        // 未知世界不猜测归类，保留为 `None` 让界面显示原值缺失。
        Some(_) => None,
    }
}

/// 发送类别投影，镜像 `public.delivery_send_category` 的判定顺序。
/// 权威归属仍是 SQL 函数（节奏与优先级只读它）；这里只用于只读列表展示，二者必须同步修改。
pub(super) fn send_category(origin_kind: &str, payload: &Value) -> Option<String> {
    if payload.get("operation").and_then(Value::as_str) != Some("send") {
        return None;
    }
    let template = payload.get("template").and_then(Value::as_str);
    let non_empty_array = |key: &str| {
        payload.get(key).and_then(Value::as_array).is_some_and(|values| !values.is_empty())
    };
    let category = if origin_kind == "lot_watch" {
        "outbid"
    } else if origin_kind == "operations_alert" {
        "operations"
    } else if matches!(origin_kind, "seller_summary" | "season_reminder") {
        "settlement"
    } else if origin_kind == "mail_intake"
        && matches!(
            template,
            Some(
                "register_key"
                    | "register_result"
                    | "rekey_key"
                    | "rekey_result"
                    | "rebind_result"
                    | "user_assets_rejected"
            )
        )
    {
        "account"
    } else if non_empty_array("equipment_secret_refs") || non_empty_array("attachment_ids") {
        "equipment"
    } else if matches!(origin_kind, "settlement" | "settlement_recovery_action") {
        "settlement"
    } else if origin_kind == "mail_intake" {
        "intake"
    } else {
        "manual"
    };
    Some(category.to_owned())
}

/// 只保留闭集内的原因码；损坏或未知原因不进入退役/抑制展示。
pub(super) fn reason_code_filtered(value: Option<&str>, allowed: &[&str]) -> Option<String> {
    let value = value?;
    allowed.contains(&value).then(|| value.to_owned())
}

/// 围栏前抑制原因：隔离、准备拒绝或告警批次终止；与退役原因互斥。
pub(super) fn suppression_reason_for(
    failure_code: Option<&str>,
    reason_code: Option<&str>,
) -> Option<String> {
    let is_suppression = |value: &str| {
        auction_application::delivery::DELIVERY_ADMIN_SUPPRESSION_REASONS.contains(&value)
    };
    failure_code
        .filter(|value| is_suppression(value))
        .or_else(|| reason_code.filter(|value| is_suppression(value)))
        .map(str::to_owned)
}

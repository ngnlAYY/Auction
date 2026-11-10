//! 模板占位符闭集与正文校验。

use super::{MAX_MAIL_TEMPLATE_BODY_BYTES, MailTemplateError};

pub(super) fn placeholder_rule(key: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match key {
        "auction_equipment_rejected" => {
            (&[], &["reason_code", "reason_text", "next_steps", "mail_reference"])
        }
        "register_key" => (&["login_key", "username"], &["login_key", "username"]),
        "register_result" | "rebind_result" | "rekey_result" => {
            (&["result_code"], &["result_code", "username"])
        }
        "rekey_key" => (&["login_key", "username"], &["login_key", "username"]),
        "settlement_winner_equipment_cod" => (
            &["item_name", "amount", "reference"],
            &["item_name", "amount", "reference", "price_breakdown", "payment_deadline"],
        ),
        "settlement_payment_carrier" => {
            (&["amount", "reference"], &["amount", "reference", "item_name", "payment_deadline"])
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            (&["amount", "reference"], &["amount", "reference", "item_name"])
        }
        "settlement_payment_reminder" => (&["reference"], &["reference"]),
        "settlement_equipment_handoff" => {
            (&["item_name", "reference"], &["item_name", "reference"])
        }
        "lot_watch" => (
            &["item_name", "lot_code", "amount"],
            &["item_name", "lot_code", "amount", "deadline_line", "lot_url_line"],
        ),
        "lot_watch_pm" => (
            &["item_name", "lot_code", "amount"],
            &[
                "item_name",
                "lot_code",
                "amount",
                "deadline_line",
                "lot_url_line",
                "notification_id",
            ],
        ),
        "operations_alert" => (&["summary"], &["summary"]),
        // 拒收类正文不接受任何占位符。
        _ => (&[], &[]),
    }
}

pub(super) fn validate_body(
    value: &str,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), MailTemplateError> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > MAX_MAIL_TEMPLATE_BODY_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(MailTemplateError::InvalidBody);
    }
    let mut cursor = 0;
    let mut seen = Vec::with_capacity(allowed.len());
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        if value[cursor..start].contains('}') {
            return Err(MailTemplateError::UnclosedPlaceholder);
        }
        let end = value[start + 1..]
            .find('}')
            .map(|relative_end| start + 1 + relative_end)
            .ok_or(MailTemplateError::UnclosedPlaceholder)?;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(MailTemplateError::UnknownPlaceholder);
        }
        if !seen.contains(&placeholder) {
            seen.push(placeholder);
        }
        cursor = end + 1;
    }
    if value[cursor..].contains('}') {
        return Err(MailTemplateError::UnclosedPlaceholder);
    }
    if required.iter().any(|placeholder| !seen.contains(placeholder)) {
        return Err(MailTemplateError::MissingPlaceholder);
    }
    Ok(())
}

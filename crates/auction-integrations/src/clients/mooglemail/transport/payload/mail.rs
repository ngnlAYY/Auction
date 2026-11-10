//! 普通 MoogleMail delivery payload 的安全校验。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::collections::HashSet;

use super::super::*;
use auction_application::mail::valid_notification_body;

/// 通知类投递的 `origin_kind` 闭集：解码、发送分派与本函数必须使用同一清单。
/// 新增通知来源时只改这里；`render_send` 曾因漏列 `seller_summary` 与
/// `season_reminder` 把已入队的通知判为 invalid_payload 永久失败。
pub(crate) fn is_notification_origin(origin_kind: &str) -> bool {
    matches!(
        origin_kind,
        "mail_intake"
            | "settlement_recovery_action"
            | "operations_alert"
            | "lot_watch"
            | "seller_summary"
            | "season_reminder"
    )
}

pub(crate) fn decode_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(MailDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if !is_notification_origin(&claim.origin_kind)
        || claim.delivery_id.is_nil()
        || !valid_line(&claim.recipient, 256, false)
        || !valid_line(&claim.origin_id, 512, false)
    {
        return Err(invalid_payload());
    }
    let payload = MailDeliveryPayload::deserialize(&claim.payload).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mail_delivery",
            "decode_mail_payload",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        invalid_payload()
    })?;
    if payload.schema_version != SCHEMA_VERSION
        || !valid_line(&payload.world, 16, false)
        || !valid_line(&payload.external_mail_id, 256, false)
        || !valid_opaque_id(&payload.external_mail_id)
        || payload.credits < 0
        || payload.cod < 0
        || payload.attachment_ids.len() > MAX_ATTACHMENT_IDS
    {
        return Err(invalid_payload());
    }
    let mut attachment_ids = HashSet::with_capacity(payload.attachment_ids.len());
    if payload
        .attachment_ids
        .iter()
        .any(|id| !valid_decimal_id(id) || !attachment_ids.insert(id.as_str()))
    {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    if claim.origin_kind == "mail_intake" && !valid_mail_intake_origin_id(claim, &payload) {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "operations_alert"
        && (claim.origin_id
            != payload
                .external_mail_id
                .strip_prefix("operations-alert:")
                .map(|value| format!("operations_alert:{value}"))
                .unwrap_or_default()
            || !valid_notification_body(payload.body.as_deref()))
    {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "lot_watch"
        && (claim.origin_id != format!("lot_watch:{}", claim.command_id)
            || payload.external_mail_id != claim.command_id.to_string()
            || payload.operation != "send"
            || payload.template.as_deref() != Some("lot_watch")
            || payload.result_code.as_deref() != Some("outbid")
            || payload.user_key_user_id.is_some()
            || !valid_notification_body(payload.body.as_deref()))
    {
        return Err(invalid_payload());
    }
    // 投稿回执是唯一允许 mail_intake 携带冻结正文的通知，正文由录入事实生成。
    let frozen_intake_body = claim.origin_kind == "mail_intake"
        && payload.operation == "send"
        && payload.template.as_deref() == Some("auction_submission_receipt");
    if !matches!(
        claim.origin_kind.as_str(),
        "operations_alert" | "lot_watch" | "seller_summary" | "season_reminder"
    ) && !frozen_intake_body
        && payload.body.is_some()
    {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "settlement_recovery_action" {
        let recovery_id =
            claim.origin_id.strip_prefix("recovery:").and_then(|value| Uuid::parse_str(value).ok());
        if recovery_id.is_none()
            || (payload.operation == "send" && payload.world != "main")
            || !matches!(payload.operation.as_str(), "send" | "take")
        {
            return Err(invalid_payload());
        }
    }
    match payload.operation.as_str() {
        "send" => {
            if payload.template.is_none()
                || payload.result_code.is_none()
                || payload.credits != 0
                || payload.cod != 0
                || !payload.attachment_ids.is_empty()
            {
                return Err(invalid_payload());
            }
            match payload.template.as_deref() {
                Some("register_key") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_none()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("registered" | "recovered")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("register_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("rekey_key") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_none()
                        || payload.result_code.as_deref() != Some("rekeyed")
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("rekey_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict" | "not_registered")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("rebind_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some(
                                "rebound"
                                    | "unchanged"
                                    | "blacklisted"
                                    | "identity_conflict"
                                    | "invalid_key"
                                    | "missing_key"
                            )
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some(
                    "auction_assets_rejected" | "auction_no_attachments" | "user_assets_rejected",
                ) => {
                    if claim.origin_kind != "mail_intake" || payload.user_key_user_id.is_some() {
                        return Err(invalid_payload());
                    }
                }
                Some("auction_equipment_rejected") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || payload
                            .result_code
                            .as_deref()
                            .and_then(equipment_rejection_copy)
                            .is_none()
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("auction_submission_receipt") => {
                    // 整封投稿合并的回执：正文必须来自录入事实，零资产且不引用登录 key。
                    if claim.origin_kind != "mail_intake"
                        || payload.result_code.as_deref() != Some("accepted")
                        || payload.user_key_user_id.is_some()
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("recovery_payment_reminder") => {
                    if claim.origin_kind != "settlement_recovery_action"
                        || payload.result_code.as_deref() != Some("payment_overdue")
                        || payload.user_key_user_id.is_some()
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("auction_seller_summary") => {
                    // 每场每卖方一封的结算汇总：正文来自已结算事实，零资产且不引用登录 key。
                    if claim.origin_kind != "seller_summary"
                        || payload.world != "main"
                        || payload.result_code.as_deref() != Some("summary")
                        || payload.user_key_user_id.is_some()
                        || !claim.origin_id.starts_with("seller_summary:")
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("isekai_season_reminder") => {
                    // 换季提醒正文绑定冻结的赛季边界；发送前不再读取当前排期配置。
                    if claim.origin_kind != "season_reminder"
                        || payload.world != "isekai"
                        || payload.result_code.as_deref() != Some("season_end")
                        || payload.user_key_user_id.is_some()
                        || !claim.origin_id.starts_with("season_reminder:")
                        || !payload.external_mail_id.starts_with("season-end:")
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("lot_watch") => {
                    if claim.origin_kind != "lot_watch" {
                        return Err(invalid_payload());
                    }
                }
                Some("operations_alert") => {
                    if claim.origin_kind != "operations_alert"
                        || payload.result_code.as_deref() != Some("operations_alert")
                        || payload.user_key_user_id.is_some()
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                _ => return Err(invalid_payload()),
            }
        }
        "return" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.credits != 0
                || payload.cod != 0
                || payload.user_key_user_id.is_some()
                || claim.origin_kind != "mail_intake"
            {
                return Err(invalid_payload());
            }
        }
        "take" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.user_key_user_id.is_some()
                || (claim.origin_kind == "mail_intake"
                    && (payload.credits != 0 || payload.cod != 0))
            {
                return Err(invalid_payload());
            }
        }
        _ => return Err(invalid_payload()),
    }
    Ok((payload, world))
}

fn valid_mail_intake_origin_id(claim: &DeliveryWorkClaim, payload: &MailDeliveryPayload) -> bool {
    let canonical = format!("{}:{}:{}", payload.world, payload.external_mail_id, payload.operation);
    if claim.origin_id == canonical {
        return true;
    }
    // 历史 Rust producer 误把命令用途写进 origin_id。兼容仅限同一 world、邮件 ID 和三种
    // 已知 operation 的逐项别名；稳定命令身份不变，未知后缀继续失败关闭。
    let legacy_operation = match payload.operation.as_str() {
        "send" => "delivery-notice",
        "return" => "delivery-return",
        "take" => "delivery-take",
        _ => return false,
    };
    claim.origin_id == format!("{}:{}:{legacy_operation}", payload.world, payload.external_mail_id)
}

#[cfg(test)]
mod tests {
    use super::is_notification_origin;

    /// 通知来源闭集必须与 `render_send` 分派一致；`seller_summary` 与
    /// `season_reminder` 是冻结正文通知，漏列会使已入队投递永久失败。
    #[test]
    fn notification_origin_closed_set_covers_every_frozen_body_template() {
        for kind in [
            "mail_intake",
            "settlement_recovery_action",
            "operations_alert",
            "lot_watch",
            "seller_summary",
            "season_reminder",
        ] {
            assert!(is_notification_origin(kind), "{kind} 必须按通知分派");
        }
        for kind in ["settlement", "", "seller_summary_x", "SEASON_REMINDER"] {
            assert!(!is_notification_origin(kind), "{kind} 不是通知来源");
        }
    }
}

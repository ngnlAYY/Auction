//! 普通 MoogleMail delivery payload 的安全校验。

use std::collections::HashSet;

use auction_application::delivery::{DeliveryTransportError, DeliveryWorkClaim};
use auction_application::mail::{
    MailNotificationFields, MailNotificationTemplate, valid_notification_body,
    valid_season_reminder_mail_id,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::protocol::{valid_decimal_id, valid_line, valid_opaque_id};
use crate::clients::mooglemail::transport::{
    MAX_ATTACHMENT_IDS, MailDeliveryPayload, SCHEMA_VERSION, invalid_payload,
};
use crate::safe_diagnostics;

/// 通知类投递的 `origin_kind` 闭集：解码、发送分派与本函数必须使用同一清单，
/// 投递可用性闸门（`DeliveryTransport::available`）同样使用本清单。
/// 新增通知来源时只改这里；漏列会把已入队的通知判为 invalid_payload 永久失败。
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
    let payload = MailDeliveryPayload::deserialize(&claim.payload).map_err(
        safe_diagnostics::error_map("mail_delivery", "decode_mail_payload", invalid_payload),
    )?;
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
    // 装备标记通知只有 Send：模板、结果码、key 引用与正文由下方 Send 分支按模板表核对，
    // 这里只核对来源特有的 origin_id 与邮件 ID。
    if claim.origin_kind == "lot_watch"
        && (claim.origin_id != format!("lot_watch:{}", claim.command_id)
            || payload.external_mail_id != claim.command_id.to_string()
            || payload.operation != "send")
    {
        return Err(invalid_payload());
    }
    // 通知的冻结正文由模板表约束；退回与领取只允许原有几类来源携带正文。
    if payload.operation != "send"
        && !matches!(
            claim.origin_kind.as_str(),
            "operations_alert" | "lot_watch" | "seller_summary" | "season_reminder"
        )
        && payload.body.is_some()
    {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "settlement_recovery_action" {
        // 付款提醒的固定 main 世界由模板表核对；领取动作沿用 recovery action 的目标世界。
        let recovery_id =
            claim.origin_id.strip_prefix("recovery:").and_then(|value| Uuid::parse_str(value).ok());
        if recovery_id.is_none() || !matches!(payload.operation.as_str(), "send" | "take") {
            return Err(invalid_payload());
        }
    }
    match payload.operation.as_str() {
        "send" => {
            if payload.credits != 0 || payload.cod != 0 || !payload.attachment_ids.is_empty() {
                return Err(invalid_payload());
            }
            // 来源、结果码、登录 key 引用（非 nil）、冻结正文与固定世界只查应用层模板表，和入队
            // 校验、资产冲突排除同一口径；表外模板一律判无效。
            let Some(template) =
                payload.template.as_deref().and_then(MailNotificationTemplate::from_name)
            else {
                return Err(invalid_payload());
            };
            if !template.binding().admits(MailNotificationFields {
                origin: &claim.origin_kind,
                world: &payload.world,
                result_code: payload.result_code.as_deref(),
                user_key_user_id: payload.user_key_user_id,
                body: payload.body.as_deref(),
            }) {
                return Err(invalid_payload());
            }
            // origin_id 与冻结边界属于各来源 owner：卖方汇总每场每卖方一封，换季提醒正文绑定
            // 冻结的赛季边界，发送前不再读取当前排期配置。
            let source_valid = match claim.origin_kind.as_str() {
                "seller_summary" => claim.origin_id.starts_with("seller_summary:"),
                "season_reminder" => {
                    claim.origin_id.starts_with("season_reminder:")
                        && valid_season_reminder_mail_id(&payload.external_mail_id)
                }
                _ => true,
            };
            if !source_valid {
                return Err(invalid_payload());
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
    use auction_application::mail::MailDeliveryOrigin;

    use super::is_notification_origin;

    /// 通知来源闭集必须与 `render_send` 分派一致，并覆盖应用层能入队的每个来源；`seller_summary`
    /// 与 `season_reminder` 是冻结正文通知，漏列会使已入队投递永久失败。
    #[test]
    fn notification_origin_closed_set_covers_every_frozen_body_template() {
        for origin in MailDeliveryOrigin::ALL {
            assert!(is_notification_origin(origin.as_str()), "{origin:?} 必须按通知分派");
        }
        for kind in ["settlement", "", "seller_summary_x", "SEASON_REMINDER"] {
            assert!(!is_notification_origin(kind), "{kind} 不是通知来源");
        }
    }
}

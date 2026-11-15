//! 普通 MoogleMail delivery payload 的安全校验。

use auction_application::delivery::{DeliveryTransportError, DeliveryWorkClaim};
use auction_application::mail::{
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryShape, MailNotificationTemplate,
};
use serde::Deserialize;

use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::protocol::valid_line;
use crate::clients::mooglemail::transport::{MailDeliveryPayload, SCHEMA_VERSION, invalid_payload};
use crate::safe_diagnostics;

/// 通知类投递的 `origin_kind` 闭集，取自应用层的投递来源闭集：解码、发送分派与投递可用性闸门
/// （`DeliveryTransport::available`）都用本函数，新增来源只改应用层闭集；漏列会把已入队的通知
/// 判为 invalid_payload 永久失败。
pub(crate) fn is_notification_origin(origin_kind: &str) -> bool {
    MailDeliveryOrigin::from_name(origin_kind).is_some()
}

pub(crate) fn decode_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(MailDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    let Some(origin) = MailDeliveryOrigin::from_name(&claim.origin_kind) else {
        return Err(invalid_payload());
    };
    if claim.delivery_id.is_nil()
        || !valid_line(&claim.recipient, 256, false)
        || !valid_line(&claim.origin_id, 512, false)
    {
        return Err(invalid_payload());
    }
    let payload = MailDeliveryPayload::deserialize(&claim.payload).map_err(
        safe_diagnostics::error_map("mail_delivery", "decode_mail_payload", invalid_payload),
    )?;
    let operation =
        MailDeliveryOperation::from_name(&payload.operation).ok_or_else(invalid_payload)?;
    let template = match payload.template.as_deref() {
        Some(name) => Some(MailNotificationTemplate::from_name(name).ok_or_else(invalid_payload)?),
        None => None,
    };
    // 来源 × 操作、资产、模板字段与来源身份只查应用层统一判定，和入队校验、资产冲突排除同一
    // 口径；表外模板与任一越界字段都判为无效载荷。
    let shape = MailDeliveryShape {
        origin,
        operation,
        command_id: claim.command_id,
        origin_id: &claim.origin_id,
        world: &payload.world,
        external_mail_id: &payload.external_mail_id,
        template,
        result_code: payload.result_code.as_deref(),
        user_key_user_id: payload.user_key_user_id,
        body: payload.body.as_deref(),
        credits: payload.credits,
        cod: payload.cod,
        attachment_ids: &payload.attachment_ids,
    };
    if payload.schema_version != SCHEMA_VERSION || !shape.admits() {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    Ok((payload, world))
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

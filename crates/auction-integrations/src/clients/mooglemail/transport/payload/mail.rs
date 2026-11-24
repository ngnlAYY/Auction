//! 普通 MoogleMail delivery payload 的安全校验。

use auction_application::delivery::{DeliveryTransportError, DeliveryWorkClaim};
use auction_application::mail::{
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryShape, MailNotificationTemplate,
};
use auction_domain::valid_positive_decimal;
use auction_time::Timestamp;
use serde::Deserialize;

use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::parser::{FrozenMailAttachment, FrozenMailManifest};
use crate::clients::mooglemail::protocol::{valid_line, valid_opaque_id};
use crate::clients::mooglemail::transport::types::ExpectedMailManifestPayload;
use crate::clients::mooglemail::transport::{
    CONSIGNMENT_DISPOSITION_SCHEMA_VERSION, MAIL_DELIVERY_SCHEMA_VERSION, MailDeliveryPayload,
    invalid_payload,
};
use crate::clients::mooglemail::transport::{
    DispositionRequest, DispositionTarget, ExpectedReturn,
};
use crate::safe_diagnostics;

/// 通知类投递的 `origin_kind` 闭集，取自应用层的投递来源闭集：解码、发送分派与投递可用性闸门
/// （`DeliveryTransport::available`）都用本函数，新增来源只改应用层闭集；漏列会把已入队的通知
/// 判为 invalid_payload 永久失败。
pub(crate) fn is_notification_origin(origin_kind: &str) -> bool {
    MailDeliveryOrigin::from_name(origin_kind)
        .is_some_and(|origin| origin != MailDeliveryOrigin::ConsignmentDisposition)
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
    let expected_schema_version = match origin {
        MailDeliveryOrigin::ConsignmentDisposition => CONSIGNMENT_DISPOSITION_SCHEMA_VERSION,
        _ => MAIL_DELIVERY_SCHEMA_VERSION,
    };
    if payload.schema_version != expected_schema_version || !shape.admits() {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    match origin {
        MailDeliveryOrigin::ConsignmentDisposition => {
            let manifest = payload.expected_manifest.as_ref().ok_or_else(invalid_payload)?;
            let mail_key = payload.mail_key.as_ref().ok_or_else(invalid_payload)?;
            let decision_id = payload.decision_id.ok_or_else(invalid_payload)?;
            let work_id = payload.work_id.ok_or_else(invalid_payload)?;
            let expected_origin_id = format!("consignment:{decision_id}:{work_id}");
            // DeliveryWorkClaim 不携带 bot_uid；这里校验载荷内原信键的规范形状，并把
            // world/MID 与冻结目标逐字比对。bot_uid 的持久绑定由 4M 的 work 行负责。
            if !valid_manifest_payload(manifest)
                || !valid_positive_decimal(&mail_key.bot_uid, 32)
                || mail_key.world_id != payload.world
                || mail_key.external_mid != payload.external_mail_id
                || decision_id.is_nil()
                || work_id.is_nil()
                || payload.attempt_id.is_none_or(|id| id.is_nil())
                || !valid_opaque_id(&payload.external_mail_id)
                || claim.origin_id != expected_origin_id
                || payload.credits != manifest.credits
                || payload.cod != manifest.cod
                || payload.attachment_ids != manifest_eids(manifest)
            {
                return Err(invalid_payload());
            }
            match operation {
                MailDeliveryOperation::Take if payload.expected_return.is_none() => {}
                MailDeliveryOperation::Return
                    if payload.expected_return.as_ref() == Some(manifest) => {}
                _ => return Err(invalid_payload()),
            }
        }
        _ if payload.mail_key.is_some()
            || payload.decision_id.is_some()
            || payload.work_id.is_some()
            || payload.attempt_id.is_some()
            || payload.expected_manifest.is_some()
            || payload.expected_return.is_some() =>
        {
            return Err(invalid_payload());
        }
        _ => {}
    }
    Ok((payload, world))
}

/// 把新来源的 claim 解成严格处置请求；Return 缺少证明时无法构造。
pub fn decode_consignment_disposition_request(
    claim: &DeliveryWorkClaim,
) -> Result<DispositionRequest, DeliveryTransportError> {
    let (payload, world) = decode_payload(claim)?;
    if MailDeliveryOrigin::from_name(&claim.origin_kind)
        != Some(MailDeliveryOrigin::ConsignmentDisposition)
    {
        return Err(invalid_payload());
    }
    let manifest = payload.expected_manifest.as_ref().ok_or_else(invalid_payload)?;
    let frozen = frozen_manifest(world, &payload, manifest)?;
    let fence = u64::try_from(claim.fencing_version).map_err(invalid_integer)?;
    let target = DispositionTarget::new(
        payload.decision_id.ok_or_else(invalid_payload)?,
        payload.work_id.ok_or_else(invalid_payload)?,
        claim.delivery_id,
        payload.attempt_id.ok_or_else(invalid_payload)?,
        fence,
        frozen.clone(),
    )
    .ok_or_else(invalid_payload)?;
    match MailDeliveryOperation::from_name(&payload.operation) {
        Some(MailDeliveryOperation::Take) => Ok(DispositionRequest::Take(target)),
        Some(MailDeliveryOperation::Return) => {
            let expected = payload.expected_return.as_ref().ok_or_else(invalid_payload)?;
            let expected = frozen_manifest(world, &payload, expected)?;
            if expected != frozen {
                return Err(invalid_payload());
            }
            Ok(DispositionRequest::Return {
                target: Box::new(target),
                expected_return: ExpectedReturn::new(expected),
            })
        }
        _ => Err(invalid_payload()),
    }
}

fn valid_manifest_payload(manifest: &ExpectedMailManifestPayload) -> bool {
    !manifest.sender.is_empty()
        && !manifest.recipient.is_empty()
        && valid_line(&manifest.sender, 256, false)
        && valid_line(&manifest.recipient, 256, false)
        && valid_line(&manifest.subject, 512, true)
        && valid_digest(&manifest.body_digest)
        && valid_digest(&manifest.manifest_digest)
        && manifest.credits >= 0
        && manifest.cod >= 0
        && auction_time::parse_minute_datetime(&manifest.received_minute_raw).is_some()
        && !manifest.attachments.is_empty()
        && manifest.attachments.len() <= 1000
        && manifest.attachments.windows(2).all(|pair| pair[0].ordinal < pair[1].ordinal)
        && manifest.attachments.iter().all(|attachment| {
            attachment.ordinal > 0
                && valid_line(&attachment.name, 512, false)
                && attachment
                    .canonical_eid
                    .as_deref()
                    .is_some_and(|eid| valid_positive_decimal(eid, 32))
        })
}

fn manifest_eids(manifest: &ExpectedMailManifestPayload) -> Vec<String> {
    manifest.attachments.iter().filter_map(|attachment| attachment.canonical_eid.clone()).collect()
}

fn frozen_manifest(
    world: MoogleMailWorld,
    payload: &MailDeliveryPayload,
    manifest: &ExpectedMailManifestPayload,
) -> Result<FrozenMailManifest, DeliveryTransportError> {
    if !valid_manifest_payload(manifest) {
        return Err(invalid_payload());
    }
    let received_at = auction_time::parse_minute_datetime(&manifest.received_minute_raw)
        .and_then(Timestamp::from_utc_datetime)
        .ok_or_else(invalid_payload)?;
    let body_digest = decode_digest(&manifest.body_digest).ok_or_else(invalid_payload)?;
    let manifest_digest = decode_digest(&manifest.manifest_digest).ok_or_else(invalid_payload)?;
    Ok(FrozenMailManifest {
        world,
        external_mail_id: payload.external_mail_id.clone(),
        sender: manifest.sender.clone(),
        recipient: manifest.recipient.clone(),
        subject: manifest.subject.clone(),
        body_digest,
        credits: u64::try_from(manifest.credits).map_err(invalid_integer)?,
        cod: u64::try_from(manifest.cod).map_err(invalid_integer)?,
        received_minute_raw: manifest.received_minute_raw.clone(),
        received_at,
        attachments: manifest
            .attachments
            .iter()
            .map(|attachment| FrozenMailAttachment {
                ordinal: attachment.ordinal,
                name: attachment.name.clone(),
                canonical_eid: attachment.canonical_eid.clone(),
            })
            .collect(),
        manifest_digest,
    })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid_integer(_: std::num::TryFromIntError) -> DeliveryTransportError {
    invalid_payload()
}

fn decode_digest(value: &str) -> Option<[u8; 32]> {
    if !valid_digest(value) {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        digest[index] = high << 4 | low;
    }
    Some(digest)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod consignment_payload_tests {
    use auction_application::core::RequestHash;
    use auction_application::delivery::DeliveryWorkClaim;
    use auction_time::Timestamp;
    use serde_json::json;
    use uuid::Uuid;

    use super::{decode_consignment_disposition_request, decode_payload};
    use crate::clients::mooglemail::transport::{DispositionAction, DispositionRequest};

    fn claim(
        origin_kind: &str,
        origin_id: String,
        payload: serde_json::Value,
    ) -> DeliveryWorkClaim {
        let now = Timestamp::now();
        DeliveryWorkClaim {
            delivery_id: Uuid::new_v4(),
            command_id: Uuid::new_v4(),
            request_hash: RequestHash::new([7; 32]),
            origin_kind: origin_kind.to_owned(),
            origin_id,
            recipient: "FixtureBot".to_owned(),
            payload,
            lease_token: Uuid::new_v4(),
            lease_until: now,
            fencing_version: 7,
            attempt_count: 0,
            worker_id: "fixture-worker".to_owned(),
            claimed_at: now,
            created_at: now,
        }
    }

    fn manifest() -> serde_json::Value {
        json!({
            "sender": "FixtureSeller",
            "recipient": "FixtureBot",
            "subject": "consign",
            "body_digest": "00".repeat(32),
            "manifest_digest": "11".repeat(32),
            "credits": 0,
            "cod": 0,
            "received_minute_raw": "2026-11-02 03:45",
            "attachments": [{
                "ordinal": 1,
                "name": "Fixture Blade",
                "canonical_eid": "900001"
            }]
        })
    }

    fn consignment_payload(
        operation: &str,
        expected_return: Option<serde_json::Value>,
    ) -> (serde_json::Value, String) {
        let manifest = manifest();
        let decision_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let payload = json!({
            "schema_version": 2,
            "operation": operation,
            "world": "main",
            "external_mail_id": "900001",
            "mail_key": {
                "bot_uid": "12345",
                "world_id": "main",
                "external_mid": "900001"
            },
            "template": null,
            "result_code": null,
            "body": null,
            "user_key_user_id": null,
            "credits": 0,
            "cod": 0,
            "attachment_ids": ["900001"],
            "decision_id": decision_id,
            "work_id": work_id,
            "attempt_id": Uuid::new_v4(),
            "expected_manifest": manifest,
            "expected_return": expected_return
        });
        (payload, format!("consignment:{decision_id}:{work_id}"))
    }

    #[test]
    fn old_payload_still_decodes_without_new_fields() {
        let payload = json!({
            "schema_version": 1,
            "operation": "take",
            "world": "main",
            "external_mail_id": "4242",
            "template": null,
            "result_code": null,
            "body": null,
            "user_key_user_id": null,
            "credits": 0,
            "cod": 0,
            "attachment_ids": ["900000001"]
        });
        let claim = claim("mail_intake", "main:4242:take".to_owned(), payload);
        assert!(decode_payload(&claim).is_ok());
    }

    #[test]
    fn consignment_return_requires_matching_expected_return() {
        let (payload, origin_id) = consignment_payload("return", None);
        let missing = claim("consignment_disposition", origin_id, payload);
        assert!(decode_payload(&missing).is_err());
        assert!(decode_consignment_disposition_request(&missing).is_err());

        let manifest = manifest();
        let (payload, origin_id) = consignment_payload("return", Some(manifest));
        let complete = claim("consignment_disposition", origin_id, payload);
        assert!(decode_payload(&complete).is_ok());
        let request =
            decode_consignment_disposition_request(&complete).expect("complete return decodes");
        assert_eq!(request.action(), DispositionAction::Return);
        assert!(matches!(request, DispositionRequest::Return { .. }));
    }

    #[test]
    fn consignment_origin_id_must_bind_payload_decision_and_work() {
        let (payload, _origin_id) = consignment_payload("take", None);
        let unrelated = format!("consignment:{}:{}", Uuid::new_v4(), Uuid::new_v4());
        let mismatched = claim("consignment_disposition", unrelated, payload);
        assert!(decode_payload(&mismatched).is_err());
        assert!(decode_consignment_disposition_request(&mismatched).is_err());
    }

    #[test]
    fn consignment_payload_requires_schema_two_and_the_original_mail_key() {
        let (payload, origin_id) = consignment_payload("take", None);
        for schema_version in [1, 3] {
            let mut changed = payload.clone();
            changed["schema_version"] = json!(schema_version);
            assert!(
                decode_payload(&claim("consignment_disposition", origin_id.clone(), changed))
                    .is_err(),
                "寄售处置只接受 schema 2"
            );
        }
        let mut missing = payload.clone();
        missing.as_object_mut().unwrap().remove("mail_key");
        assert!(
            decode_payload(&claim("consignment_disposition", origin_id.clone(), missing)).is_err()
        );
        for (field, value) in [("world_id", "isekai"), ("external_mid", "900002")] {
            let mut changed = payload.clone();
            changed["mail_key"][field] = json!(value);
            assert!(
                decode_payload(&claim("consignment_disposition", origin_id.clone(), changed))
                    .is_err(),
                "mail_key.{field} 必须与处置目标一致"
            );
        }
        let mut take_with_return = payload;
        take_with_return["expected_return"] = manifest();
        assert!(
            decode_payload(&claim("consignment_disposition", origin_id, take_with_return)).is_err()
        );
    }
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
            assert_eq!(
                is_notification_origin(origin.as_str()),
                origin != MailDeliveryOrigin::ConsignmentDisposition,
                "{origin:?} 的通知分派口径"
            );
        }
        for kind in ["settlement", "", "seller_summary_x", "SEASON_REMINDER"] {
            assert!(!is_notification_origin(kind), "{kind} 不是通知来源");
        }
    }
}

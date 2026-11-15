//! 普通 outbox 载荷的闭集分类：只有严格解码并能证明与目标装备无关的载荷才可从冲突门排除。

use std::collections::HashSet;

use auction_application::mail::{
    MailNotificationFields, MailNotificationTemplate, valid_season_reminder_mail_id,
};
use auction_domain::valid_text as valid_line;
use serde::Deserialize;
use uuid::Uuid;

use super::AssetConflictTarget;

/// 普通outbox的闭集schema；不能把缺失装备引用字段本身当作无装备证明。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttachmentPayload {
    pub(super) schema_version: u8,
    pub(super) operation: String,
    pub(super) world: String,
    pub(super) external_mail_id: String,
    pub(super) template: Option<String>,
    pub(super) result_code: Option<String>,
    pub(super) body: Option<String>,
    pub(super) user_key_user_id: Option<Uuid>,
    pub(super) credits: i64,
    pub(super) cod: i64,
    pub(super) attachment_ids: Vec<String>,
}

/// 资金责任预览仅忽略闭集普通入站通知的零资产历史，不将其标记成功或恢复写入权限。
pub(crate) fn assetless_mail_notification(
    value: &serde_json::Value,
    origin_id: &str,
    command_id: Uuid,
) -> bool {
    let Ok(payload) = AttachmentPayload::deserialize(value) else {
        return false;
    };
    payload.schema_version == 1
        && matches!(payload.world.as_str(), "main" | "isekai")
        && payload.operation == "send"
        && payload.credits == 0
        && payload.cod == 0
        && payload.attachment_ids.is_empty()
        && auction_application::settlement::valid_external_positive_id(&payload.external_mail_id)
        && valid_attachment_origin(&payload, "mail_intake", origin_id, command_id, false)
        && valid_notification(&payload, "mail_intake")
}

pub(super) fn unrelated_attachment_payload(
    value: &serde_json::Value,
    origin: &str,
    origin_id: &str,
    command_id: Uuid,
    forum_notice_owner: bool,
    claim: &AssetConflictTarget<'_>,
) -> bool {
    let Ok(payload) = AttachmentPayload::deserialize(value) else {
        return false;
    };
    if payload.schema_version != 1
        || !matches!(payload.world.as_str(), "main" | "isekai")
        || !valid_line(&payload.external_mail_id, 256)
        || payload.credits < 0
        || payload.cod < 0
        || payload.attachment_ids.len() > 1000
        || payload.attachment_ids.iter().any(|eid| {
            eid.is_empty()
                || eid.len() > 32
                || !eid.bytes().all(|byte| byte.is_ascii_digit())
                || eid.bytes().all(|byte| byte == b'0')
        })
        || !unique_attachment_ids(&payload.attachment_ids)
        || !valid_attachment_origin(&payload, origin, origin_id, command_id, forum_notice_owner)
    {
        return false;
    }
    let valid_operation = match payload.operation.as_str() {
        "send" => {
            payload.credits == 0
                && payload.cod == 0
                && payload.attachment_ids.is_empty()
                && valid_notification(&payload, origin)
        }
        "return" | "take" => {
            payload.template.is_none()
                && payload.result_code.is_none()
                && payload.user_key_user_id.is_none()
                && payload.body.is_none()
                && match (origin, payload.operation.as_str()) {
                    ("mail_intake", _) => payload.credits == 0 && payload.cod == 0,
                    ("settlement_recovery_action", "take") => {
                        payload.attachment_ids.windows(2).all(|pair| pair[0] < pair[1])
                    }
                    _ => false,
                }
        }
        _ => false,
    };
    // attachment_ids是外部EID，不是equipment UUID；按十进制值比较，兼容既有数字字符串。
    valid_operation
        && (payload.world != claim.equipment_world.as_str()
            || !payload.attachment_ids.iter().any(|eid| {
                eid.trim_start_matches('0') == claim.external_equipment_id.trim_start_matches('0')
            }))
}

pub(super) fn valid_attachment_origin(
    payload: &AttachmentPayload,
    origin: &str,
    origin_id: &str,
    command_id: Uuid,
    forum_notice_owner: bool,
) -> bool {
    if payload.template.as_deref() == Some("forum_auction_notice") {
        // 完整origin已按真实notify账本重建；不能只凭forum:前缀或模板进入无装备集合。
        return forum_notice_owner
            && origin == "mail_intake"
            && !command_id.is_nil()
            && origin_id == payload.external_mail_id;
    }
    match origin {
        "mail_intake" => {
            let legacy_operation = match payload.operation.as_str() {
                "send" => "delivery-notice",
                "return" => "delivery-return",
                "take" => "delivery-take",
                _ => return false,
            };
            origin_id
                == format!("{}:{}:{}", payload.world, payload.external_mail_id, payload.operation)
                || origin_id
                    == format!("{}:{}:{legacy_operation}", payload.world, payload.external_mail_id)
        }
        // 付款提醒的固定 main 世界由模板表核对；领取动作沿用 recovery action 的目标世界。
        "settlement_recovery_action" => {
            origin_id.strip_prefix("recovery:").is_some_and(canonical_uuid)
        }
        "operations_alert" => {
            payload.external_mail_id.strip_prefix("operations-alert:").is_some_and(|batch_id| {
                canonical_uuid(batch_id) && origin_id == format!("operations_alert:{batch_id}")
            })
        }
        "lot_watch" => {
            !command_id.is_nil()
                && origin_id == format!("lot_watch:{command_id}")
                && payload.external_mail_id == command_id.to_string()
        }
        // 零资产玩家通知：结算汇总与换季提醒都不携带装备引用，
        // 也不能被误判成与资金责任有关的附件投递。固定世界由模板表核对。
        "seller_summary" => {
            !command_id.is_nil()
                && origin_id.starts_with("seller_summary:")
                && payload.external_mail_id == command_id.to_string()
        }
        "season_reminder" => {
            origin_id.starts_with("season_reminder:")
                && valid_season_reminder_mail_id(&payload.external_mail_id)
        }
        _ => false,
    }
}

/// 通知只按应用层模板表核对来源、结果码、key 引用、冻结正文与固定世界，与入队校验、发送前
/// 解码同一口径（投稿回执等携带冻结正文的模板因此不会被误判为无法识别）。表外模板与越界字段
/// 都无法证明与目标装备无关，按冲突失败关闭；已退役模板的历史行仍按原形状识别为零资产通知。
fn valid_notification(payload: &AttachmentPayload, origin: &str) -> bool {
    payload.template.as_deref().and_then(MailNotificationTemplate::from_name).is_some_and(
        |template| {
            template.binding().admits_history(MailNotificationFields {
                origin,
                world: &payload.world,
                result_code: payload.result_code.as_deref(),
                user_key_user_id: payload.user_key_user_id,
                body: payload.body.as_deref(),
            })
        },
    )
}

// 调用方先验证数量及单项格式；去重仍按原字符串，不合并带前导零的不同历史身份。
pub(super) fn unique_attachment_ids(ids: &[String]) -> bool {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().all(|id| seen.insert(id.as_str()))
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

#[cfg(test)]
mod tests {
    use auction_application::mail::{MailNotificationTemplate, MailTemplateField};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        AttachmentPayload, assetless_mail_notification, valid_attachment_origin, valid_notification,
    };

    const ORIGINS: [&str; 6] = [
        "mail_intake",
        "settlement_recovery_action",
        "operations_alert",
        "lot_watch",
        "seller_summary",
        "season_reminder",
    ];

    /// 零资产通知载荷；模板相关字段取自模板表。
    fn table_payload(template: MailNotificationTemplate, result_code: &str) -> AttachmentPayload {
        let binding = template.binding();
        AttachmentPayload {
            schema_version: 1,
            operation: "send".to_owned(),
            world: binding.world.unwrap_or("main").to_owned(),
            external_mail_id: "4243".to_owned(),
            template: Some(template.as_str().to_owned()),
            result_code: Some(result_code.to_owned()),
            body: (binding.body == MailTemplateField::Required)
                .then(|| "Frozen notice body".to_owned()),
            user_key_user_id: (binding.user_key == MailTemplateField::Required).then(Uuid::new_v4),
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
        }
    }

    /// 资产冲突排除按模板表识别零资产通知：闭集内每个结果码（含退役模板的历史行）都识别；
    /// 来源、模板名、结果码、key 引用、冻结正文与固定世界任一越界都不能证明与装备无关。
    #[test]
    fn notification_recognition_follows_the_template_binding() {
        for template in MailNotificationTemplate::ALL {
            let binding = template.binding();
            let origin = binding.origin.as_str();
            for code in binding.result_codes {
                let payload = table_payload(template, code);
                assert!(valid_notification(&payload, origin), "{template:?}/{code}");
            }
            let valid = || table_payload(template, binding.result_codes[0]);
            for other in ORIGINS.into_iter().filter(|other| *other != origin) {
                assert!(!valid_notification(&valid(), other), "{template:?}@{other}");
            }
            let mut invalid = Vec::new();
            let mut payload = valid();
            payload.result_code = Some("unknown_result_code".to_owned());
            invalid.push(("unknown result code", payload));
            let mut payload = valid();
            payload.result_code = None;
            invalid.push(("missing result code", payload));
            let mut payload = valid();
            payload.template = Some(template.as_str().to_uppercase());
            invalid.push(("non-canonical template", payload));
            let mut payload = valid();
            payload.user_key_user_id = match binding.user_key {
                MailTemplateField::Required => None,
                MailTemplateField::Forbidden => Some(Uuid::new_v4()),
            };
            invalid.push(("flipped user key", payload));
            let mut payload = valid();
            payload.body = match binding.body {
                MailTemplateField::Required => None,
                MailTemplateField::Forbidden => Some("Injected body".to_owned()),
            };
            invalid.push(("flipped body", payload));
            if binding.user_key == MailTemplateField::Required {
                let mut payload = valid();
                payload.user_key_user_id = Some(Uuid::nil());
                invalid.push(("nil user key", payload));
            }
            if binding.body == MailTemplateField::Required {
                let mut payload = valid();
                payload.body = Some(" padded body ".to_owned());
                invalid.push(("padded body", payload));
            }
            if let Some(world) = binding.world {
                let mut payload = valid();
                payload.world = if world == "main" { "isekai" } else { "main" }.to_owned();
                invalid.push(("other world", payload));
            }
            for (case, payload) in invalid {
                assert!(!valid_notification(&payload, origin), "{template:?}: {case}");
            }
        }
    }

    /// 与 outbox 落库同形的投稿回执载荷；`body` 为冻结正文。
    fn submission_receipt(result_code: &str, body: Option<&str>) -> serde_json::Value {
        json!({
            "schema_version": 1,
            "operation": "send",
            "world": "main",
            "external_mail_id": "4243",
            "template": "auction_submission_receipt",
            "result_code": result_code,
            "user_key_user_id": null,
            "credits": 0,
            "cod": 0,
            "attachment_ids": [],
            "body": body,
        })
    }

    /// 资金责任预览把合法投稿回执当作零资产入站通知；缺正文或结果码越界时不能证明，仍计入阻断。
    #[test]
    fn submission_receipt_is_an_assetless_mail_notification() {
        let command_id = Uuid::new_v4();
        let origin_id = "main:4243:send";
        let receipt = submission_receipt("accepted", Some("Fixture submission receipt."));
        assert!(assetless_mail_notification(&receipt, origin_id, command_id));
        for invalid in [
            submission_receipt("accepted", None),
            submission_receipt("accepted", Some(" padded body ")),
            submission_receipt("registered", Some("Fixture submission receipt.")),
        ] {
            assert!(!assetless_mail_notification(&invalid, origin_id, command_id), "{invalid}");
        }
        let mut keyed = receipt;
        keyed["user_key_user_id"] = json!(Uuid::new_v4());
        assert!(!assetless_mail_notification(&keyed, origin_id, command_id));
    }

    fn season_reminder(external_mail_id: &str) -> AttachmentPayload {
        AttachmentPayload {
            schema_version: 1,
            operation: "send".to_owned(),
            world: "isekai".to_owned(),
            external_mail_id: external_mail_id.to_owned(),
            template: Some("isekai_season_reminder".to_owned()),
            result_code: Some("season_end".to_owned()),
            body: Some("fixture notification body".to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
        }
    }

    /// 换季提醒只有规范的冻结边界才算已验证的零资产通知；其余写法按关联不全失败关闭。
    #[test]
    fn season_reminder_origin_requires_canonical_boundary() {
        let origin_id = "season_reminder:1793523600:alice";
        let canonical = season_reminder("season-end:2026-11-01T09:00:00Z");
        assert!(valid_attachment_origin(
            &canonical,
            "season_reminder",
            origin_id,
            Uuid::new_v4(),
            false
        ));
        assert!(valid_notification(&canonical, "season_reminder"));
        for external_mail_id in [
            "season-end:bogus",
            "season-end:2026-11-01T09:00:00+00:00",
            "season-end:2026-11-01T09:00:00.000Z",
        ] {
            assert!(
                !valid_attachment_origin(
                    &season_reminder(external_mail_id),
                    "season_reminder",
                    origin_id,
                    Uuid::new_v4(),
                    false
                ),
                "{external_mail_id}"
            );
        }
    }
}

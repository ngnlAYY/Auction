//! 普通 outbox 载荷的闭集分类：只有严格解码并能证明与目标装备无关的载荷才可从冲突门排除。

use auction_application::mail::{
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryShape, MailNotificationTemplate,
};
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

impl AttachmentPayload {
    /// 已落库载荷落在统一投递判定内（与入队校验、发送前解码同一口径）。未知 schema、来源、
    /// 操作或模板名都无法证明与目标装备无关，按冲突失败关闭。
    pub(super) fn admitted(&self, origin: &str, origin_id: &str, command_id: Uuid) -> bool {
        let (Some(origin), Some(operation)) = (
            MailDeliveryOrigin::from_name(origin),
            MailDeliveryOperation::from_name(&self.operation),
        ) else {
            return false;
        };
        let template = match self.template.as_deref() {
            Some(name) => match MailNotificationTemplate::from_name(name) {
                Some(template) => Some(template),
                None => return false,
            },
            None => None,
        };
        self.schema_version == 1
            && MailDeliveryShape {
                origin,
                operation,
                command_id,
                origin_id,
                world: &self.world,
                external_mail_id: &self.external_mail_id,
                template,
                result_code: self.result_code.as_deref(),
                user_key_user_id: self.user_key_user_id,
                body: self.body.as_deref(),
                credits: self.credits,
                cod: self.cod,
                attachment_ids: &self.attachment_ids,
            }
            .admits()
    }
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
    payload.operation == "send"
        && auction_application::settlement::valid_external_positive_id(&payload.external_mail_id)
        && payload.admitted("mail_intake", origin_id, command_id)
}

pub(super) fn unrelated_attachment_payload(
    value: &serde_json::Value,
    origin: &str,
    origin_id: &str,
    command_id: Uuid,
    claim: &AssetConflictTarget<'_>,
) -> bool {
    let Ok(payload) = AttachmentPayload::deserialize(value) else {
        return false;
    };
    // attachment_ids是外部EID，不是equipment UUID；按十进制值比较，兼容既有数字字符串。
    payload.admitted(origin, origin_id, command_id)
        && (payload.world != claim.equipment_world.as_str()
            || !payload.attachment_ids.iter().any(|eid| {
                eid.trim_start_matches('0') == claim.external_equipment_id.trim_start_matches('0')
            }))
}

#[cfg(test)]
mod tests {
    use auction_application::mail::{
        MailDeliveryOrigin, MailNotificationField, MailNotificationTemplate,
    };
    use auction_domain::{LotId, SettlementId, SettlementObligationKind, SettlementWorld};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        AssetConflictTarget, AttachmentPayload, assetless_mail_notification,
        unrelated_attachment_payload,
    };

    /// 冲突门目标：main 世界的一件已知装备。
    fn target() -> AssetConflictTarget<'static> {
        AssetConflictTarget {
            kind: SettlementObligationKind::WinnerEquipmentCod,
            obligation_id: SettlementId::from_uuid(Uuid::new_v4()),
            lot_id: LotId::from_uuid(Uuid::new_v4()),
            external_equipment_id: "424242",
            equipment_world: SettlementWorld::Main,
        }
    }

    /// 已落库的普通投递：载荷与 `deliveries` 行上的来源、origin_id、命令 ID。
    struct Stored {
        payload: AttachmentPayload,
        origin: &'static str,
        origin_id: String,
        command_id: Uuid,
    }

    impl Stored {
        fn admitted(&self) -> bool {
            self.payload.admitted(self.origin, &self.origin_id, self.command_id)
        }

        fn json(&self) -> serde_json::Value {
            json!({
                "schema_version": self.payload.schema_version,
                "operation": self.payload.operation,
                "world": self.payload.world,
                "external_mail_id": self.payload.external_mail_id,
                "template": self.payload.template,
                "result_code": self.payload.result_code,
                "body": self.payload.body,
                "user_key_user_id": self.payload.user_key_user_id,
                "credits": self.payload.credits,
                "cod": self.payload.cod,
                "attachment_ids": self.payload.attachment_ids,
            })
        }

        fn unrelated(&self) -> bool {
            unrelated_attachment_payload(
                &self.json(),
                self.origin,
                &self.origin_id,
                self.command_id,
                &target(),
            )
        }
    }

    /// 按来源生成满足来源身份的零资产通知；模板相关字段取自模板表。
    fn table_notification(
        template: MailNotificationTemplate,
        origin: MailDeliveryOrigin,
        result_code: &str,
    ) -> Stored {
        let binding = template.binding();
        let command_id = Uuid::new_v4();
        let (origin_id, external_mail_id) = match origin {
            MailDeliveryOrigin::MailIntake => {
                (format!("{}:4243:send", binding.world.unwrap_or("main")), "4243".to_owned())
            }
            MailDeliveryOrigin::SettlementRecoveryAction => {
                (format!("recovery:{}", Uuid::new_v4()), "4243".to_owned())
            }
            MailDeliveryOrigin::OperationsAlert => {
                let batch_id = Uuid::new_v4();
                (format!("operations_alert:{batch_id}"), format!("operations-alert:{batch_id}"))
            }
            MailDeliveryOrigin::LotWatch => {
                (format!("lot_watch:{command_id}"), command_id.to_string())
            }
            MailDeliveryOrigin::SellerSummary => {
                (format!("seller_summary:{}:alice", Uuid::new_v4()), command_id.to_string())
            }
            MailDeliveryOrigin::SeasonReminder => (
                "season_reminder:1793523600:alice".to_owned(),
                "season-end:2026-11-01T09:00:00Z".to_owned(),
            ),
        };
        Stored {
            payload: AttachmentPayload {
                schema_version: 1,
                operation: "send".to_owned(),
                world: binding.world.unwrap_or("main").to_owned(),
                external_mail_id,
                template: Some(template.as_str().to_owned()),
                result_code: Some(result_code.to_owned()),
                body: (binding.body == MailNotificationField::Required)
                    .then(|| "Frozen notice body".to_owned()),
                user_key_user_id: (binding.user_key == MailNotificationField::Required)
                    .then(Uuid::new_v4),
                credits: 0,
                cod: 0,
                attachment_ids: Vec::new(),
            },
            origin: origin.as_str(),
            origin_id,
            command_id,
        }
    }

    /// 资产动作：来源身份由调用方给出，模板字段全空。
    fn asset_action(
        origin: &'static str,
        operation: &str,
        origin_id: String,
        external_mail_id: &str,
        attachment_ids: &[&str],
    ) -> Stored {
        Stored {
            payload: AttachmentPayload {
                schema_version: 1,
                operation: operation.to_owned(),
                world: "main".to_owned(),
                external_mail_id: external_mail_id.to_owned(),
                template: None,
                result_code: None,
                body: None,
                user_key_user_id: None,
                credits: 0,
                cod: 0,
                attachment_ids: attachment_ids.iter().map(|value| (*value).to_owned()).collect(),
            },
            origin,
            origin_id,
            command_id: Uuid::new_v4(),
        }
    }

    /// 资产冲突排除按模板表识别零资产通知：闭集内每个结果码都识别；
    /// 来源、模板名、结果码、key 引用、冻结正文与固定世界任一越界都不能证明与装备无关。
    #[test]
    fn notification_recognition_follows_the_template_binding() {
        for template in MailNotificationTemplate::ALL {
            let binding = template.binding();
            for code in binding.result_codes {
                let stored = table_notification(template, binding.origin, code);
                assert!(stored.admitted(), "{template:?}/{code}");
                assert!(stored.unrelated(), "{template:?}/{code} 零资产通知与装备无关");
            }
            let code = binding.result_codes[0];
            let valid = || table_notification(template, binding.origin, code);
            for other in
                MailDeliveryOrigin::ALL.into_iter().filter(|other| *other != binding.origin)
            {
                let stored = table_notification(template, other, code);
                assert!(!stored.admitted(), "{template:?}@{other:?}");
            }
            let mut invalid = Vec::new();
            let mut stored = valid();
            stored.payload.result_code = Some("unknown_result_code".to_owned());
            invalid.push(("unknown result code", stored));
            let mut stored = valid();
            stored.payload.result_code = None;
            invalid.push(("missing result code", stored));
            let mut stored = valid();
            stored.payload.template = Some(template.as_str().to_uppercase());
            invalid.push(("non-canonical template", stored));
            let mut stored = valid();
            stored.payload.user_key_user_id = match binding.user_key {
                MailNotificationField::Required => None,
                MailNotificationField::Forbidden => Some(Uuid::new_v4()),
            };
            invalid.push(("flipped user key", stored));
            let mut stored = valid();
            stored.payload.body = match binding.body {
                MailNotificationField::Required => None,
                MailNotificationField::Forbidden => Some("Injected body".to_owned()),
            };
            invalid.push(("flipped body", stored));
            if binding.user_key == MailNotificationField::Required {
                let mut stored = valid();
                stored.payload.user_key_user_id = Some(Uuid::nil());
                invalid.push(("nil user key", stored));
            }
            if binding.body == MailNotificationField::Required {
                let mut stored = valid();
                stored.payload.body = Some(" padded body ".to_owned());
                invalid.push(("padded body", stored));
            }
            if let Some(world) = binding.world {
                let mut stored = valid();
                stored.payload.world = if world == "main" { "isekai" } else { "main" }.to_owned();
                invalid.push(("other world", stored));
            }
            for (case, stored) in invalid {
                assert!(!stored.admitted(), "{template:?}: {case}");
                assert!(!stored.unrelated(), "{template:?}: {case} 必须按冲突失败关闭");
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
        let mut keyed = receipt.clone();
        keyed["user_key_user_id"] = json!(Uuid::new_v4());
        assert!(!assetless_mail_notification(&keyed, origin_id, command_id));
        // 资金预览路径另要求正整数邮件 ID：前导零即便在统一判定内也不能证明。
        let mut leading_zero = receipt;
        leading_zero["external_mail_id"] = json!("04243");
        assert!(!assetless_mail_notification(&leading_zero, "main:04243:send", command_id));
    }

    /// 换季提醒只有规范的冻结边界才算已验证的零资产通知；其余写法按关联不全失败关闭。
    #[test]
    fn season_reminder_origin_requires_canonical_boundary() {
        let canonical = table_notification(
            MailNotificationTemplate::IsekaiSeasonReminder,
            MailDeliveryOrigin::SeasonReminder,
            "season_end",
        );
        assert!(canonical.admitted());
        for external_mail_id in [
            "season-end:bogus",
            "season-end:2026-11-01T09:00:00+00:00",
            "season-end:2026-11-01T09:00:00.000Z",
        ] {
            let mut stored = table_notification(
                MailNotificationTemplate::IsekaiSeasonReminder,
                MailDeliveryOrigin::SeasonReminder,
                "season_end",
            );
            stored.payload.external_mail_id = external_mail_id.to_owned();
            assert!(!stored.admitted(), "{external_mail_id}");
        }
    }

    /// 冲突排除与入队、发送前解码同一口径：非规范 UUID、全零与 33 位附件 ID、超长
    /// external_mail_id 与闭集外的来源 × 操作组合都无法证明与装备无关；同形的规范值可以排除。
    #[test]
    fn conflict_exclusion_rejects_shapes_outside_the_unified_judgement() {
        let recovery_id = Uuid::new_v4();
        let canonical = format!("recovery:{recovery_id}");
        assert!(
            asset_action("settlement_recovery_action", "take", canonical.clone(), "4243", &["7"])
                .unrelated()
        );
        for origin_id in [
            format!("recovery:{}", recovery_id.to_string().to_uppercase()),
            format!("recovery:{}", recovery_id.simple()),
            format!("recovery:{}", Uuid::nil()),
        ] {
            let stored =
                asset_action("settlement_recovery_action", "take", origin_id, "4243", &["7"]);
            assert!(!stored.unrelated(), "{}", stored.origin_id);
        }
        let mut alert = table_notification(
            MailNotificationTemplate::OperationsAlert,
            MailDeliveryOrigin::OperationsAlert,
            "operations_alert",
        );
        assert!(alert.unrelated());
        let upper_batch = Uuid::new_v4().to_string().to_uppercase();
        alert.origin_id = format!("operations_alert:{upper_batch}");
        alert.payload.external_mail_id = format!("operations-alert:{upper_batch}");
        assert!(!alert.unrelated(), "大写批次 UUID 必须按冲突失败关闭");

        let take = |attachments: &[&str]| {
            asset_action("mail_intake", "take", "main:4243:take".to_owned(), "4243", attachments)
        };
        assert!(take(&["007", &"9".repeat(32)]).unrelated());
        for attachments in [&["000"][..], &[&"1".repeat(33)], &["5", "5"]] {
            assert!(!take(attachments).unrelated(), "{attachments:?}");
        }
        let long_mail_id = "7".repeat(257);
        let long = asset_action(
            "mail_intake",
            "return",
            format!("main:{long_mail_id}:return"),
            &long_mail_id,
            &["7"],
        );
        assert!(!long.unrelated(), "超过 256 字节的 external_mail_id 必须按冲突失败关闭");

        let summary = Uuid::new_v4();
        for (origin, operation, origin_id, external_mail_id) in [
            ("settlement_recovery_action", "return", canonical.clone(), "4243".to_owned()),
            (
                "seller_summary",
                "return",
                format!("seller_summary:{summary}:alice"),
                summary.to_string(),
            ),
            (
                "season_reminder",
                "take",
                "season_reminder:1793523600:alice".to_owned(),
                "season-end:2026-11-01T09:00:00Z".to_owned(),
            ),
        ] {
            let stored = asset_action(origin, operation, origin_id, &external_mail_id, &["7"]);
            assert!(!stored.unrelated(), "{origin}/{operation}");
        }
        let unsorted =
            asset_action("settlement_recovery_action", "take", canonical, "4243", &["9", "10"]);
        assert!(!unsorted.unrelated(), "恢复领取附件必须按字符串升序");
    }
}

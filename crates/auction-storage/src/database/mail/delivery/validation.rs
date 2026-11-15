//! MoogleMail delivery 命令的闭集校验与通知载荷边界。

use std::collections::HashSet;

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationFields,
    valid_season_reminder_mail_id,
};
use uuid::Uuid;

pub(super) fn validate_command(command: &MailDeliveryCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || !auction_domain::valid_text(&command.origin_id, 512)
        || !matches!(command.world_id.as_str(), "main" | "isekai")
        || command.external_mail_id.is_empty()
        || command.external_mail_id != command.external_mail_id.trim()
        || command.recipient.is_empty()
        || command.recipient != command.recipient.trim()
        || command.recipient.chars().any(char::is_control)
        || command.credits < 0
        || command.cod < 0
        || command.attachment_ids.len() > 1000
        || command.attachment_ids.iter().any(|value| {
            value.is_empty()
                || value != value.trim()
                || value.len() > 64
                || !value.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(RepositoryError::Failed("MoogleMail delivery 命令无效".to_owned()).into());
    }
    if !matches!(
        command.origin,
        MailDeliveryOrigin::MailIntake
            | MailDeliveryOrigin::SettlementRecoveryAction
            | MailDeliveryOrigin::OperationsAlert
            | MailDeliveryOrigin::LotWatch
            | MailDeliveryOrigin::SellerSummary
            | MailDeliveryOrigin::SeasonReminder
    ) {
        return Err(RepositoryError::Failed("MoogleMail delivery owner 无效".to_owned()).into());
    }
    if command.origin == MailDeliveryOrigin::SettlementRecoveryAction
        && command
            .origin_id
            .strip_prefix("recovery:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
    {
        return Err(
            RepositoryError::Failed("MoogleMail recovery delivery 来源无效".to_owned()).into()
        );
    }
    if command.origin == MailDeliveryOrigin::OperationsAlert
        && command
            .origin_id
            .strip_prefix("operations_alert:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
    {
        return Err(RepositoryError::Failed("operations alert delivery 来源无效".to_owned()).into());
    }
    // 通知的冻结正文由模板表约束（见下方 Send 分支）；退回与领取只允许原有几类来源携带正文。
    if command.operation != MailDeliveryOperation::Send
        && !matches!(
            command.origin,
            MailDeliveryOrigin::OperationsAlert
                | MailDeliveryOrigin::LotWatch
                | MailDeliveryOrigin::SellerSummary
                | MailDeliveryOrigin::SeasonReminder
        )
        && command.notification_body.is_some()
    {
        return Err(RepositoryError::Failed(
            "普通 MoogleMail delivery 不得携带通知正文".to_owned(),
        )
        .into());
    }
    let mut attachment_ids = HashSet::with_capacity(command.attachment_ids.len());
    if command.attachment_ids.iter().any(|value| !attachment_ids.insert(value.as_str())) {
        return Err(RepositoryError::Failed("MoogleMail delivery 附件重复".to_owned()).into());
    }
    match command.operation {
        MailDeliveryOperation::Send => {
            let Some(template) = command.template else {
                return Err(RepositoryError::Failed("MoogleMail 通知模板缺失".to_owned()).into());
            };
            if !command.attachment_ids.is_empty() || command.credits != 0 || command.cod != 0 {
                return Err(RepositoryError::Failed("MoogleMail 通知载荷越界".to_owned()).into());
            }
            // 来源、结果码、登录 key 引用（非 nil）、冻结正文与固定世界只查应用层模板表，和资产
            // 冲突排除、发送前解码同一口径：任一侧单独放宽或收紧，合法通知都会卡在入队（停住
            // 入站邮箱）或在发送前判为无效。
            if !template.binding().admits(MailNotificationFields {
                origin: command.origin.as_str(),
                world: &command.world_id,
                result_code: command.result_code.as_deref(),
                user_key_user_id: command.user_key_user_id,
                body: command.notification_body.as_deref(),
            }) {
                return Err(RepositoryError::Failed(format!(
                    "MoogleMail {} 通知载荷与模板约束不符",
                    template.as_str()
                ))
                .into());
            }
            // origin_id 与 external_mail_id 的形状属于各来源 owner，模板表不覆盖。
            let source_valid = match command.origin {
                MailDeliveryOrigin::SellerSummary => {
                    command.origin_id.starts_with("seller_summary:")
                        && command.external_mail_id == command.command_id.to_string()
                }
                MailDeliveryOrigin::SeasonReminder => {
                    command.origin_id.starts_with("season_reminder:")
                        && valid_season_reminder_mail_id(&command.external_mail_id)
                }
                MailDeliveryOrigin::LotWatch => {
                    command.origin_id == format!("lot_watch:{}", command.command_id)
                        && command.external_mail_id == command.command_id.to_string()
                }
                MailDeliveryOrigin::MailIntake
                | MailDeliveryOrigin::SettlementRecoveryAction
                | MailDeliveryOrigin::OperationsAlert => true,
            };
            if !source_valid {
                return Err(RepositoryError::Failed(format!(
                    "MoogleMail {} 通知来源标识无效",
                    command.origin.as_str()
                ))
                .into());
            }
        }
        MailDeliveryOperation::Return => {
            if command.template.is_some()
                || command.result_code.is_some()
                || command.credits != 0
                || command.cod != 0
                || command.user_key_user_id.is_some()
                || matches!(
                    command.origin,
                    MailDeliveryOrigin::OperationsAlert | MailDeliveryOrigin::LotWatch
                )
            {
                return Err(
                    RepositoryError::Failed("MoogleMail 退回/领取载荷越界".to_owned()).into()
                );
            }
        }
        MailDeliveryOperation::Take => {
            if command.template.is_some()
                || command.result_code.is_some()
                || command.user_key_user_id.is_some()
                || matches!(
                    command.origin,
                    MailDeliveryOrigin::OperationsAlert | MailDeliveryOrigin::LotWatch
                )
            {
                return Err(RepositoryError::Failed("MoogleMail 领取载荷越界".to_owned()).into());
            }
            if command.origin == MailDeliveryOrigin::MailIntake
                && (command.credits != 0 || command.cod != 0)
            {
                return Err(
                    RepositoryError::Failed("MoogleMail 普通领取不得携带金额".to_owned()).into()
                );
            }
            if command.origin == MailDeliveryOrigin::SettlementRecoveryAction
                && command.attachment_ids.windows(2).any(|values| values[0] >= values[1])
            {
                return Err(RepositoryError::Failed(
                    "MoogleMail recovery 领取附件未按稳定顺序排列".to_owned(),
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use auction_application::mail::{
        EquipmentRejectionReason, MailNotificationField, MailNotificationTemplate,
    };
    use auction_domain::CommandId;
    use uuid::Uuid;

    use super::*;

    fn receipt_command(
        template: MailNotificationTemplate,
        result_code: &str,
        body: Option<&str>,
    ) -> MailDeliveryCommand {
        MailDeliveryCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            request_hash: auction_application::core::RequestHash::new([7; 32]),
            origin: MailDeliveryOrigin::MailIntake,
            origin_id: "main:123:send".to_owned(),
            operation: MailDeliveryOperation::Send,
            world_id: "main".to_owned(),
            external_mail_id: "123".to_owned(),
            recipient: "Clo16".to_owned(),
            template: Some(template),
            result_code: Some(result_code.to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: body.map(str::to_owned),
        }
    }

    #[test]
    fn frozen_submission_receipt_body_is_accepted() {
        let body = "Your auction submission has been received.\n\nSession: 42";
        let command = receipt_command(
            MailNotificationTemplate::AuctionSubmissionReceipt,
            "accepted",
            Some(body),
        );
        assert!(validate_command(&command).is_ok());
    }

    #[test]
    fn submission_receipt_requires_accepted_code_and_valid_body() {
        let command = receipt_command(
            MailNotificationTemplate::AuctionSubmissionReceipt,
            "registered",
            Some("Receipt\n"),
        );
        assert!(validate_command(&command).is_err());
        let command =
            receipt_command(MailNotificationTemplate::AuctionSubmissionReceipt, "accepted", None);
        assert!(validate_command(&command).is_err());
    }

    fn key_command(
        template: MailNotificationTemplate,
        result_code: &str,
        user_key_user_id: Option<Uuid>,
    ) -> MailDeliveryCommand {
        let mut command = receipt_command(template, result_code, None);
        command.user_key_user_id = user_key_user_id;
        command
    }

    #[test]
    fn rekey_key_requires_rekeyed_code_and_non_nil_user_key() {
        let user_id = Some(Uuid::new_v4());
        let command = key_command(MailNotificationTemplate::RekeyKey, "rekeyed", user_id);
        assert!(validate_command(&command).is_ok(), "{:?}", validate_command(&command).err());

        for invalid in [
            key_command(MailNotificationTemplate::RekeyKey, "rekeyed", None),
            key_command(MailNotificationTemplate::RekeyKey, "rekeyed", Some(Uuid::nil())),
            key_command(MailNotificationTemplate::RekeyKey, "registered", user_id),
        ] {
            assert!(validate_command(&invalid).is_err(), "{invalid:?}");
        }
        let mut wrong_origin = command;
        wrong_origin.origin = MailDeliveryOrigin::SettlementRecoveryAction;
        wrong_origin.origin_id = format!("recovery:{}", Uuid::new_v4());
        assert!(validate_command(&wrong_origin).is_err());
    }

    #[test]
    fn rekey_result_rejects_user_key_and_unknown_codes() {
        for code in ["blacklisted", "identity_conflict", "not_registered"] {
            let command = key_command(MailNotificationTemplate::RekeyResult, code, None);
            assert!(validate_command(&command).is_ok(), "{code}");
        }
        let with_key = key_command(
            MailNotificationTemplate::RekeyResult,
            "not_registered",
            Some(Uuid::new_v4()),
        );
        assert!(validate_command(&with_key).is_err());
        let rekeyed = key_command(MailNotificationTemplate::RekeyResult, "rekeyed", None);
        assert!(validate_command(&rekeyed).is_err());
    }

    #[test]
    fn register_key_rejects_missing_or_nil_user_key() {
        let command =
            key_command(MailNotificationTemplate::RegisterKey, "registered", Some(Uuid::new_v4()));
        assert!(validate_command(&command).is_ok());
        for user_key_user_id in [None, Some(Uuid::nil())] {
            let command =
                key_command(MailNotificationTemplate::RegisterKey, "recovered", user_key_user_id);
            assert!(validate_command(&command).is_err(), "{user_key_user_id:?}");
        }
    }

    #[test]
    fn other_intake_notices_still_reject_frozen_bodies() {
        let command = receipt_command(
            MailNotificationTemplate::AuctionEquipmentRejected,
            "auction_equipment_rejected",
            Some("Injected body\n"),
        );
        assert!(validate_command(&command).is_err());
    }

    fn player_notification_command(
        origin: MailDeliveryOrigin,
        template: MailNotificationTemplate,
        result_code: &str,
        world: &str,
        origin_id: String,
        external_mail_id: String,
    ) -> MailDeliveryCommand {
        MailDeliveryCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            request_hash: auction_application::core::RequestHash::new([9; 32]),
            origin,
            origin_id,
            operation: MailDeliveryOperation::Send,
            world_id: world.to_owned(),
            external_mail_id,
            recipient: "alice".to_owned(),
            template: Some(template),
            result_code: Some(result_code.to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: Some("Frozen body".to_owned()),
        }
    }

    #[test]
    fn seller_summary_and_season_reminder_payloads_are_accepted() {
        let mut summary = player_notification_command(
            MailDeliveryOrigin::SellerSummary,
            MailNotificationTemplate::AuctionSellerSummary,
            "summary",
            "main",
            "seller_summary:11111111-1111-1111-1111-111111111111:alice".to_owned(),
            String::new(),
        );
        summary.external_mail_id = summary.command_id.to_string();
        assert!(validate_command(&summary).is_ok(), "{:?}", validate_command(&summary).err());

        let reminder = player_notification_command(
            MailDeliveryOrigin::SeasonReminder,
            MailNotificationTemplate::IsekaiSeasonReminder,
            "season_end",
            "isekai",
            "season_reminder:1793523600:alice".to_owned(),
            "season-end:2026-11-01T09:00:00Z".to_owned(),
        );
        assert!(validate_command(&reminder).is_ok(), "{:?}", validate_command(&reminder).err());

        let wrong_world = player_notification_command(
            MailDeliveryOrigin::SeasonReminder,
            MailNotificationTemplate::IsekaiSeasonReminder,
            "season_end",
            "main",
            "season_reminder:1793523600:alice".to_owned(),
            "season-end:2026-11-01T09:00:00Z".to_owned(),
        );
        assert!(validate_command(&wrong_world).is_err());
    }

    #[test]
    fn season_reminder_rejects_non_canonical_boundary() {
        for external_mail_id in [
            "season-end:bogus",
            "season-end:2026-13-45T25:61:61Z",
            "season-end:2026-11-01T09:00:00+00:00",
            "season-end:2026-11-01T09:00:00.000Z",
            "season-end:2026-11-01 09:00:00Z",
        ] {
            let reminder = player_notification_command(
                MailDeliveryOrigin::SeasonReminder,
                MailNotificationTemplate::IsekaiSeasonReminder,
                "season_end",
                "isekai",
                "season_reminder:1793523600:alice".to_owned(),
                external_mail_id.to_owned(),
            );
            assert!(validate_command(&reminder).is_err(), "{external_mail_id}");
        }
    }

    /// 按来源生成满足 origin_id / external_mail_id 形状的通知命令；模板相关字段取自模板表。
    fn table_command(
        template: MailNotificationTemplate,
        origin: MailDeliveryOrigin,
        result_code: &str,
    ) -> MailDeliveryCommand {
        let binding = template.binding();
        let command_id = Uuid::new_v4();
        let (origin_id, external_mail_id) = match origin {
            MailDeliveryOrigin::MailIntake => ("main:4242:send".to_owned(), "4242".to_owned()),
            MailDeliveryOrigin::SettlementRecoveryAction => {
                (format!("recovery:{}", Uuid::new_v4()), "4242".to_owned())
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
        MailDeliveryCommand {
            command_id: CommandId::from_uuid(command_id),
            request_hash: auction_application::core::RequestHash::new([3; 32]),
            origin,
            origin_id,
            operation: MailDeliveryOperation::Send,
            world_id: binding.world.unwrap_or("main").to_owned(),
            external_mail_id,
            recipient: "alice".to_owned(),
            template: Some(template),
            result_code: Some(result_code.to_owned()),
            user_key_user_id: (binding.user_key == MailNotificationField::Required)
                .then(Uuid::new_v4),
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: (binding.body == MailNotificationField::Required)
                .then(|| "Frozen notice body".to_owned()),
        }
    }

    /// 入队校验逐模板按模板表放行：闭集内每个结果码都能入队，来源、结果码、key 引用、冻结正文
    /// 与固定世界任一越界都拒绝。
    #[test]
    fn send_validation_follows_the_template_binding() {
        for template in MailNotificationTemplate::ALL {
            let binding = template.binding();
            for code in binding.result_codes {
                let command = table_command(template, binding.origin, code);
                assert!(
                    validate_command(&command).is_ok(),
                    "{template:?}/{code}: {:?}",
                    validate_command(&command).err()
                );
            }
            let valid = table_command(template, binding.origin, binding.result_codes[0]);
            let mut invalid = MailDeliveryOrigin::ALL
                .into_iter()
                .filter(|origin| *origin != binding.origin)
                .map(|origin| table_command(template, origin, binding.result_codes[0]))
                .collect::<Vec<_>>();
            let mut unknown_code = valid.clone();
            unknown_code.result_code = Some("unknown_result_code".to_owned());
            let mut missing_code = valid.clone();
            missing_code.result_code = None;
            let mut flipped_key = valid.clone();
            flipped_key.user_key_user_id = match binding.user_key {
                MailNotificationField::Required => None,
                MailNotificationField::Forbidden => Some(Uuid::new_v4()),
            };
            let mut flipped_body = valid.clone();
            flipped_body.notification_body = match binding.body {
                MailNotificationField::Required => None,
                MailNotificationField::Forbidden => Some("Injected body".to_owned()),
            };
            invalid.extend([unknown_code, missing_code, flipped_key, flipped_body]);
            if binding.user_key == MailNotificationField::Required {
                let mut nil_key = valid.clone();
                nil_key.user_key_user_id = Some(Uuid::nil());
                invalid.push(nil_key);
            }
            if binding.body == MailNotificationField::Required {
                let mut padded_body = valid.clone();
                padded_body.notification_body = Some(" padded body ".to_owned());
                invalid.push(padded_body);
            }
            if let Some(world) = binding.world {
                let mut other_world = valid.clone();
                other_world.world_id = if world == "main" { "isekai" } else { "main" }.to_owned();
                invalid.push(other_world);
            }
            for command in invalid {
                assert!(validate_command(&command).is_err(), "{template:?} 必须拒绝 {command:?}");
            }
        }
    }

    /// 回归：付款提醒的固定世界曾只在资产冲突排除与发送前解码核对，入队放行 isekai 后只能在
    /// 发送前判为无效载荷。
    #[test]
    fn recovery_payment_reminder_is_bound_to_main() {
        let mut reminder = table_command(
            MailNotificationTemplate::RecoveryPaymentReminder,
            MailDeliveryOrigin::SettlementRecoveryAction,
            "payment_overdue",
        );
        assert!(validate_command(&reminder).is_ok());
        reminder.world_id = "isekai".to_owned();
        assert!(validate_command(&reminder).is_err());
    }

    /// 回归：拒收说明曾经接受任意非空结果码，发送端只能渲染闭集内的原因。
    #[test]
    fn rejection_notices_only_accept_renderable_result_codes() {
        for (template, code) in [
            (MailNotificationTemplate::AuctionAssetsRejected, "auction_no_attachments"),
            (MailNotificationTemplate::AuctionNoAttachments, "auction_assets_rejected"),
            (MailNotificationTemplate::UserAssetsRejected, "registered"),
            (MailNotificationTemplate::AuctionEquipmentRejected, "unknown_reason"),
        ] {
            let command = table_command(template, MailDeliveryOrigin::MailIntake, code);
            assert!(validate_command(&command).is_err(), "{template:?}/{code}");
        }
        for reason in EquipmentRejectionReason::ALL {
            let command = table_command(
                MailNotificationTemplate::AuctionEquipmentRejected,
                MailDeliveryOrigin::MailIntake,
                reason.as_str(),
            );
            assert!(validate_command(&command).is_ok(), "{reason:?}");
        }
    }
}

//! MoogleMail delivery 命令的闭集校验与通知载荷边界。

use std::collections::HashSet;

use auction_application::{
    ApplicationError, MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin,
    RepositoryError, valid_notification_body,
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
    if command.origin == MailDeliveryOrigin::LotWatch
        && command.template != Some(auction_application::MailNotificationTemplate::LotWatch)
    {
        return Err(RepositoryError::Failed("装备标记通知模板无效".to_owned()).into());
    }
    if command.origin == MailDeliveryOrigin::OperationsAlert
        && (command
            .origin_id
            .strip_prefix("operations_alert:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
            || command.notification_body.is_none())
    {
        return Err(RepositoryError::Failed("operations alert delivery 来源无效".to_owned()).into());
    }
    if !matches!(
        command.origin,
        MailDeliveryOrigin::OperationsAlert
            | MailDeliveryOrigin::LotWatch
            | MailDeliveryOrigin::SellerSummary
            | MailDeliveryOrigin::SeasonReminder
    ) && command.notification_body.is_some()
        && !(command.origin == MailDeliveryOrigin::MailIntake
            && command.template
                == Some(auction_application::MailNotificationTemplate::AuctionSubmissionReceipt))
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
            if command.template.is_none()
                || command.result_code.as_deref().is_none_or(str::is_empty)
            {
                return Err(RepositoryError::Failed("MoogleMail 通知模板缺失".to_owned()).into());
            }
            if !command.attachment_ids.is_empty() || command.credits != 0 || command.cod != 0 {
                return Err(RepositoryError::Failed("MoogleMail 通知载荷越界".to_owned()).into());
            }
            match command.template {
                Some(auction_application::MailNotificationTemplate::RegisterKey) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_none()
                        || !matches!(
                            command.result_code.as_deref(),
                            Some("registered" | "recovered")
                        )
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 注册 key 通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RegisterResult) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                        || !matches!(
                            command.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict")
                        )
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 注册结果通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RebindResult) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                        || !matches!(
                            command.result_code.as_deref(),
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
                        return Err(RepositoryError::Failed(
                            "MoogleMail 换绑结果通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RecoveryPaymentReminder) => {
                    if command.origin != MailDeliveryOrigin::SettlementRecoveryAction
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("payment_overdue")
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail recovery 提醒通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::AuctionSubmissionReceipt) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.operation != MailDeliveryOperation::Send
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("accepted")
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 投稿回执载荷无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::AuctionSellerSummary) => {
                    if command.origin != MailDeliveryOrigin::SellerSummary
                        || command.operation != MailDeliveryOperation::Send
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("summary")
                        || command.world_id != "main"
                        || !command.origin_id.starts_with("seller_summary:")
                        || command.external_mail_id != command.command_id.to_string()
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 卖方汇总通知载荷无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::IsekaiSeasonReminder) => {
                    if command.origin != MailDeliveryOrigin::SeasonReminder
                        || command.operation != MailDeliveryOperation::Send
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("season_end")
                        || command.world_id != "isekai"
                        || !command.origin_id.starts_with("season_reminder:")
                        || !command.external_mail_id.starts_with("season-end:")
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 换季提醒通知载荷无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::LotWatch) => {
                    if command.origin != MailDeliveryOrigin::LotWatch
                        || command.operation != MailDeliveryOperation::Send
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("outbid")
                        || command.origin_id != format!("lot_watch:{}", command.command_id)
                        || command.external_mail_id != command.command_id.to_string()
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(
                            RepositoryError::Failed("装备标记通知来源无效".to_owned()).into()
                        );
                    }
                }
                Some(auction_application::MailNotificationTemplate::OperationsAlert) => {
                    if command.origin != MailDeliveryOrigin::OperationsAlert
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("operations_alert")
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(RepositoryError::Failed(
                            "operations alert 通知载荷无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(_) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 普通通知不得携带用户 key 引用".to_owned(),
                        )
                        .into());
                    }
                }
                None => {
                    return Err(
                        RepositoryError::Failed("MoogleMail 通知模板缺失".to_owned()).into()
                    );
                }
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
    use self::parent::*;
    use super as parent;
    use auction_application::MailNotificationTemplate;
    use auction_domain::CommandId;
    use uuid::Uuid;

    fn receipt_command(
        template: MailNotificationTemplate,
        result_code: &str,
        body: Option<&str>,
    ) -> MailDeliveryCommand {
        MailDeliveryCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            request_hash: auction_application::RequestHash::new([7; 32]),
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
            request_hash: auction_application::RequestHash::new([9; 32]),
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
}

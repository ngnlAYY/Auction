//! MoogleMail delivery 命令的闭集校验与通知载荷边界。

use auction_application::{
    ApplicationError, MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin,
    RepositoryError,
};
use uuid::Uuid;

pub(super) fn validate_command(command: &MailDeliveryCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || command.origin_id.is_empty()
        || command.origin_id != command.origin_id.trim()
        || command.origin_id.len() > 512
        || command.origin_id.chars().any(char::is_control)
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
        && (command
            .origin_id
            .strip_prefix("operations_alert:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
            || command.notification_body.is_none())
    {
        return Err(RepositoryError::Failed("operations alert delivery 来源无效".to_owned()).into());
    }
    if command.origin != MailDeliveryOrigin::OperationsAlert && command.notification_body.is_some()
    {
        return Err(RepositoryError::Failed(
            "普通 MoogleMail delivery 不得携带通知正文".to_owned(),
        )
        .into());
    }
    if command
        .attachment_ids
        .iter()
        .enumerate()
        .any(|(index, value)| command.attachment_ids[..index].contains(value))
    {
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
                || command.origin == MailDeliveryOrigin::OperationsAlert
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
                || command.origin == MailDeliveryOrigin::OperationsAlert
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

fn valid_notification_body(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= (16 << 10)
            && value == value.trim()
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

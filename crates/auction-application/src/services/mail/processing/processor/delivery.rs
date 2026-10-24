//! MoogleMail delivery outbox 的无密文编排。

use super::super::commands::{decision, delivery_request_hash, stable_command_id};
use super::*;

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    pub(super) async fn reject_with_return_and_notice(
        &self,
        message: &MailMessage,
        attachment_ids: Vec<String>,
        template: MailNotificationTemplate,
        decision_code: &str,
        reason_code: &str,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let attachment_count = attachment_ids.len();
        let return_id = self.enqueue_return(message, attachment_ids).await?;
        let notice_id = self.enqueue_notice(message, template, reason_code, None).await?;
        decision(
            "equipment",
            "rejected",
            decision_code,
            json!({
                "attachment_count": attachment_count,
                "delivery_ids": [return_id, notice_id],
                "reason_code": reason_code,
            }),
        )
    }

    pub(super) async fn enqueue_return(
        &self,
        message: &MailMessage,
        attachment_ids: Vec<String>,
    ) -> Result<Uuid, ApplicationError> {
        self.enqueue_delivery(
            message,
            MailDeliveryOperation::Return,
            None,
            None,
            None,
            attachment_ids,
        )
        .await
    }

    pub(super) async fn enqueue_take(
        &self,
        message: &MailMessage,
        attachment_ids: Vec<String>,
    ) -> Result<Uuid, ApplicationError> {
        self.enqueue_delivery(
            message,
            MailDeliveryOperation::Take,
            None,
            None,
            None,
            attachment_ids,
        )
        .await
    }

    pub(super) async fn enqueue_notice(
        &self,
        message: &MailMessage,
        template: MailNotificationTemplate,
        result_code: &str,
        user_key_user_id: Option<Uuid>,
    ) -> Result<Uuid, ApplicationError> {
        self.enqueue_delivery(
            message,
            MailDeliveryOperation::Send,
            Some(template),
            Some(result_code.to_owned()),
            user_key_user_id,
            Vec::new(),
        )
        .await
    }

    async fn enqueue_delivery(
        &self,
        message: &MailMessage,
        operation: MailDeliveryOperation,
        template: Option<MailNotificationTemplate>,
        result_code: Option<String>,
        user_key_user_id: Option<Uuid>,
        attachment_ids: Vec<String>,
    ) -> Result<Uuid, ApplicationError> {
        let command_operation = match operation {
            MailDeliveryOperation::Return => DELIVERY_RETURN_OPERATION,
            MailDeliveryOperation::Take => DELIVERY_TAKE_OPERATION,
            MailDeliveryOperation::Send => DELIVERY_NOTICE_OPERATION,
        };
        let suffix = template.map_or_else(
            || command_operation.to_owned(),
            |value| format!("{command_operation}:{}", value.as_str()),
        );
        let command_id = stable_command_id(message, &suffix);
        let request_hash = delivery_request_hash(
            command_id,
            message,
            operation,
            template,
            result_code.as_deref(),
            user_key_user_id,
            &attachment_ids,
        )?;
        let result: MailDeliveryResult = self
            .deliveries
            .enqueue(MailDeliveryCommand {
                command_id,
                request_hash,
                origin: MailDeliveryOrigin::MailIntake,
                origin_id: format!(
                    "{}:{}:{}",
                    message.world_id,
                    message.external_mail_id,
                    operation.as_str()
                ),
                operation,
                world_id: message.world_id.clone(),
                external_mail_id: message.external_mail_id.clone(),
                recipient: message.sender.clone(),
                template,
                result_code,
                user_key_user_id,
                credits: 0,
                cod: 0,
                attachment_ids,
                notification_body: None,
            })
            .await?;
        if result.delivery_id.is_nil() {
            return Err(MailProcessingError::OwnerResultInvalid.into());
        }
        Ok(result.delivery_id)
    }
}

//! MoogleMail delivery outbox 的无密文编排。

use super::super::commands::{decision, delivery_request_hash, stable_command_id};
use super::{
    DELIVERY_NOTICE_OPERATION, DELIVERY_RETURN_OPERATION, DELIVERY_TAKE_OPERATION,
    MailProcessorService,
};
use crate::errors::ApplicationError;
use crate::ports::core::Clock;
use crate::ports::mail::{
    MailAuctionIntakePort, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailDeliveryResult, MailEquipmentReader, MailMessage, MailNoticeEnqueue,
    MailNoticeLimit, MailNotificationTemplate, MailProcessingDecision, MailProcessingError,
    MailUserPort,
};
use crate::ports::observability::external_metrics::{MailNoticeClass, MailWorld};
use serde_json::json;
use uuid::Uuid;

/// 账号回信模板：注册与轮换各自独立计数，但同一窗口内只回一次。
pub(super) const REGISTER_REPLY_TEMPLATES: &[&str] = &["register_key", "register_result"];
pub(super) const REKEY_REPLY_TEMPLATES: &[&str] = &["rekey_key", "rekey_result"];

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
        let notice_id = self.enqueue_explanation(message, template, reason_code).await?;
        let mut delivery_ids = vec![return_id];
        delivery_ids.extend(notice_id);
        decision(
            "equipment",
            "rejected",
            decision_code,
            json!({
                "attachment_count": attachment_count,
                "delivery_ids": delivery_ids,
                "reason_code": reason_code,
                "notice_suppressed": notice_id.is_none(),
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

    /// 投稿接收回执：整封投稿合并一封，正文由已提交事实冻结。
    ///
    /// 回执只是通知，不能改变已提交的接受决定；场次已开拍、正文超出预算或投稿事实缺失时
    /// 直接跳过，不发旧场次回执，也不补造内容。重复处理由稳定命令 ID 去重。
    pub(super) async fn enqueue_submission_receipt(
        &self,
        message: &MailMessage,
        submission_id: Uuid,
    ) -> Result<Option<Uuid>, ApplicationError> {
        let Some(facts) = self.auctions.load_submission_receipt(submission_id).await? else {
            return Ok(None);
        };
        if crate::services::mail::submission_receipt_invalid_reason(&facts.auction_status).is_some()
        {
            return Ok(None);
        }
        let Some(body) = crate::services::mail::submission_receipt_body(&facts) else {
            return Ok(None);
        };
        let command = self.delivery_command(
            message,
            MailDeliveryOperation::Send,
            Some(MailNotificationTemplate::AuctionSubmissionReceipt),
            Some("accepted".to_owned()),
            None,
            Vec::new(),
            Some(body),
        )?;
        let result = self.deliveries.enqueue(command).await?;
        valid_delivery_id(result).map(Some)
    }

    /// 账号回信（注册/轮换）：同一账号在窗口内只寄一封，超出的仍保留命令与审计。
    pub(super) async fn enqueue_account_notice(
        &self,
        message: &MailMessage,
        template: MailNotificationTemplate,
        result_code: &str,
        user_key_user_id: Option<Uuid>,
        templates: &'static [&'static str],
    ) -> Result<Option<Uuid>, ApplicationError> {
        let policy = self.reply_policy();
        let limit = MailNoticeLimit {
            templates,
            since: self.clock.now()
                - chrono::Duration::seconds(bounded_window_seconds(
                    policy.account_reply_window_seconds,
                )),
            maximum: 1,
        };
        let command = self.notice_command(message, template, result_code, user_key_user_id)?;
        match self.deliveries.enqueue_within_limit(command, limit).await? {
            MailNoticeEnqueue::Enqueued(result) => Ok(Some(valid_delivery_id(result)?)),
            MailNoticeEnqueue::Suppressed => {
                self.observe_suppression(message, MailNoticeClass::AccountReply);
                Ok(None)
            }
        }
    }

    /// 说明信：按发件人/论坛账号共用一个 24 小时窗口，附带资产仍由调用方独立退回。
    pub(super) async fn enqueue_explanation(
        &self,
        message: &MailMessage,
        template: MailNotificationTemplate,
        result_code: &str,
    ) -> Result<Option<Uuid>, ApplicationError> {
        let policy = self.reply_policy();
        let limit = crate::ports::mail::explanation_notice_limit(
            policy.explanation_mail_window_seconds,
            policy.explanation_mail_limit,
            self.clock.now(),
        );
        let command = self.notice_command(message, template, result_code, None)?;
        match self.deliveries.enqueue_within_limit(command, limit).await? {
            MailNoticeEnqueue::Enqueued(result) => Ok(Some(valid_delivery_id(result)?)),
            MailNoticeEnqueue::Suppressed => {
                self.observe_suppression(message, MailNoticeClass::Explanation);
                Ok(None)
            }
        }
    }

    fn notice_command(
        &self,
        message: &MailMessage,
        template: MailNotificationTemplate,
        result_code: &str,
        user_key_user_id: Option<Uuid>,
    ) -> Result<MailDeliveryCommand, ApplicationError> {
        self.delivery_command(
            message,
            MailDeliveryOperation::Send,
            Some(template),
            Some(result_code.to_owned()),
            user_key_user_id,
            Vec::new(),
            None,
        )
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
        let command = self.delivery_command(
            message,
            operation,
            template,
            result_code,
            user_key_user_id,
            attachment_ids,
            None,
        )?;
        let result = self.deliveries.enqueue(command).await?;
        valid_delivery_id(result)
    }

    /// `notification_body` 只用于已冻结正文的通知（投稿回执）；其他通知必须为 `None`。
    // 入队参数逐项对应冻结命令字段，拆容器会模糊必填/可选边界。
    #[allow(clippy::too_many_arguments)]
    fn delivery_command(
        &self,
        message: &MailMessage,
        operation: MailDeliveryOperation,
        template: Option<MailNotificationTemplate>,
        result_code: Option<String>,
        user_key_user_id: Option<Uuid>,
        attachment_ids: Vec<String>,
        notification_body: Option<String>,
    ) -> Result<MailDeliveryCommand, ApplicationError> {
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
        Ok(MailDeliveryCommand {
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
            notification_body,
        })
    }

    fn observe_suppression(&self, message: &MailMessage, class: MailNoticeClass) {
        let world = if message.world_id == "main" { MailWorld::Main } else { MailWorld::Isekai };
        self.metrics.observe(
            crate::ports::observability::external_metrics::ExternalMetricsEvent::MailNoticeSuppressed {
                world,
                class,
            },
        );
    }
}

fn valid_delivery_id(result: MailDeliveryResult) -> Result<Uuid, ApplicationError> {
    if result.delivery_id.is_nil() {
        return Err(MailProcessingError::OwnerResultInvalid.into());
    }
    Ok(result.delivery_id)
}

/// 配置窗口已经以秒为单位校验；转换到 chrono 前再限制一次，避免超界构造 panic。
fn bounded_window_seconds(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(604_800).clamp(60, 604_800)
}

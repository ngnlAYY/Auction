//! MoogleMail subject 路由、注册/换绑命令和未知邮件处理。

use super::super::commands::{
    decision, extract_login_key, has_user_assets, settlement_return_obligation_id,
    stable_command_id, user_request_hash,
};
use super::MailProcessorService;
use super::delivery::{REGISTER_REPLY_TEMPLATES, REKEY_REPLY_TEMPLATES};
use crate::errors::ApplicationError;
use crate::ports::core::Clock;
use crate::ports::mail::{
    MailAuctionIntakePort, MailDeliveryEnqueuer, MailEquipmentReader, MailMessage,
    MailNotificationTemplate, MailProcessingDecision, MailProcessingError, MailUserCommandKind,
    MailUserPort, MailUserRebindCommand, MailUserRegisterCommand, MailUserRekeyCommand,
    MailUserResultCode,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    pub(super) async fn process_message(
        &self,
        message: MailMessage,
        pending_equipment_diagnostic: &mut Option<
            crate::ports::mail::PendingMailEquipmentDiagnostic,
        >,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        message.validate()?;
        if !self.world_ids.iter().any(|world_id| world_id == &message.world_id) {
            return Err(MailProcessingError::InvalidConfiguration.into());
        }
        // 标的提交与领取入队之间可能发生取消；接受事实优先于当前远端内容，禁止重试时
        // 因页面暂时不可读或附件已被领取而改判为退回。
        if let Some(decision) = self.resume_accepted_auction(&message).await? {
            return Ok(decision);
        }
        // Users 的创建/更新时刻只来自本地处理时钟；远端邮件时间可能偏快，不能污染账号元数据。
        // 装备投稿仍保留既有接收时间门禁，不在本修复中改变其排期语义。
        let now = self.clock.now();
        // 旧版使用 Unicode CaseFold 路由，但刻意不裁剪标题首尾空白；保持这一边界可以
        // 兼容合法的大小写变体，同时不把带隐藏空白的未知标题误认成业务命令。
        let routing_subject = message.subject.to_ascii_lowercase();
        match routing_subject.as_str() {
            "auction" => {
                let intake_now = now.max(message.received_at);
                self.process_auction(message, intake_now, pending_equipment_diagnostic).await
            }
            "register" => self.process_register(message, now).await,
            "rebind" => self.process_rebind(message, now).await,
            "rekey" if self.rekey_enabled => self.process_rekey(message, now).await,
            subject => match settlement_return_obligation_id(subject) {
                Some(obligation_id) => self.process_settlement_return(message, obligation_id),
                None => self.process_unknown(message).await,
            },
        }
    }

    /// 将结算退回邮件转换为严格的 recovery 证据。
    /// 退回邮件本身不再创建第二个 delivery 动作；它只把 obligation、资产和金额事实写入
    /// `inbound_mail`。后续 recovery Worker 会在 Serializable 事务中重新比对这份证据，并决定
    /// 是否创建领取/取消 action，避免在邮件解析器里直接改变 recovery 状态。
    fn process_settlement_return(
        &self,
        message: MailMessage,
        obligation_id: Uuid,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !message.claimable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        decision(
            "delivery",
            "accepted",
            "settlement_recovery_returned",
            json!({
                "obligation_id": obligation_id.to_string(),
                "credits": message.credits,
                "cod": message.cod,
                "attachment_ids": message.sorted_attachment_ids(),
            }),
        )
    }

    async fn process_register(
        &self,
        message: MailMessage,
        now: DateTime<Utc>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "registration").await;
        }
        let command_id = stable_command_id(&message, "user-register");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Register)?;
        let result = self
            .users
            .register_or_recover(MailUserRegisterCommand {
                command_id,
                request_hash,
                username: message.sender.clone(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Register)?;
        let (status, template, user_key_user_id) = match result.code {
            MailUserResultCode::Registered | MailUserResultCode::Recovered => {
                ("accepted", MailNotificationTemplate::RegisterKey, result.user_id)
            }
            MailUserResultCode::Blacklisted | MailUserResultCode::IdentityConflict => {
                ("rejected", MailNotificationTemplate::RegisterResult, None)
            }
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        // 决策 53：同一账号 24 小时内只回一封信；超出仍记录命令与审计，不再寄出。
        let delivery_id = self
            .enqueue_account_notice(
                &message,
                template,
                result.code.as_str(),
                user_key_user_id,
                REGISTER_REPLY_TEMPLATES,
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(delivery_id);
        decision(
            "registration",
            status,
            if status == "accepted" { "registration_accepted" } else { "registration_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": delivery_id.is_none(),
            }),
        )
    }

    /// `rekey` 不带资产：轮换登录 key、注销全部有效会话，并在 24 小时预算内回信新 key。
    async fn process_rekey(
        &self,
        message: MailMessage,
        now: DateTime<Utc>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "rekey").await;
        }
        let command_id = stable_command_id(&message, "user-rekey");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Rekey)?;
        let result = self
            .users
            .rekey_login_key(MailUserRekeyCommand {
                command_id,
                request_hash,
                username: message.sender.clone(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Rekey)?;
        let (status, template, user_key_user_id) = match result.code {
            MailUserResultCode::Rekeyed => {
                ("accepted", MailNotificationTemplate::RekeyKey, result.user_id)
            }
            MailUserResultCode::Blacklisted
            | MailUserResultCode::IdentityConflict
            | MailUserResultCode::NotRegistered => {
                ("rejected", MailNotificationTemplate::RekeyResult, None)
            }
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        let delivery_id = self
            .enqueue_account_notice(
                &message,
                template,
                result.code.as_str(),
                user_key_user_id,
                REKEY_REPLY_TEMPLATES,
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(delivery_id);
        decision(
            "rekey",
            status,
            if status == "accepted" { "rekey_accepted" } else { "rekey_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": delivery_id.is_none(),
            }),
        )
    }

    async fn process_rebind(
        &self,
        message: MailMessage,
        now: DateTime<Utc>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "rebind").await;
        }
        let old_key = match extract_login_key(&message.body) {
            Ok(key) => key,
            Err(result_code) => {
                let notice_id = self
                    .enqueue_explanation(
                        &message,
                        MailNotificationTemplate::RebindResult,
                        result_code,
                    )
                    .await?;
                let mut delivery_ids = Vec::new();
                delivery_ids.extend(notice_id);
                return decision(
                    "rebind",
                    "rejected",
                    "rebind_rejected",
                    json!({
                        "owner_result_code": result_code,
                        "delivery_ids": delivery_ids,
                        "notice_suppressed": notice_id.is_none(),
                    }),
                );
            }
        };
        let command_id = stable_command_id(&message, "user-rebind");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Rebind)?;
        let old_key_hash = old_key.public_key_hash();
        let result = self
            .users
            .rebind_username(MailUserRebindCommand {
                command_id,
                request_hash,
                old_key_hash,
                next_username: message.sender.clone(),
                history_id: Uuid::new_v4(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Rebind)?;
        let status = match result.code {
            MailUserResultCode::Rebound | MailUserResultCode::Unchanged => "accepted",
            MailUserResultCode::Blacklisted
            | MailUserResultCode::IdentityConflict
            | MailUserResultCode::InvalidKey => "rejected",
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        let notice_id = self
            .enqueue_explanation(
                &message,
                MailNotificationTemplate::RebindResult,
                result.code.as_str(),
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(notice_id);
        decision(
            "rebind",
            status,
            if status == "accepted" { "rebind_accepted" } else { "rebind_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": notice_id.is_none(),
            }),
        )
    }

    async fn process_unknown(
        &self,
        message: MailMessage,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !has_user_assets(&message) {
            return decision("unknown", "accepted", "archived", json!({}));
        }
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let attachment_ids = message.sorted_attachment_ids();
        let attachment_count = attachment_ids.len();
        let delivery_id = self.enqueue_return(&message, attachment_ids).await?;
        decision(
            "unknown",
            "rejected",
            "unknown_assets_rejected",
            json!({ "attachment_count": attachment_count, "delivery_ids": [delivery_id] }),
        )
    }

    async fn reject_user_assets(
        &self,
        message: &MailMessage,
        mail_kind: &str,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let attachment_ids = message.sorted_attachment_ids();
        let attachment_count = attachment_ids.len();
        let return_id = self.enqueue_return(message, attachment_ids).await?;
        // 说明信受发件人预算限制；资产仍照常退回给发件人。
        let notice_id = self
            .enqueue_explanation(
                message,
                MailNotificationTemplate::UserAssetsRejected,
                "user_assets_rejected",
            )
            .await?;
        let mut delivery_ids = vec![return_id];
        delivery_ids.extend(notice_id);
        decision(
            mail_kind,
            "rejected",
            "user_assets_rejected",
            json!({
                "attachment_count": attachment_count,
                "delivery_ids": delivery_ids,
                "notice_suppressed": notice_id.is_none(),
            }),
        )
    }
}

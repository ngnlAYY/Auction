//! MoogleMail 业务路由、用户命令和 delivery 编排。

use super::commands::{
    auction_request_hash, decision, delivery_request_hash, extract_login_key, has_user_assets,
    settlement_return_obligation_id, settlement_return_subject, stable_command_id,
    user_request_hash,
};
use super::pricing::resolve_start_price_plan;
use super::*;

const AUCTION_OPERATION: &str = "auction-ingest";
const DELIVERY_RETURN_OPERATION: &str = "delivery-return";
const DELIVERY_TAKE_OPERATION: &str = "delivery-take";
const DELIVERY_NOTICE_OPERATION: &str = "delivery-notice";

/// MoogleMail 业务处理器配置。
#[derive(Debug, Clone)]
pub struct MailProcessorConfig {
    pub start_price: i64,
    pub equipment_filter: MailEquipmentFilter,
}

/// MoogleMail 业务处理器。
///
/// 拍卖、注册和换绑命令都先交给对应 owner，再通过无密文 delivery outbox 安排外部动作；
/// 结算退回邮件只记录可审计证据，由 Recovery/Settlement owner 后续消费。
pub struct MailProcessorService<E, A, D, U, C> {
    world_ids: Vec<String>,
    start_price: i64,
    equipment_filter: MailEquipmentFilter,
    equipment: Arc<E>,
    auctions: Arc<A>,
    deliveries: Arc<D>,
    users: Arc<U>,
    clock: Arc<C>,
}

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    /// 建立单世界 MoogleMail 业务处理器；保留该入口兼容单世界装配。
    pub fn new(
        world_id: String,
        config: MailProcessorConfig,
        equipment: Arc<E>,
        auctions: Arc<A>,
        deliveries: Arc<D>,
        users: Arc<U>,
        clock: Arc<C>,
    ) -> Result<Self, MailProcessingError> {
        Self::new_for_worlds(vec![world_id], config, equipment, auctions, deliveries, users, clock)
    }

    /// 建立可安全处理多个 MoogleMail 世界的共享业务处理器。
    pub fn new_for_worlds(
        world_ids: Vec<String>,
        config: MailProcessorConfig,
        equipment: Arc<E>,
        auctions: Arc<A>,
        deliveries: Arc<D>,
        users: Arc<U>,
        clock: Arc<C>,
    ) -> Result<Self, MailProcessingError> {
        if world_ids.is_empty()
            || world_ids.iter().any(|world_id| !matches!(world_id.as_str(), "main" | "isekai"))
            || world_ids
                .iter()
                .enumerate()
                .any(|(index, world_id)| world_ids[..index].contains(world_id))
            || config.start_price <= 0
            || config.equipment_filter.validate().is_err()
        {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(Self {
            world_ids,
            start_price: config.start_price,
            equipment_filter: config.equipment_filter,
            equipment,
            auctions,
            deliveries,
            users,
            clock,
        })
    }

    async fn process_message(
        &self,
        message: MailMessage,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        message.validate()?;
        if !self.world_ids.iter().any(|world_id| world_id == &message.world_id) {
            return Err(MailProcessingError::InvalidConfiguration.into());
        }
        let now = self.clock.now().max(message.received_at);
        // 旧版使用 Unicode CaseFold 路由，但刻意不裁剪标题首尾空白；保持这一边界可以
        // 兼容合法的大小写变体，同时不把带隐藏空白的未知标题误认成业务命令。
        let routing_subject = message.subject.to_ascii_lowercase();
        match routing_subject.as_str() {
            "auction" => self.process_auction(message, now).await,
            "register" => self.process_register(message, now).await,
            "rebind" => self.process_rebind(message, now).await,
            subject if settlement_return_subject(subject) => {
                let obligation_id = settlement_return_obligation_id(subject)
                    .ok_or(MailProcessingError::InvalidMessage)?;
                self.process_settlement_return(message, obligation_id)
            }
            _ => self.process_unknown(message).await,
        }
    }

    /// 将结算退回邮件转换为严格的 recovery 证据。
    ///
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
                game_player_id: message.sender.clone(),
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
        let delivery_id =
            self.enqueue_notice(&message, template, result.code.as_str(), user_key_user_id).await?;
        decision(
            "registration",
            status,
            if status == "accepted" { "registration_accepted" } else { "registration_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": [delivery_id],
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
                let delivery_id = self
                    .enqueue_notice(
                        &message,
                        MailNotificationTemplate::RebindResult,
                        result_code,
                        None,
                    )
                    .await?;
                return decision(
                    "rebind",
                    "rejected",
                    "rebind_rejected",
                    json!({
                        "owner_result_code": result_code,
                        "delivery_ids": [delivery_id],
                    }),
                );
            }
        };
        let command_id = stable_command_id(&message, "user-rebind");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Rebind)?;
        let old_key_hash = old_key.public_key_hash();
        let result = self
            .users
            .rebind_game_identity(MailUserRebindCommand {
                command_id,
                request_hash,
                old_key_hash,
                next_username: message.sender.clone(),
                next_game_player_id: message.sender.clone(),
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
        let delivery_id = self
            .enqueue_notice(
                &message,
                MailNotificationTemplate::RebindResult,
                result.code.as_str(),
                None,
            )
            .await?;
        decision(
            "rebind",
            status,
            if status == "accepted" { "rebind_accepted" } else { "rebind_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": [delivery_id],
            }),
        )
    }

    async fn process_auction(
        &self,
        mut message: MailMessage,
        now: DateTime<Utc>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        let attachment_ids = message.sorted_attachment_ids();
        if message.credits > 0 || message.cod > 0 {
            return self
                .reject_with_return_and_notice(
                    &message,
                    attachment_ids,
                    MailNotificationTemplate::AuctionAssetsRejected,
                    "auction_assets_rejected",
                    "auction_assets_rejected",
                )
                .await;
        }
        if message.attachments.is_empty() {
            let delivery_id = self
                .enqueue_notice(
                    &message,
                    MailNotificationTemplate::AuctionNoAttachments,
                    "auction_no_attachments",
                    None,
                )
                .await?;
            return decision(
                "equipment",
                "rejected",
                "auction_no_attachments",
                json!({ "attachment_count": 0, "delivery_ids": [delivery_id] }),
            );
        }
        if !message.claimable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }

        let start_prices =
            match resolve_start_price_plan(&message.body, &message.attachments, self.start_price) {
                Ok(prices) => prices,
                Err(reason_code) if message.returnable => {
                    return self
                        .reject_with_return_and_notice(
                            &message,
                            attachment_ids,
                            MailNotificationTemplate::AuctionEquipmentRejected,
                            "auction_equipment_rejected",
                            reason_code,
                        )
                        .await;
                }
                Err(_) => return Err(MailProcessingError::UnsafeDisposition.into()),
            };

        let attachments = std::mem::take(&mut message.attachments);
        let mut items = Vec::with_capacity(attachments.len());
        for (index, attachment) in attachments.into_iter().enumerate() {
            let external_equipment_id = attachment.external_equipment_id.clone();
            let mut snapshot = match self.equipment.read(message.world_id.clone(), attachment).await
            {
                Ok(snapshot) => snapshot,
                Err(_) if message.returnable => {
                    return self
                        .reject_with_return_and_notice(
                            &message,
                            attachment_ids,
                            MailNotificationTemplate::AuctionEquipmentRejected,
                            "auction_equipment_rejected",
                            "auction_equipment_rejected",
                        )
                        .await;
                }
                Err(_) => return Err(MailProcessingError::UnsafeDisposition.into()),
            };
            if snapshot.external_equipment_id != external_equipment_id
                || snapshot.validate_for_mail(now).is_err()
            {
                snapshot.clear_secret();
                if message.returnable {
                    return self
                        .reject_with_return_and_notice(
                            &message,
                            attachment_ids,
                            MailNotificationTemplate::AuctionEquipmentRejected,
                            "auction_equipment_rejected",
                            "auction_equipment_rejected",
                        )
                        .await;
                }
                return Err(MailProcessingError::UnsafeDisposition.into());
            }
            let rejection_reason = self.equipment_filter.evaluate(&snapshot);
            if !rejection_reason.is_empty() {
                snapshot.clear_secret();
                if message.returnable {
                    return self
                        .reject_with_return_and_notice(
                            &message,
                            attachment_ids,
                            MailNotificationTemplate::AuctionEquipmentRejected,
                            "auction_equipment_rejected",
                            rejection_reason,
                        )
                        .await;
                }
                return Err(MailProcessingError::UnsafeDisposition.into());
            }
            items.push(crate::MailAuctionItem {
                external_equipment_id: snapshot.external_equipment_id.clone(),
                name: snapshot.name.clone(),
                category: snapshot.category.clone(),
                level: snapshot.level,
                attributes: snapshot.attributes.clone(),
                source_fingerprint: snapshot.source_fingerprint,
                collected_at: now,
                start_price: start_prices[index],
                key: snapshot.key,
            });
        }

        let aggregate_id = mail_auction_aggregate_id(&message.world_id);
        let command_id = stable_command_id(&message, AUCTION_OPERATION);
        let request_hash = auction_request_hash(&message, aggregate_id, &items)?;
        let result = self
            .auctions
            .ingest_mail(MailAuctionIntakeCommand {
                command_id,
                aggregate_id,
                request_hash,
                world_id: message.world_id.clone(),
                mailbox_key: message.mailbox_key.clone(),
                external_mail_id: message.external_mail_id.clone(),
                seller: message.sender.clone(),
                received_at: message.received_at,
                content_digest: message.content_digest,
                items,
                now,
            })
            .await?;
        result.validate()?;
        let delivery_id = self.enqueue_take(&message, attachment_ids).await?;
        decision(
            "equipment",
            "accepted",
            "auction_accepted",
            json!({
                "auction_id": result.auction_id,
                "submission_id": result.submission_id,
                "added_count": result.added_count,
                "existing_count": result.existing_count,
                "preview_revision": result.preview_revision,
                "attachment_count": result.lot_ids.len(),
                "delivery_ids": [delivery_id],
            }),
        )
    }

    async fn process_unknown(
        &self,
        message: MailMessage,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        let attachment_ids = message.sorted_attachment_ids();
        if attachment_ids.is_empty() {
            return decision("unknown", "accepted", "archived", json!({}));
        }
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let delivery_id = self.enqueue_return(&message, attachment_ids.clone()).await?;
        decision(
            "unknown",
            "rejected",
            "unknown_assets_rejected",
            json!({ "attachment_count": attachment_ids.len(), "delivery_ids": [delivery_id] }),
        )
    }

    async fn reject_with_return_and_notice(
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
        let return_id = self.enqueue_return(message, attachment_ids.clone()).await?;
        let notice_id = self.enqueue_notice(message, template, reason_code, None).await?;
        decision(
            "equipment",
            "rejected",
            decision_code,
            json!({
                "attachment_count": attachment_ids.len(),
                "delivery_ids": [return_id, notice_id],
                "reason_code": reason_code,
            }),
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
        let return_id = self.enqueue_return(message, attachment_ids.clone()).await?;
        let notice_id = self
            .enqueue_notice(
                message,
                MailNotificationTemplate::UserAssetsRejected,
                "user_assets_rejected",
                None,
            )
            .await?;
        decision(
            mail_kind,
            "rejected",
            "user_assets_rejected",
            json!({
                "attachment_count": attachment_ids.len(),
                "delivery_ids": [return_id, notice_id],
            }),
        )
    }

    async fn enqueue_return(
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

    async fn enqueue_take(
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

    async fn enqueue_notice(
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
        let operation_name = match operation {
            MailDeliveryOperation::Return => DELIVERY_RETURN_OPERATION,
            MailDeliveryOperation::Take => DELIVERY_TAKE_OPERATION,
            MailDeliveryOperation::Send => DELIVERY_NOTICE_OPERATION,
        };
        let suffix = template.map_or_else(
            || operation_name.to_owned(),
            |value| format!("{operation_name}:{}", value.as_str()),
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
                    message.world_id, message.external_mail_id, operation_name
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

impl<E, A, D, U, C> MailProcessor for MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader + 'static,
    A: MailAuctionIntakePort + 'static,
    D: MailDeliveryEnqueuer + 'static,
    U: MailUserPort + 'static,
    C: Clock + 'static,
{
    fn process(&self, message: MailMessage) -> ApplicationFuture<'_, MailProcessingDecision> {
        Box::pin(async move { self.process_message(message).await })
    }
}

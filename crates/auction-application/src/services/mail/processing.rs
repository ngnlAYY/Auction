//! MoogleMail 业务处理与拍卖入站用例。
//!
//! 业务处理先完成外部装备读取和安全校验，再调用拍卖 owner，最后为领取/退回/通知安排
//! delivery outbox。拍卖事实与密钥信封由单独的 owner 事务提交；重复处理依靠稳定命令 ID
//! 和请求摘要重放，不依靠内存标记。

use std::{collections::HashMap, sync::Arc};

use auction_domain::{CommandId, EquipmentId};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AdminIntakeError, ApplicationError, ApplicationFuture, Clock, CompetitionTransaction,
    EquipmentKeySealerPort, MailAuctionIntakeCommand, MailAuctionIntakePort,
    MailAuctionIntakeRecord, MailAuctionIntakeResult, MailAuctionIntakeTransaction,
    MailAuctionIntakeWrite, MailAuctionItemWrite, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryResult, MailEquipmentAttachment,
    MailEquipmentFilter, MailEquipmentReader, MailLoginKey, MailMessage, MailNotificationTemplate,
    MailProcessingDecision, MailProcessingError, MailProcessor, MailUserCommandKind, MailUserPort,
    MailUserRebindCommand, MailUserRegisterCommand, MailUserResultCode, StoredCommand,
    TransactionPolicy, TransactionRunner, mail_auction_aggregate_id,
};

const AUCTION_OPERATION: &str = "auction-ingest";
const DELIVERY_RETURN_OPERATION: &str = "delivery-return";
const DELIVERY_TAKE_OPERATION: &str = "delivery-take";
const DELIVERY_NOTICE_OPERATION: &str = "delivery-notice";

/// MoogleMail 拍卖入站应用服务。
pub struct MailAuctionIntakeService<R, K, C> {
    repository: Arc<R>,
    sealer: Arc<K>,
    clock: Arc<C>,
}

impl<R, K, C> MailAuctionIntakeService<R, K, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + MailAuctionIntakeTransaction,
    K: EquipmentKeySealerPort,
    C: Clock,
{
    /// 建立 MoogleMail 拍卖入站用例。
    pub fn new(repository: Arc<R>, sealer: Arc<K>, clock: Arc<C>) -> Self {
        Self { repository, sealer, clock }
    }

    /// 将一封已经完成装备读取的 MoogleMail 原子写入拍卖预览。
    pub async fn ingest_mail(
        &self,
        mut command: MailAuctionIntakeCommand,
    ) -> Result<MailAuctionIntakeResult, ApplicationError> {
        validate_command(&command)?;
        command.now = self.clock.now().max(command.now);
        let expected_aggregate = mail_auction_aggregate_id(&command.world_id);
        if command.aggregate_id != expected_aggregate {
            return Err(MailProcessingError::InvalidAuctionCommand.into());
        }

        if let Some(result) = self.find_replay(&command).await? {
            return Ok(result);
        }

        let mut item_writes = Vec::with_capacity(command.items.len());
        for mut item in command.items.drain(..) {
            let equipment_id = EquipmentId::from_uuid(Uuid::new_v4());
            let key_envelope = self
                .sealer
                .seal(equipment_id, &item.key)
                .map_err(|_: AdminIntakeError| MailProcessingError::KeyUnavailable)?;
            item.clear_secret();
            item_writes.push(MailAuctionItemWrite {
                equipment_id,
                lot_id: Uuid::new_v4(),
                external_equipment_id: item.external_equipment_id,
                name: item.name,
                category: item.category,
                level: item.level,
                attributes: item.attributes,
                collected_at: item.collected_at,
                start_price: item.start_price,
                key_envelope,
            });
        }

        let write = MailAuctionIntakeWrite {
            command_id: command.command_id,
            aggregate_id: command.aggregate_id,
            world_id: command.world_id.clone(),
            mailbox_key: command.mailbox_key.clone(),
            external_mail_id: command.external_mail_id.clone(),
            seller: command.seller.clone(),
            received_at: command.received_at,
            now: command.now,
            submission_id: Uuid::new_v4(),
            items: item_writes,
            request_hash: command.request_hash,
        };
        let command_id = command.command_id;
        let aggregate_id = command.aggregate_id;
        let request_hash = command.request_hash;
        let world_id = command.world_id.clone();
        let mailbox_key = command.mailbox_key.clone();
        let result = self
            .repository
            .run(TransactionPolicy::serializable(), move |transaction| {
                let write = write.clone();
                let world_id = world_id.clone();
                let mailbox_key = mailbox_key.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        return replay_result(stored, aggregate_id, request_hash);
                    }
                    let result = transaction.add_mail_auction_intake(write).await?;
                    result.validate()?;
                    transaction
                        .append_mail_auction_record(MailAuctionIntakeRecord {
                            command_id,
                            world_id,
                            mailbox_key,
                            result: result.clone(),
                        })
                        .await?;
                    Ok(result)
                })
            })
            .await?;
        Ok(result)
    }

    async fn find_replay(
        &self,
        command: &MailAuctionIntakeCommand,
    ) -> Result<Option<MailAuctionIntakeResult>, ApplicationError> {
        let command_id = command.command_id;
        let aggregate_id = command.aggregate_id;
        let request_hash = command.request_hash;
        let stored = self
            .repository
            .run(TransactionPolicy::read_committed(), move |transaction| {
                Box::pin(async move {
                    transaction.find_command(command_id).await.map_err(ApplicationError::from)
                })
            })
            .await?;
        stored.map(|value| replay_result(value, aggregate_id, request_hash)).transpose()
    }
}

impl<R, K, C> MailAuctionIntakePort for MailAuctionIntakeService<R, K, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + MailAuctionIntakeTransaction,
    K: EquipmentKeySealerPort + 'static,
    C: Clock + 'static,
{
    fn ingest_mail(
        &self,
        command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        Box::pin(async move { MailAuctionIntakeService::ingest_mail(self, command).await })
    }
}

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

fn validate_command(command: &MailAuctionIntakeCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.aggregate_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || !matches!(command.world_id.as_str(), "main" | "isekai")
        || !valid_text(&command.mailbox_key, 256)
        || !valid_text(&command.external_mail_id, 512)
        || !valid_text(&command.seller, 256)
        || command.received_at.timestamp_nanos_opt().is_none()
        || command.now < command.received_at
        || command.items.is_empty()
        || command.items.len() > 1000
        || command.content_digest == [0; 32]
    {
        return Err(MailProcessingError::InvalidAuctionCommand.into());
    }
    for (index, item) in command.items.iter().enumerate() {
        if !decimal_identity(&item.external_equipment_id)
            || !valid_text(&item.name, 512)
            || !valid_text(&item.category, 128)
            || item.level.is_some_and(|level| !(0..=100_000).contains(&level))
            || item.source_fingerprint == [0; 32]
            || !item.attributes.is_object()
            || item.collected_at < command.received_at
            || item.collected_at > command.now
            || item.start_price <= 0
            || index > 0
                && command.items[..index]
                    .iter()
                    .any(|previous| previous.external_equipment_id == item.external_equipment_id)
        {
            return Err(MailProcessingError::InvalidAuctionCommand.into());
        }
    }
    Ok(())
}

fn replay_result(
    stored: StoredCommand,
    aggregate_id: Uuid,
    request_hash: crate::RequestHash,
) -> Result<MailAuctionIntakeResult, ApplicationError> {
    if !stored.matches_request("mail_auction_intake", aggregate_id, request_hash)
        || stored.result_code != "accepted"
    {
        return Err(MailProcessingError::CommandConflict.into());
    }
    let result: MailAuctionIntakeResult = serde_json::from_slice(&stored.result_payload)
        .map_err(|_| MailProcessingError::StoredResultCorrupt)?;
    result.validate()?;
    Ok(result)
}

fn decision(
    mail_kind: &str,
    status: &str,
    decision_code: &str,
    metadata: Value,
) -> Result<MailProcessingDecision, ApplicationError> {
    let result = MailProcessingDecision {
        mail_kind: mail_kind.to_owned(),
        status: status.to_owned(),
        decision_code: decision_code.to_owned(),
        metadata,
    };
    result.validate()?;
    Ok(result)
}

fn has_user_assets(message: &MailMessage) -> bool {
    !message.attachments.is_empty() || message.credits > 0 || message.cod > 0
}

fn extract_login_key(body: &str) -> Result<MailLoginKey, &'static str> {
    const MISSING: &str = "missing_key";
    const INVALID: &str = "invalid_key";
    let mut selected = None;
    for token in body.split_whitespace() {
        if token.len() != 43 {
            continue;
        }
        if selected.is_some() {
            return Err(INVALID);
        }
        selected = Some(token);
    }
    let Some(encoded) = selected else {
        return Err(MISSING);
    };
    MailLoginKey::from_encoded(encoded).map_err(|_| INVALID)
}

fn stable_command_id(message: &MailMessage, operation: &str) -> CommandId {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/mooglemail/{operation}/{}/{}/{}",
            message.mailbox_key, message.world_id, message.external_mail_id
        )
        .as_bytes(),
    )
    .into()
}

fn auction_request_hash(
    message: &MailMessage,
    aggregate_id: Uuid,
    items: &[crate::MailAuctionItem],
) -> Result<crate::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        world_id: &'a str,
        mailbox_key: &'a str,
        external_mail_id: &'a str,
        seller: &'a str,
        received_at: DateTime<Utc>,
        content_digest: [u8; 32],
        items: Vec<Item<'a>>,
    }
    #[derive(Serialize)]
    struct Item<'a> {
        external_equipment_id: &'a str,
        name: &'a str,
        category: &'a str,
        level: Option<i32>,
        attributes: &'a Value,
        source_fingerprint: [u8; 32],
        start_price: i64,
    }
    let items = items
        .iter()
        .map(|item| Item {
            external_equipment_id: &item.external_equipment_id,
            name: &item.name,
            category: &item.category,
            level: item.level,
            attributes: &item.attributes,
            source_fingerprint: item.source_fingerprint,
            start_price: item.start_price,
        })
        .collect();
    Ok(crate::RequestHash::from_request(
        "mail_auction_intake",
        aggregate_id,
        &Request {
            world_id: &message.world_id,
            mailbox_key: &message.mailbox_key,
            external_mail_id: &message.external_mail_id,
            seller: &message.sender,
            received_at: message.received_at,
            content_digest: message.content_digest,
            items,
        },
    )?)
}

fn delivery_request_hash(
    command_id: CommandId,
    message: &MailMessage,
    operation: MailDeliveryOperation,
    template: Option<MailNotificationTemplate>,
    result_code: Option<&str>,
    user_key_user_id: Option<Uuid>,
    attachment_ids: &[String],
) -> Result<crate::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        operation: &'static str,
        world_id: &'a str,
        external_mail_id: &'a str,
        recipient: &'a str,
        template: Option<&'static str>,
        result_code: Option<&'a str>,
        user_key_user_id: Option<Uuid>,
        attachment_ids: &'a [String],
    }
    Ok(crate::RequestHash::from_request(
        "mail_delivery",
        command_id.into(),
        &Request {
            operation: operation.as_str(),
            world_id: &message.world_id,
            external_mail_id: &message.external_mail_id,
            recipient: &message.sender,
            template: template.map(MailNotificationTemplate::as_str),
            result_code,
            user_key_user_id,
            attachment_ids,
        },
    )?)
}

fn user_request_hash(
    message: &MailMessage,
    command_id: CommandId,
    kind: MailUserCommandKind,
) -> Result<crate::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        operation: &'static str,
        world_id: &'a str,
        mailbox_key: &'a str,
        external_mail_id: &'a str,
        sender: &'a str,
        content_digest: [u8; 32],
    }
    let operation = match kind {
        MailUserCommandKind::Register => "mail-user-register-v1",
        MailUserCommandKind::Rebind => "mail-user-rebind-v1",
    };
    Ok(crate::RequestHash::from_request(
        operation,
        command_id.into(),
        &Request {
            operation,
            world_id: &message.world_id,
            mailbox_key: &message.mailbox_key,
            external_mail_id: &message.external_mail_id,
            sender: &message.sender,
            content_digest: message.content_digest,
        },
    )?)
}

fn settlement_return_subject(subject: &str) -> bool {
    settlement_return_obligation_id(subject).is_some()
}

fn settlement_return_obligation_id(subject: &str) -> Option<Uuid> {
    let (base, encoded) = subject.rsplit_once(" [")?;
    if !subject.ends_with(']')
        || !(base.eq_ignore_ascii_case("auction equipment delivery")
            || base.eq_ignore_ascii_case("auction payment request"))
    {
        return None;
    }
    let encoded = encoded.strip_suffix(']')?;
    let obligation_id = Uuid::parse_str(encoded).ok()?;
    obligation_id.to_string().eq_ignore_ascii_case(encoded).then_some(obligation_id)
}

/// 严格解析旧版拍卖邮件中的起拍价指令。
///
/// 空正文使用配置默认价；非空正文必须为每个附件提供恰好一条 `URL start amount` 指令。
/// 装备 ID 与 key 会同时和详情页附件交叉校验，避免只凭正文中的可伪造 URL 录入资产。
fn resolve_start_price_plan(
    body: &str,
    attachments: &[MailEquipmentAttachment],
    default_price: i64,
) -> Result<Vec<i64>, &'static str> {
    const INVALID: &str = "start_price_invalid";
    const MISSING: &str = "start_price_missing";
    const DUPLICATE: &str = "start_price_duplicate";
    const IDENTITY_MISMATCH: &str = "start_price_identity_mismatch";
    const NOT_FOUND: &str = "start_price_attachment_not_found";

    if default_price <= 0 || attachments.is_empty() {
        return Err(INVALID);
    }
    let mut index_by_id = HashMap::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        if !decimal_identity(&attachment.external_equipment_id)
            || index_by_id.insert(attachment.external_equipment_id.as_str(), index).is_some()
        {
            return Err(INVALID);
        }
    }
    let mut prices = vec![default_price; attachments.len()];
    if body.is_empty() {
        return Ok(prices);
    }
    let normalized = body.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err(INVALID);
    }
    let mut seen = vec![false; attachments.len()];
    for raw_line in normalized.split('\n') {
        let fields = raw_line.split_whitespace().collect::<Vec<_>>();
        let (target, amount_field) = if attachments.len() == 1
            && fields.len() == 2
            && fields[0].eq_ignore_ascii_case("start")
        {
            (0, fields[1])
        } else if fields.len() == 3 && fields[1].eq_ignore_ascii_case("start") {
            let (equipment_id, equipment_key) =
                parse_start_equipment_reference(fields[0]).ok_or(INVALID)?;
            let target = *index_by_id.get(equipment_id).ok_or(NOT_FOUND)?;
            let matched = attachments[target]
                .key
                .with_bytes(|key| key.eq_ignore_ascii_case(equipment_key.as_bytes()))
                .unwrap_or(false);
            if !matched {
                return Err(IDENTITY_MISMATCH);
            }
            (target, fields[2])
        } else {
            return Err(INVALID);
        };
        if seen[target] {
            return Err(DUPLICATE);
        }
        prices[target] = parse_start_price_amount(amount_field).ok_or(INVALID)?;
        seen[target] = true;
    }
    if seen.iter().any(|found| !found) {
        return Err(MISSING);
    }
    Ok(prices)
}

fn parse_start_equipment_reference(value: &str) -> Option<(&str, &str)> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let without_fragment = value.split_once('#').map_or(value, |(prefix, _)| prefix);
    let path = without_fragment.split_once('?').map_or(without_fragment, |(prefix, _)| prefix);
    let marker = "/equip/";
    let suffix = path.rsplit_once(marker)?.1;
    let mut fields = suffix.split('/');
    let equipment_id = fields.next()?;
    let equipment_key = fields.next()?;
    if fields.next().is_some()
        || !decimal_identity(equipment_id)
        || equipment_key.len() != 10
        || !equipment_key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((equipment_id, equipment_key))
}

fn parse_start_price_amount(value: &str) -> Option<i64> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    let lowered = value.to_ascii_lowercase();
    let (digits, factor) = if let Some(digits) = lowered.strip_suffix('k') {
        (digits, 1_000_i64)
    } else if let Some(digits) = lowered.strip_suffix('m') {
        (digits, 1_000_000_i64)
    } else {
        (lowered.as_str(), 1_i64)
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()?.checked_mul(factor).filter(|amount| *amount > 0)
}

fn decimal_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

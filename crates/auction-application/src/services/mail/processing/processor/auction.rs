//! MoogleMail 装备读取、过滤和拍卖入站编排。

use super::super::commands::{auction_request_hash, decision, stable_command_id};
use super::super::pricing::resolve_start_price_plan;
use super::*;
use crate::{
    AdminIntakeError, MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticContext,
    MailEquipmentDiagnosticStage, MailEquipmentEvidenceStatus, MailEquipmentObservation,
    MailEquipmentReadOutcome,
};

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    pub(super) async fn resume_accepted_auction(
        &self,
        message: &MailMessage,
    ) -> Result<Option<MailProcessingDecision>, ApplicationError> {
        let Some(accepted) = self
            .auctions
            .find_accepted(MailAuctionIntakeLookup {
                command_id: stable_command_id(message, AUCTION_OPERATION),
                world_id: message.world_id.clone(),
                mailbox_key: message.mailbox_key.clone(),
                external_mail_id: message.external_mail_id.clone(),
                seller: message.sender.clone(),
                received_at: message.received_at,
            })
            .await?
        else {
            return Ok(None);
        };
        accepted.validate()?;
        self.finish_accepted_auction(message, accepted.result, accepted.attachment_ids)
            .await
            .map(Some)
    }

    pub(super) async fn process_auction(
        &self,
        mut message: MailMessage,
        now: DateTime<Utc>,
        pending_equipment_diagnostic: &mut Option<crate::PendingMailEquipmentDiagnostic>,
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
                Err(failure) if message.returnable => {
                    let mut result = self
                        .reject_with_return_and_notice(
                            &message,
                            attachment_ids,
                            MailNotificationTemplate::AuctionEquipmentRejected,
                            "auction_equipment_rejected",
                            failure.reason_code,
                        )
                        .await;
                    if let Ok(decision) = &mut result
                        && let Some(metadata) = decision.metadata.as_object_mut()
                    {
                        metadata.insert(
                            "pricing_diagnostic".to_owned(),
                            json!({
                                "detail_code": failure.detail_code,
                                "body_bytes": failure.body_bytes,
                                "token_count": failure.token_count,
                                "field_index": failure.field_index,
                                "attachment_count": failure.attachment_count,
                            }),
                        );
                    }
                    return result;
                }
                Err(_) => return Err(MailProcessingError::UnsafeDisposition.into()),
            };

        let attachments = std::mem::take(&mut message.attachments);
        let mut items = Vec::with_capacity(attachments.len());
        for (index, attachment) in attachments.into_iter().enumerate() {
            let external_equipment_id = attachment.external_equipment_id.clone();
            let context = MailEquipmentDiagnosticContext {
                mailbox_key: message.mailbox_key.clone(),
                world_id: message.world_id.clone(),
                external_mail_id: message.external_mail_id.clone(),
                external_equipment_id: external_equipment_id.clone(),
                attachment_position: u16::try_from(index + 1).unwrap_or(u16::MAX),
            };
            let (read_result, mut observation) =
                match self.equipment.read_diagnostic(context.clone(), attachment).await {
                    MailEquipmentReadOutcome::Loaded { snapshot, observation } => {
                        (Ok(snapshot), observation)
                    }
                    MailEquipmentReadOutcome::Failed { error, observation } => {
                        (Err(error), observation)
                    }
                };
            if observation.context != context {
                // 错误关联的证据不能归入本封邮件；诊断损坏仍不得改变原始读取结果。
                observation.context = context;
                observation.prepared = None;
                observation.source_detail_code = None;
                observation.preparation_status = MailEquipmentEvidenceStatus::PrepareFailed;
                observation.preparation_error = Some("invalid_context".to_owned());
            }
            let mut snapshot = match read_result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    // 只有来源明确无效才安排退回；暂时不可用等错误保留给 Jobs 重试，诊断仍留证。
                    let result = match error {
                        ApplicationError::AdminIntake(AdminIntakeError::InvalidSource) => {
                            if message.returnable {
                                self.reject_with_return_and_notice(
                                    &message,
                                    attachment_ids,
                                    MailNotificationTemplate::AuctionEquipmentRejected,
                                    "auction_equipment_rejected",
                                    "auction_equipment_rejected",
                                )
                                .await
                            } else {
                                Err(MailProcessingError::UnsafeDisposition.into())
                            }
                        }
                        error => Err(error),
                    };
                    let stage = observation.stage;
                    let category = observation.category;
                    return self.defer_equipment_diagnostic(
                        result,
                        observation,
                        stage,
                        category,
                        None,
                        pending_equipment_diagnostic,
                    );
                }
            };
            // 读取可能跨越多个时钟刻度，必须用读取完成后的应用时钟校验来源采集时间。
            // 不截断或钳制来源时间；真实未来时间仍失败关闭，命令原有业务时间与幂等字段不变。
            let validation_issue = snapshot.mail_validation_issue(self.clock.now());
            let identity_mismatch = snapshot.external_equipment_id != external_equipment_id;
            if identity_mismatch || validation_issue.is_some() {
                snapshot.clear_secret();
                let result = if message.returnable {
                    self.reject_with_return_and_notice(
                        &message,
                        attachment_ids,
                        MailNotificationTemplate::AuctionEquipmentRejected,
                        "auction_equipment_rejected",
                        "auction_equipment_rejected",
                    )
                    .await
                } else {
                    Err(MailProcessingError::UnsafeDisposition.into())
                };
                let (category, detail) = if identity_mismatch {
                    (MailEquipmentDiagnosticCategory::IdentityMismatch, None)
                } else {
                    (
                        MailEquipmentDiagnosticCategory::SnapshotInvalid,
                        validation_issue.map(str::to_owned),
                    )
                };
                return self.defer_equipment_diagnostic(
                    result,
                    observation,
                    MailEquipmentDiagnosticStage::Validate,
                    category,
                    detail,
                    pending_equipment_diagnostic,
                );
            }
            let rejection_reason = self.equipment_filter.evaluate(&snapshot);
            if !rejection_reason.is_empty() {
                snapshot.clear_secret();
                let result = if message.returnable {
                    self.reject_with_return_and_notice(
                        &message,
                        attachment_ids,
                        MailNotificationTemplate::AuctionEquipmentRejected,
                        "auction_equipment_rejected",
                        rejection_reason,
                    )
                    .await
                } else {
                    Err(MailProcessingError::UnsafeDisposition.into())
                };
                return self.defer_equipment_diagnostic(
                    result,
                    observation,
                    MailEquipmentDiagnosticStage::Validate,
                    MailEquipmentDiagnosticCategory::FilterRejected,
                    Some(rejection_reason.to_owned()),
                    pending_equipment_diagnostic,
                );
            }
            items.push(crate::MailAuctionItem {
                external_equipment_id: snapshot.external_equipment_id.clone(),
                name: snapshot.name.clone(),
                category: snapshot.category.clone(),
                level: snapshot.level,
                attributes: snapshot.attributes.clone(),
                source_fingerprint: snapshot.source_fingerprint,
                collected_at: snapshot.collected_at,
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
                now: self.clock.now().max(now),
            })
            .await?;
        self.finish_accepted_auction(&message, result, attachment_ids).await
    }

    async fn finish_accepted_auction(
        &self,
        message: &MailMessage,
        result: MailAuctionIntakeResult,
        attachment_ids: Vec<String>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        result.validate()?;
        let delivery_id = self.enqueue_take(message, attachment_ids).await?;
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

    fn defer_equipment_diagnostic(
        &self,
        result: Result<MailProcessingDecision, ApplicationError>,
        observation: MailEquipmentObservation,
        stage: MailEquipmentDiagnosticStage,
        category: MailEquipmentDiagnosticCategory,
        detail_code: Option<String>,
        pending: &mut Option<crate::PendingMailEquipmentDiagnostic>,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        // 这里只构造任务，不轮询留证 future；Worker 会在业务 item_timeout 结束后执行。
        *pending = Some(crate::PendingMailEquipmentDiagnostic::new(self.equipment.retain_failure(
            observation,
            stage,
            category,
            detail_code,
        )));
        result
    }
}

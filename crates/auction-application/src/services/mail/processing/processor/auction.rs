//! MoogleMail 装备读取、过滤和拍卖入站编排。

use super::super::commands::{auction_request_hash, decision, stable_command_id};
use super::super::pricing::resolve_start_price_plan;
use super::*;

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    pub(super) async fn process_auction(
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
}

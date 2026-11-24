//! MoogleMail 拍卖入站命令的原子写入服务。

use std::sync::Arc;

use auction_domain::EquipmentId;
use auction_domain::consignment::ListingEquipmentIdentity;
use uuid::Uuid;

use super::commands::{replay_result, validate_command};
use crate::errors::ApplicationError;
use crate::ports::admin::{AdminIntakeError, EquipmentKeySealerPort};
use crate::ports::core::ApplicationFuture;
use crate::ports::core::competition::canonical_equipment_lock_identity;
use crate::ports::core::{Clock, CompetitionTransaction, TransactionPolicy, TransactionRunner};
use crate::ports::mail::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeRecord, MailAuctionIntakeResult,
    MailAuctionIntakeTransaction, MailAuctionIntakeWrite, MailAuctionItemWrite,
    MailProcessingError, SubmissionReceiptFacts, mail_auction_aggregate_id,
};

pub struct MailAuctionIntakeService<R, K, C> {
    repository: Arc<R>,
    sealer: Arc<K>,
    clock: Arc<C>,
}

/// 写事务前的只读预判：重放、既有投稿续办与实物强占用门控。
struct MailAuctionIntakePreflight {
    replay: Option<MailAuctionIntakeResult>,
    existing_submission: bool,
    equipment_held: bool,
}

impl<R, K, C> MailAuctionIntakeService<R, K, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + MailAuctionIntakeTransaction,
    K: EquipmentKeySealerPort,
    C: Clock,
{
    pub fn new(repository: Arc<R>, sealer: Arc<K>, clock: Arc<C>) -> Self {
        Self { repository, sealer, clock }
    }

    /// 从持久化接受事实恢复原领取资产；不得重新请求远端页面来决定是否退回。
    pub async fn find_accepted(
        &self,
        lookup: MailAuctionIntakeLookup,
    ) -> Result<Option<MailAuctionAcceptance>, ApplicationError> {
        lookup.validate()?;
        let accepted = self
            .repository
            .run(TransactionPolicy::read_committed(), move |transaction| {
                let lookup = lookup.clone();
                Box::pin(async move {
                    transaction
                        .load_mail_auction_acceptance(lookup)
                        .await
                        .map_err(ApplicationError::from)
                })
            })
            .await?;
        if let Some(accepted) = &accepted {
            accepted.validate()?;
        }
        Ok(accepted)
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

        // 不变量 2：同一 `(world, 规范 EID)` 的拍卖建 lot 与寄售强占用共用同一实物锁；
        // 原文带前导零的旧 EID 先按数据库别名规则规范化，无法构造身份的件不取锁。
        let equipment_identities = command
            .items
            .iter()
            .filter_map(|item| {
                canonical_equipment_lock_identity(&command.world_id, &item.external_equipment_id)
            })
            .collect::<Vec<_>>();

        // 勘误 E1 与维护者门控决定：重放和既有投稿续办先于实物锁；只有预判到强占用时才
        // 取租约。预判不是授权，事务内仍会显式检查并进行 lot 守卫。
        let preflight = self.preflight(&command, &equipment_identities).await?;
        if let Some(result) = preflight.replay {
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
        // 无强占用或已有投稿时保持原路径：不取租约、不占出价队列配额。已有投稿故意仍走
        // 写事务，让存储层按既有顺序返回 `Conflict`，与升级前行为一致。
        let result = self
            .repository
            .run_with_equipment_gate(
                preflight.equipment_held && !preflight.existing_submission,
                &equipment_identities,
                TransactionPolicy::serializable(),
                move |transaction| {
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
                },
            )
            .await?;
        Ok(result)
    }

    async fn preflight(
        &self,
        command: &MailAuctionIntakeCommand,
        equipment_identities: &[ListingEquipmentIdentity],
    ) -> Result<MailAuctionIntakePreflight, ApplicationError> {
        let command_id = command.command_id;
        let aggregate_id = command.aggregate_id;
        let request_hash = command.request_hash;
        let mailbox_key = command.mailbox_key.clone();
        let external_mail_id = command.external_mail_id.clone();
        let equipment_identities = equipment_identities.to_vec();
        let (stored, existing_submission, equipment_held) = self
            .repository
            .run(TransactionPolicy::read_committed(), move |transaction| {
                let mailbox_key = mailbox_key.clone();
                let external_mail_id = external_mail_id.clone();
                let equipment_identities = equipment_identities.clone();
                Box::pin(async move {
                    let stored = transaction
                        .find_command(command_id)
                        .await
                        .map_err(ApplicationError::from)?;
                    if stored.is_some() {
                        return Ok::<_, ApplicationError>((stored, false, false));
                    }
                    let existing_submission = transaction
                        .mail_auction_submission_exists(&mailbox_key, &external_mail_id)
                        .await
                        .map_err(ApplicationError::from)?;
                    let equipment_held = if existing_submission {
                        false
                    } else {
                        transaction
                            .any_equipment_held(&equipment_identities)
                            .await
                            .map_err(ApplicationError::from)?
                    };
                    Ok((stored, existing_submission, equipment_held))
                })
            })
            .await?;
        let replay =
            stored.map(|value| replay_result(value, aggregate_id, request_hash)).transpose()?;
        Ok(MailAuctionIntakePreflight { replay, existing_submission, equipment_held })
    }
}

impl<R, K, C> MailAuctionIntakePort for MailAuctionIntakeService<R, K, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + MailAuctionIntakeTransaction,
    K: EquipmentKeySealerPort + 'static,
    C: Clock + 'static,
{
    fn find_accepted(
        &self,
        lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>> {
        Box::pin(async move { MailAuctionIntakeService::find_accepted(self, lookup).await })
    }

    fn ingest_mail(
        &self,
        command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        Box::pin(async move { MailAuctionIntakeService::ingest_mail(self, command).await })
    }

    fn load_submission_receipt(
        &self,
        submission_id: Uuid,
    ) -> ApplicationFuture<'_, Option<SubmissionReceiptFacts>> {
        Box::pin(async move {
            if submission_id.is_nil() {
                return Err(MailProcessingError::InvalidAuctionCommand.into());
            }
            let facts = self
                .repository
                .run(TransactionPolicy::read_committed(), move |transaction| {
                    Box::pin(async move {
                        transaction
                            .load_submission_receipt(submission_id)
                            .await
                            .map_err(ApplicationError::from)
                    })
                })
                .await?;
            if let Some(facts) = &facts {
                facts.validate()?;
            }
            Ok(facts)
        })
    }
}

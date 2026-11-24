//! 隔离解除：在同一顶层事务内复判、写终局与解除，并交还原 owner。

use std::collections::BTreeSet;

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DepositOperation, ReleaseQuarantineCommand,
    ReleaseReceipt,
};
use auction_domain::CommandId;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::deposit::{ReleaseKind, ReleasePlan};
use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use super::disposition::{WorkSeed, create_work_in_tx, insert_final_decision};
use super::identity::{StoredQuarantine, load_quarantine, load_release};
use super::observations::{self, StoredObservation};
use super::reads::{release_facts, release_receipt_from_existing};
use super::{Outcome, conflict, invariant, replay_receipt};
use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::recovery::scheduler::RecoveryHandoff;
use crate::database::settlement::fulfillment::consume_return_for_mail_in_tx;

pub(crate) async fn release_quarantine_port(
    repository: &DepositRepository,
    command: ReleaseQuarantineCommand,
) -> Result<ReleaseReceipt, ConsignmentError> {
    let pool = repository.store().pool();
    let mut pre =
        pool.acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let quarantine = load_quarantine(&mut pre, command.quarantine_id)
        .await
        .map_err(WriteError::into_application)?
        .ok_or(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))?;
    let observation =
        observations::load_observation(&mut pre, &quarantine.mail_key, command.observation_id)
            .await
            .map_err(WriteError::into_application)?
            .ok_or(ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven))?;
    let mapped_seller = super::quarantine::seller_by_name(&mut pre, observation.sender.as_deref())
        .await
        .map_err(WriteError::into_application)?;
    drop(pre);
    let key = quarantine.mail_key.clone();
    let mut seller_set = BTreeSet::new();
    if let Some(seller) = mapped_seller {
        seller_set.insert(seller.as_uuid());
    }
    let sellers: Vec<Uuid> = seller_set.into_iter().collect();
    let world = key.world();
    let equipment: Vec<ListingEquipmentIdentity> = observation
        .items
        .iter()
        .filter_map(|item| {
            item.canonical_eid.as_ref().and_then(|eid| {
                ListingEquipmentIdentity::new(
                    auction_domain::consignment::settlement_world_code(item.world.unwrap_or(world)),
                    eid,
                )
                .ok()
            })
        })
        .collect();
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let mail_keys = [key.clone()];
    let input = command;
    let transaction =
        with_deposit_transaction(repository.store(), keys, &mail_keys, &input, |input, tx| {
            let sellers = sellers.clone();
            let equipment = equipment.clone();
            let key = key.clone();
            Box::pin(async move {
                tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
                let arbitration = tx
                    .arbitrate(
                        input.actor.clone(),
                        DepositOperation::ReleaseQuarantine,
                        DepositTarget::Quarantine(input.quarantine_id),
                        input.request_hash,
                        input.request_id.as_deref(),
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<ReleaseReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => Ok(Outcome::Accepted(receipt)),
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_release_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let now = database_now(writer.connection()).await?;
                let quarantine =
                    load_quarantine(writer.connection(), input.quarantine_id)
                        .await?
                        .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
                let observation =
                    observations::load_observation(writer.connection(), &key, input.observation_id)
                        .await?
                        .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
                if let Some(existing) = load_release(writer.connection(), &key).await? {
                    if existing.kind != input.proposed_kind {
                        return Err(conflict(DepositBusinessRejection::DispositionConflict));
                    }
                    let receipt = release_receipt_from_existing(
                        input.command_id,
                        &quarantine,
                        &existing,
                        writer.connection(),
                    )
                    .await?;
                    let _ = writer.finish(receipt.clone(), now).await?;
                    return Ok(Outcome::Accepted(receipt));
                }
                let linked: Option<i64> = sqlx::query_scalar(
                    "SELECT quarantine_version FROM public.mail_envelope_observations
                     WHERE id = $1 AND quarantine_id = $2",
                )
                .bind(input.observation_id)
                .bind(input.quarantine_id)
                .fetch_optional(writer.connection())
                .await?;
                if linked.and_then(|value| u64::try_from(value).ok()) != Some(quarantine.version)
                    || quarantine.version <= 1
                {
                    return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
                }
                if quarantine.version != input.expected_version {
                    return Err(conflict(DepositBusinessRejection::StaleVersion));
                }
                if !quarantine.active {
                    return Err(conflict(DepositBusinessRejection::DispositionConflict));
                }
                if observation.legacy_auction_owner.is_some()
                    || matches!(
                        input.proposed_kind,
                        ReleaseKind::AuctionReturnCase | ReleaseKind::AuctionRecoveryReturn
                    )
                {
                    let evidence = observations::legacy_return_evidence_for_observation(
                        writer.connection(),
                        observation.observation_id,
                    )
                    .await?;
                    super::release_legacy::ensure_legacy_inbound(
                        writer.connection(),
                        &quarantine,
                        &observation,
                        &evidence,
                        now,
                    )
                    .await?;
                }
                let facts =
                    release_facts(&quarantine, &observation, input, now, true, writer.connection())
                        .await?;
                let plan = auction_domain::consignment::deposit::classify_release(
                    &facts,
                    input.proposed_kind,
                )
                .map_err(|error| conflict(super::domain_rejection(error)))?;
                match plan.kind {
                    ReleaseKind::Accept | ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {
                        release_normal(writer, &quarantine, &observation, &plan, now).await
                    }
                    ReleaseKind::AuctionReturnCase => {
                        release_to_return_case(writer, input, &plan, now).await
                    }
                    ReleaseKind::AuctionRecoveryReturn => {
                        release_to_recovery(writer, input, &plan, now).await
                    }
                    ReleaseKind::AcquireReturn => {
                        Err(conflict(DepositBusinessRejection::EvidenceUnproven))
                    }
                }
            })
        });
    transaction.await.map_err(WriteError::into_application)?.into_result()
}

async fn release_normal(
    mut writer: crate::database::consignment::commands::DepositWriter<'_, '_, '_>,
    quarantine: &StoredQuarantine,
    observation: &StoredObservation,
    plan: &ReleasePlan,
    now: Timestamp,
) -> Result<Outcome<ReleaseReceipt>, WriteError> {
    let command_id = writer.command_id();
    let decision_id = Uuid::new_v4();
    let release_id = Uuid::new_v4();
    let kind = release_db_kind(plan.kind);
    insert_final_decision(
        writer.connection(),
        &quarantine.mail_key,
        decision_id,
        kind,
        observation.observation_id,
        command_id,
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, NULL, NULL, $11)",
    )
    .bind(release_id)
    .bind(quarantine.mail_key.authenticated_bot_uid())
    .bind(auction_domain::consignment::settlement_world_code(quarantine.mail_key.world()))
    .bind(quarantine.mail_key.external_mid())
    .bind(quarantine.quarantine_id)
    .bind(i64::try_from(quarantine.version)?)
    .bind(observation.observation_id)
    .bind(decision_id)
    .bind(kind)
    .bind(command_id)
    .bind(now)
    .execute(writer.connection())
    .await?;
    let released = sqlx::query(
        "UPDATE public.mail_quarantines
         SET state = 'released', version = version + 1, updated_at = $2
         WHERE id = $1 AND state = 'active' AND version = $3",
    )
    .bind(quarantine.quarantine_id)
    .bind(now)
    .bind(i64::try_from(quarantine.version)?)
    .execute(writer.connection())
    .await?;
    if released.rows_affected() != 1 {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    let mut receipt = ReleaseReceipt::released(
        CommandId::from_uuid(command_id),
        quarantine.quarantine_id,
        release_id,
        decision_id,
        plan.kind,
    );
    if plan.kind == ReleaseKind::Accept {
        let action = auction_application::consignment::DispositionAction::Take;
        let (delivery_id, delivery_command_id) =
            super::disposition::delivery_identity(decision_id, action);
        create_work_in_tx(
            writer.connection(),
            &WorkSeed {
                work_id: Uuid::new_v4(),
                delivery_id,
                delivery_command_id,
                decision_id,
                action,
                mail_key: &quarantine.mail_key,
                observation,
                subject: "consign",
                return_manifest: None,
                now,
            },
        )
        .await?;
        let accepted = crate::database::consignment::deposit::accept_items_in_tx(
            &mut writer,
            decision_id,
            &quarantine.mail_key,
            &plan.matched_items,
            now,
        )
        .await?;
        if accepted.len() != plan.matched_items.len() {
            return Err(invariant("consignment_release_accept_count"));
        }
        receipt.listing_changes = writer.listing_changes();
    }
    if plan.kind == ReleaseKind::Return {
        let action = auction_application::consignment::DispositionAction::Return;
        let (delivery_id, delivery_command_id) =
            super::disposition::delivery_identity(decision_id, action);
        create_work_in_tx(
            writer.connection(),
            &WorkSeed {
                work_id: Uuid::new_v4(),
                delivery_id,
                delivery_command_id,
                decision_id,
                action,
                mail_key: &quarantine.mail_key,
                observation,
                subject: "consign",
                return_manifest: plan.return_manifest.as_ref(),
                now,
            },
        )
        .await?;
    }
    let _ = writer.finish(receipt.clone(), now).await?;
    Ok(Outcome::Accepted(receipt))
}

fn release_db_kind(kind: ReleaseKind) -> &'static str {
    match kind {
        ReleaseKind::Accept => "accepted",
        ReleaseKind::Return => "return_decided",
        ReleaseKind::ArchiveEmpty => "archived_empty",
        ReleaseKind::AcquireReturn => "acquire_return",
        ReleaseKind::AuctionReturnCase => "auction_return_case",
        ReleaseKind::AuctionRecoveryReturn => "auction_recovery_return",
    }
}

async fn release_to_return_case(
    mut writer: crate::database::consignment::commands::DepositWriter<'_, '_, '_>,
    command: &ReleaseQuarantineCommand,
    plan: &ReleasePlan,
    now: Timestamp,
) -> Result<Outcome<ReleaseReceipt>, WriteError> {
    let return_mail_id =
        plan.return_mail_id.ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let handoff =
        handoff_return_case_in_tx(writer.tracked(), return_mail_id, command.observation_id, now)
            .await
            .map_err(WriteError::Application)?;
    let quarantine = load_quarantine(writer.connection(), command.quarantine_id)
        .await?
        .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
    let decision_id = Uuid::new_v4();
    let release_id = Uuid::new_v4();
    persist_owner_release(
        &mut writer,
        &quarantine,
        decision_id,
        release_id,
        command.observation_id,
        "auction_return_case",
        Some(return_mail_id),
        Some(handoff.return_evidence_id),
        None,
        now,
    )
    .await?;
    let mut receipt = ReleaseReceipt::released(
        command.command_id,
        command.quarantine_id,
        release_id,
        decision_id,
        ReleaseKind::AuctionReturnCase,
    );
    receipt.obligation_id = handoff.obligation_id;
    receipt.return_mail_id = Some(return_mail_id);
    receipt.return_evidence_id = Some(handoff.return_evidence_id);
    let _ = writer.finish(receipt.clone(), now).await?;
    Ok(Outcome::Accepted(receipt))
}

async fn release_to_recovery(
    mut writer: crate::database::consignment::commands::DepositWriter<'_, '_, '_>,
    command: &ReleaseQuarantineCommand,
    plan: &ReleasePlan,
    now: Timestamp,
) -> Result<Outcome<ReleaseReceipt>, WriteError> {
    let return_mail_id =
        plan.return_mail_id.ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let recovery_id =
        plan.recovery_id.ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let obligation_id =
        plan.obligation_id.ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    let handoff = handoff_recovery_return_in_tx(
        writer.tracked(),
        return_mail_id,
        recovery_id,
        obligation_id,
        command.observation_id,
        now,
    )
    .await
    .map_err(WriteError::Application)?;
    let quarantine = load_quarantine(writer.connection(), command.quarantine_id)
        .await?
        .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
    let decision_id = Uuid::new_v4();
    let release_id = Uuid::new_v4();
    persist_owner_release(
        &mut writer,
        &quarantine,
        decision_id,
        release_id,
        command.observation_id,
        "auction_recovery_return",
        None,
        None,
        Some(handoff.handoff_id),
        now,
    )
    .await?;
    let mut receipt = ReleaseReceipt::released(
        command.command_id,
        command.quarantine_id,
        release_id,
        decision_id,
        ReleaseKind::AuctionRecoveryReturn,
    );
    receipt.obligation_id = Some(handoff.obligation_id);
    receipt.return_mail_id = Some(return_mail_id);
    receipt.recovery_id = Some(recovery_id);
    receipt.recovery_handoff_id = Some(handoff.handoff_id);
    let _ = writer.finish(receipt.clone(), now).await?;
    Ok(Outcome::Accepted(receipt))
}

#[allow(clippy::too_many_arguments, reason = "逐列写同一解除事实")]
async fn persist_owner_release(
    writer: &mut crate::database::consignment::commands::DepositWriter<'_, '_, '_>,
    quarantine: &StoredQuarantine,
    decision_id: Uuid,
    release_id: Uuid,
    observation_id: Uuid,
    kind: &'static str,
    case_return_mail_id: Option<Uuid>,
    case_evidence_id: Option<Uuid>,
    recovery_handoff_id: Option<Uuid>,
    now: Timestamp,
) -> Result<(), WriteError> {
    let command_id = writer.command_id();
    insert_final_decision(
        writer.connection(),
        &quarantine.mail_key,
        decision_id,
        kind,
        observation_id,
        command_id,
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(release_id)
    .bind(quarantine.mail_key.authenticated_bot_uid())
    .bind(auction_domain::consignment::settlement_world_code(quarantine.mail_key.world()))
    .bind(quarantine.mail_key.external_mid())
    .bind(quarantine.quarantine_id)
    .bind(i64::try_from(quarantine.version)?)
    .bind(observation_id)
    .bind(decision_id)
    .bind(kind)
    .bind(command_id)
    .bind(case_return_mail_id)
    .bind(case_evidence_id)
    .bind(recovery_handoff_id)
    .bind(now)
    .execute(writer.connection())
    .await?;
    let released = sqlx::query(
        "UPDATE public.mail_quarantines
         SET state = 'released', version = version + 1, updated_at = $2
         WHERE id = $1 AND state = 'active' AND version = $3",
    )
    .bind(quarantine.quarantine_id)
    .bind(now)
    .bind(i64::try_from(quarantine.version)?)
    .execute(writer.connection())
    .await?;
    if released.rows_affected() != 1 {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    Ok(())
}

/// 交还原拍卖退件案件：复用旧消费者的事务内写入链，返回可空义务与原证据引用。
pub(crate) async fn handoff_return_case_in_tx(
    transaction: &mut TrackedTransaction<'_, Postgres>,
    return_mail_id: Uuid,
    observation_id: Uuid,
    now: Timestamp,
) -> Result<ReturnCaseHandoff, ConsignmentError> {
    if !observation_matches_inbound(transaction, observation_id, return_mail_id)
        .await
        .map_err(WriteError::into_application)?
    {
        return Err(ConsignmentError::Unavailable);
    }
    let consumption = consume_return_for_mail_in_tx(transaction, return_mail_id, now)
        .await
        .map_err(ConsignmentError::from)?;
    Ok(ReturnCaseHandoff {
        obligation_id: consumption.obligation_id,
        return_evidence_id: return_mail_id,
    })
}

/// 交还原拍卖恢复流程：只写唯一接管引用，等待旧消费者在下一事务分类与消费。
pub(crate) async fn handoff_recovery_return_in_tx(
    transaction: &mut TrackedTransaction<'_, Postgres>,
    return_mail_id: Uuid,
    recovery_id: Uuid,
    obligation_id: Uuid,
    observation_id: Uuid,
    now: Timestamp,
) -> Result<RecoveryHandoff, ConsignmentError> {
    crate::database::recovery::scheduler::handoff_recovery_return_in_tx(
        transaction,
        return_mail_id,
        recovery_id,
        obligation_id,
        observation_id,
        now,
    )
    .await
    .map_err(ConsignmentError::from)
}

/// 隔离解除引用的观察必须与退件入站邮件同世界、同 MID；若旧链路已建立 K 映射，认证 Bot 也必须
/// 一致。没有映射时沿用 E1 的旧直达兼容，不以 Bot UID 缺失拒绝。
async fn observation_matches_inbound(
    transaction: &mut TrackedTransaction<'_, Postgres>,
    observation_id: Uuid,
    return_mail_id: Uuid,
) -> Result<bool, WriteError> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM public.mail_envelope_observations AS observation
             JOIN public.inbound_mail AS inbound
               ON inbound.id = $2
              AND inbound.world_id = observation.world_id
              AND inbound.external_mail_id = observation.external_mid
             LEFT JOIN public.mail_original_links AS link ON link.inbound_id = inbound.id
             WHERE observation.id = $1
               AND (link.bot_uid IS NULL OR link.bot_uid = observation.bot_uid)
         )",
    )
    .bind(observation_id)
    .bind(return_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(WriteError::from)
}

/// 原案件交接返回的原 owner 引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReturnCaseHandoff {
    /// 可空义务 UUID。
    pub(crate) obligation_id: Option<Uuid>,
    /// 原退件证据 UUID。
    pub(crate) return_evidence_id: Uuid,
}

#[cfg(feature = "test-db")]
impl DepositRepository {
    /// PG 测试入口：直接调用原案件交接原语；成功返回可空义务，失败返回诊断文本。
    pub async fn test_handoff_return_case(
        &self,
        return_mail_id: Uuid,
        observation_id: Uuid,
    ) -> Result<Option<Uuid>, String> {
        use sqlx::Connection as _;

        let mut connection =
            self.store().pool().acquire().await.map_err(|error| format!("{error:?}"))?;
        let transaction = connection.begin().await.map_err(|error| format!("{error:?}"))?;
        let mut tracked = TrackedTransaction::borrowed(transaction);
        match handoff_return_case_in_tx(
            &mut tracked,
            return_mail_id,
            observation_id,
            Timestamp::now(),
        )
        .await
        {
            Ok(handoff) => {
                tracked.commit().await.map_err(|error| format!("{error:?}"))?;
                Ok(handoff.obligation_id)
            }
            Err(error) => {
                tracked.rollback().await.map_err(|error| format!("{error:?}"))?;
                Err(error.to_string())
            }
        }
    }
}

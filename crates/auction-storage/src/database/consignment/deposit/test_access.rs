//! 4B 事务内原语的 PG 测试窄入口；只在 `test-db` 构建。

use std::collections::BTreeSet;

use auction_application::consignment::{
    ConsignmentError, CustodyListingChange, DepositActor, DepositOperation, EnvelopeDisposition,
    EnvelopeReceipt, ExpireReservationCommand, IssueReservationCommand, ListingStateChange,
    ProofDisposition, ProofReceipt, ReservationReceipt, ReservationRetired,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::MatchedDepositItem;
use auction_time::Timestamp;
use uuid::Uuid;

use super::repository::DepositRepository;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::custody::apply_take_custody_in_tx;
use crate::database::consignment::deposit::outcome::{FirstOutcome, PortOutcome, replay};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::reservations::{
    expire_children, expire_stage, issue_children, issue_stage, pre_read_reservation,
};

fn invalid_deposit_request<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidDepositRequest
}

/// 整封接受原语返回的逐件事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestAcceptedItem {
    /// 原始附件序号。
    pub ordinal: u32,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 预约 UUID。
    pub reservation_id: Uuid,
    /// position UUID。
    pub position_id: Uuid,
    /// 接受后的挂单版本。
    pub listing_version: u64,
}

impl DepositRepository {
    /// 在测试事务内按数据库锁后时刻调用整封接受原语。
    pub async fn test_accept_items(
        &self,
        command_id: Uuid,
        decision_id: Uuid,
        mail_key: OriginalMailKey,
        items: Vec<MatchedDepositItem>,
    ) -> Result<Vec<TestAcceptedItem>, ConsignmentError> {
        self.test_accept_items_inner(command_id, decision_id, mail_key, items, None).await
    }

    /// 在测试事务内用固定锁后时刻调用整封接受原语，用于截止等号边界。
    pub async fn test_accept_items_at(
        &self,
        command_id: Uuid,
        decision_id: Uuid,
        mail_key: OriginalMailKey,
        items: Vec<MatchedDepositItem>,
        at: Timestamp,
    ) -> Result<Vec<TestAcceptedItem>, ConsignmentError> {
        self.test_accept_items_inner(command_id, decision_id, mail_key, items, Some(at)).await
    }

    async fn test_accept_items_inner(
        &self,
        command_id: Uuid,
        decision_id: Uuid,
        mail_key: OriginalMailKey,
        items: Vec<MatchedDepositItem>,
        at: Option<Timestamp>,
    ) -> Result<Vec<TestAcceptedItem>, ConsignmentError> {
        let mut sellers = BTreeSet::new();
        let mut equipment = BTreeSet::new();
        for item in &items {
            sellers.insert(item.seller_user_id.as_uuid());
            equipment.insert((
                item.equipment.world_id().to_owned(),
                item.equipment.external_equipment_id().to_owned(),
            ));
        }
        let sellers: Vec<_> = sellers.into_iter().collect();
        let equipment: Vec<_> = equipment
            .into_iter()
            .map(|(world, eid)| {
                auction_domain::consignment::ListingEquipmentIdentity::new(&world, &eid)
                    .map_err(invalid_deposit_request)
            })
            .collect::<Result<_, _>>()?;
        let input = (self, command_id, decision_id, mail_key, items, sellers, equipment, at);
        let keys = LockKeys { command_id: input.1, sellers: &input.5, equipment: &input.6 };
        let mail_keys = [input.3.clone()];
        with_deposit_transaction(self.store(), keys, &mail_keys, &input, |input, tx| {
            Box::pin(async move {
                tx.ensure_lock_set(&input.5, &input.6, std::slice::from_ref(&input.3))?;
                let arbitration = tx
                    .arbitrate(
                        DepositActor::Worker,
                        DepositOperation::DecideEnvelope,
                        DepositTarget::MailKey(input.3.clone()),
                        RequestHash::new([0x5c; 32]),
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(_) => {
                        return Err(WriteError::invariant("test_accept_command_replayed"));
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let now = match input.7 {
                    Some(at) => at,
                    None => database_now(writer.connection()).await?,
                };
                let bot_uid = input.3.authenticated_bot_uid();
                let world = auction_domain::consignment::settlement_world_code(input.3.world());
                let external_mid = input.3.external_mid();
                let observation_id: Uuid = sqlx::query_scalar(
                    "SELECT id FROM public.mail_envelope_observations
                     WHERE bot_uid=$1 AND world_id=$2 AND external_mid=$3
                     ORDER BY observed_at DESC LIMIT 1",
                )
                .bind(bot_uid)
                .bind(world)
                .bind(external_mid)
                .fetch_one(writer.connection())
                .await?;
                sqlx::query(
                    "INSERT INTO public.mail_initial_decisions
                        (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
                         quarantine_id, decided_at)
                     VALUES ($1, $2, $3, $4, $5, $6, 'accepted', NULL, $7)",
                )
                .bind(bot_uid)
                .bind(world)
                .bind(external_mid)
                .bind(Uuid::new_v4())
                .bind(observation_id)
                .bind(input.1)
                .bind(now)
                .execute(writer.connection())
                .await
                .map_err(WriteError::from)?;
                sqlx::query(
                    "INSERT INTO public.mail_final_dispositions
                        (bot_uid, world_id, external_mid, id, kind, source_observation_id,
                         command_id, decided_at)
                     VALUES ($1, $2, $3, $4, 'accepted', $5, $6, $7)",
                )
                .bind(bot_uid)
                .bind(world)
                .bind(external_mid)
                .bind(input.2)
                .bind(observation_id)
                .bind(input.1)
                .bind(now)
                .execute(writer.connection())
                .await
                .map_err(WriteError::from)?;
                let delivery_id = Uuid::new_v4();
                let work_id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO public.deliveries
                        (id, command_id, request_digest, origin_kind, origin_id, recipient,
                         payload, status, fencing_version, attempt_count, created_at, updated_at)
                     VALUES ($1, $2, $3, 'manual', 'test-accept', 'FakeBot', '{}'::jsonb,
                             'prepared', 0, 0, $4, $4)",
                )
                .bind(delivery_id)
                .bind(Uuid::new_v4())
                .bind([0x5a_u8; 32].as_slice())
                .bind(now)
                .execute(writer.connection())
                .await
                .map_err(WriteError::from)?;
                sqlx::query(
                    "INSERT INTO public.mail_disposition_work
                        (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
                         delivery_id, expected_observation_id, manifest_digest, created_at)
                     VALUES ($1, $2, $3, $4, $5, NULL, 'take', $6, $7, $8, $9)",
                )
                .bind(work_id)
                .bind(bot_uid)
                .bind(world)
                .bind(external_mid)
                .bind(input.2)
                .bind(delivery_id)
                .bind(observation_id)
                .bind([0x5b_u8; 32].as_slice())
                .bind(now)
                .execute(writer.connection())
                .await
                .map_err(WriteError::from)?;
                let accepted =
                    super::items::accept_items_in_tx(&mut writer, input.2, &input.3, &input.4, now)
                        .await?;
                let mut receipt = EnvelopeReceipt::new(
                    CommandId::from_uuid(input.1),
                    EnvelopeDisposition::Accepted,
                    input.2,
                );
                receipt.listing_changes = writer.listing_changes();
                writer.finish(receipt, now).await?;
                Ok(accepted
                    .into_iter()
                    .map(|item| TestAcceptedItem {
                        ordinal: item.ordinal,
                        listing_id: item.listing_id,
                        reservation_id: item.reservation_id,
                        position_id: item.position_id,
                        listing_version: item.listing_version,
                    })
                    .collect())
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 在测试事务内先写证明与起写事实，再调用取得原语，最后写 `apply_mail_proof` 命令。
    ///
    /// 子事实只引用 writer 的命令 ID；命令行最后写入，000134 的延迟外键在提交时校验。
    /// 本入口只用于验证原语的 position、挂单和事件投影。
    pub async fn test_apply_take(
        &self,
        command_id: Uuid,
        decision_id: Uuid,
        proof_id: Uuid,
        attempt_id: Uuid,
        listing_id: Uuid,
    ) -> Result<Vec<CustodyListingChange>, ConsignmentError> {
        let Some(identity) = super::primitives::pre_read_identity(self.store(), listing_id)
            .await
            .map_err(WriteError::into_application)?
        else {
            return Err(ConsignmentError::InvalidListingId);
        };
        let sellers = [identity.seller_user_id];
        let equipment = [identity.equipment];
        let input =
            (self, command_id, decision_id, proof_id, attempt_id, listing_id, sellers, equipment);
        let keys = LockKeys { command_id: input.1, sellers: &input.6, equipment: &input.7 };
        with_deposit_transaction(self.store(), keys, &[], &input, |input, tx| {
            Box::pin(async move {
                tx.ensure_lock_set(&input.6, &input.7, &[])?;
                let now = database_now(tx.connection()).await?;
                let arbitration = tx
                    .arbitrate(
                        DepositActor::Worker,
                        DepositOperation::ApplyMailProof,
                        DepositTarget::OriginalAttempt(input.4),
                        RequestHash::new([0x5f; 32]),
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(_) => {
                        return Err(WriteError::invariant("test_take_command_replayed"));
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let fixture: (Uuid, Uuid, Uuid, Uuid, Uuid) = sqlx::query_as(
                    "SELECT work.id, work.delivery_id, work.expected_observation_id,
                            session.id, session.credential_id
                     FROM public.mail_disposition_work work
                     JOIN public.mail_envelope_observations observation
                       ON observation.id = work.expected_observation_id
                     JOIN public.mail_evidence_sessions session
                       ON session.id = observation.session_id
                     WHERE work.decision_id = $1 AND work.action = 'take'",
                )
                .bind(input.2)
                .fetch_one(writer.connection())
                .await?;
                sqlx::query(
                    "INSERT INTO public.mail_disposition_attempts
                        (id, work_id, delivery_id, fence, credential_id, credential_version,
                         runtime_generation, session_id, expected_digest, prepared_at)
                     VALUES ($1, $2, $3, 1, $4, 1, 1, $5, $6, $7)",
                )
                .bind(input.4)
                .bind(fixture.0)
                .bind(fixture.1)
                .bind(fixture.4)
                .bind(fixture.3)
                .bind([0xe1_u8; 32].as_slice())
                .bind(now)
                .execute(writer.connection())
                .await?;
                sqlx::query(
                    "INSERT INTO public.mail_disposition_write_starts
                        (work_id, decision_id, delivery_id, attempt_id, fence, started_at,
                         command_id)
                     VALUES ($1, $2, $3, $4, 1, $5, $6)",
                )
                .bind(fixture.0)
                .bind(input.2)
                .bind(fixture.1)
                .bind(input.4)
                .bind(now)
                .bind(input.1)
                .execute(writer.connection())
                .await?;
                sqlx::query(
                    "INSERT INTO public.mail_disposition_proofs
                        (id, work_id, decision_id, delivery_id, fence, attempt_id, kind,
                         asset_leg, observation_id, validator_version, digest, conclusion,
                         proven_at)
                     VALUES ($1, $2, $3, $4, 1, $5, 'exact_take', 'original', $6, 'v1',
                             $7, 'take_proven', $8)",
                )
                .bind(input.3)
                .bind(fixture.0)
                .bind(input.2)
                .bind(fixture.1)
                .bind(input.4)
                .bind(fixture.2)
                .bind([0xe2_u8; 32].as_slice())
                .bind(now)
                .execute(writer.connection())
                .await?;
                let previous_version: i64 = sqlx::query_scalar(
                    "SELECT state_version FROM public.consignment_listings WHERE id = $1",
                )
                .bind(input.5)
                .fetch_one(writer.connection())
                .await?;
                let changes = apply_take_custody_in_tx(
                    &mut writer,
                    input.2,
                    input.3,
                    now,
                    input.0.settings().max_open_per_user,
                )
                .await?;
                let previous_version = u64::try_from(previous_version).unwrap_or(u64::MAX);
                let mut receipt = ProofReceipt::new(
                    CommandId::from_uuid(input.1),
                    ProofDisposition::TakeProven,
                    input.2,
                    input.3,
                );
                receipt.listing_changes = changes
                    .iter()
                    .filter(|change| change.listing_version > previous_version)
                    .map(|change| ListingStateChange {
                        listing_id: change.listing_id,
                        listing_version: change.listing_version,
                        state: change.state,
                    })
                    .collect();
                receipt.custody_changes = changes.clone();
                writer.finish(receipt, now).await?;
                Ok(changes)
            })
        })
        .await
        .map_err(WriteError::into_application)
    }
}

#[cfg(feature = "test-db")]
impl DepositRepository {
    /// 在测试事务内用固定锁后时刻签发预约，用于季末、待寄截止与 TTL 等号边界。
    pub async fn test_issue_reservation_at(
        &self,
        command: IssueReservationCommand,
        at: Timestamp,
    ) -> Result<ReservationReceipt, ConsignmentError> {
        let identity = super::primitives::pre_read_identity(self.store(), command.listing_id)
            .await
            .map_err(WriteError::into_application)?;
        let mut sellers = BTreeSet::from([command.seller_user_id.as_uuid()]);
        if let Some(identity) = &identity {
            sellers.insert(identity.seller_user_id);
        }
        let sellers: Vec<_> = sellers.into_iter().collect();
        let equipment: Vec<_> =
            identity.iter().map(|identity| identity.equipment.clone()).collect();
        let input = (self, command, sellers, equipment, at);
        let keys = LockKeys {
            command_id: input.1.command_id.as_uuid(),
            sellers: &input.2,
            equipment: &input.3,
        };
        with_deposit_transaction(
            self.store(),
            keys,
            &[],
            &input,
            |(repository, command, sellers, equipment, at), tx| {
                Box::pin(async move {
                    tx.ensure_lock_set(sellers, equipment, &[])?;
                    match tx
                        .arbitrate(
                            DepositActor::PublicUser(command.seller_user_id),
                            DepositOperation::IssueReservation,
                            DepositTarget::Listing(command.listing_id),
                            command.request_hash,
                            command.request_id.as_deref(),
                        )
                        .await?
                    {
                        DepositArbitration::Replay(stored) => replay(&stored),
                        DepositArbitration::First(writer) => {
                            match issue_stage(repository, command, writer, *at).await? {
                                FirstOutcome::Rejected(rejection) => {
                                    Ok(PortOutcome::Rejected(rejection))
                                }
                                FirstOutcome::Accepted { receipt, children } => {
                                    issue_children(tx.connection(), children).await?;
                                    Ok(PortOutcome::Accepted(receipt))
                                }
                            }
                        }
                    }
                })
            },
        )
        .await
        .map_err(WriteError::into_application)?
        .into_result()
    }
}

#[cfg(feature = "test-db")]
impl DepositRepository {
    /// 在测试事务内用固定锁后时刻退休预约，用于预约截止等号边界。
    pub async fn test_expire_reservation_at(
        &self,
        command: ExpireReservationCommand,
        at: Timestamp,
    ) -> Result<ReservationRetired, ConsignmentError> {
        let identity = pre_read_reservation(self.store(), command.reservation_id)
            .await
            .map_err(WriteError::into_application)?;
        let sellers: Vec<_> =
            identity.iter().map(|(_, identity)| identity.seller_user_id).collect();
        let equipment: Vec<_> =
            identity.iter().map(|(_, identity)| identity.equipment.clone()).collect();
        let input = (self, command, sellers, equipment, at);
        let keys = LockKeys {
            command_id: input.1.command_id.as_uuid(),
            sellers: &input.2,
            equipment: &input.3,
        };
        with_deposit_transaction(
            self.store(),
            keys,
            &[],
            &input,
            |(repository, command, sellers, equipment, at), tx| {
                Box::pin(async move {
                    tx.ensure_lock_set(sellers, equipment, &[])?;
                    match tx
                        .arbitrate(
                            DepositActor::Worker,
                            DepositOperation::ExpireReservation,
                            DepositTarget::Reservation(command.reservation_id),
                            command.request_hash,
                            None,
                        )
                        .await?
                    {
                        DepositArbitration::Replay(stored) => replay(&stored),
                        DepositArbitration::First(writer) => {
                            match expire_stage(repository, command, writer, *at).await? {
                                FirstOutcome::Rejected(rejection) => {
                                    Ok(PortOutcome::Rejected(rejection))
                                }
                                FirstOutcome::Accepted { receipt, children } => {
                                    expire_children(tx.connection(), children).await?;
                                    Ok(PortOutcome::Accepted(receipt))
                                }
                            }
                        }
                    }
                })
            },
        )
        .await
        .map_err(WriteError::into_application)?
        .into_result()
    }
}

//! 隔离首次事实与整封决定的同一事务写入。
//!
//! 首次隔离回执永久可重放；隔离版本独立于最终处置槽。观察追加、重检与解除分别在
//! `disposition`、`recheck`、`release` 叶子中实现，但都复用本文件的 K 与观察事实。

use std::collections::BTreeSet;

use auction_application::consignment::{
    AppendQuarantineObservationCommand, ConsignmentError, DecideEnvelopeCommand, DepositActor,
    DepositBusinessRejection, DepositOperation, EnvelopeDisposition, EnvelopeReceipt,
    EnvelopeStore, ObservationReceipt, QuarantineReference, RecheckQuarantineCommand,
    RecheckReceipt, ReleaseQuarantineCommand, ReleaseReceipt,
};
use auction_application::consignment::{
    canonical_system_recheck_request_hash, system_command_id, system_recheck_business_key,
};
use auction_application::core::lower_hex;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, EnvelopeItemKind, EnvelopeObservation, EnvelopeRoute, LockedEnvelopeFacts,
    QuarantineReason, decide_envelope,
};
use auction_domain::consignment::{
    ListingEquipmentIdentity, ListingVersion, OriginalMailKey, check_listing_history,
};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::disposition::{
    decision_kind, insert_final_decision, insert_initial_decision, work_action,
};
use super::identity::{MailKeyColumns, QuarantineMetadata, readiness_capabilities};
use super::observations::{self, StoredObservation};
use super::{Outcome, conflict, invariant, invariant_decoding, replay_receipt};
use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::consignment::eligibility::history::read_listing_history;

impl EnvelopeStore for DepositRepository {
    fn decide_envelope(
        &self,
        command: DecideEnvelopeCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, EnvelopeReceipt> {
        Box::pin(async move { decide_envelope_port(self, command).await })
    }

    fn append_quarantine_observation(
        &self,
        command: AppendQuarantineObservationCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, ObservationReceipt> {
        Box::pin(async move { super::disposition::append_observation_port(self, command).await })
    }

    fn recheck_quarantine(
        &self,
        command: RecheckQuarantineCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, RecheckReceipt> {
        Box::pin(async move { super::recheck::recheck_quarantine_port(self, command).await })
    }

    fn release_quarantine(
        &self,
        command: ReleaseQuarantineCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, ReleaseReceipt> {
        Box::pin(async move { super::release::release_quarantine_port(self, command).await })
    }
}

async fn decide_envelope_port(
    repository: &DepositRepository,
    command: DecideEnvelopeCommand,
) -> Result<EnvelopeReceipt, ConsignmentError> {
    let key = command.mail_key.clone();
    let sellers = envelope_sellers(&command.facts);
    let equipment = envelope_equipment(&command.facts);
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
                        DepositActor::Worker,
                        DepositOperation::DecideEnvelope,
                        DepositTarget::MailKey(key.clone()),
                        input.request_hash,
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<EnvelopeReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => Ok(Outcome::Accepted(receipt)),
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_envelope_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let now = database_now(writer.connection()).await?;
                let receipt = decide_envelope_in_tx(&mut writer, input, now).await?;
                let _ = writer.finish(receipt.clone(), now).await?;
                Ok(Outcome::Accepted(receipt))
            })
        });
    transaction.await.map_err(WriteError::into_application)?.into_result()
}

/// 在调用方已开的同一顶层事务内写入整封第一次决定。
///
/// 调用方须已按 4K 锁序锁定命令、用户、实物与 K，并已取得本封的 `now`；函数在锁内重读
/// 观察与预约，用 4A 领域规则复算，与命令决定不一致即拒绝。成功时不写命令行，调用方
/// 最后用 `DepositWriter::finish` 写回执，命令行随之最后插入。
pub(crate) async fn decide_envelope_in_tx(
    writer: &mut crate::database::consignment::commands::DepositWriter<'_, '_, '_>,
    command: &DecideEnvelopeCommand,
    now: Timestamp,
) -> Result<EnvelopeReceipt, WriteError> {
    let key = &command.mail_key;
    let command_id = writer.command_id();
    let stored = observations::load_observation(
        writer.connection(),
        key,
        command.facts.observation.observation_id,
    )
    .await?
    .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
    if !observation_matches_command(&stored, &command.facts.observation) {
        return Err(conflict(DepositBusinessRejection::EvidenceConflict));
    }
    let locked =
        lock_envelope_facts(writer.connection(), key, &stored, &command.facts, now).await?;
    let EnvelopeRoute::Consignment(recomputed) = decide_envelope(&locked) else {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    };
    if recomputed != command.decision {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    match &recomputed {
        EnvelopeDecision::Quarantine(reason) => {
            let decision_id = Uuid::new_v4();
            let quarantine_id = Uuid::new_v4();
            insert_initial_decision(
                writer.connection(),
                key,
                decision_id,
                stored.observation_id,
                command_id,
                "quarantined",
                Some(quarantine_id),
                now,
            )
            .await?;
            let metadata = QuarantineMetadata::new(&stored.mailbox_key);
            persist_quarantine(
                writer.connection(),
                key,
                quarantine_id,
                decision_id,
                stored.observation_id,
                *reason,
                &metadata,
                now,
            )
            .await?;
            // 裁定：首次隔离在同一事务排一条系统重检；重检命中旧拍卖 owner 后由系统自动
            // 交接，其它解除仍由 Admin。命令与工作同提交，隔离回执永久可重放。
            enqueue_first_recheck(writer.connection(), quarantine_id, now).await?;
            let mut receipt = EnvelopeReceipt::new(
                command.command_id,
                EnvelopeDisposition::Quarantined,
                decision_id,
            );
            receipt.quarantine = Some(QuarantineReference { quarantine_id, version: 1 });
            Ok(receipt)
        }
        decision => {
            let decision_id = Uuid::new_v4();
            let kind = decision_kind(decision);
            insert_initial_decision(
                writer.connection(),
                key,
                decision_id,
                stored.observation_id,
                command_id,
                kind,
                None,
                now,
            )
            .await?;
            insert_final_decision(
                writer.connection(),
                key,
                decision_id,
                kind,
                stored.observation_id,
                command_id,
                now,
            )
            .await?;
            if let Some(action) = work_action(decision) {
                let (delivery_id, delivery_command_id) =
                    super::disposition::delivery_identity(decision_id, action);
                super::disposition::create_work_in_tx(
                    writer.connection(),
                    &super::disposition::WorkSeed {
                        work_id: Uuid::new_v4(),
                        delivery_id,
                        delivery_command_id,
                        decision_id,
                        action,
                        mail_key: key,
                        observation: &stored,
                        subject: "consign",
                        return_manifest: match decision {
                            EnvelopeDecision::Return(manifest) => Some(manifest),
                            _ => None,
                        },
                        now,
                    },
                )
                .await?;
            }
            if let EnvelopeDecision::Accept(items) = decision {
                let accepted = crate::database::consignment::deposit::accept_items_in_tx(
                    writer,
                    decision_id,
                    key,
                    items,
                    now,
                )
                .await?;
                if accepted.len() != items.len() {
                    return Err(invariant("consignment_envelope_accept_count"));
                }
            }
            let disposition = match decision {
                EnvelopeDecision::Accept(_) => EnvelopeDisposition::Accepted,
                EnvelopeDecision::Return(_) => EnvelopeDisposition::ReturnDecided,
                EnvelopeDecision::ArchiveEmpty => EnvelopeDisposition::ArchivedEmpty,
                EnvelopeDecision::Quarantine(_) => {
                    return Err(invariant("consignment_envelope_quarantine_branch"));
                }
            };
            let mut receipt = EnvelopeReceipt::new(command.command_id, disposition, decision_id);
            if disposition == EnvelopeDisposition::Accepted {
                receipt.listing_changes = writer.listing_changes();
            }
            Ok(receipt)
        }
    }
}

/// 首次隔离同事务写入系统重检命令与 queued 工作。
async fn enqueue_first_recheck(
    connection: &mut sqlx::PgConnection,
    quarantine_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let command_id = system_command_id(
        DepositOperation::RecheckQuarantine,
        &system_recheck_business_key(quarantine_id),
    );
    let request_hash = canonical_system_recheck_request_hash(quarantine_id, 1);
    let work_id = Uuid::new_v4();
    let receipt = RecheckReceipt::queued(command_id, quarantine_id, work_id);
    let body = receipt.body();
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, replay_key_id, listing_id, result_status, result_code,
             result_body, created_at, target_kind, target_id, target_bot_uid,
             target_world_id, target_external_mid, target_season_id, target_calendar_version)
         VALUES ($1, 2, 'system', $7, 'recheck_quarantine', $2,
                 $3, NULL, NULL, 202, 'ok', $4, $5, 'quarantine', $6, NULL, NULL, NULL,
                 NULL, NULL)",
    )
    .bind(command_id.as_uuid())
    .bind(quarantine_id.to_string())
    .bind(request_hash.as_bytes())
    .bind(&body)
    .bind(now)
    .bind(quarantine_id)
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.mail_quarantine_rechecks
            (id, quarantine_id, requested_version, command_id, state, available_at,
             lease_token, lease_until, fence, attempt_count, completed_at)
         VALUES ($1, $2, 1, $3, 'queued', $4, NULL, NULL, 0, 0, NULL)",
    )
    .bind(work_id)
    .bind(quarantine_id)
    .bind(command_id.as_uuid())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

pub(crate) fn envelope_sellers(facts: &LockedEnvelopeFacts) -> Vec<Uuid> {
    let mut sellers = BTreeSet::new();
    if let Some(seller) = facts.envelope_seller_user_id {
        sellers.insert(seller.as_uuid());
    }
    for reservation in &facts.reservations {
        sellers.insert(reservation.seller_user_id.as_uuid());
    }
    for item in &facts.observation.items {
        if let Some(seller) = item.seller_user_id {
            sellers.insert(seller.as_uuid());
        }
    }
    sellers.into_iter().collect()
}

pub(crate) fn envelope_equipment(facts: &LockedEnvelopeFacts) -> Vec<ListingEquipmentIdentity> {
    let mut equipment = BTreeSet::new();
    for reservation in &facts.reservations {
        equipment.insert((
            reservation.equipment.world_id().to_owned(),
            reservation.equipment.external_equipment_id().to_owned(),
        ));
    }
    let world = facts.mail_key.as_ref().map_or(SettlementWorld::Main, OriginalMailKey::world);
    for item in &facts.observation.items {
        if let Some(eid) = item.canonical_eid.as_deref() {
            equipment.insert((
                auction_domain::consignment::settlement_world_code(item.world.unwrap_or(world))
                    .to_owned(),
                eid.to_owned(),
            ));
        }
    }
    equipment
        .into_iter()
        .filter_map(|(world, eid)| ListingEquipmentIdentity::new(&world, &eid).ok())
        .collect()
}

fn observation_matches_command(
    stored: &StoredObservation,
    observation: &EnvelopeObservation,
) -> bool {
    stored.observation_id == observation.observation_id
        && stored.session_binding_digest == observation.session_binding_digest
        && stored.manifest_digest == observation.manifest_digest
        && stored.parser_version == observation.parser_version
        && stored.subject_is_consign == observation.subject_is_consign
        && stored.sender_identity == observation.sender_identity
        && stored.list_detail_consistent == observation.list_detail_consistent
        && stored.asset_count == observation.asset_count
        && stored.duplicate_raw_eid == observation.duplicate_raw_eid
        && stored.canonical_alias_ambiguous == observation.canonical_alias_ambiguous
        && stored.unknown_attachment == observation.unknown_attachment
        && stored.credits == observation.credits
        && stored.cod == observation.cod
        && stored.returnable == Some(observation.returnable)
        && stored.takeable == observation.takeable
        && stored.system_rts == observation.system_rts
        && stored.legacy_auction_owner == observation.legacy_auction_owner
        && stored.items.len() == observation.items.len()
        && stored.items.iter().zip(observation.items.iter()).all(|(stored, command)| {
            stored.ordinal == command.ordinal
                && stored.kind == command.kind
                && stored.raw_eid == command.raw_eid
                && stored.canonical_eid == command.canonical_eid
                && stored.amount == command.amount
                && stored.display_name == command.display_name
                && stored.world == command.world
                && stored.tradeable == command.tradeable
                && stored.history_ok == command.history_ok
                && stored.protected_ref == command.protected_ref
        })
}

async fn lock_envelope_facts(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    stored: &StoredObservation,
    command_facts: &LockedEnvelopeFacts,
    now: Timestamp,
) -> Result<LockedEnvelopeFacts, WriteError> {
    let mut reservations = Vec::with_capacity(command_facts.reservations.len());
    for reservation in &command_facts.reservations {
        let row = sqlx::query(
            "SELECT r.id, r.listing_id, r.seller_user_id, r.world_id, r.canonical_eid,
                    r.generation, r.expires_at, r.state, l.state_version
             FROM public.consignment_deposit_reservations AS r
             JOIN public.consignment_listings AS l ON l.id = r.listing_id
             WHERE r.id = $1 FOR UPDATE OF r, l",
        )
        .bind(reservation.reservation_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
        let binding = reservation_binding(&row)?;
        if binding != *reservation {
            return Err(invariant("consignment_envelope_reservation_changed"));
        }
        reservations.push(binding);
    }
    // 发件人到卖家的映射是可变事实，必须在锁内按当前账户重读；歧义或缺失保持 None，
    // 由领域复算失败关闭。
    let seller = seller_by_name(connection, stored.sender.as_deref()).await?;
    let observation = observation_with_locked_history(connection, key, stored, seller).await?;
    let capabilities = readiness_capabilities(connection, key.world(), now).await?;
    let already_decided: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.mail_initial_decisions
            WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3
         )",
    )
    .bind(key.authenticated_bot_uid())
    .bind(auction_domain::consignment::settlement_world_code(key.world()))
    .bind(key.external_mid())
    .fetch_one(&mut *connection)
    .await?;
    Ok(LockedEnvelopeFacts {
        mail_key: Some(key.clone()),
        observation,
        reservations,
        envelope_seller_user_id: seller,
        capabilities,
        read_failure: false,
        already_decided,
        now,
    })
}

/// 用锁内读到的完整历史重算逐件 `history_ok`，再组装领域观察。
///
/// 不变量 2、6、10：`derived_facts.items[].history_ok` 只是观察时快照，交接链可能在
/// 观察后变化；决定与解除必须在已持有命令、用户、实物与 K 锁之后按当前快照重算。
/// 本函数只读历史，不取得新锁。卖家映射缺失或历史不通过时该件失败关闭为 `false`；
/// 实物身份不合法时按不变量返回错误，整个事务回滚。
pub(super) async fn observation_with_locked_history(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    stored: &StoredObservation,
    seller: Option<UserId>,
) -> Result<EnvelopeObservation, WriteError> {
    let mut observation = observations::observation_from_stored(stored, seller);
    for item in &mut observation.items {
        if item.kind != EnvelopeItemKind::Equipment {
            item.history_ok = false;
            continue;
        }
        let (Some(seller), Some(canonical_eid)) = (seller, item.canonical_eid.as_deref()) else {
            item.history_ok = false;
            continue;
        };
        let world = item.world.unwrap_or_else(|| key.world());
        let Ok(identity) = ListingEquipmentIdentity::new(
            auction_domain::consignment::settlement_world_code(world),
            canonical_eid,
        ) else {
            return Err(invariant("consignment_envelope_history_identity"));
        };
        let history = read_listing_history(connection, &identity).await?;
        item.history_ok = check_listing_history(&identity, seller, &history.aliases).is_ok();
    }
    Ok(observation)
}

pub(crate) async fn seller_by_name(
    connection: &mut PgConnection,
    sender: Option<&str>,
) -> Result<Option<UserId>, WriteError> {
    let Some(sender) = sender.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.users WHERE lower(username) = lower($1) ORDER BY id LIMIT 2",
    )
    .bind(sender)
    .fetch_all(&mut *connection)
    .await?;
    Ok(match ids.as_slice() {
        [seller] => Some(UserId::from_uuid(*seller)),
        _ => None,
    })
}

pub(crate) fn reservation_binding(
    row: &sqlx::postgres::PgRow,
) -> Result<auction_domain::consignment::deposit::ReservationBinding, WriteError> {
    let world: String = row.try_get("world_id")?;
    let state = match row.try_get::<String, _>("state")?.as_str() {
        "active" => auction_domain::consignment::deposit::ReservationState::Active,
        "retired" => auction_domain::consignment::deposit::ReservationState::Retired,
        "consumed" => auction_domain::consignment::deposit::ReservationState::Consumed,
        _ => return Err(invariant("consignment_reservation_state")),
    };
    let version: i64 = row.try_get("state_version")?;
    Ok(auction_domain::consignment::deposit::ReservationBinding {
        reservation_id: row.try_get("id")?,
        generation: u64::try_from(row.try_get::<i64, _>("generation")?)
            .map_err(invariant_decoding("consignment_reservation_generation"))?,
        listing_id: row.try_get("listing_id")?,
        listing_version: ListingVersion::new(u64::try_from(version)?)
            .map_err(invariant_decoding("consignment_reservation_version"))?,
        seller_user_id: UserId::from_uuid(row.try_get("seller_user_id")?),
        equipment: ListingEquipmentIdentity::new(
            &world,
            &row.try_get::<String, _>("canonical_eid")?,
        )
        .map_err(invariant_decoding("consignment_reservation_equipment"))?,
        expires_at: row.try_get("expires_at")?,
        state,
    })
}

#[allow(clippy::too_many_arguments, reason = "逐列复刻隔离首次事实")]
async fn persist_quarantine(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    quarantine_id: Uuid,
    decision_id: Uuid,
    initial_observation_id: Uuid,
    reason: QuarantineReason,
    metadata: &QuarantineMetadata,
    now: Timestamp,
) -> Result<(), WriteError> {
    let columns = MailKeyColumns::from_key(key);
    sqlx::query(
        "INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, 1, 'active', $6, $7, $8, $8)",
    )
    .bind(quarantine_id)
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(decision_id)
    .bind(metadata.encode()?)
    .bind(reason.as_str())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let linked_remote = linked_remote_id(quarantine_id, initial_observation_id.as_bytes());
    observations::write_observation_in_tx(
        connection,
        observations::ObservationWrite::Copy {
            source_id: initial_observation_id,
            observation_id: Uuid::new_v4(),
            remote_observation_id: &linked_remote,
            quarantine_id,
            quarantine_version: 1,
            legacy_auction_owner: None,
        },
    )
    .await?;
    Ok(())
}

/// 隔离内观察使用独立远端身份，避免与原始未链接观察的全局唯一键冲突。
pub(super) fn linked_remote_id(quarantine_id: Uuid, remote: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"auction/consignment/quarantine-observation/v1\0");
    digest.update(quarantine_id.as_bytes());
    digest.update(b"\0");
    digest.update(remote);
    format!("q-{}", lower_hex(digest.finalize().as_slice()))
}

// 两个旧 owner 写端由 `release` 叶子承载；这里保留合同指定的调用点路径，集成车道按此接入。
#[allow(unused_imports, reason = "合同指定的集成调用点路径")]
pub(crate) use super::release::handoff_recovery_return_in_tx;
#[allow(unused_imports, reason = "合同指定的集成调用点路径")]
pub(crate) use super::release::handoff_return_case_in_tx;

//! Admin 隔离详情与解除分类所需的只读事实。

use auction_application::consignment::{
    AdminFinalDecision, ConsignmentError, DepositBusinessRejection, ListingStateChange,
    ReleaseQuarantineCommand,
};
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    AuctionRecoveryEvidence, AuctionRecoveryKind, AuctionReturnEvidence, AuctionReturnKind,
    EnvelopeItemKind, LegacyAuctionOwner, LockedReleaseFacts, MatchedDepositItem, ReleaseKind,
    ReleaseSource, ReturnItem, ReturnItemReason, ReturnManifest, SenderIdentity,
};
use auction_domain::consignment::{
    ListingEquipmentIdentity, ListingState, ListingVersion, OriginalMailKey,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::identity::{MailKeyColumns, StoredQuarantine, StoredRelease};
use super::observations::{self, StoredObservation};
use super::{conflict, invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;

/// 构造解除分类的锁内事实。
pub(crate) async fn release_facts(
    quarantine: &StoredQuarantine,
    observation: &StoredObservation,
    command: &ReleaseQuarantineCommand,
    now: Timestamp,
    observation_is_new: bool,
    connection: &mut PgConnection,
) -> Result<LockedReleaseFacts, WriteError> {
    let source =
        release_source(connection, &quarantine.mail_key, observation.legacy_auction_owner).await?;
    let contradiction: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_observation_conflicts WHERE observation_id = $1)",
    )
    .bind(observation.observation_id)
    .fetch_one(&mut *connection)
    .await?;
    let seller =
        super::quarantine::seller_by_name(connection, observation.sender.as_deref()).await?;
    // 不变量 2、6：观察行只保留观察时的 history_ok；解除接受必须在已持有命令、用户、
    // 实物与 K 锁之后按锁内历史重算，历史回退视为拟定接受与锁内事实冲突。
    let locked_observation = super::quarantine::observation_with_locked_history(
        connection,
        &quarantine.mail_key,
        observation,
        seller,
    )
    .await?;
    let history_regressed = observation
        .items
        .iter()
        .zip(locked_observation.items.iter())
        .any(|(observed, locked)| observed.history_ok && !locked.history_ok);
    if command.proposed_kind == ReleaseKind::Accept && history_regressed {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    let mut observation = observation.clone();
    for (observed, locked) in observation.items.iter_mut().zip(locked_observation.items) {
        observed.history_ok = locked.history_ok;
    }
    let allowlisted =
        sender_allowlisted(connection, &quarantine.mail_key, observation.sender.as_deref()).await?;
    let season_open = season_acceptance_open(connection, quarantine.mail_key.world(), now).await?;
    let matched =
        matched_release_items(connection, &quarantine.mail_key, seller, &observation, now).await?;
    let accept_deadline = matched.iter().map(|(_, deadline)| *deadline).min();
    let readiness = readiness_row(connection, quarantine.mail_key.world(), now).await?;
    let return_safe = observation.returnable == Some(true)
        && readiness.as_ref().is_some_and(|(_, return_ready)| *return_ready);
    let archive_empty_proven = observation.asset_count == Some(0)
        && observation.items.is_empty()
        && observation.credits == Some(0)
        && observation.cod == Some(0);
    Ok(LockedReleaseFacts {
        quarantine_id: command.quarantine_id,
        expected_version: command.expected_version,
        actual_version: quarantine.version,
        quarantine_active: quarantine.active,
        observation_id: observation.observation_id,
        observation_is_new,
        observation_digest: observation.manifest_digest,
        observation_contradicts_manifest: contradiction,
        source,
        subject_is_consign: observation.subject_is_consign,
        sender_verified: observation.sender_identity == SenderIdentity::Verified,
        allowlisted,
        season_open_for_acceptance: season_open,
        assets_clear: !observation.unknown_attachment
            && observation.items.iter().all(|item| item.kind != EnvelopeItemKind::Unknown),
        archive_empty_proven,
        return_safe,
        matched_items: matched.into_iter().map(|(item, _)| item).collect(),
        return_manifest: Some(ReturnManifest {
            items: observation
                .items
                .iter()
                .map(|item| ReturnItem {
                    ordinal: item.ordinal,
                    canonical_eid: item.canonical_eid.clone(),
                    reason: ReturnItemReason::Unregistered,
                })
                .collect(),
        }),
        accept_deadline,
        opposite_action_exists: false,
        existing_release: None,
        disposition_taken: false,
        now,
    })
}

async fn release_source(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    owner: Option<LegacyAuctionOwner>,
) -> Result<ReleaseSource, WriteError> {
    let Some(owner) = owner else {
        return Ok(ReleaseSource::Consignment);
    };
    match owner {
        LegacyAuctionOwner::SellerPayout
        | LegacyAuctionOwner::BuyerRefund
        | LegacyAuctionOwner::UnsoldReturn
        | LegacyAuctionOwner::WithdrawnReturn => {
            let return_mail_id = find_return_mail(connection, key)
                .await?
                .ok_or_else(|| invariant("consignment_release_return_mail"))?;
            let kind = match owner {
                LegacyAuctionOwner::SellerPayout => AuctionReturnKind::SellerPayout,
                LegacyAuctionOwner::BuyerRefund => AuctionReturnKind::BuyerRefund,
                LegacyAuctionOwner::UnsoldReturn => AuctionReturnKind::UnsoldReturn,
                _ => AuctionReturnKind::WithdrawnReturn,
            };
            Ok(ReleaseSource::AuctionReturn(AuctionReturnEvidence {
                kind,
                obligation_id: None,
                return_mail_id,
                evidence_id: Uuid::new_v4(),
            }))
        }
        LegacyAuctionOwner::WinnerEquipmentCod
        | LegacyAuctionOwner::WinnerPaymentCarrierCod
        | LegacyAuctionOwner::WinnerEquipmentHandoff => {
            let return_mail_id = find_return_mail(connection, key)
                .await?
                .ok_or_else(|| invariant("consignment_release_return_mail"))?;
            // 不变量 6：恢复记录只由入站退件证据里的义务唯一确定。`return_mail_id` 预关联只在
            // 恢复消费时写入，解除不能依赖它；零匹配或多匹配都失败关闭。
            let recoveries = sqlx::query(
                "SELECT recovery.id, recovery.payment_obligation_id,
                        recovery.equipment_obligation_id
                 FROM public.inbound_mail AS inbound
                 JOIN public.settlement_recoveries AS recovery
                   ON inbound.decision_metadata ->> 'obligation_id' IN (
                       recovery.payment_obligation_id::text,
                       recovery.equipment_obligation_id::text
                   )
                 WHERE inbound.id = $1",
            )
            .bind(return_mail_id)
            .fetch_all(connection)
            .await?;
            let [row] = recoveries.as_slice() else {
                return Err(invariant("consignment_release_recovery"));
            };
            // 旧 owner 决定义务腿：main COD 与 isekai 载体付款取付款义务，isekai 交割取交割
            // 义务。不能再用 COALESCE 猜测，否则上游错误形状会被静默修正。
            let obligation_id = match owner {
                LegacyAuctionOwner::WinnerEquipmentHandoff => {
                    let equipment: Option<Uuid> = row.try_get("equipment_obligation_id")?;
                    equipment.ok_or_else(|| invariant("consignment_release_recovery_obligation"))?
                }
                _ => row.try_get("payment_obligation_id")?,
            };
            let kind = match owner {
                LegacyAuctionOwner::WinnerEquipmentCod => AuctionRecoveryKind::WinnerEquipmentCod,
                LegacyAuctionOwner::WinnerPaymentCarrierCod => {
                    AuctionRecoveryKind::WinnerPaymentCarrierCod
                }
                _ => AuctionRecoveryKind::WinnerEquipmentHandoff,
            };
            Ok(ReleaseSource::AuctionRecovery(AuctionRecoveryEvidence {
                kind,
                obligation_id,
                return_mail_id,
                recovery_id: row.try_get("id")?,
                asset_matches: false,
            }))
        }
    }
}

async fn find_return_mail(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
) -> Result<Option<Uuid>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT inbound_id FROM public.mail_original_links
         WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(&mut *connection)
    .await?;
    if linked.is_some() {
        return Ok(linked);
    }
    sqlx::query_scalar(
        "SELECT id FROM public.inbound_mail
         WHERE world_id = $1 AND external_mail_id = $2
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(connection)
    .await
    .map_err(WriteError::from)
}

async fn sender_allowlisted(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    sender: Option<&str>,
) -> Result<bool, WriteError> {
    let row = sqlx::query(
        "SELECT allowlist_mode, allowed_senders
         FROM public.consignment_mail_readiness WHERE world_id = $1",
    )
    .bind(auction_domain::consignment::settlement_world_code(key.world()))
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(false) };
    if row.try_get::<String, _>("allowlist_mode")? == "disabled" {
        return Ok(true);
    }
    let Some(sender) = sender.filter(|value| !value.is_empty()) else {
        return Ok(false);
    };
    let allowed: Vec<String> = row.try_get("allowed_senders")?;
    Ok(allowed.iter().any(|value| value == sender))
}

async fn readiness_row(
    connection: &mut PgConnection,
    world: SettlementWorld,
    now: Timestamp,
) -> Result<Option<(Uuid, bool)>, WriteError> {
    let row = sqlx::query(
        "SELECT session_id, return_ready
         FROM public.consignment_mail_readiness
         WHERE world_id = $1 AND valid_until > $2",
    )
    .bind(auction_domain::consignment::settlement_world_code(world))
    .bind(now)
    .fetch_optional(connection)
    .await?;
    row.map(|row| Ok((row.try_get("session_id")?, row.try_get("return_ready")?))).transpose()
}

async fn season_acceptance_open(
    connection: &mut PgConnection,
    world: SettlementWorld,
    now: Timestamp,
) -> Result<bool, WriteError> {
    if world == SettlementWorld::Main {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.consignment_season_windows
            WHERE world_id = 'isekai' AND start_utc <= $1 AND admission_cutoff_utc > $1
         )",
    )
    .bind(now)
    .fetch_one(connection)
    .await
    .map_err(WriteError::from)
}

async fn matched_release_items(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    seller_user_id: Option<UserId>,
    observation: &StoredObservation,
    now: Timestamp,
) -> Result<Vec<(MatchedDepositItem, Timestamp)>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let mut matched = Vec::new();
    for item in &observation.items {
        if item.kind != EnvelopeItemKind::Equipment {
            continue;
        }
        if !item.tradeable || !item.history_ok || item.world != Some(key.world()) {
            continue;
        }
        let Some(eid) = item.canonical_eid.as_deref() else { continue };
        let Some(seller) = seller_user_id else { continue };
        let row = sqlx::query(
            "SELECT r.id, r.generation, r.listing_id, r.seller_user_id, r.world_id,
                    r.canonical_eid, r.expires_at, l.state AS listing_state,
                    l.state_version
             FROM public.consignment_deposit_reservations AS r
             JOIN public.consignment_listings AS l ON l.id = r.listing_id
             WHERE r.world_id = $1 AND r.canonical_eid = $2 AND r.state = 'active'
               AND r.seller_user_id = $3
             FOR UPDATE OF r, l",
        )
        .bind(columns.world_id)
        .bind(eid)
        .bind(seller.as_uuid())
        .fetch_optional(&mut *connection)
        .await?;
        let Some(row) = row else { continue };
        let expires_at: Timestamp = row.try_get("expires_at")?;
        if expires_at <= now
            || row.try_get::<String, _>("listing_state")? != ListingState::AwaitingDeposit.as_str()
        {
            continue;
        }
        let version: i64 = row.try_get("state_version")?;
        let generation: i64 = row.try_get("generation")?;
        matched.push((
            MatchedDepositItem {
                ordinal: item.ordinal,
                listing_id: row.try_get("listing_id")?,
                listing_version: ListingVersion::new(u64::try_from(version)?)
                    .map_err(invariant_decoding("consignment_release_version"))?,
                seller_user_id: UserId::from_uuid(row.try_get("seller_user_id")?),
                reservation_id: row.try_get("id")?,
                reservation_generation: u64::try_from(generation)
                    .map_err(invariant_decoding("consignment_release_generation"))?,
                equipment: ListingEquipmentIdentity::new(
                    &row.try_get::<String, _>("world_id")?,
                    &row.try_get::<String, _>("canonical_eid")?,
                )
                .map_err(invariant_decoding("consignment_release_equipment"))?,
                credential_ref: item.protected_ref,
            },
            expires_at,
        ));
    }
    Ok(matched)
}

/// 从已解除事实重建相容回执；同命令由账本原样重放。
pub(crate) async fn release_receipt_from_existing(
    command_id: auction_domain::CommandId,
    quarantine: &StoredQuarantine,
    release: &StoredRelease,
    connection: &mut PgConnection,
) -> Result<auction_application::consignment::ReleaseReceipt, WriteError> {
    let mut receipt = auction_application::consignment::ReleaseReceipt::released(
        command_id,
        quarantine.quarantine_id,
        release.release_id,
        release.decision_id,
        release.kind,
    );
    receipt.obligation_id = release.obligation_id;
    receipt.return_mail_id = release.return_mail_id;
    receipt.return_evidence_id = release.return_evidence_id;
    receipt.recovery_id = release.recovery_id;
    receipt.recovery_handoff_id = release.recovery_handoff_id;
    if release.kind == ReleaseKind::Accept {
        receipt.listing_changes = accepted_listing_changes(connection, release.decision_id).await?;
    }
    if release.kind == ReleaseKind::AcquireReturn {
        // 回执形状要求接管四项齐全：从同一最终决定的接管行回填，与首个解除命令的回执一致（不变量 1）。
        // 接管与解除同事务提交，缺行即矛盾，失败关闭（不变量 6）。
        let (intake_id, round, take_work_id, take_delivery_id) =
            sqlx::query_as::<_, (Uuid, i32, Uuid, Uuid)>(
                "SELECT id, original_round, take_work_id, take_delivery_id
                 FROM public.consignment_return_intakes WHERE decision_id = $1",
            )
            .bind(release.decision_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| invariant("consignment_release_intake_missing"))?;
        receipt.return_intake_id = Some(intake_id);
        receipt.original_round = Some(
            u32::try_from(round).map_err(invariant_decoding("consignment_release_intake_round"))?,
        );
        receipt.take_work_id = Some(take_work_id);
        receipt.take_delivery_id = Some(take_delivery_id);
    }
    Ok(receipt)
}

/// 读取整封接受时的逐件 L；状态取接受版本的事件，不使用之后的挂单投影。
pub(crate) async fn accepted_listing_changes(
    connection: &mut PgConnection,
    decision_id: Uuid,
) -> Result<Vec<ListingStateChange>, WriteError> {
    let changes = sqlx::query_as::<_, (Uuid, i64, String)>(
        "SELECT i.listing_id, i.accepted_listing_version, event.to_state
         FROM public.consignment_deposit_items AS i
         JOIN public.consignment_events AS event
           ON event.listing_id = i.listing_id
          AND event.state_version = i.accepted_listing_version
         WHERE i.decision_id = $1 ORDER BY i.ordinal",
    )
    .bind(decision_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(listing_id, version, state)| {
        Ok(ListingStateChange {
            listing_id,
            listing_version: u64::try_from(version)
                .map_err(invariant_decoding("consignment_release_listing_version"))?,
            state: state
                .parse()
                .map_err(invariant_decoding("consignment_release_listing_state"))?,
        })
    })
    .collect::<Result<Vec<_>, WriteError>>()?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id = $1",
    )
    .bind(decision_id)
    .fetch_one(&mut *connection)
    .await?;
    if i64::try_from(changes.len()).ok() != Some(total) {
        return Err(invariant("consignment_release_listing_event_missing"));
    }
    Ok(changes)
}

/// 读取已解除隔离的最终决定；非法分支引用失败关闭。
pub(crate) async fn read_final_decision(
    connection: &mut crate::database::pool::TrackedTransaction<'_, sqlx::Postgres>,
    quarantine_id: Uuid,
) -> Result<AdminFinalDecision, ConsignmentError> {
    let row = sqlx::query(
        "SELECT r.id AS release_id, r.decision_id, r.kind, r.case_return_mail_id,
                r.observation_id, r.case_evidence_id, r.recovery_handoff_id,
                h.recovery_id,
                -- 交原案的义务来自原退件证据（E2 可空），交恢复的义务来自接管事实。
                COALESCE(h.obligation_id, e.obligation_id) AS obligation_id,
                h.return_mail_id AS recovery_return_mail_id, r.released_at
         FROM public.mail_quarantine_releases AS r
         LEFT JOIN public.settlement_recovery_return_handoffs AS h
           ON h.id = r.recovery_handoff_id
         LEFT JOIN public.settlement_return_evidence AS e
           ON e.return_mail_id = r.case_return_mail_id
         WHERE r.quarantine_id = $1",
    )
    .bind(quarantine_id)
    .fetch_optional(&mut **connection)
    .await
    .map_err(storage)?
    .ok_or(ConsignmentError::Unavailable)?;
    let kind_text: String = row.try_get("kind").map_err(storage)?;
    let kind = release_kind(&kind_text).ok_or(ConsignmentError::Unavailable)?;
    let observation_id: Uuid = row.try_get("observation_id").map_err(storage)?;
    let owner = observations::legacy_owner_for_observation(connection, observation_id)
        .await
        .map_err(WriteError::into_application)?;
    let decision_id: Uuid = row.try_get("decision_id").map_err(storage)?;
    let case_return_mail_id: Option<Uuid> = row.try_get("case_return_mail_id").map_err(storage)?;
    let case_evidence_id: Option<Uuid> = row.try_get("case_evidence_id").map_err(storage)?;
    let recovery_handoff_id: Option<Uuid> = row.try_get("recovery_handoff_id").map_err(storage)?;
    let recovery_id: Option<Uuid> = row.try_get("recovery_id").map_err(storage)?;
    let obligation_id: Option<Uuid> = row.try_get("obligation_id").map_err(storage)?;
    let recovery_return_mail_id: Option<Uuid> =
        row.try_get("recovery_return_mail_id").map_err(storage)?;
    let return_mail_id = case_return_mail_id.or(recovery_return_mail_id);
    let internal =
        matches!(kind, ReleaseKind::Accept | ReleaseKind::Return | ReleaseKind::ArchiveEmpty);
    if internal
        && (owner.is_some()
            || return_mail_id.is_some()
            || case_evidence_id.is_some()
            || recovery_id.is_some()
            || recovery_handoff_id.is_some()
            || obligation_id.is_some())
    {
        return Err(ConsignmentError::Unavailable);
    }
    if kind == ReleaseKind::AcquireReturn {
        // 第 4 期没有寄售原轮 intake 写端；缺少分支必填引用时失败关闭，不能补空。
        return Err(ConsignmentError::Unavailable);
    }
    if kind == ReleaseKind::AuctionReturnCase
        && (!is_case_owner(owner)
            || return_mail_id.is_none()
            || case_evidence_id.is_none()
            || recovery_id.is_some()
            || recovery_handoff_id.is_some())
    {
        return Err(ConsignmentError::Unavailable);
    }
    if kind == ReleaseKind::AuctionRecoveryReturn
        && (!is_recovery_owner(owner)
            || return_mail_id.is_none()
            || case_evidence_id.is_some()
            || recovery_id.is_none()
            || recovery_handoff_id.is_none()
            || obligation_id.is_none())
    {
        return Err(ConsignmentError::Unavailable);
    }
    let listing_changes = if kind == ReleaseKind::Accept {
        accepted_listing_changes(connection, decision_id)
            .await
            .map_err(WriteError::into_application)?
    } else {
        Vec::new()
    };
    Ok(AdminFinalDecision {
        release_id: row.try_get("release_id").map_err(storage)?,
        decision_id,
        kind,
        owner,
        released_at: row.try_get("released_at").map_err(storage)?,
        listing_changes,
        obligation_id,
        return_mail_id,
        return_evidence_id: case_evidence_id,
        recovery_id,
        recovery_handoff_id,
        return_intake_id: None,
        original_round: None,
        take_work_id: None,
        take_delivery_id: None,
    })
}

fn is_case_owner(owner: Option<LegacyAuctionOwner>) -> bool {
    matches!(
        owner,
        Some(
            LegacyAuctionOwner::SellerPayout
                | LegacyAuctionOwner::BuyerRefund
                | LegacyAuctionOwner::UnsoldReturn
                | LegacyAuctionOwner::WithdrawnReturn
        )
    )
}

fn is_recovery_owner(owner: Option<LegacyAuctionOwner>) -> bool {
    matches!(
        owner,
        Some(
            LegacyAuctionOwner::WinnerEquipmentCod
                | LegacyAuctionOwner::WinnerPaymentCarrierCod
                | LegacyAuctionOwner::WinnerEquipmentHandoff
        )
    )
}

fn release_kind(value: &str) -> Option<ReleaseKind> {
    Some(match value {
        "accepted" => ReleaseKind::Accept,
        "return_decided" => ReleaseKind::Return,
        "archived_empty" => ReleaseKind::ArchiveEmpty,
        "acquire_return" => ReleaseKind::AcquireReturn,
        "auction_return_case" => ReleaseKind::AuctionReturnCase,
        "auction_recovery_return" => ReleaseKind::AuctionRecoveryReturn,
        _ => return None,
    })
}

fn storage(error: sqlx::Error) -> ConsignmentError {
    WriteError::from(error).into_application()
}

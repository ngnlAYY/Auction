//! 提交一次活围栏的解析观察以及相应挂单、工作和许可事实。

use auction_application::consignment::{
    ApplyListingLookupCommand, AutoCloseEvidence, ConsignmentError, ListingBusinessRejection,
    ListingCommandOutcome, ListingCommandRejection, ListingEquipmentLookupError,
    ListingLookupEffect, ListingLookupSnapshot, ListingOperation, LookupLeaseUpdate,
    LookupObservation, LookupTransientReason, MAX_LISTING_LOOKUP_ATTEMPTS, ResolveListingService,
};
use auction_domain::consignment::{
    ListingChange, ListingError, ListingReason, ListingSnapshot, ListingState, ListingTransition,
    ListingTrigger, check_listing_history, transition_listing,
};
use auction_domain::{LotId, UserId};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::credentials::retire_secret;
use super::errors::{WriteError, invalid_projection};
use super::facts::{ChangedOperation, Decision};
use super::ledger::{Actor, Arbitration, CommandWriter, RequestProof, Target};
use super::receipts;
use super::session::CommandTransaction;
use super::snapshot;
use super::target::{existing, lock_seller, lock_target, now};
use super::termination::{forced_facts, retire_occupants, stage_closure};
use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::accounts::{OwnerMatch, match_current_owner};
use crate::database::consignment::eligibility::history::read_listing_history;
use crate::database::consignment::eligibility::lots::unassociated_collision_lots;
use crate::database::consignment::eligibility::occupancy::{
    EquipmentOccupant, lock_equipment_occupants,
};
use crate::database::consignment::listings::projection::{ListingLockIdentity, LockedListing};
use crate::database::consignment::lookup::control::{LookupControl, lock_control};
use crate::database::consignment::lookup::work::{
    LookupFenceCheck, LookupResolution, MatchedLookupLease, check_lookup_fence, finish_lookup,
};

/// 先重放原观察，再核挂单、工作及许可的活围栏。不变量 1、2、3、5、6、7、8。
pub(super) async fn apply_lookup(
    store: &DatabaseStore,
    cmd: ApplyListingLookupCommand,
) -> Result<ListingCommandOutcome<ListingLookupEffect>, ConsignmentError> {
    existing(store, cmd.command_id, cmd.fence.listing_id(), &cmd, |cmd, identity, tx| {
        Box::pin(execute(cmd, identity, tx))
    })
    .await
}

async fn execute(
    cmd: &ApplyListingLookupCommand,
    identity: Option<&ListingLockIdentity>,
    tx: &mut CommandTransaction<'_>,
) -> Result<ListingCommandOutcome<ListingLookupEffect>, WriteError> {
    let listing_id = cmd.fence.listing_id();
    let mut writer = match tx
        .arbitrate(
            Actor::Worker,
            ListingOperation::ApplyLookup,
            Target::Listing(listing_id),
            RequestProof::Digest(cmd.request_hash),
        )
        .await?
    {
        Arbitration::Replay(stored) => return receipts::lookup(tx.connection(), &stored).await,
        Arbitration::First(writer) => writer,
    };
    let seller = lock_seller(writer.connection(), identity, listing_id).await?;
    // 发布可能清退其他旧单。2E 先按 UUID 一次锁定本件全部活动投影。
    if let (Some(identity), LookupObservation::Equipment(_)) = (identity, &cmd.observation) {
        let sampled_at = now(writer.connection()).await?;
        lock_equipment_occupants(writer.connection(), &identity.equipment, sampled_at).await?;
    }
    let Some(current) = lock_target(writer.connection(), listing_id).await? else {
        return Err(ConsignmentError::LeaseLost.into());
    };
    let control = lock_control(writer.connection()).await?;
    let LookupFenceCheck::Matched(lease) =
        check_lookup_fence(writer.connection(), cmd.fence).await?
    else {
        return Err(ConsignmentError::LeaseLost.into());
    };
    if current.snapshot.state != ListingState::Resolving {
        return Err(ConsignmentError::LeaseLost.into());
    }
    let facts = forced_facts(writer.connection(), &current, seller.as_ref(), &control).await?;
    let as_of = now(writer.connection()).await?;
    match transition_listing(Some(current.snapshot), ListingTrigger::AutoClose, as_of, facts) {
        Ok(ListingTransition::Changed { listing, change }) => {
            return finish_forced(
                writer,
                cmd,
                &current,
                &control,
                lease,
                ForcedClosure { listing, change, written_at: as_of },
                &[],
            )
            .await;
        }
        Err(ListingError::InvalidTransition { action: "auto_close", .. }) => {}
        Err(error) => return Err(invalid_projection(error)),
        Ok(_) => return Err(WriteError::invariant("consignment_lookup_forced_transition")),
    }

    let mut collisions = Vec::new();
    let mut observation = None;
    let (trigger, resolution) = match &cmd.observation {
        LookupObservation::Failed(error) => {
            if let Some(reason) = error.transient_reason() {
                let retry_after = match error {
                    ListingEquipmentLookupError::SourceRateLimited(delay) => *delay,
                    _ => None,
                };
                let resolution = LookupResolution::Transient { reason, retry_after };
                if lease.attempt_count() < i16::from(MAX_LISTING_LOOKUP_ATTEMPTS) {
                    let next_attempt_at =
                        complete(writer.connection(), lease, resolution)
                            .await?
                            .ok_or_else(|| WriteError::invariant("consignment_lookup_retry_at"))?;
                    writer
                        .finish(
                            Decision::LookupRescheduled { listing_id, next_attempt_at, reason },
                            as_of,
                        )
                        .await?;
                    return Ok(ListingCommandOutcome::Accepted(ListingLookupEffect::Rescheduled {
                        next_attempt_at,
                        reason,
                    }));
                }
                (ListingTrigger::LookupRejected(ListingReason::LookupExhausted), resolution)
            } else {
                let reason = match error {
                    ListingEquipmentLookupError::Permanent(reason) => (*reason).into(),
                    ListingEquipmentLookupError::CredentialUnavailable => {
                        ListingReason::CredentialUnavailable
                    }
                    _ => return Err(WriteError::invariant("consignment_lookup_failure")),
                };
                (ListingTrigger::LookupRejected(reason), LookupResolution::Done)
            }
        }
        LookupObservation::Equipment(equipment)
            if equipment.identity() != &current.identity.equipment =>
        {
            (
                ListingTrigger::LookupRejected(ListingReason::SourceIdentityMismatch),
                LookupResolution::Done,
            )
        }
        LookupObservation::Equipment(equipment) => {
            let occupants =
                lock_equipment_occupants(writer.connection(), equipment.identity(), as_of).await?;
            collisions = retire_occupants(&mut writer, &occupants, Some(listing_id), as_of).await?;
            let rejected =
                publication_rejection(writer.connection(), &current, equipment, &occupants).await?;
            observation = Some(equipment);
            let trigger = rejected.map_or(
                ListingTrigger::LookupAccepted(cmd.publication_ttls),
                ListingTrigger::LookupRejected,
            );
            (trigger, LookupResolution::Done)
        }
    };

    let snapshot_id = if let Some(observation) = observation {
        // 挂单代次与已仲裁命令分别受唯一约束；解析命令 UUID 同时可用作独立快照的主键。
        let snapshot_id = cmd.command_id.as_uuid();
        snapshot::insert(
            writer.connection(),
            snapshot_id,
            listing_id,
            cmd.fence.generation(),
            observation,
        )
        .await?;
        Some(snapshot_id)
    } else {
        None
    };
    // 不变量 2、6：快照与占用写入可能跨过解析总截止。最终迁移、更新时间与新期限
    // 使用同一个数据库墙钟；同一事务快照内的强制事实仍按领域优先序裁定。
    let written_at = now(writer.connection()).await?;
    let (listing, change) =
        match transition_listing(Some(current.snapshot), trigger, written_at, facts)
            .map_err(invalid_projection)?
        {
            ListingTransition::Changed { listing, change } => (listing, change),
            ListingTransition::ForcedTermination { listing, change } => {
                return finish_forced(
                    writer,
                    cmd,
                    &current,
                    &control,
                    lease,
                    ForcedClosure { listing, change, written_at },
                    &collisions,
                )
                .await;
            }
            ListingTransition::Unchanged(_) => {
                return Err(WriteError::invariant("consignment_lookup_transition"));
            }
        };
    writer.stage_change(&current, listing, snapshot_id, change, written_at).await?;
    if complete(writer.connection(), lease, resolution).await?.is_some() {
        return Err(WriteError::invariant("consignment_lookup_terminal_work"));
    }
    retire_secret(writer.connection(), listing_id, written_at).await?;
    writer
        .finish_with_collisions(
            Decision::Changed { listing_id, operation: ChangedOperation::ApplyLookup },
            &collisions,
            written_at,
        )
        .await?;
    Ok(ListingCommandOutcome::Accepted(ListingLookupEffect::ListingChanged(listing)))
}

struct ForcedClosure {
    listing: ListingSnapshot,
    change: ListingChange,
    written_at: Timestamp,
}

/// 不变量 1、2、3、5、7、8：关闭归系统命令，解析拒绝归原 Worker 命令，二者同提交。
async fn finish_forced(
    mut writer: CommandWriter<'_, '_>,
    cmd: &ApplyListingLookupCommand,
    current: &LockedListing,
    control: &LookupControl,
    lease: MatchedLookupLease,
    closure: ForcedClosure,
    other_collisions: &[Uuid],
) -> Result<ListingCommandOutcome<ListingLookupEffect>, WriteError> {
    let ForcedClosure { listing, change, written_at } = closure;
    let listing_id = current.listing_id;
    let reason =
        listing.reason.ok_or_else(|| WriteError::invariant("consignment_lookup_forced_reason"))?;
    let evidence = closure_evidence(writer.connection(), current, control, reason).await?;
    // 已收到的 429 仍施加全局冷却；其他活围栏提交完成工作并释放本次许可。
    let resolution = match cmd.observation {
        LookupObservation::Failed(ListingEquipmentLookupError::SourceRateLimited(retry_after)) => {
            LookupResolution::Transient {
                reason: LookupTransientReason::SourceRateLimited,
                retry_after,
            }
        }
        _ => LookupResolution::Done,
    };
    complete(writer.connection(), lease, resolution).await?;
    let (system_id, request_hash) = ResolveListingService::forced_close_identity(
        listing_id,
        current.snapshot.state_version,
        reason,
        evidence,
        cmd.command_id,
    );
    // 本事务已经持有目标挂单锁。系统关闭不另外取会话命令锁，重复命令由全局主键失败关闭。
    let mut system_tx = CommandTransaction {
        command_id: system_id.as_uuid(),
        keys: writer.tx.keys,
        connection: writer.connection(),
    };
    let Arbitration::First(mut system_writer) = system_tx
        .arbitrate(
            Actor::System,
            ListingOperation::AutoClose,
            Target::Listing(listing_id),
            RequestProof::Digest(request_hash),
        )
        .await?
    else {
        return Err(WriteError::invariant("consignment_lookup_system_command_replay"));
    };
    let collision = stage_closure(&mut system_writer, current, listing, change, written_at).await?;
    system_writer
        .finish_with_collisions(
            Decision::Changed { listing_id, operation: ChangedOperation::AutoClose },
            collision.as_slice(),
            written_at,
        )
        .await?;
    let rejection = ListingBusinessRejection::ListingNotActive;
    writer
        .finish_with_collisions(
            Decision::Rejected { listing_id: Some(listing_id), reason: rejection },
            other_collisions,
            written_at,
        )
        .await?;
    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(rejection)))
}

async fn closure_evidence(
    conn: &mut PgConnection,
    current: &LockedListing,
    control: &LookupControl,
    reason: ListingReason,
) -> Result<Option<AutoCloseEvidence>, WriteError> {
    let evidence = match reason {
        ListingReason::AuctionLotCreated => {
            let lot_id = unassociated_collision_lots(conn, current.listing_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| WriteError::invariant("consignment_lookup_collision_evidence"))?;
            AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(lot_id))
        }
        ListingReason::SellerIneligible => {
            return Ok(sqlx::query_scalar(
                "SELECT h.id FROM public.user_blacklist_history h
                 JOIN public.consignment_listings l ON l.seller_user_id = h.user_id
                 WHERE l.id = $1 AND NOT (h.id = ANY(l.blacklist_baseline_ids))
                 ORDER BY h.id LIMIT 1",
            )
            .bind(current.listing_id)
            .fetch_optional(conn)
            .await?
            .map(AutoCloseEvidence::SellerIneligible));
        }
        ListingReason::ServiceDisabled => {
            AutoCloseEvidence::ServiceDisabled(control.enabled_epoch.cast_unsigned())
        }
        ListingReason::LookupDeadline => {
            AutoCloseEvidence::LookupDeadline(current.snapshot.expires_at)
        }
        _ => return Err(WriteError::invariant("consignment_lookup_forced_reason")),
    };
    Ok(Some(evidence))
}

async fn complete(
    conn: &mut PgConnection,
    lease: MatchedLookupLease,
    resolution: LookupResolution,
) -> Result<Option<Timestamp>, WriteError> {
    match finish_lookup(conn, lease, resolution).await? {
        LookupLeaseUpdate::Applied(next) => Ok(next),
        // 最终 CAS 可能晚于首次围栏检查。用错误退出，强制回滚已暂存的投影与清退。
        LookupLeaseUpdate::Lost => Err(ConsignmentError::LeaseLost.into()),
    }
}

async fn publication_rejection(
    conn: &mut PgConnection,
    current: &LockedListing,
    snapshot: &ListingLookupSnapshot,
    occupants: &[EquipmentOccupant],
) -> Result<Option<ListingReason>, WriteError> {
    if !snapshot.details().label().tradeable {
        return Ok(Some(ListingReason::NotTradeable));
    }
    let seller = UserId::from_uuid(current.identity.seller_user_id);
    if match_current_owner(conn, seller, snapshot.owner_name()).await? != OwnerMatch::Seller {
        return Ok(Some(ListingReason::OwnerMismatch));
    }
    let history = read_listing_history(conn, snapshot.identity()).await?;
    match check_listing_history(snapshot.identity(), seller, &history.aliases) {
        Ok(()) => {}
        Err(ListingError::HistoryAliasConflict) => {
            return Ok(Some(ListingReason::HistoryAliasConflict));
        }
        Err(ListingError::HistoryUnproven) => return Ok(Some(ListingReason::HistoryUnproven)),
        Err(ListingError::HistoryOwnerMismatch) => {
            return Ok(Some(ListingReason::HistoryOwnerMismatch));
        }
        Err(ListingError::EquipmentBusy) => return Ok(Some(ListingReason::EquipmentBusy)),
        Err(error) => return Err(invalid_projection(error)),
    }
    Ok(occupants
        .iter()
        .any(|row| {
            row.listing_id != current.listing_id && row.published && row.forced_reason.is_none()
        })
        .then_some(ListingReason::EquipmentBusy))
}

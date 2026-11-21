//! 提交一次活围栏的解析观察以及相应挂单、工作和许可事实。

use auction_application::consignment::{
    ApplyListingLookupCommand, ConsignmentError, ListingCommandOutcome,
    ListingEquipmentLookupError, ListingLookupEffect, ListingLookupSnapshot, ListingOperation,
    LookupLeaseUpdate, LookupObservation, LookupTransientReason, MAX_LISTING_LOOKUP_ATTEMPTS,
};
use auction_domain::UserId;
use auction_domain::consignment::{
    ForcedTerminationFacts, ListingChange, ListingError, ListingReason, ListingState,
    ListingTransition, ListingTrigger, check_listing_history, transition_listing,
};
use auction_time::Timestamp;
use sqlx::PgConnection;

use super::credentials::retire_secret;
use super::errors::{WriteError, invalid_projection};
use super::facts::{ChangedOperation, Decision};
use super::ledger::{Actor, Arbitration, RequestProof, Target};
use super::receipts;
use super::session::CommandTransaction;
use super::snapshot;
use super::target::{existing, lock_seller, lock_target, now};
use super::termination::{forced_facts, retire_occupants, stage_closure};
use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::accounts::{OwnerMatch, match_current_owner};
use crate::database::consignment::eligibility::history::read_listing_history;
use crate::database::consignment::eligibility::occupancy::{
    EquipmentOccupant, lock_equipment_occupants,
};
use crate::database::consignment::listings::projection::{ListingLockIdentity, LockedListing};
use crate::database::consignment::lookup::control::lock_control;
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
            // 000117 限 apply_lookup 记 200，采用已提交的系统终态回执，绝不接受发布。
            // 若确实已收到 429，同时保留有效请求施加的全局冷却。
            let resolution = match cmd.observation {
                LookupObservation::Failed(ListingEquipmentLookupError::SourceRateLimited(
                    retry_after,
                )) => LookupResolution::Transient {
                    reason: LookupTransientReason::SourceRateLimited,
                    retry_after,
                },
                _ => LookupResolution::Done,
            };
            complete(writer.connection(), lease, resolution).await?;
            let collision = stage_closure(&mut writer, &current, listing, change, as_of).await?;
            writer
                .finish_with_collisions(
                    Decision::Changed { listing_id, operation: ChangedOperation::ApplyLookup },
                    collision.as_slice(),
                    as_of,
                )
                .await?;
            return Ok(ListingCommandOutcome::Accepted(ListingLookupEffect::ListingChanged(
                listing,
            )));
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

    let ListingTransition::Changed { listing, change } = transition_listing(
        Some(current.snapshot),
        trigger,
        as_of,
        ForcedTerminationFacts::default(),
    )
    .map_err(invalid_projection)?
    else {
        return Err(WriteError::invariant("consignment_lookup_transition"));
    };
    let snapshot_id = if matches!(
        change,
        ListingChange::Published | ListingChange::Rejected { keep_snapshot: true }
    ) {
        let observation =
            observation.ok_or_else(|| WriteError::invariant("consignment_lookup_snapshot"))?;
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
    writer.stage_change(&current, listing, snapshot_id, change, as_of).await?;
    if complete(writer.connection(), lease, resolution).await?.is_some() {
        return Err(WriteError::invariant("consignment_lookup_terminal_work"));
    }
    retire_secret(writer.connection(), listing_id, as_of).await?;
    writer
        .finish_with_collisions(
            Decision::Changed { listing_id, operation: ChangedOperation::ApplyLookup },
            &collisions,
            as_of,
        )
        .await?;
    Ok(ListingCommandOutcome::Accepted(ListingLookupEffect::ListingChanged(listing)))
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

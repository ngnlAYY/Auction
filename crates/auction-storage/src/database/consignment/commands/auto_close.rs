//! 复核独立系统命令的证据并提交强制终止或补记指定碰撞。

use auction_application::consignment::{
    AutoCloseEvidence, AutoCloseListingCommand, ConsignmentError, ListingAutoCloseEffect,
    ListingCommandOutcome, ListingOperation,
};
use auction_domain::consignment::{
    ListingError, ListingReason, ListingState, ListingTransition, ListingTrigger,
    transition_listing,
};
use auction_time::Timestamp;
use sqlx::PgConnection;

use super::errors::{WriteError, invalid_projection};
use super::facts::{ChangedOperation, Decision};
use super::ledger::{Actor, Arbitration, RequestProof, Target};
use super::receipts;
use super::session::CommandTransaction;
use super::target::{existing, lock_seller, lock_target, now};
use super::termination::{forced_facts, stage_closure};
use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::lots::{
    INSERT_OBSERVED_COLLISION_SQL, observed_collision_is_unassociated,
};
use crate::database::consignment::listings::projection::{ListingLockIdentity, LockedListing};
use crate::database::consignment::lookup::control::{LookupControl, lock_control};

/// 系统命令 ID 和规范摘要由应用层派生。存储在锁内重证候选，不变量 1、2、3、6、8。
pub(super) async fn auto_close(
    store: &DatabaseStore,
    cmd: AutoCloseListingCommand,
) -> Result<ListingCommandOutcome<ListingAutoCloseEffect>, ConsignmentError> {
    existing(store, cmd.command_id, cmd.candidate.listing_id, &cmd, |cmd, identity, tx| {
        Box::pin(execute(cmd, identity, tx))
    })
    .await
}

enum Observation {
    ConditionNotMet,
    Confirmed,
    UnassociatedLot,
}

async fn execute(
    cmd: &AutoCloseListingCommand,
    identity: Option<&ListingLockIdentity>,
    tx: &mut CommandTransaction<'_>,
) -> Result<ListingCommandOutcome<ListingAutoCloseEffect>, WriteError> {
    let listing_id = cmd.candidate.listing_id;
    let mut writer = match tx
        .arbitrate(
            Actor::System,
            ListingOperation::AutoClose,
            Target::Listing(listing_id),
            RequestProof::Digest(cmd.request_hash),
        )
        .await?
    {
        Arbitration::Replay(stored) => {
            return receipts::auto_closed(tx.connection(), &stored).await;
        }
        Arbitration::First(writer) => writer,
    };
    let seller = lock_seller(writer.connection(), identity, listing_id).await?;
    let Some(current) = lock_target(writer.connection(), listing_id).await? else {
        return Err(ConsignmentError::ConditionNotMet.into());
    };
    let control = lock_control(writer.connection()).await?;
    let facts = forced_facts(writer.connection(), &current, seller.as_ref(), &control).await?;
    let now = now(writer.connection()).await?;
    let evidence = cmd.candidate.evidence;
    let observed =
        observe(writer.connection(), &current, evidence, &control, facts.seller_ineligible, now)
            .await?;
    if matches!(observed, Observation::ConditionNotMet) {
        return Err(ConsignmentError::ConditionNotMet.into());
    }
    match transition_listing(Some(current.snapshot), ListingTrigger::AutoClose, now, facts) {
        Ok(ListingTransition::Changed { listing, change }) => {
            let collision = stage_closure(&mut writer, &current, listing, change, now).await?;
            writer
                .finish_with_collisions(
                    Decision::Changed { listing_id, operation: ChangedOperation::AutoClose },
                    collision.as_slice(),
                    now,
                )
                .await?;
            Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Terminated(listing)))
        }
        Err(ListingError::InvalidTransition { action: "auto_close", .. })
            if !current.snapshot.state.is_active() =>
        {
            let conflicts_recorded = matches!(observed, Observation::UnassociatedLot);
            writer
                .finish(Decision::AutoCloseObserved { listing_id, conflicts_recorded }, now)
                .await?;
            if let (true, AutoCloseEvidence::AuctionLotCreated(lot_id)) =
                (conflicts_recorded, evidence)
            {
                let inserted = sqlx::query(INSERT_OBSERVED_COLLISION_SQL)
                    .bind(listing_id)
                    .bind(now)
                    .bind(cmd.command_id.as_uuid())
                    .bind(lot_id.as_uuid())
                    .execute(tx.connection())
                    .await?;
                if inserted.rows_affected() != 1 {
                    return Err(WriteError::invariant("consignment_observed_lot_missing"));
                }
            }
            Ok(ListingCommandOutcome::Accepted(if conflicts_recorded {
                ListingAutoCloseEffect::ConflictsRecorded
            } else {
                ListingAutoCloseEffect::Unchanged
            }))
        }
        Err(ListingError::InvalidTransition { action: "auto_close", .. }) => {
            Err(ConsignmentError::ConditionNotMet.into())
        }
        Err(error) => Err(invalid_projection(error)),
        Ok(_) => Err(WriteError::invariant("consignment_auto_close_transition")),
    }
}

async fn observe(
    conn: &mut PgConnection,
    listing: &LockedListing,
    evidence: AutoCloseEvidence,
    control: &LookupControl,
    seller_ineligible: bool,
    now: Timestamp,
) -> Result<Observation, WriteError> {
    let confirmed = match evidence {
        AutoCloseEvidence::AuctionLotCreated(lot_id) => {
            if observed_collision_is_unassociated(conn, listing.listing_id, lot_id.as_uuid())
                .await?
            {
                return Ok(Observation::UnassociatedLot);
            }
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public.consignment_listing_auction_conflicts
                  WHERE listing_id = $1 AND lot_id = $2)",
            )
            .bind(listing.listing_id)
            .bind(lot_id.as_uuid())
            .fetch_one(conn)
            .await?
        }
        AutoCloseEvidence::SellerIneligible(fact_id) => {
            // 端口只定义拉黑历史 UUID，没有独立的账号停用证据身份。无可信历史 UUID 时
            // 不占系统命令 ID。用登记时冻结的集合判定新旧，业务时刻不代表提交顺序。
            (seller_ineligible || listing.snapshot.reason == Some(ListingReason::SellerIneligible))
                && sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM public.user_blacklist_history h
                      JOIN public.consignment_listings l ON h.user_id = COALESCE(l.seller_user_id,
                          (SELECT u.id FROM public.users u
                           WHERE lower(u.username) = lower(l.seller_game_name)))
                      WHERE h.id = $1 AND l.id = $2 AND NOT (h.id = ANY(l.blacklist_baseline_ids)))",
                )
                .bind(fact_id)
                .bind(listing.listing_id)
                .fetch_one(conn)
                .await?
        }
        AutoCloseEvidence::ServiceDisabled(epoch) => {
            epoch > listing.service_epoch.cast_unsigned()
                && epoch <= control.enabled_epoch.cast_unsigned()
        }
        AutoCloseEvidence::LookupDeadline(deadline) => deadline_confirmed(
            listing,
            deadline,
            now,
            ListingState::Resolving,
            ListingReason::LookupDeadline,
        ),
        AutoCloseEvidence::DisplayExpired(deadline) => deadline_confirmed(
            listing,
            deadline,
            now,
            ListingState::Displayed,
            ListingReason::DisplayExpired,
        ),
        AutoCloseEvidence::DepositExpired(deadline) => deadline_confirmed(
            listing,
            deadline,
            now,
            ListingState::AwaitingDeposit,
            ListingReason::DepositExpired,
        ),
    };
    Ok(if confirmed { Observation::Confirmed } else { Observation::ConditionNotMet })
}

fn deadline_confirmed(
    listing: &LockedListing,
    deadline: Timestamp,
    now: Timestamp,
    state: ListingState,
    reason: ListingReason,
) -> bool {
    // 已持久过期仍保留冻结截止和原因，可用于独立观察的原样重放。
    (listing.snapshot.state == state || listing.snapshot.reason == Some(reason))
        && listing.snapshot.expires_at == deadline
        && now >= deadline
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::{
        FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingReason,
        ListingSnapshot, ListingState, ListingVersion,
    };
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::deadline_confirmed;
    use crate::database::consignment::listings::projection::{ListingLockIdentity, LockedListing};

    #[test]
    fn cutoff_equality_requires_exact_frozen_deadline_and_state() {
        let deadline: Timestamp = "2026-01-02T00:00:00Z".parse().unwrap();
        let before = deadline - Duration::microseconds(1);
        let row = LockedListing {
            listing_id: Uuid::from_u128(1),
            identity: ListingLockIdentity {
                seller_user_id: Uuid::from_u128(2),
                seller_game_name: None,
                equipment: ListingEquipmentIdentity::new("main", "42").unwrap(),
            },
            snapshot: ListingSnapshot {
                mode: ListingMode::Direct,
                state: ListingState::Resolving,
                price: ListingPrice::new(101).unwrap(),
                state_version: ListingVersion::FIRST,
                expires_at: deadline,
                reason: None,
            },
            current_snapshot_id: None,
            fee_bps: FeeBps::DIRECT,
            service_epoch: 1,
            blacklist_count_at_register: 0,
            updated_at: deadline - Duration::days(1),
        };
        assert!(!deadline_confirmed(
            &row,
            deadline,
            before,
            ListingState::Resolving,
            ListingReason::LookupDeadline
        ));
        assert!(!deadline_confirmed(
            &row,
            before,
            deadline,
            ListingState::Resolving,
            ListingReason::LookupDeadline
        ));
        assert!(!deadline_confirmed(
            &row,
            deadline,
            deadline,
            ListingState::Displayed,
            ListingReason::DisplayExpired
        ));
        assert!(deadline_confirmed(
            &row,
            deadline,
            deadline,
            ListingState::Resolving,
            ListingReason::LookupDeadline
        ));
    }
}

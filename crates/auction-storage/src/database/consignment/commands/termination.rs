//! 强制终止、旧占用清退及同事务追加碰撞关联。

use std::collections::BTreeMap;

use auction_application::consignment::{
    ListingBusinessRejection, ListingCommandOutcome, ListingCommandRejection,
};
use auction_domain::consignment::{
    ForcedTerminationFacts, ListingChange, ListingReason, ListingSnapshot, ListingTransition,
    ListingTrigger, transition_listing,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::credentials::retire_secret;
use super::errors::{WriteError, invalid_projection};
use super::facts::Decision;
use super::ledger::CommandWriter;
use crate::database::consignment::eligibility::accounts::SellerEligibility;
use crate::database::consignment::eligibility::lots::{
    INSERT_COLLISION_SET_SQL, listing_collision,
};
use crate::database::consignment::eligibility::occupancy::EquipmentOccupant;
use crate::database::consignment::listings::projection::{LockedListing, lock_listings};
use crate::database::consignment::lookup::control::LookupControl;
use crate::database::consignment::lookup::work::cancel_lookup;

/// 按同一事务快照收集目标强制事实，交给领域决定优先序。不变量 2、6。
pub(super) async fn forced_facts(
    conn: &mut PgConnection,
    listing: &LockedListing,
    seller: Option<&SellerEligibility>,
    control: &LookupControl,
) -> Result<ForcedTerminationFacts, WriteError> {
    if !listing.snapshot.state.is_active() {
        return Ok(ForcedTerminationFacts::default());
    }
    Ok(ForcedTerminationFacts {
        auction_lot_created: listing_collision(conn, listing.listing_id)
            .await?
            .ok_or_else(|| WriteError::invariant("consignment_locked_listing_missing"))?,
        seller_ineligible: !seller.is_some_and(|seller| {
            seller.active
                && !seller.active_blacklist
                && seller.blacklist_count == listing.blacklist_count_at_register
        }),
        service_disabled: !control.admitting || control.enabled_epoch != listing.service_epoch,
    })
}

/// 系统终结与工作撤销、登记信封退休同步。不变量 2、3、5、7、8。
pub(super) async fn stage_closure(
    writer: &mut CommandWriter<'_, '_>,
    current: &LockedListing,
    listing: ListingSnapshot,
    change: ListingChange,
    now: Timestamp,
) -> Result<Option<Uuid>, WriteError> {
    writer.stage_system_closure(current, listing, change, now).await?;
    close_resources(writer.connection(), current.listing_id, now).await?;
    Ok((listing.reason == Some(ListingReason::AuctionLotCreated)).then_some(current.listing_id))
}

/// 主动关闭与系统清退共用已锁单例下的工作撤销及凭证退休。
pub(super) async fn close_resources(
    conn: &mut PgConnection,
    listing_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    cancel_lookup(conn, listing_id).await?;
    retire_secret(conn, listing_id, now).await
}

/// 把强制终止事实和本人或 Admin 的确定拒绝一起定案。不变量 1、3、8。
pub(super) async fn finish_forced_rejection<T>(
    mut writer: CommandWriter<'_, '_>,
    current: &LockedListing,
    listing: ListingSnapshot,
    change: ListingChange,
    now: Timestamp,
) -> Result<ListingCommandOutcome<T>, WriteError> {
    let collision = stage_closure(&mut writer, current, listing, change, now).await?;
    // 强制关闭同样结束该挂单的有效预约：否则预约码仍可读、实物占用不放。
    let command_id = writer.tx.command_id;
    crate::database::consignment::eligibility::claims::cancel_active_reservations(
        writer.connection(),
        current.listing_id,
        command_id,
        now,
    )
    .await?;
    writer
        .finish_with_collisions(
            Decision::Rejected {
                listing_id: Some(current.listing_id),
                reason: ListingBusinessRejection::ListingNotActive,
            },
            collision.as_slice(),
            now,
        )
        .await?;
    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
        ListingBusinessRejection::ListingNotActive,
    )))
}

/// 在 2E 已按 UUID 锁住的同件占用集合中，仅清退它判为逻辑失效的其他挂单。
pub(super) async fn retire_occupants(
    writer: &mut CommandWriter<'_, '_>,
    occupants: &[EquipmentOccupant],
    target: Option<Uuid>,
    now: Timestamp,
) -> Result<Vec<Uuid>, WriteError> {
    let expired: BTreeMap<_, _> = occupants
        .iter()
        .filter(|row| Some(row.listing_id) != target)
        .filter_map(|row| row.forced_reason.map(|reason| (row.listing_id, reason)))
        .collect();
    if expired.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<_> = expired.keys().copied().collect();
    let rows = lock_listings(writer.connection(), &ids).await?;
    let mut collisions = Vec::new();
    let mut retired = Vec::new();
    for current in &rows {
        let reason = expired
            .get(&current.listing_id)
            .copied()
            .ok_or_else(|| WriteError::invariant("consignment_occupant_reason"))?;
        // 代管态只由寄售代管生命周期收口：旧 v1 迁移对它们一律拒绝，不能因此中止本命令。
        if current.snapshot.state.is_custody_managed() {
            continue;
        }
        let facts = ForcedTerminationFacts {
            auction_lot_created: reason == ListingReason::AuctionLotCreated,
            seller_ineligible: reason == ListingReason::SellerIneligible,
            service_disabled: reason == ListingReason::ServiceDisabled,
        };
        let ListingTransition::Changed { listing, change } =
            transition_listing(Some(current.snapshot), ListingTrigger::AutoClose, now, facts)
                .map_err(invalid_projection)?
        else {
            return Err(WriteError::invariant("consignment_occupant_transition"));
        };
        if listing.reason != Some(reason) {
            return Err(WriteError::invariant("consignment_occupant_reason_mismatch"));
        }
        writer.stage_system_closure(current, listing, change, now).await?;
        cancel_lookup(writer.connection(), current.listing_id).await?;
        if reason == ListingReason::AuctionLotCreated {
            collisions.push(current.listing_id);
        }
        retired.push(current.listing_id);
    }
    if rows.len() != ids.len() {
        return Err(WriteError::invariant("consignment_occupant_missing"));
    }
    // 先处理全部相关工作，再退休凭证。按 UUID 保持同件多行的锁顺序。
    for listing_id in retired {
        retire_secret(writer.connection(), listing_id, now).await?;
    }
    Ok(collisions)
}

/// 在原回执行写入后按挂单一次集合追加，而非逐 lot 往返。不变量 2、3、8。
pub(super) async fn append_collision_sets(
    conn: &mut PgConnection,
    command_id: Uuid,
    listing_ids: &[Uuid],
    now: Timestamp,
) -> Result<(), WriteError> {
    for &listing_id in listing_ids {
        sqlx::query(INSERT_COLLISION_SET_SQL)
            .bind(listing_id)
            .bind(now)
            .bind(command_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

//! 从白名单列解码装备、挂单与工作事实，交给端口构造器核验状态组合。

use auction_application::consignment::{
    ConsignmentError, ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel,
    ListingEquipmentStat, ListingEquipmentStatGroup, ListingLookupProgress, LookupTransientReason,
    PrivateListing, PrivateListingView, PrivateListingViewKind,
};
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingPrice, ListingReason, ListingState, ListingVersion,
};
use serde::Deserialize;
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

use crate::database::consignment::reads::{storage_error, unavailable};

pub(super) const EQUIPMENT_JOIN: &str = " LEFT JOIN public.consignment_snapshots p
    ON p.listing_id = l.id AND p.id = l.current_snapshot_id";
pub(super) const WORK_JOIN: &str = " LEFT JOIN public.consignment_lookup_work w
    ON w.listing_id = l.id AND w.generation = 1";
pub(super) const PRIVATE_COLUMNS: &str = "SELECT l.id, l.world_id, l.external_equipment_id,
    l.mode, l.state, l.reason_code, l.state_version, l.price, l.fee_bps, l.pricing_version,
    l.created_at, l.updated_at, l.expires_at, l.current_snapshot_id, effective.forced_reason,
    p.name, p.category, p.alt_name, p.level_kind, p.level_number, p.tradeable, p.observed_at,
    p.stat_groups, w.attempt_count, w.next_attempt_at, w.last_reason,
    l.seller_user_id IS NOT NULL AS seller_verified";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stat {
    name: String,
    value: f64,
    base: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatGroup {
    name: String,
    stats: Vec<Stat>,
}

pub(super) fn equipment(
    row: &PgRow,
    with_stats: bool,
) -> Result<ListingEquipmentDetails, ConsignmentError> {
    let kind: &str = row.try_get("level_kind").map_err(storage_error)?;
    let number: Option<i32> = row.try_get("level_number").map_err(storage_error)?;
    let level = match (kind, number) {
        ("assigned", Some(number)) => ListingEquipmentLevel::Assigned(number.cast_unsigned()),
        ("unassigned", None) => ListingEquipmentLevel::Unassigned,
        ("soulbound", None) => ListingEquipmentLevel::Soulbound,
        _ => return Err(ConsignmentError::Unavailable),
    };
    let stat_groups = if with_stats {
        let Json(groups): Json<Vec<StatGroup>> =
            row.try_get("stat_groups").map_err(storage_error)?;
        groups
            .into_iter()
            .map(|group| ListingEquipmentStatGroup {
                name: group.name,
                stats: group
                    .stats
                    .into_iter()
                    .map(|Stat { name, value, base }| ListingEquipmentStat { name, value, base })
                    .collect(),
            })
            .collect()
    } else {
        Vec::new()
    };
    ListingEquipmentDetails::new(
        row.try_get("observed_at").map_err(storage_error)?,
        ListingEquipmentLabel {
            name: row.try_get("name").map_err(storage_error)?,
            alt_name: row.try_get("alt_name").map_err(storage_error)?,
            category: row.try_get("category").map_err(storage_error)?,
            level,
            tradeable: row.try_get("tradeable").map_err(storage_error)?,
        },
        stat_groups,
    )
    .map_err(unavailable)
}

fn lookup_reason(row: &PgRow) -> Result<Option<LookupTransientReason>, ConsignmentError> {
    match row.try_get("last_reason").map_err(storage_error)? {
        None => Ok(None),
        Some("source_unavailable") => Ok(Some(LookupTransientReason::SourceUnavailable)),
        Some("source_timeout") => Ok(Some(LookupTransientReason::SourceTimeout)),
        Some("source_rate_limited") => Ok(Some(LookupTransientReason::SourceRateLimited)),
        Some(_) => Err(ConsignmentError::Unavailable),
    }
}

pub(super) fn private(row: &PgRow, paused: bool) -> Result<PrivateListing, ConsignmentError> {
    let mode =
        row.try_get::<&str, _>("mode").map_err(storage_error)?.parse().map_err(unavailable)?;
    let stored =
        row.try_get::<&str, _>("state").map_err(storage_error)?.parse().map_err(unavailable)?;
    let forced: Option<&str> = row.try_get("forced_reason").map_err(storage_error)?;
    let persisted: Option<&str> = row.try_get("reason_code").map_err(storage_error)?;
    let reason = forced.or(persisted).map(ListingReason::parse).transpose().map_err(unavailable)?;
    let effective = if forced.is_some() {
        reason
            .ok_or(ConsignmentError::Unavailable)?
            .terminal_state()
            .parse()
            .map_err(unavailable)?
    } else {
        stored
    };
    let snapshot_id: Option<Uuid> = row.try_get("current_snapshot_id").map_err(storage_error)?;
    let equipment = snapshot_id.map(|_| equipment(row, true)).transpose()?;
    let expires_at = row.try_get("expires_at").map_err(storage_error)?;
    let kind = match (effective, reason, equipment) {
        (ListingState::Resolving, None, None) => {
            let attempts: i16 = row.try_get("attempt_count").map_err(storage_error)?;
            PrivateListingViewKind::Resolving(ListingLookupProgress::new(
                u8::try_from(attempts).map_err(unavailable)?,
                row.try_get("next_attempt_at").map_err(storage_error)?,
                lookup_reason(row)?,
                paused,
                expires_at,
            )?)
        }
        (ListingState::Displayed, None, Some(equipment)) => {
            PrivateListingViewKind::Displayed(equipment)
        }
        (ListingState::AwaitingDeposit, None, Some(equipment)) => {
            PrivateListingViewKind::AwaitingDeposit(equipment)
        }
        (ListingState::Rejected, Some(reason), Some(equipment)) => {
            PrivateListingViewKind::Ineligible { equipment, reason }
        }
        (ListingState::Rejected, Some(reason), None) => {
            // 工作行可能保留上一次暂时失败。只有重试耗尽的挂单展示这一分类。
            let last_lookup_reason =
                if reason == ListingReason::LookupExhausted { lookup_reason(row)? } else { None };
            PrivateListingViewKind::LookupFailed { reason, last_lookup_reason }
        }
        (ListingState::Closed, Some(reason), equipment) => {
            PrivateListingViewKind::Closed { reason, equipment }
        }
        (ListingState::Expired, Some(reason), equipment) => {
            PrivateListingViewKind::Expired { reason, equipment }
        }
        _ => return Err(ConsignmentError::Unavailable),
    };
    let pricing_version: i16 = row.try_get("pricing_version").map_err(storage_error)?;
    if pricing_version != 1 {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(PrivateListing {
        listing_id: row.try_get("id").map_err(storage_error)?,
        version: ListingVersion::new(
            row.try_get::<i64, _>("state_version").map_err(storage_error)?.cast_unsigned(),
        )
        .map_err(unavailable)?,
        identity: ListingEquipmentIdentity::new(
            row.try_get("world_id").map_err(storage_error)?,
            row.try_get("external_equipment_id").map_err(storage_error)?,
        )
        .map_err(unavailable)?,
        price: ListingPrice::new(row.try_get("price").map_err(storage_error)?)
            .map_err(unavailable)?,
        pricing_version: pricing_version.cast_unsigned(),
        fee_bps: FeeBps::new(i64::from(row.try_get::<i16, _>("fee_bps").map_err(storage_error)?))
            .map_err(unavailable)?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
        expires_at,
        view: PrivateListingView::new(mode, stored, kind)?,
    })
}

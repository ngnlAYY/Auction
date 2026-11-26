//! v2 行投影：v1 可表达的状态交给 v1 解码器核验，代管态在这里解码并由端口构造器核验形状。

use auction_application::consignment::{
    ActiveHold, ConsignmentError, CustodyFacts, PositionSummary, PrivateListingV2,
    PrivateListingViewV2,
};
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::{
    FeeBps, HeldOffMarketReason, ListingEquipmentIdentity, ListingPrice, ListingReason,
    ListingState, ListingVersion,
};
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::database::consignment::reads::projection::{equipment, private};
use crate::database::consignment::reads::{storage_error, unavailable};

fn custody_facts(row: &PgRow) -> Result<CustodyFacts, ConsignmentError> {
    let position = row
        .try_get::<Option<uuid::Uuid>, _>("position_id")
        .map_err(storage_error)?
        .map(|position_id| {
            let state: &str = row.try_get("position_state").map_err(storage_error)?;
            let generation: Option<i64> =
                row.try_get("position_generation").map_err(storage_error)?;
            Ok::<_, ConsignmentError>(PositionSummary {
                position_id,
                state: PositionState::parse(state).ok_or(ConsignmentError::Unavailable)?,
                version: row
                    .try_get::<i64, _>("position_version")
                    .map_err(storage_error)?
                    .cast_unsigned(),
                generation: generation.map(i64::cast_unsigned),
            })
        })
        .transpose()?;
    let hold = row
        .try_get::<Option<uuid::Uuid>, _>("hold_id")
        .map_err(storage_error)?
        .map(|hold_id| {
            Ok::<_, ConsignmentError>(ActiveHold {
                hold_id,
                set_at: row.try_get("hold_set_at").map_err(storage_error)?,
            })
        })
        .transpose()?;
    Ok(CustodyFacts {
        position,
        hold,
        withdrawal_intent_registered: row.try_get("withdrawal_intent").map_err(storage_error)?,
        loss_case_id: row.try_get("loss_case_id").map_err(storage_error)?,
        // 重列资格不在行投影里：只有本人详情在同一读快照内按写端判据另行填充。
        relist_allowed: false,
    })
}

/// 有效态在 v1 闭集内（含 available 被强制终结后的终态）时，沿用 v1 解码器及其全部形状核验。
fn is_legacy_effective(state: ListingState) -> bool {
    matches!(
        state,
        ListingState::Resolving
            | ListingState::Displayed
            | ListingState::AwaitingDeposit
            | ListingState::Rejected
            | ListingState::Expired
            | ListingState::Closed
    )
}

pub(super) fn private_v2(row: &PgRow, paused: bool) -> Result<PrivateListingV2, ConsignmentError> {
    let stored: ListingState =
        row.try_get::<&str, _>("state").map_err(storage_error)?.parse().map_err(unavailable)?;
    let forced: Option<&str> = row.try_get("forced_reason").map_err(storage_error)?;
    let persisted: Option<&str> = row.try_get("reason_code").map_err(storage_error)?;
    let effective = match forced {
        Some(code) => ListingReason::parse(code)
            .map_err(unavailable)?
            .terminal_state()
            .parse()
            .map_err(unavailable)?,
        None => stored,
    };
    let custody = custody_facts(row)?;
    let seller_verified = row.try_get("seller_verified").map_err(storage_error)?;
    if is_legacy_effective(effective) {
        return PrivateListingV2::from_legacy(private(row, paused)?, seller_verified, custody);
    }
    // 强制终结只作用于 v1 活动态与 available，代管态的有效态必等于持久态。
    if forced.is_some() {
        return Err(ConsignmentError::Unavailable);
    }
    let details = equipment(row, true)?;
    let view = match stored {
        ListingState::DepositAccepted => PrivateListingViewV2::DepositAccepted(details),
        ListingState::Available => PrivateListingViewV2::Available(details),
        ListingState::SalePending => PrivateListingViewV2::SalePending(details),
        ListingState::WithdrawalPending => PrivateListingViewV2::WithdrawalPending(details),
        ListingState::HeldOffMarket => PrivateListingViewV2::HeldOffMarket {
            equipment: details,
            reason: HeldOffMarketReason::parse(persisted.ok_or(ConsignmentError::Unavailable)?)
                .map_err(unavailable)?,
        },
        _ => return Err(ConsignmentError::Unavailable),
    };
    let pricing_version: i16 = row.try_get("pricing_version").map_err(storage_error)?;
    if pricing_version != 1 {
        return Err(ConsignmentError::Unavailable);
    }
    PrivateListingV2 {
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
        expires_at: row.try_get("expires_at").map_err(storage_error)?,
        view,
        seller_verified,
        custody,
    }
    .validated()
}

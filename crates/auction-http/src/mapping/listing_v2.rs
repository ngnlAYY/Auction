//! 本人与 Admin 共用的 v2 挂单投影：应用层记录到 `OwnedListingV2`。
//!
//! 允许动作由调用方传入：本人传 `PrivateListingV2::seller_actions`，Admin 传空（运营动作在
//! Admin 记录的 `admin_actions`）。本模块不判断任何动作规则，也不读取预约码或装备 key。

use auction_application::consignment::{
    ConsignmentError, ListingEquipmentDetails, ListingEquipmentLevel, MAX_LISTING_LOOKUP_ATTEMPTS,
    PrivateListingV2, PrivateListingViewKind, PrivateListingViewV2,
};
use auction_contracts::{
    EquipmentLevel, EquipmentStat, EquipmentStatGroup, EquipmentView, EquipmentViewSource,
    ListingFeeBps, ListingGeneration, ListingPrice, ListingPricingVersion, ListingVersion,
    ListingViewV2, LookupProgress, OwnedListingV2,
};
use auction_domain::consignment::deposit::SellerAction;
use auction_domain::consignment::quote_v1;
use serde::de::DeserializeOwned;
use serde::de::value::StrDeserializer;

/// 投影一条 v2 记录；任何字段越界或闭集漂移都失败关闭。
pub(crate) fn owned_v2(
    listing: &PrivateListingV2,
    allowed_actions: &[SellerAction],
) -> Result<OwnedListingV2, ConsignmentError> {
    let quote = quote_v1(listing.price, listing.fee_bps);
    Ok(OwnedListingV2 {
        listing_id: listing.listing_id,
        version: ListingVersion::new(wire_integer(listing.version.value())?)
            .ok_or(ConsignmentError::Unavailable)?,
        world_id: wire_code(listing.identity.world_id())?,
        external_equipment_id: listing.identity.external_equipment_id().to_owned(),
        mode: wire_code(listing.view.mode().as_str())?,
        price: ListingPrice::new(listing.price.value()).ok_or(ConsignmentError::Unavailable)?,
        pricing_version: ListingPricingVersion::new(i64::from(listing.pricing_version))
            .ok_or(ConsignmentError::Unavailable)?,
        fee_bps: ListingFeeBps::new(i64::from(listing.fee_bps.value()))
            .ok_or(ConsignmentError::Unavailable)?,
        estimated_fee: quote.fee,
        estimated_seller_amount: quote.seller_amount,
        created_at: listing.created_at,
        updated_at: listing.updated_at,
        expires_at: listing.expires_at,
        generation: listing
            .generation()
            .map(|generation| {
                ListingGeneration::new(wire_integer(generation)?)
                    .ok_or(ConsignmentError::Unavailable)
            })
            .transpose()?,
        position_id: listing.custody.position.map(|position| position.position_id),
        loss_case_id: listing.custody.loss_case_id,
        stored_state: wire_code(listing.view.stored_state().as_str())?,
        effective_state: wire_code(listing.view.effective_state().as_str())?,
        effective_reason: listing.view.effective_reason_code().map(wire_code).transpose()?,
        allowed_actions: allowed_actions
            .iter()
            .map(|action| wire_code(action.as_str()))
            .collect::<Result<_, _>>()?,
        view: view(&listing.view)?,
    })
}

/// 本人视图不带 hold_id：止售标识只经 Admin 记录的 `hold` 展示。
fn view(view: &PrivateListingViewV2) -> Result<ListingViewV2, ConsignmentError> {
    Ok(match view {
        PrivateListingViewV2::Legacy(legacy) => legacy_view(legacy.kind())?,
        PrivateListingViewV2::DepositAccepted(details) => {
            ListingViewV2::DepositAccepted { equipment: equipment_view(details) }
        }
        PrivateListingViewV2::Available(details) => {
            ListingViewV2::Available { equipment: equipment_view(details) }
        }
        PrivateListingViewV2::SalePending(details) => {
            ListingViewV2::SalePending { equipment: equipment_view(details) }
        }
        PrivateListingViewV2::HeldOffMarket { equipment, reason } => ListingViewV2::HeldOffMarket {
            equipment: equipment_view(equipment),
            reason: wire_code(reason.as_str())?,
            hold_id: None,
        },
        PrivateListingViewV2::WithdrawalPending(details) => {
            ListingViewV2::WithdrawalPending { equipment: equipment_view(details) }
        }
    })
}

fn legacy_view(kind: &PrivateListingViewKind) -> Result<ListingViewV2, ConsignmentError> {
    Ok(match kind {
        PrivateListingViewKind::Resolving(progress) => ListingViewV2::Resolving {
            progress: LookupProgress {
                attempts: i64::from(progress.attempts()),
                max_attempts: i64::from(MAX_LISTING_LOOKUP_ATTEMPTS),
                next_attempt_at: progress.next_attempt_at,
                last_reason: progress
                    .last_reason
                    .map(|reason| wire_code(reason.as_str()))
                    .transpose()?,
                paused: progress.paused,
                deadline_at: progress.deadline_at,
            },
        },
        PrivateListingViewKind::LookupFailed { reason, last_lookup_reason } => {
            ListingViewV2::LookupFailed {
                reason: wire_code(reason.as_str())?,
                last_lookup_reason: last_lookup_reason
                    .map(|reason| wire_code(reason.as_str()))
                    .transpose()?,
            }
        }
        PrivateListingViewKind::Ineligible { reason, equipment } => ListingViewV2::Ineligible {
            equipment: equipment_view(equipment),
            reason: wire_code(reason.as_str())?,
        },
        PrivateListingViewKind::Displayed(equipment) => {
            ListingViewV2::Displayed { equipment: equipment_view(equipment) }
        }
        PrivateListingViewKind::AwaitingDeposit(equipment) => {
            ListingViewV2::AwaitingDeposit { equipment: equipment_view(equipment) }
        }
        PrivateListingViewKind::Closed { reason, equipment } => ListingViewV2::Closed {
            reason: wire_code(reason.as_str())?,
            equipment: equipment.as_ref().map(equipment_view),
        },
        PrivateListingViewKind::Expired { reason, equipment } => ListingViewV2::Expired {
            reason: wire_code(reason.as_str())?,
            equipment: equipment.as_ref().map(equipment_view),
        },
    })
}

fn wire_integer(value: u64) -> Result<i64, ConsignmentError> {
    let Ok(value) = i64::try_from(value) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(value)
}

/// DTO 拥有闭集枚举，复用已发布的线协议值；未知值不合成默认状态。
fn wire_code<T: DeserializeOwned>(code: &str) -> Result<T, ConsignmentError> {
    let Ok(value) = T::deserialize(StrDeserializer::<serde::de::value::Error>::new(code)) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(value)
}

fn equipment_view(details: &ListingEquipmentDetails) -> EquipmentView {
    let label = details.label();
    let level = match label.level {
        ListingEquipmentLevel::Assigned(value) => {
            EquipmentLevel::Assigned { value: i64::from(value) }
        }
        ListingEquipmentLevel::Unassigned => EquipmentLevel::Unassigned {},
        ListingEquipmentLevel::Soulbound => EquipmentLevel::Soulbound {},
    };
    EquipmentView {
        name: label.name.clone(),
        alt_name: label.alt_name.clone(),
        category: label.category.clone(),
        level,
        tradeable: label.tradeable,
        stat_groups: details
            .stat_groups()
            .iter()
            .map(|group| EquipmentStatGroup {
                name: group.name.clone(),
                stats: group
                    .stats
                    .iter()
                    .map(|stat| EquipmentStat {
                        name: stat.name.clone(),
                        value: stat.value,
                        base: stat.base,
                    })
                    .collect(),
            })
            .collect(),
        source: EquipmentViewSource::HvDataEquipV1,
        observed_at: details.observed_at(),
    }
}

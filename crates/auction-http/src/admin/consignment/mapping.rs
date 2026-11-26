//! 将挂单读模型与已仲裁回执投影到 Admin 契约。

use auction_application::consignment::{
    AdminListingDetail as DetailRead, AdminListingRecord, ConsignmentError, ListingClosed,
    ListingEquipmentDetails, ListingEquipmentLevel, ListingPageWindow, ListingReadPage,
    PrivateListing, PrivateListingViewKind,
};
use auction_contracts::{
    AdminListing, AdminListingDetail, CommandReceipt, EquipmentLevel, EquipmentStat,
    EquipmentStatGroup, EquipmentView, EquipmentViewSource, ListingAuctionConflict,
    ListingDisposition, ListingFeeBps, ListingOperation, ListingPage, ListingPrice,
    ListingPricingVersion, ListingState, ListingVersion, ListingView, LookupProgress, OwnedListing,
};
use auction_domain::consignment::{ListingVersion as DomainVersion, quote_v1};
use serde::de::{DeserializeOwned, value::StrDeserializer};
use uuid::Uuid;

/// 读侧快照提供排序及精确总数。回显已校验的窗口，端口若返回别的窗口则失败关闭。
pub(super) fn page(
    result: ListingReadPage<AdminListingRecord>,
    requested: ListingPageWindow,
) -> Result<ListingPage<AdminListing>, ConsignmentError> {
    if result.window != requested {
        return Err(ConsignmentError::Unavailable);
    }
    let Ok(total) = i64::try_from(result.total) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(ListingPage {
        items: result.items.into_iter().map(record).collect::<Result<_, _>>()?,
        total,
        page: i64::from(requested.page()),
        page_size: i64::from(requested.page_size()),
    })
}

/// 碰撞按查询端给定的顺序投影，条数和截断标记的一致性由 DTO 序列化校验。
pub(super) fn detail(
    result: DetailRead,
    listing_id: Uuid,
) -> Result<AdminListingDetail, ConsignmentError> {
    if result.record.listing().listing_id != listing_id
        || result.conflicts.iter().any(|conflict| conflict.lot_id.is_nil())
    {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(AdminListingDetail {
        record: record(result.record)?,
        conflicts: result
            .conflicts
            .into_iter()
            .map(|conflict| ListingAuctionConflict {
                lot_id: conflict.lot_id.as_uuid(),
                detected_at: conflict.detected_at,
            })
            .collect(),
        conflicts_truncated: result.conflicts_truncated,
    })
}

/// 不变量 1、6、7：原回执只返回本次目标、原版本及可空标价，私有说明留在查询投影。
pub(super) fn closed(
    result: ListingClosed,
    command_id: Uuid,
    listing_id: Uuid,
    expected_version: ListingVersion,
) -> Result<CommandReceipt, ConsignmentError> {
    let version = wire_version(result.version)?;
    if result.listing_id() != listing_id || version.get() != expected_version.get() + 1 {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(CommandReceipt {
        command_id,
        operation: ListingOperation::AdminDelist,
        listing_id: Some(result.listing_id()),
        listing_version: Some(version),
        state: Some(ListingState::Closed),
        disposition: ListingDisposition::Closed,
        price: result
            .price
            .map(|price| ListingPrice::new(price.value()).ok_or(ConsignmentError::Unavailable))
            .transpose()?,
    })
}

fn record(result: AdminListingRecord) -> Result<AdminListing, ConsignmentError> {
    // 匿名非寄售挂单的可空卖家随 Admin 契约变更开放；此前失败关闭。
    let seller_user_id = result.seller_user_id.ok_or(ConsignmentError::Unavailable)?;
    if result.listing().listing_id.is_nil() || seller_user_id.is_nil() {
        return Err(ConsignmentError::Unavailable);
    }
    let listing = owned_projection(result.listing())?;
    let can_delist = result.can_delist();
    Ok(AdminListing {
        listing,
        seller_user_id: seller_user_id.as_uuid(),
        seller_name: result.seller_name,
        admin_delist_reason: result.admin_delist_reason.map(|reason| reason.as_str().to_owned()),
        can_delist,
    })
}

fn owned_projection(listing: &PrivateListing) -> Result<OwnedListing, ConsignmentError> {
    let (state, reason) = listing.view.effective_state();
    let pricing_version = ListingPricingVersion::new(i64::from(listing.pricing_version))
        .ok_or(ConsignmentError::Unavailable)?;
    let quote = quote_v1(listing.price, listing.fee_bps);
    Ok(OwnedListing {
        listing_id: listing.listing_id,
        version: wire_version(listing.version)?,
        world_id: wire_code(listing.identity.world_id())?,
        external_equipment_id: listing.identity.external_equipment_id().to_owned(),
        mode: wire_code(listing.view.mode().as_str())?,
        price: ListingPrice::new(listing.price.value()).ok_or(ConsignmentError::Unavailable)?,
        pricing_version,
        fee_bps: ListingFeeBps::new(i64::from(listing.fee_bps.value()))
            .ok_or(ConsignmentError::Unavailable)?,
        estimated_fee: quote.fee,
        estimated_seller_amount: quote.seller_amount,
        created_at: listing.created_at,
        updated_at: listing.updated_at,
        expires_at: listing.expires_at,
        stored_state: wire_code(listing.view.stored_state().as_str())?,
        effective_state: wire_code(state.as_str())?,
        effective_reason: reason.map(|reason| wire_code(reason.as_str())).transpose()?,
        allowed_actions: Vec::new(),
        view: view(listing.view.kind())?,
    })
}

fn wire_version(version: DomainVersion) -> Result<ListingVersion, ConsignmentError> {
    i64::try_from(version.value())
        .ok()
        .and_then(ListingVersion::new)
        .ok_or(ConsignmentError::Unavailable)
}

fn view(kind: &PrivateListingViewKind) -> Result<ListingView, ConsignmentError> {
    Ok(match kind {
        PrivateListingViewKind::Resolving(progress) => ListingView::Resolving {
            progress: LookupProgress {
                attempts: i64::from(progress.attempts()),
                max_attempts: i64::from(
                    auction_application::consignment::MAX_LISTING_LOOKUP_ATTEMPTS,
                ),
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
            ListingView::LookupFailed {
                reason: wire_code(reason.as_str())?,
                last_lookup_reason: last_lookup_reason
                    .map(|reason| wire_code(reason.as_str()))
                    .transpose()?,
            }
        }
        PrivateListingViewKind::Ineligible { reason, equipment } => ListingView::Ineligible {
            equipment: equipment_view(equipment),
            reason: wire_code(reason.as_str())?,
        },
        PrivateListingViewKind::Displayed(equipment) => {
            ListingView::Displayed { equipment: equipment_view(equipment) }
        }
        PrivateListingViewKind::AwaitingDeposit(equipment) => ListingView::AwaitingDeposit {
            equipment: equipment_view(equipment),
            accepts_mail: false,
        },
        PrivateListingViewKind::Closed { reason, equipment } => ListingView::Closed {
            reason: wire_code(reason.as_str())?,
            equipment: equipment.as_ref().map(equipment_view),
        },
        PrivateListingViewKind::Expired { reason, equipment } => ListingView::Expired {
            reason: wire_code(reason.as_str())?,
            equipment: equipment.as_ref().map(equipment_view),
        },
    })
}

/// 两侧各自拥有闭集枚举，复用已发布的线协议值；未知值不合成默认状态。
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

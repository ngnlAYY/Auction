//! 将寄售应用端口的安全事实逐字段映射为 Public DTO。

use auction_application::consignment::{
    AnonymousRegistrationReceipt, ConsignmentError, ListingClosed, ListingEquipmentDetails,
    ListingEquipmentLevel, ListingPageWindow, ListingPriceDisposition, ListingPriceReceipt,
    ListingReadPage, ListingRegistered, MAX_LISTING_LOOKUP_ATTEMPTS, OwnedListingCommandReceipt,
    OwnedListingDecision, OwnedListingRecord, PrivateListingViewKind, PublicListingDetail,
    PublicListingState, PublicListingSummary, SellerListingAction, StoredListingDecision,
};
use auction_contracts as dto;
use auction_domain::CommandId;
use auction_domain::consignment::{ListingMode, quote_v1};
use serde::Deserialize;
use serde::de::value::StrDeserializer;

pub(super) fn page<T, U>(
    page: ListingReadPage<T>,
    requested: ListingPageWindow,
    map: impl FnMut(T) -> Result<U, ConsignmentError>,
) -> Result<dto::ListingPage<U>, ConsignmentError> {
    if page.window != requested {
        return Err(ConsignmentError::Unavailable);
    }
    let Ok(total) = i64::try_from(page.total) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(dto::ListingPage {
        items: page.items.into_iter().map(map).collect::<Result<_, _>>()?,
        total,
        page: i64::from(requested.page()),
        page_size: i64::from(requested.page_size()),
    })
}

pub(super) fn domain_mode(mode: dto::ListingMode) -> ListingMode {
    match mode {
        dto::ListingMode::Direct => ListingMode::Direct,
        dto::ListingMode::Consigned => ListingMode::Consigned,
    }
}

fn wire_mode(mode: ListingMode) -> dto::ListingMode {
    match mode {
        ListingMode::Direct => dto::ListingMode::Direct,
        ListingMode::Consigned => dto::ListingMode::Consigned,
    }
}

// 复用 DTO 的闭集解码器核对应用层的固定码，任何版本漂移都失败关闭。
pub(super) fn wire_code<T: for<'de> Deserialize<'de>>(
    code: &'static str,
) -> Result<T, ConsignmentError> {
    let Ok(value) = T::deserialize(StrDeserializer::<serde::de::value::Error>::new(code)) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(value)
}

pub(super) fn price(value: i64) -> Result<dto::ListingPrice, ConsignmentError> {
    dto::ListingPrice::new(value).ok_or(ConsignmentError::Unavailable)
}

pub(super) fn version(value: u64) -> Result<dto::ListingVersion, ConsignmentError> {
    let Ok(value) = i64::try_from(value) else {
        return Err(ConsignmentError::Unavailable);
    };
    dto::ListingVersion::new(value).ok_or(ConsignmentError::Unavailable)
}

pub(super) fn level(level: ListingEquipmentLevel) -> dto::EquipmentLevel {
    match level {
        ListingEquipmentLevel::Assigned(value) => {
            dto::EquipmentLevel::Assigned { value: i64::from(value) }
        }
        ListingEquipmentLevel::Unassigned => dto::EquipmentLevel::Unassigned {},
        ListingEquipmentLevel::Soulbound => dto::EquipmentLevel::Soulbound {},
    }
}

pub(super) fn equipment(details: &ListingEquipmentDetails) -> dto::EquipmentView {
    let label = details.label();
    dto::EquipmentView {
        name: label.name.clone(),
        alt_name: label.alt_name.clone(),
        category: label.category.clone(),
        level: level(label.level),
        tradeable: label.tradeable,
        stat_groups: details
            .stat_groups()
            .iter()
            .map(|group| dto::EquipmentStatGroup {
                name: group.name.clone(),
                stats: group
                    .stats
                    .iter()
                    .map(|stat| dto::EquipmentStat {
                        name: stat.name.clone(),
                        value: stat.value,
                        base: stat.base,
                    })
                    .collect(),
            })
            .collect(),
        source: dto::EquipmentViewSource::HvDataEquipV1,
        observed_at: details.observed_at(),
    }
}

pub(super) fn public_summary(
    entry: PublicListingSummary,
) -> Result<dto::ListingSummary, ConsignmentError> {
    let (mode, state) = match entry.state {
        PublicListingState::Displayed => {
            (dto::ListingMode::Direct, dto::PublicListingState::Displayed)
        }
        PublicListingState::AwaitingDeposit => {
            (dto::ListingMode::Consigned, dto::PublicListingState::AwaitingDeposit)
        }
    };
    Ok(dto::ListingSummary {
        listing_id: entry.listing_id,
        version: version(entry.version.value())?,
        mode,
        state,
        world_id: wire_code(entry.identity.world_id())?,
        external_equipment_id: entry.identity.external_equipment_id().to_owned(),
        name: entry.name,
        category: entry.category,
        level: level(entry.level),
        price: price(entry.price.value())?,
        seller_name: entry.seller_name,
        seller_verified: entry.seller_verified,
        updated_at: entry.updated_at,
        expires_at: entry.expires_at,
        purchasable: false,
    })
}

pub(super) fn public_detail(
    entry: PublicListingDetail,
) -> Result<dto::ListingDetail, ConsignmentError> {
    Ok(dto::ListingDetail {
        summary: public_summary(entry.summary)?,
        equipment: equipment(&entry.equipment),
    })
}

pub(super) fn owned(entry: OwnedListingRecord) -> Result<dto::OwnedListing, ConsignmentError> {
    let listing = entry.listing();
    let (effective_state, effective_reason) = listing.view.effective_state();
    let pricing_version = dto::ListingPricingVersion::new(i64::from(listing.pricing_version))
        .ok_or(ConsignmentError::Unavailable)?;
    let quote = quote_v1(listing.price, listing.fee_bps);
    Ok(dto::OwnedListing {
        listing_id: listing.listing_id,
        version: version(listing.version.value())?,
        world_id: wire_code(listing.identity.world_id())?,
        external_equipment_id: listing.identity.external_equipment_id().to_owned(),
        mode: wire_mode(listing.view.mode()),
        price: price(listing.price.value())?,
        pricing_version,
        fee_bps: dto::ListingFeeBps::new(i64::from(listing.fee_bps.value()))
            .ok_or(ConsignmentError::Unavailable)?,
        estimated_fee: quote.fee,
        estimated_seller_amount: quote.seller_amount,
        created_at: listing.created_at,
        updated_at: listing.updated_at,
        expires_at: listing.expires_at,
        stored_state: wire_code(listing.view.stored_state().as_str())?,
        effective_state: wire_code(effective_state.as_str())?,
        effective_reason: effective_reason.map(|reason| wire_code(reason.as_str())).transpose()?,
        allowed_actions: entry
            .allowed_actions()
            .iter()
            .map(|action| match action {
                SellerListingAction::Reprice => dto::ListingAction::Reprice,
                SellerListingAction::Withdraw => dto::ListingAction::Withdraw,
            })
            .collect(),
        view: owned_view(listing.view.kind())?,
    })
}

fn owned_view(view: &PrivateListingViewKind) -> Result<dto::ListingView, ConsignmentError> {
    Ok(match view {
        PrivateListingViewKind::Resolving(progress) => dto::ListingView::Resolving {
            progress: dto::LookupProgress {
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
            dto::ListingView::LookupFailed {
                reason: wire_code(reason.as_str())?,
                last_lookup_reason: last_lookup_reason
                    .map(|reason| wire_code(reason.as_str()))
                    .transpose()?,
            }
        }
        PrivateListingViewKind::Ineligible { equipment: details, reason } => {
            dto::ListingView::Ineligible {
                equipment: equipment(details),
                reason: wire_code(reason.as_str())?,
            }
        }
        PrivateListingViewKind::Displayed(details) => {
            dto::ListingView::Displayed { equipment: equipment(details) }
        }
        PrivateListingViewKind::AwaitingDeposit(details) => {
            dto::ListingView::AwaitingDeposit { equipment: equipment(details), accepts_mail: false }
        }
        PrivateListingViewKind::Closed { reason, equipment: details } => dto::ListingView::Closed {
            reason: wire_code(reason.as_str())?,
            equipment: details.as_ref().map(equipment),
        },
        PrivateListingViewKind::Expired { reason, equipment: details } => {
            dto::ListingView::Expired {
                reason: wire_code(reason.as_str())?,
                equipment: details.as_ref().map(equipment),
            }
        }
    })
}

pub(super) fn registered(
    command_id: CommandId,
    result: ListingRegistered,
) -> Result<dto::CommandReceipt, ConsignmentError> {
    Ok(dto::CommandReceipt {
        command_id: command_id.as_uuid(),
        operation: dto::ListingOperation::Register,
        listing_id: Some(result.listing_id()),
        listing_version: Some(version(result.version().value())?),
        state: Some(wire_code(result.state().as_str())?),
        disposition: dto::ListingDisposition::Registered,
        price: Some(price(result.price.value())?),
    })
}

pub(super) fn repriced(
    command_id: CommandId,
    result: ListingPriceReceipt,
) -> Result<dto::CommandReceipt, ConsignmentError> {
    Ok(dto::CommandReceipt {
        command_id: command_id.as_uuid(),
        operation: dto::ListingOperation::Reprice,
        listing_id: Some(result.listing_id()),
        listing_version: Some(version(result.version.value())?),
        state: Some(wire_code(result.state().as_str())?),
        disposition: match result.disposition {
            ListingPriceDisposition::Repriced => dto::ListingDisposition::Repriced,
            ListingPriceDisposition::Unchanged => dto::ListingDisposition::Unchanged,
        },
        price: Some(price(result.price.value())?),
    })
}

pub(super) fn closed(
    command_id: CommandId,
    result: ListingClosed,
) -> Result<dto::CommandReceipt, ConsignmentError> {
    Ok(dto::CommandReceipt {
        command_id: command_id.as_uuid(),
        operation: dto::ListingOperation::Withdraw,
        listing_id: Some(result.listing_id()),
        listing_version: Some(version(result.version.value())?),
        state: Some(wire_code(result.state().as_str())?),
        disposition: dto::ListingDisposition::Closed,
        price: result.price.map(|amount| price(amount.value())).transpose()?,
    })
}

pub(super) fn stored_command(
    entry: OwnedListingCommandReceipt,
) -> Result<dto::StoredCommandResult, ConsignmentError> {
    use OwnedListingDecision::{Register, Reprice, Withdraw};
    use StoredListingDecision::{Accepted, Rejected};

    let command_id = entry.command_id;
    let decision = entry.decision;
    let (status, code) = decision.status_and_code();
    let receipt = match decision {
        Register(Accepted(result)) => stored_receipt(registered(command_id, result)?),
        Reprice(Accepted(result)) => stored_receipt(repriced(command_id, result)?),
        Withdraw(Accepted(result)) => stored_receipt(closed(command_id, result)?),
        Register(Rejected(_rejection))
        | Reprice(Rejected(_rejection))
        | Withdraw(Rejected(_rejection)) => {
            // 当前应用账本的拒绝只带安全分类，按裁定的可空字段表保留 null。
            dto::StoredCommandReceipt {
                command_id: command_id.as_uuid(),
                operation: wire_code(decision.operation().as_str())?,
                listing_id: None,
                listing_version: None,
                state: None,
                disposition: dto::ListingDisposition::Rejected,
                price: None,
            }
        }
    };
    Ok(dto::StoredCommandResult { http_status: i64::from(status), code: wire_code(code)?, receipt })
}

/// 免登录登记回执：首次决定原样映射，受理时附挂单当前有效态，不回显自填名。
pub(super) fn direct_command(
    receipt: AnonymousRegistrationReceipt,
) -> Result<dto::DirectCommandResult, ConsignmentError> {
    let listing = receipt
        .listing
        .map(|listing| {
            let (state, reason) = listing.view.effective_state();
            Ok::<_, ConsignmentError>(dto::DirectListingProgress {
                listing_id: listing.listing_id,
                version: version(listing.version.value())?,
                effective_state: wire_code(state.as_str())?,
                effective_reason: reason.map(|reason| wire_code(reason.as_str())).transpose()?,
                expires_at: listing.expires_at,
            })
        })
        .transpose()?;
    // 不变量 6：回执与挂单的矛盾组合由 DTO 在序列化时拒绝，读路径按依赖不可用返回。
    Ok(dto::DirectCommandResult { result: stored_command(receipt.command)?, listing })
}

fn stored_receipt(receipt: dto::CommandReceipt) -> dto::StoredCommandReceipt {
    dto::StoredCommandReceipt {
        command_id: receipt.command_id,
        operation: receipt.operation,
        listing_id: receipt.listing_id,
        listing_version: receipt.listing_version,
        state: receipt.state,
        disposition: receipt.disposition,
        price: receipt.price,
    }
}

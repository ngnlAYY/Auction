//! Admin v2 挂单列表与详情：含代管态、有效止售、position 摘要与 Admin 允许动作。
//!
//! 认证沿用既有 Admin 会话守卫；读取不受寄售开关影响。本人动作恒空，运营动作只来自领域
//! `admin_actions`，响应不含预约码、装备 key 或原信正文（不变量 7）。

use std::time::Duration;

use auction_application::consignment::{
    AdminListingDetailV2, AdminListingRecordV2, ConsignmentError, ListingBusinessRejection,
    ListingPageWindow, ListingReadPage,
};
use auction_contracts::{
    AdminHoldV2, AdminListingActionV2, AdminListingDetailV2 as DetailDto, AdminListingPageV2,
    AdminListingV2, AdminPositionV2, ListingAuctionConflict, ListingGeneration, ListingVersion,
    PositionStateV2,
};
use auction_domain::consignment::ListingState;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use serde::de::DeserializeOwned;
use serde::de::value::StrDeserializer;
use uuid::Uuid;

use super::boundary::{deposit_error as error, ok};
use super::input::{list_query, listing_target};
use super::quarantine::AdminDepositState;
use crate::mapping::owned_v2;
use crate::session::authenticated_admin_session;

const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// 查询 Admin v2 历史；服务开关不阻断认证后的查询。
pub(super) async fn list<S: AdminDepositState>(
    State(state): State<S>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let query = match list_query(&uri) {
        Ok(query) => query,
        Err(failure) => return error(&headers, failure),
    };
    let Some(port) = state.deposit().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.list_admin_v2(query)).await {
        Ok(Ok(page)) => page_dto(page, query.listings.window),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|page| ok("挂单列表已加载", page))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 只向已认证 Admin 返回私有说明、有效状态、止售、position 摘要及碰撞记录。
pub(super) async fn detail<S: AdminDepositState>(
    State(state): State<S>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let listing_id = match listing_target(&uri, path) {
        Ok(listing_id) => listing_id,
        Err(failure) => return error(&headers, failure),
    };
    let Some(port) = state.deposit().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.admin_detail_v2(listing_id)).await {
        Ok(Ok(Some(detail))) => detail_dto(detail, listing_id),
        Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|detail| ok("挂单详情已加载", detail))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 读侧快照提供排序及精确总数；回显已校验的窗口，端口若返回别的窗口则失败关闭。
fn page_dto(
    page: ListingReadPage<AdminListingRecordV2>,
    requested: ListingPageWindow,
) -> Result<AdminListingPageV2, ConsignmentError> {
    if page.window != requested {
        return Err(ConsignmentError::Unavailable);
    }
    let Ok(total) = i64::try_from(page.total) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(AdminListingPageV2 {
        items: page.items.iter().map(record_dto).collect::<Result<_, _>>()?,
        total,
        page: i64::from(requested.page()),
        page_size: i64::from(requested.page_size()),
    })
}

fn detail_dto(
    detail: AdminListingDetailV2,
    listing_id: Uuid,
) -> Result<DetailDto, ConsignmentError> {
    if detail.record.listing.listing_id != listing_id
        || detail.conflicts.iter().any(|conflict| conflict.lot_id.is_nil())
    {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(DetailDto {
        record: record_dto(&detail.record)?,
        conflicts: detail
            .conflicts
            .into_iter()
            .map(|conflict| ListingAuctionConflict {
                lot_id: conflict.lot_id.as_uuid(),
                detected_at: conflict.detected_at,
            })
            .collect(),
        conflicts_truncated: detail.conflicts_truncated,
    })
}

fn record_dto(record: &AdminListingRecordV2) -> Result<AdminListingV2, ConsignmentError> {
    let listing = &record.listing;
    // 匿名非寄售挂单的可空卖家随 Admin 契约变更开放；此前失败关闭。
    let seller_user_id = record.seller_user_id.ok_or(ConsignmentError::Unavailable)?;
    if listing.listing_id.is_nil() || seller_user_id.is_nil() {
        return Err(ConsignmentError::Unavailable);
    }
    // Admin 视图不带卖家动作；止售只在在库下架态展示，hold_id 只在这里出现。
    let held = listing.view.effective_state() == ListingState::HeldOffMarket;
    let hold = listing
        .custody
        .hold
        .filter(|_| held)
        .map(|hold| AdminHoldV2 { hold_id: hold.hold_id, set_at: hold.set_at });
    let position = listing
        .custody
        .position
        .map(|position| {
            Ok::<_, ConsignmentError>(AdminPositionV2 {
                position_id: position.position_id,
                state: wire_code::<PositionStateV2>(position.state.as_str())?,
                version: wire_version(position.version)?,
                generation: position.generation.map(wire_generation).transpose()?,
            })
        })
        .transpose()?;
    Ok(AdminListingV2 {
        listing: owned_v2(listing, &[])?,
        seller_user_id: seller_user_id.as_uuid(),
        seller_name: record.seller_name.clone(),
        admin_delist_reason: record
            .admin_delist_reason
            .as_ref()
            .map(|reason| reason.as_str().to_owned()),
        hold,
        position,
        admin_actions: listing
            .admin_actions()
            .into_iter()
            .map(|action| wire_code::<AdminListingActionV2>(action.as_str()))
            .collect::<Result<_, _>>()?,
    })
}

fn wire_version(value: u64) -> Result<ListingVersion, ConsignmentError> {
    i64::try_from(value).ok().and_then(ListingVersion::new).ok_or(ConsignmentError::Unavailable)
}

fn wire_generation(value: u64) -> Result<ListingGeneration, ConsignmentError> {
    i64::try_from(value).ok().and_then(ListingGeneration::new).ok_or(ConsignmentError::Unavailable)
}

/// DTO 拥有闭集枚举，复用已发布的线协议值；未知值不合成默认状态。
fn wire_code<T: DeserializeOwned>(code: &str) -> Result<T, ConsignmentError> {
    let Ok(value) = T::deserialize(StrDeserializer::<serde::de::value::Error>::new(code)) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(value)
}

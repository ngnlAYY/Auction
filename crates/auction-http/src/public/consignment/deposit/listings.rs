//! Public v2 本人挂单列表与详情：含代管态、处分代与服务端允许动作。
//!
//! 只按会话用户读取，非本人与不存在同样返回 404（不变量 7：不泄露他人挂单是否存在）。
//! 响应只含白名单 DTO 字段，不含预约码、装备 key 或原信正文；无缓存头由路由层统一加。

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_domain::UserId;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};

use super::{DepositState, no_query, read_failure};
use crate::mapping::owned_v2;
use crate::public::consignment::input;
use crate::public::consignment::mapping;
use crate::public::consignment::response::{RouteResult, loaded};

/// 按当前会话与有效态读取本人 v2 历史；寄售停用后历史仍可查。
pub(super) async fn list_owned<S: DepositState>(
    State(state): State<S>,
    headers: HeaderMap,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    let query = input::private_query(&uri)?;
    let reader = state.deposit().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let page = reader
        .list_owned_v2(UserId::from_uuid(identity.user_id), query)
        .await
        .map_err(read_failure)?;
    loaded(mapping::page(page, query.window, |entry| {
        // 有效状态由读端口判定；不符合有效态筛选时拒绝整页，不让客户端看到被筛选漏掉的行。
        if query.state.is_some_and(|state| state != entry.view.effective_state()) {
            return Err(ConsignmentError::Unavailable);
        }
        owned_v2(&entry, &entry.seller_actions())
    })?)
}

/// 按当前会话读取单条 v2 挂单。
pub(super) async fn owned_detail<S: DepositState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    no_query(&uri)?;
    let listing_id = input::listing_id(path)?;
    let reader = state.deposit().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let entry = reader
        .owned_detail_v2(UserId::from_uuid(identity.user_id), listing_id)
        .await
        .map_err(read_failure)?
        .ok_or_else(|| ConsignmentError::from(ListingBusinessRejection::ListingNotFound))?;
    if entry.listing_id != listing_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    loaded(owned_v2(&entry, &entry.seller_actions())?)
}

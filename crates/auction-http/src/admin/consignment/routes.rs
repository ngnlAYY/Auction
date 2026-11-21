//! Admin 寄售列表、详情和下架处理器。

use std::net::SocketAddr;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_application::core::diagnostics::DiagnosticContext;
use auction_domain::CommandId;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;

use super::AdminConsignmentState;
use super::boundary::{error, ok};
use super::input::{delist_request, list_query, listing_target};
use super::mapping;
use crate::diagnostics;
use crate::session::{authenticated_admin_mutation, authenticated_admin_session};

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const DELIST_TIMEOUT: Duration = Duration::from_secs(5);

/// 查询 Admin 历史，服务开关不阻断认证后的查询。
pub(super) async fn list<S: AdminConsignmentState>(
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
    let Some(port) = state.consignment_read() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.list_admin(query)).await {
        Ok(Ok(page)) => mapping::page(page, query.listings.window),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|page| ok("挂单列表已加载", page))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 只向已认证 Admin 返回私有说明、落库和有效状态及碰撞记录。
pub(super) async fn detail<S: AdminConsignmentState>(
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
    let Some(port) = state.consignment_read() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.admin_detail(listing_id)).await {
        Ok(Ok(Some(detail))) => mapping::detail(detail, listing_id),
        Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|detail| ok("挂单详情已加载", detail))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 不变量 1、2、3、6、7、8：认证后原样交给用例。账本仲裁、有效状态和审计都由用例及其事务决定。
pub(super) async fn delist<S: AdminConsignmentState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> Response {
    let identity = match authenticated_admin_mutation(state.admin_state(), &headers, address).await
    {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    let listing_id = match listing_target(&uri, path) {
        Ok(listing_id) => listing_id,
        Err(failure) => return error(&headers, failure),
    };
    let input = match delist_request(&headers, body).await {
        Ok(input) => input,
        Err(response) => return *response,
    };
    let Some(service) = state.consignment_manage() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    // 当前用例的命令输入没有请求关联号。审计身份来自会话，关联号仍留在 HTTP 诊断上下文。
    let command = service.admin_delist(
        &identity,
        CommandId::from_uuid(input.command_id),
        listing_id,
        input.expected_version.get().cast_unsigned(),
        input.reason,
    );
    let result = match tokio::time::timeout(DELIST_TIMEOUT, command).await {
        Ok(Ok(closed)) => {
            mapping::closed(closed, input.command_id, listing_id, input.expected_version)
                .and_then(|receipt| ok("挂单已下架", receipt))
                .or(Err(ConsignmentError::OutcomeUnknown))
        }
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => {
            diagnostics::warn(
                "admin_consignment",
                "delist_timeout",
                diagnostics::timeout_kind(&elapsed),
                DiagnosticContext::default(),
            );
            Err(ConsignmentError::OutcomeUnknown)
        }
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

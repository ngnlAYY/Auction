//! Admin v2 隔离详情、重检、解除与代管件运营止售处理器。
//!
//! 认证、Origin/CSRF、正文上限与严格方法守卫沿用既有 Admin 边界；命令版本、证据与
//! 原 owner 交接由应用用例和存储事务决定。路由挂载由集成车道按本模块的 `mount` 接入。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, DepositCommandService, DepositRead, ListingBusinessRejection,
    ReleaseQuarantineAdminInput,
};
use auction_contracts::{
    AdminDelistListing, ConsignmentDelistReceiptV2, ReleaseConsignmentHoldRequest,
};
use auction_domain::CommandId;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use axum::{Router, middleware};

use super::AdminConsignmentState;
use super::boundary::{accepted, deposit_error as error, disabled, ok};
use super::input::{
    delist_request, deposit_detail_query, deposit_path, deposit_target, deposit_write,
};
use crate::middleware::boundary::request_id;
use crate::middleware::method_strict;
use crate::middleware::response_cache;
use crate::session::{authenticated_admin_mutation, authenticated_admin_session};

mod mapping;

#[cfg(test)]
mod tests;

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Admin v2 寄售能力；读取与命令端口缺失时返回 503。
#[derive(Clone, Default)]
pub struct AdminDepositCapabilities {
    /// 有效开关；新运营止售在关闭时返回 404，续办旧隔离与解除不受影响。
    pub enabled: bool,
    /// 隔离详情与观察读取。
    pub read: Option<Arc<dyn DepositRead>>,
    /// 重检、解除、止售与撤回用例。
    pub commands: Option<Arc<DepositCommandService>>,
}

/// 路由只借用既有 Admin 会话、v1 下架能力与 v2 收件能力。
pub(crate) trait AdminDepositState: AdminConsignmentState {
    /// 当前装配为此进程提供的 v2 收件能力。
    fn deposit(&self) -> &AdminDepositCapabilities;
}

/// 将五条 Admin v2 路由放入给定路由器，并沿用无缓存边界。
pub(crate) fn mount<S: AdminDepositState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route(
            "/api/v2/admin/consignment/deposits/{quarantine_id}",
            method_strict::get(detail::<S>),
        )
        .route(
            "/api/v2/admin/consignment/deposits/{quarantine_id}/recheck",
            method_strict::post(recheck::<S>),
        )
        .route(
            "/api/v2/admin/consignment/deposits/{quarantine_id}/release-quarantine",
            method_strict::post(release::<S>),
        )
        .route(
            "/api/v2/admin/consignment/listings/{listing_id}/delist",
            method_strict::post(delist::<S>),
        )
        .route(
            "/api/v2/admin/consignment/listings/{listing_id}/release-hold",
            method_strict::post(release_hold::<S>),
        )
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

async fn detail<S: AdminDepositState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let quarantine_id = match deposit_path(path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let query = match deposit_detail_query(&uri) {
        Ok(query) => query.into_port(quarantine_id),
        Err(failure) => return error(&headers, failure),
    };
    let Some(port) = state.deposit().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.quarantine(query)).await {
        Ok(Ok(Some(detail))) => mapping::detail_dto(detail, quarantine_id),
        Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|detail| ok("隔离详情已加载", detail))
        .unwrap_or_else(|failure| error(&headers, failure))
}

async fn recheck<S: AdminDepositState>(
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
    let quarantine_id = match deposit_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let input: auction_contracts::QuarantineRecheckRequest =
        match deposit_write(&headers, body).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Some(service) = state.deposit().commands.as_ref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let expected_version = match mapping::quarantine_version(input.expected_version.get()) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let command = service.recheck_quarantine(
        identity.actor,
        CommandId::from_uuid(input.command_id),
        quarantine_id,
        expected_version,
        request_id(&headers),
    );
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => mapping::recheck_dto(receipt, quarantine_id)
            .and_then(|receipt| accepted("隔离重检已排入", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

fn timeout_error<T>(_elapsed: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

async fn release<S: AdminDepositState>(
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
    let quarantine_id = match deposit_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let input: auction_contracts::QuarantineReleaseRequest =
        match deposit_write(&headers, body).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Some(service) = state.deposit().commands.as_ref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let expected_version = match mapping::quarantine_version(input.expected_version.get()) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let command = service.release_quarantine_admin(ReleaseQuarantineAdminInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(input.command_id),
        quarantine_id,
        expected_version,
        observation_id: input.observation_id,
        proposed_kind: mapping::release_kind(input.proposed_kind),
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => mapping::release_receipt_dto(
            receipt,
            quarantine_id,
            input.command_id,
            input.proposed_kind,
        )
        .and_then(|receipt| ok("隔离已解除", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

async fn delist<S: AdminDepositState>(
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
    let listing_id = match super::input::listing_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let input: AdminDelistListing = match delist_request(&headers, body).await {
        Ok(input) => input,
        Err(response) => return *response,
    };
    let capabilities = state.deposit();
    let command_id = CommandId::from_uuid(input.command_id);
    let expected_version = match super::input::domain_listing_version(input.expected_version.get())
    {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let reason =
        match auction_application::consignment::ListingDelistReason::new(input.reason.clone()) {
            Ok(value) => value,
            Err(failure) => return error(&headers, failure),
        };
    let Some(service) = capabilities.commands.as_ref() else {
        if capabilities.enabled {
            return error(&headers, ConsignmentError::Unavailable);
        }
        return disabled(&headers);
    };
    if !capabilities.enabled {
        // 不变量 1、D9 §6.3：关闭只截止新承诺；关闭前已接受的同 ID 同请求返回原回执。
        let command = service.replay_set_operations_hold(
            identity.actor.clone(),
            command_id,
            listing_id,
            expected_version,
            reason,
            request_id(&headers),
        );
        let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
            Ok(Ok(Some(receipt))) => match mapping::delist_receipt_dto(receipt) {
                Ok(receipt @ ConsignmentDelistReceiptV2::Closed { .. }) => {
                    ok("挂单已下架", receipt)
                }
                Ok(receipt @ ConsignmentDelistReceiptV2::HoldSet { .. }) => {
                    ok("运营止售已设置", receipt)
                }
                Err(failure) => Err(failure),
            },
            Ok(Ok(None)) => return disabled(&headers),
            Ok(Err(failure)) => Err(failure),
            Err(elapsed) => Err(timeout_error(elapsed)),
        };
        return result.unwrap_or_else(|failure| error(&headers, failure));
    }
    let command = service.set_operations_hold(
        identity.actor.clone(),
        command_id,
        listing_id,
        expected_version,
        reason,
        request_id(&headers),
    );
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => match mapping::delist_receipt_dto(receipt) {
            Ok(receipt @ ConsignmentDelistReceiptV2::Closed { .. }) => ok("挂单已下架", receipt),
            Ok(receipt @ ConsignmentDelistReceiptV2::HoldSet { .. }) => {
                ok("运营止售已设置", receipt)
            }
            Err(failure) => Err(failure),
        },
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

async fn release_hold<S: AdminDepositState>(
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
    let listing_id = match super::input::listing_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let input: ReleaseConsignmentHoldRequest = match deposit_write(&headers, body).await {
        Ok(input) => input,
        Err(response) => return *response,
    };
    let Some(service) = state.deposit().commands.as_ref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let expected_version = match super::input::domain_listing_version(input.expected_version.get())
    {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let chain = service.release_operations_hold(
        identity.actor,
        CommandId::from_uuid(input.command_id),
        listing_id,
        expected_version,
        input.hold_id,
        request_id(&headers),
    );
    let result = match tokio::time::timeout(WRITE_TIMEOUT, chain).await {
        Ok(Ok(receipt)) => {
            mapping::hold_dto(receipt).and_then(|receipt| ok("运营止售已解除", receipt))
        }
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

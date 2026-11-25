//! Admin 损失案件处理器：按 position 读案件、录入灭失证据（rulings Q4）、确认灭失（7C-4b），追加人工结果。
//!
//! 认证、Origin/CSRF、正文上限与严格方法守卫沿用既有 Admin 边界；版本仲裁、幂等与事务由应用用例和
//! 存储决定。回执与读模型没有任何金额字段，本接口没有付款或发行能力（决定 22）。
//! 路由挂载与装配由集成车道按本模块的 `mount` 与 `AdminLossState` 接入。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConfirmPositionLossInput, ConsignmentError, ListingBusinessRejection, LossCaseAdminService,
    PositionLossConfirmService, RecordLossOutcomeInput, RecordLossProofInput,
};
use auction_contracts::{
    ConfirmPositionLossRequest, RecordLossCaseOutcomeRequest, RecordLossProofRequest,
};
use auction_domain::CommandId;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use axum::{Router, middleware};

use super::AdminConsignmentState;
use super::boundary::{deposit_error as error, ok};
use super::input::deposit_write;
use crate::middleware::boundary::request_id;
use crate::middleware::method_strict;
use crate::middleware::response_cache;
use crate::session::{authenticated_admin_mutation, authenticated_admin_session};

mod input;
mod mapping;

#[cfg(test)]
mod tests;

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Admin 损失案件能力；缺失时返回 503。读取、灭失确认与人工结果都不随寄售开关关闭（D9 §11.2-3：
/// 损失属于原已代管件的清偿与办结，不是新承诺）。
#[derive(Clone, Default)]
pub struct AdminLossCapabilities {
    /// 案件读取与人工结果用例。
    pub cases: Option<Arc<LossCaseAdminService>>,
    /// 灭失证据录入与灭失确认用例（同一灭失写端口）。
    pub confirm: Option<Arc<PositionLossConfirmService>>,
}

/// 路由只借用既有 Admin 会话与损失案件能力。
pub(crate) trait AdminLossState: AdminConsignmentState {
    /// 当前装配为此进程提供的损失案件能力。
    fn loss(&self) -> &AdminLossCapabilities;
}

/// 将四条 Admin 损失案件路由放入给定路由器，并沿用无缓存边界。
pub(crate) fn mount<S: AdminLossState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route(
            "/api/v2/admin/consignment/positions/{position_id}/loss-case",
            method_strict::get(detail::<S>),
        )
        .route(
            "/api/v2/admin/consignment/positions/{position_id}/loss-proofs",
            method_strict::post(record_proof::<S>),
        )
        .route(
            "/api/v2/admin/consignment/positions/{position_id}/confirm-loss",
            method_strict::post(confirm_loss::<S>),
        )
        .route(
            "/api/v2/admin/consignment/positions/{position_id}/loss-case/outcomes",
            method_strict::post(record_outcome::<S>),
        )
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

/// 超时与未知写同为结果未知，调用方只能用原命令 ID 与原正文重试。
fn timeout_error<T>(_elapsed: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

async fn detail<S: AdminLossState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let position_id = match input::position_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let Some(service) = state.loss().cases.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, service.case(position_id)).await {
        Ok(Ok(Some(record))) => mapping::case_dto(record, position_id),
        Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|case| ok("损失案件已加载", case))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 不变量 1、2、6、8：认证后原样交给用例；版本仲裁、幂等与追加都在存储事务内。
/// 默认由处理器先到期并返回 503 outcome_unknown，外层先到期则为 504；两者都只能原命令原样重试。
async fn record_outcome<S: AdminLossState>(
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
    let position_id = match input::position_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let request: RecordLossCaseOutcomeRequest = match deposit_write(&headers, body).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(service) = state.loss().cases.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let expected_case_version = match mapping::case_version(request.expected_case_version.get()) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let command = service.record_outcome(RecordLossOutcomeInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(request.command_id),
        position_id,
        case_id: request.case_id,
        expected_case_version,
        outcome: mapping::outcome_kind(request.outcome),
        basis_id: request.basis_id,
        private_note: request.private_note.clone(),
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => mapping::receipt_dto(receipt, &request)
            .and_then(|receipt| ok("损失案件人工结果已记录", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

/// 不变量 1、6、7、8：认证后原样交给用例；摘要只由存储在锁内推导，证据只追加，同 ID 同请求回原回执。
/// 私有说明只进审计，不回显。超时与提交未知同为 503 `outcome_unknown`，只能原命令原样重试。
async fn record_proof<S: AdminLossState>(
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
    let position_id = match input::position_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let request: RecordLossProofRequest = match deposit_write(&headers, body).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(service) = state.loss().confirm.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let command = service.record_proof(RecordLossProofInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(request.command_id),
        position_id,
        source_kind: request.source_kind.clone(),
        observation_ref: request.observation_ref.clone(),
        last_known_ref: request.last_known_ref.clone(),
        private_note: request.private_note.clone(),
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => mapping::proof_receipt_dto(receipt, request.command_id, position_id)
            .and_then(|receipt| ok("灭失证据已记录", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

/// 不变量 1、2、6、8：认证后原样交给用例；锁内权责分流、唯一损失事实、案件与卖家回执都在存储事务内。
/// 私有说明只进审计，不回显。超时与提交未知同为 503 `outcome_unknown`，只能原命令原样重试。
async fn confirm_loss<S: AdminLossState>(
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
    let position_id = match input::position_target(&uri, path) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let request: ConfirmPositionLossRequest = match deposit_write(&headers, body).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(service) = state.loss().confirm.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let expected_version = match mapping::case_version(request.expected_version.get()) {
        Ok(value) => value,
        Err(failure) => return error(&headers, failure),
    };
    let command = service.confirm(ConfirmPositionLossInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(request.command_id),
        position_id,
        expected_version,
        proof_id: request.proof_id,
        reason: request.reason.clone(),
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => mapping::confirm_receipt_dto(receipt, &request, position_id)
            .and_then(|receipt| ok("代管件灭失已确认", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(timeout_error(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

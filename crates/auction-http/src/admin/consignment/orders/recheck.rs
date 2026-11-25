//! Admin 订单 recheck 处理器：把复核排入持久工作，不改任何订单状态。
//!
//! 认证、Origin/CSRF、正文上限与严格方法守卫沿用既有 Admin 边界；版本仲裁、幂等与事务由应用用例和
//! 存储决定。写超时与未知结果同为结果未知，调用方只能用原命令 ID 与原正文重试（不变量 1）。

use std::net::SocketAddr;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, RecheckOrderInput, RecheckOrderReceipt};
use auction_contracts::{
    RecheckDisposition, RecheckOperation, RecheckOrderReceiptV2, RecheckOrderRequest,
};
use auction_domain::CommandId;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;

use super::super::boundary::{accepted, deposit_error as error};
use super::super::input::deposit_write;
use super::{AdminOrderWriteState, contract_target, outcome_unknown, positive};
use crate::middleware::boundary::request_id;
use crate::session::authenticated_admin_mutation;

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// 回执必须与请求的命令与合同一致，否则写结果无法确认，只能原样重试。
fn receipt_dto(
    receipt: RecheckOrderReceipt,
    request: &RecheckOrderRequest,
    contract_id: uuid::Uuid,
) -> Result<RecheckOrderReceiptV2, ConsignmentError> {
    if receipt.command_id.as_uuid() != request.command_id || receipt.contract_id != contract_id {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let dto = RecheckOrderReceiptV2 {
        schema_version: 2,
        operation: RecheckOperation::Recheck,
        disposition: RecheckDisposition::RecheckQueued,
        command_id: request.command_id,
        contract_id,
        contract_version: positive(receipt.contract_version).map_err(outcome_unknown)?,
        work_id: receipt.work_id,
    };
    serde_json::to_value(&dto).map_err(outcome_unknown)?;
    Ok(dto)
}

/// 不变量 1、5、6：认证后原样交给用例；复核工作持久化在存储事务内。
pub(super) async fn recheck<S: AdminOrderWriteState>(
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
    let contract_id = match contract_target(&uri, path, &[]) {
        Ok((contract_id, _)) => contract_id,
        Err(failure) => return error(&headers, failure),
    };
    let request: RecheckOrderRequest = match deposit_write(&headers, body).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(service) = state.order_writes().admin.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let Ok(expected_version) = u64::try_from(request.expected_version) else {
        return error(&headers, ConsignmentError::InvalidDepositRequest);
    };
    let command = service.recheck(RecheckOrderInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(request.command_id),
        contract_id,
        expected_version,
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => receipt_dto(receipt, &request, contract_id)
            .and_then(|receipt| accepted("订单复核已排队", receipt)),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(outcome_unknown(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

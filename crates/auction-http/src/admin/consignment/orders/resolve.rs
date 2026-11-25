//! Admin 订单 resolve 处理器：动作是闭集，退款类动作的金额只由 6B 单写者决定，补寄只排队新轮。
//!
//! 认证、Origin/CSRF、正文上限与严格方法守卫沿用既有 Admin 边界；请求没有金额、收件人或装备字段，
//! 多余字段整体拒绝。写超时与未知结果同为结果未知，调用方只能用原命令 ID 与原正文重试（不变量 1）。

use std::net::SocketAddr;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, OrderResolveAction, OrderResolveReceipt, ResolveOrderInput,
};
use auction_contracts::{
    OrderResolveActionV2, ResolveOperation, ResolveOrderReceiptV2, ResolveOrderRequest,
};
use auction_domain::CommandId;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use uuid::Uuid;

use super::super::boundary::{accepted, deposit_error as error, ok};
use super::super::input::deposit_write;
use super::{AdminOrderWriteState, contract_target, outcome_unknown, positive};
use crate::middleware::boundary::request_id;
use crate::session::authenticated_admin_mutation;

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// wire 动作到应用动作：一一对应的闭集，没有默认分支。
const fn action_of(action: OrderResolveActionV2) -> OrderResolveAction {
    match action {
        OrderResolveActionV2::CancelUnpaid => OrderResolveAction::CancelUnpaid,
        OrderResolveActionV2::RefundReturned => OrderResolveAction::RefundReturned,
        OrderResolveActionV2::RefundLost => OrderResolveAction::RefundLost,
        OrderResolveActionV2::RefundSeason => OrderResolveAction::RefundSeason,
        OrderResolveActionV2::RetainEntitlement => OrderResolveAction::RetainEntitlement,
        OrderResolveActionV2::RedispatchOriginal => OrderResolveAction::RedispatchOriginal,
    }
}

const fn action_dto(action: OrderResolveAction) -> OrderResolveActionV2 {
    match action {
        OrderResolveAction::CancelUnpaid => OrderResolveActionV2::CancelUnpaid,
        OrderResolveAction::RefundReturned => OrderResolveActionV2::RefundReturned,
        OrderResolveAction::RefundLost => OrderResolveActionV2::RefundLost,
        OrderResolveAction::RefundSeason => OrderResolveActionV2::RefundSeason,
        OrderResolveAction::RetainEntitlement => OrderResolveActionV2::RetainEntitlement,
        OrderResolveAction::RedispatchOriginal => OrderResolveActionV2::RedispatchOriginal,
    }
}

/// 回执必须与请求的命令、合同与动作一致，否则写结果无法确认，只能原样重试；补寄回执只能对应补寄请求。
fn receipt_dto(
    receipt: OrderResolveReceipt,
    request: &ResolveOrderRequest,
    contract_id: Uuid,
) -> Result<ResolveOrderReceiptV2, ConsignmentError> {
    let unknown = |_: ConsignmentError| ConsignmentError::OutcomeUnknown;
    let dto = match receipt {
        OrderResolveReceipt::ResolutionRecorded {
            command_id,
            contract_id: receipt_contract,
            contract_version,
            action,
            resolution_id,
        } => {
            if command_id.as_uuid() != request.command_id
                || receipt_contract != contract_id
                || action_dto(action) != request.action
            {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            ResolveOrderReceiptV2::ResolutionRecorded {
                schema_version: 2,
                operation: ResolveOperation::Resolve,
                command_id: request.command_id,
                contract_id,
                contract_version: positive(contract_version).map_err(unknown)?,
                action: request.action,
                resolution_id,
            }
        }
        OrderResolveReceipt::RedispatchQueued {
            command_id,
            contract_id: receipt_contract,
            contract_version,
            dispatch_id,
        } => {
            if command_id.as_uuid() != request.command_id
                || receipt_contract != contract_id
                || request.action != OrderResolveActionV2::RedispatchOriginal
            {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            ResolveOrderReceiptV2::RedispatchQueued {
                schema_version: 2,
                operation: ResolveOperation::Resolve,
                command_id: request.command_id,
                contract_id,
                contract_version: positive(contract_version).map_err(unknown)?,
                dispatch_id,
            }
        }
    };
    serde_json::to_value(&dto).map_err(outcome_unknown)?;
    Ok(dto)
}

/// 不变量 1、4、5、6：认证后原样交给用例；动作闭集、证据核对、相反付款拒绝与幂等都在存储事务内。
pub(super) async fn resolve<S: AdminOrderWriteState>(
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
    let request: ResolveOrderRequest = match deposit_write(&headers, body).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(service) = state.order_writes().admin.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let Ok(expected_version) = u64::try_from(request.expected_version) else {
        return error(&headers, ConsignmentError::InvalidDepositRequest);
    };
    let command = service.resolve(ResolveOrderInput {
        admin_actor: identity.actor,
        command_id: CommandId::from_uuid(request.command_id),
        contract_id,
        expected_version,
        action: action_of(request.action),
        evidence_id: request.evidence_id,
        request_id: request_id(&headers),
    });
    let result = match tokio::time::timeout(WRITE_TIMEOUT, command).await {
        Ok(Ok(receipt)) => receipt_dto(receipt, &request, contract_id).and_then(|dto| match dto {
            ResolveOrderReceiptV2::ResolutionRecorded { .. } => ok("订单处置已记录", dto),
            ResolveOrderReceiptV2::RedispatchQueued { .. } => accepted("补寄已排队", dto),
        }),
        Ok(Err(failure)) => Err(failure),
        Err(elapsed) => Err(outcome_unknown(elapsed)),
    };
    result.unwrap_or_else(|failure| error(&headers, failure))
}

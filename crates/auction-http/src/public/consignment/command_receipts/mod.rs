//! 本人 v2 寄售命令回执查询处理器（D9 §6.1、§6.3）。
//!
//! `GET /api/v2/me/consignment/commands/{command_id}` 只认 Public 登录会话（`private_identity`），
//! 受限权利 Cookie 不能读取；端口只按会话用户查询，非本人与不存在同形 404（不变量 7）。成功回执
//! 复用各写路由的同一 DTO 映射，与原写响应的 `data` 逐字段相同；确定拒绝只给安全码、命令 ID 与
//! 首次状态。读取不随寄售开关关闭：结果未知的客户端在重放前先查这里，404 只代表当次未见（不变量 1）。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, OwnedCommandReadV2, OwnedCommandV2, OwnedDepositDecision, PurchaseOutcome,
};
use auction_contracts::{
    OwnedCommandOperationV2, OwnedCommandReceiptV2, RejectedCommandV2, RejectedDispositionV2,
    WithdrawReceiptV2,
};
use auction_domain::{CommandId, UserId};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use uuid::Uuid;

use super::ConsignmentState;
use super::deposit::{reservation_dto, withdrawal_dto};
use super::input;
use super::mapping;
use super::purchase;
use super::recovery::relist_receipt_dto;
use super::response::{self, RouteResult};
use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};

#[cfg(test)]
mod tests;

/// 本人命令回执读取能力；端口缺失时返回 503。
#[derive(Clone, Default)]
pub struct CommandReceiptCapabilities {
    /// 按原玩家 actor 读取 v2 命令账本。
    pub read: Option<Arc<dyn OwnedCommandReadV2>>,
}

/// 路由只借用既有 Public 会话与回执读取能力。
pub(crate) trait CommandReceiptState: ConsignmentState {
    /// 当前装配为此进程提供的回执读取能力。
    fn command_receipts(&self) -> &CommandReceiptCapabilities;
}

/// 把本人 v2 命令回执路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: CommandReceiptState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route(
            "/api/v2/me/consignment/commands/{command_id}",
            method_strict::get(owned_command::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

async fn owned_command<S: CommandReceiptState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    input::no_query(&uri)?;
    let command_id = CommandId::from_uuid(input::command_id(path)?);
    let reader = state.command_receipts().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let command = reader
        .owned_command_v2(UserId::from_uuid(identity.user_id), command_id)
        .await
        .map_err(read_failure)?
        .ok_or(ConsignmentError::CommandNotFound)?;
    let receipt = receipt_dto(command, command_id).map_err(read_failure)?;
    // 答非所问的账本行按依赖不可用处理，绝不把别的命令当作本次结果。
    if receipt_command_id(&receipt) != command_id.as_uuid() {
        return Err(ConsignmentError::Unavailable.into());
    }
    response::loaded(receipt)
}

fn receipt_dto(
    command: OwnedCommandV2,
    command_id: CommandId,
) -> Result<OwnedCommandReceiptV2, ConsignmentError> {
    use OwnedDepositDecision::{Accepted, Rejected};

    Ok(match command {
        OwnedCommandV2::Purchase(PurchaseOutcome::Purchased(receipt)) => {
            OwnedCommandReceiptV2::Purchase(purchase::receipt_dto(receipt)?)
        }
        OwnedCommandV2::Purchase(PurchaseOutcome::Rejected(rejection)) => rejected(
            OwnedCommandOperationV2::Purchase,
            command_id,
            rejection.code(),
            rejection.http_status(),
        )?,
        OwnedCommandV2::Relist(Accepted(receipt)) => {
            OwnedCommandReceiptV2::Relist(relist_receipt_dto(receipt)?)
        }
        OwnedCommandV2::Withdraw(Accepted(receipt)) => {
            OwnedCommandReceiptV2::Withdraw(withdrawal_dto(receipt)?)
        }
        OwnedCommandV2::IssueReservation(Accepted(receipt)) => {
            OwnedCommandReceiptV2::IssueReservation(reservation_dto(receipt)?)
        }
        OwnedCommandV2::Relist(Rejected(rejection)) => rejected(
            OwnedCommandOperationV2::Relist,
            command_id,
            rejection.code(),
            rejection.status(),
        )?,
        OwnedCommandV2::Withdraw(Rejected(rejection)) => rejected(
            OwnedCommandOperationV2::Withdraw,
            command_id,
            rejection.code(),
            rejection.status(),
        )?,
        OwnedCommandV2::IssueReservation(Rejected(rejection)) => rejected(
            OwnedCommandOperationV2::IssueReservation,
            command_id,
            rejection.code(),
            rejection.status(),
        )?,
    })
}

/// 确定拒绝只给安全码、命令 ID 与首次状态；码不在 v2 本人闭集内时失败关闭（不变量 6）。
fn rejected(
    operation: OwnedCommandOperationV2,
    command_id: CommandId,
    code: &'static str,
    http_status: u16,
) -> Result<OwnedCommandReceiptV2, ConsignmentError> {
    Ok(OwnedCommandReceiptV2::Rejected(RejectedCommandV2 {
        schema_version: 2,
        operation,
        disposition: RejectedDispositionV2::Rejected,
        command_id: command_id.as_uuid(),
        code: mapping::wire_code(code)?,
        http_status: i64::from(http_status),
    }))
}

const fn receipt_command_id(receipt: &OwnedCommandReceiptV2) -> Uuid {
    match receipt {
        OwnedCommandReceiptV2::Purchase(receipt) => receipt.command_id,
        OwnedCommandReceiptV2::Relist(receipt) => receipt.command_id,
        OwnedCommandReceiptV2::IssueReservation(receipt) => receipt.command_id,
        OwnedCommandReceiptV2::Rejected(receipt) => receipt.command_id,
        OwnedCommandReceiptV2::Withdraw(
            WithdrawReceiptV2::WithdrawalRequested { command_id, .. }
            | WithdrawReceiptV2::WithdrawalAccepted { command_id, .. }
            | WithdrawReceiptV2::Closed { command_id, .. },
        ) => *command_id,
    }
}

// 读端口失败与映射失败都只表示没有可信回执；资源不可见由端口的 Ok(None) 单独表达。
fn read_failure(_failure: ConsignmentError) -> ConsignmentError {
    ConsignmentError::Unavailable
}

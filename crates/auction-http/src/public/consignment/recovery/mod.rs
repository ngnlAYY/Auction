//! 卖家本人重列与损失回执处理器（第 7 期 7C-3、7C-4）。
//!
//! 处理器只做认证、严格输入解码、端口调用与 DTO 映射；命令仲裁、资格、额度、止售、季界与上代封闭
//! 都由应用用例与存储事务决定。重列是新承诺，寄售关闭时先只读重放已接受的同 ID 命令，账本没有才返回
//! 404 `consignment_disabled`；损失回执只读、不随开关关闭（D9 §7）。两条路由都只认 Public 登录会话（`private_identity`），
//! 受限权利 Cookie 不能重列或读回执（D9 §5.3）。非本人与不存在同形 404（不变量 7）。

use std::net::SocketAddr;
use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, DepositOperation, ListingBusinessRejection, LossCaseState,
    ManualRelistService, ReceiptDisposition, RelistReceipt, SellerLossCaseRead,
    SellerLossCaseRecord,
};
use auction_contracts::{
    ListingGeneration, ListingStateV2, RelistDispositionV2, RelistListingV2, RelistOperationV2,
    RelistReceiptV2, SellerLossCaseV2,
};
use auction_domain::consignment::{ListingState, ListingVersion};
use auction_domain::{CommandId, UserId};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use uuid::Uuid;

use super::ConsignmentState;
use super::input;
use super::mapping;
use super::response::{self, RouteResult};
use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};
use crate::query::parse_public_uuid;

#[cfg(test)]
mod tests;

/// 卖家重列与损失回执的已装配能力。端口缺失时返回 503。
#[derive(Clone, Default)]
pub struct RecoveryCapabilities {
    /// 有效开关；关闭后首次重列返回 404，损失回执读取不受影响。
    pub enabled: bool,
    /// 卖家手动重列用例。
    pub relist: Option<Arc<ManualRelistService>>,
    /// 卖家本人损失回执读端口。
    pub loss: Option<Arc<dyn SellerLossCaseRead>>,
}

/// 路由只借用既有 Public 会话与重列 / 损失回执能力。
pub(crate) trait RecoveryState: ConsignmentState {
    /// 当前装配为此进程提供的能力。
    fn recovery(&self) -> &RecoveryCapabilities;
}

/// 把重列与本人损失回执两条路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: RecoveryState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/consignment/listings/{listing_id}/relist", method_strict::post(relist::<S>))
        .route(
            "/api/v2/me/consignment/positions/{position_id}/loss-case",
            method_strict::get(loss_case::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

/// 不变量 1、6：守卫与认证先于目标与正文解码；同 ID 同请求由写端账本回原回执，异请求 409。
async fn relist<S: RecoveryState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = input::listing_id(path)?;
    let wire: RelistListingV2 = input::decode_write(&parts.headers, body).await?;
    let capabilities = state.recovery();
    let expected_version =
        ListingVersion::new(u64::try_from(wire.expected_version.get()).map_err(invalid_request)?)
            .map_err(invalid_request)?;
    let seller = UserId::from_uuid(identity.user_id);
    let command_id = CommandId::from_uuid(wire.command_id);
    let request_id = boundary::request_id(&parts.headers);
    // 不变量 1、D9 §6.3：重列是新承诺，软关闭截止它；但关闭前已接受的同 ID 同请求仍回原结果，
    // 所以关闭时先只读重放，账本没有该命令才返回首次禁用的 404，绝不在关闭时进入写端。
    if !capabilities.enabled {
        let Some(service) = capabilities.relist.as_ref() else {
            return Err(ConsignmentError::Disabled.into());
        };
        let Some(receipt) =
            service.replay(seller, command_id, listing_id, expected_version, request_id).await?
        else {
            return Err(ConsignmentError::Disabled.into());
        };
        return response::applied("已重新上架", relist_dto(receipt, listing_id, &wire)?);
    }
    let service = capabilities.relist.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let receipt =
        service.relist(seller, command_id, listing_id, expected_version, request_id).await?;
    response::applied("已重新上架", relist_dto(receipt, listing_id, &wire)?)
}

/// 回执必须回答本请求（命令、目标、固定处置与状态），否则按结果未知处理，客户端只能原样重放。
fn relist_dto(
    receipt: RelistReceipt,
    listing_id: Uuid,
    wire: &RelistListingV2,
) -> Result<RelistReceiptV2, ConsignmentError> {
    if receipt.listing_id != listing_id || receipt.command_id.as_uuid() != wire.command_id {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    relist_receipt_dto(receipt)
}

/// 只按回执自身的固定处置与状态映射；调用方另核命令与目标。本人回执查询复用同一映射。
pub(super) fn relist_receipt_dto(
    receipt: RelistReceipt,
) -> Result<RelistReceiptV2, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation != DepositOperation::Relist
        || receipt.disposition != ReceiptDisposition::Relisted
        || receipt.state != ListingState::Available
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(RelistReceiptV2 {
        schema_version: 2,
        operation: RelistOperationV2::Relist,
        disposition: RelistDispositionV2::Relisted,
        command_id: receipt.command_id.as_uuid(),
        listing_id: receipt.listing_id,
        listing_version: mapping::version(receipt.listing_version.value())
            .map_err(outcome_unknown)?,
        state: ListingStateV2::Available,
        price: mapping::price(receipt.price.value()).map_err(outcome_unknown)?,
        generation: i64::try_from(receipt.generation)
            .ok()
            .and_then(ListingGeneration::new)
            .ok_or(ConsignmentError::OutcomeUnknown)?,
    })
}

async fn loss_case<S: RecoveryState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    input::no_query(&uri)?;
    let Ok(Path(raw)) = path else {
        return Err(ConsignmentError::InvalidListingId.into());
    };
    let position_id = parse_public_uuid(&raw).map_err(|()| ConsignmentError::InvalidListingId)?;
    let reader = state.recovery().loss.as_ref().ok_or(ConsignmentError::Unavailable)?;
    // 非本人与不存在同形：端口对两者都返回空。
    let record = reader
        .seller_loss_case(UserId::from_uuid(identity.user_id), position_id)
        .await
        .map_err(read_failure)?
        .ok_or(ConsignmentError::Business(ListingBusinessRejection::ListingNotFound))?;
    if record.position_id != position_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    response::loaded(loss_dto(record))
}

/// 只映射编号、状态与回执时刻；端口记录本身不带私有说明、证据或金额（不变量 6、7）。
fn loss_dto(record: SellerLossCaseRecord) -> SellerLossCaseV2 {
    SellerLossCaseV2 {
        position_id: record.position_id,
        loss_case_id: record.loss_case_id,
        state: match record.state {
            LossCaseState::Opened => auction_contracts::LossCaseState::Opened,
            LossCaseState::Reviewing => auction_contracts::LossCaseState::Reviewing,
            LossCaseState::Closed => auction_contracts::LossCaseState::Closed,
        },
        recorded_at: record.recorded_at,
        receipt_id: record.receipt_id,
    }
}

fn invalid_request<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidListingRequest
}

fn outcome_unknown<T>(_error: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

fn read_failure<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

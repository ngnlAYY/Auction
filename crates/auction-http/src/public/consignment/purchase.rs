//! 买家购买入口与公开 v2 可购目录 / 详情处理器。
//!
//! 处理器只做认证、严格输入解码、端口调用与 DTO 映射；命令仲裁、锁内判定与确定拒绝都由应用用例
//! 和存储事务决定。确定写前的拒绝（含 `funding_unavailable`）才以其业务码返回；存储不可用与提交
//! 结果未知保持各自的 503 码，绝不伪造确定拒绝（不变量 1、4、6）。路由挂载由集成车道按本模块的
//! `mount` 接入。

use std::net::SocketAddr;
use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, PublicCatalogDetailV2, PublicCatalogEntryV2,
    PublicCatalogReadV2, PurchaseCommandService, PurchaseOutcome, PurchaseReceipt,
};
use auction_contracts::{
    ListingActionV2, ListingDetailV2, ListingGeneration, ListingMode, ListingStateV2,
    ListingSummaryV2, OrderStateV2, PublicListingStateV2, PurchaseDispositionV2,
    PurchaseOperationV2, PurchaseReceiptV2, PurchaseRejectionCodeV2, PurchaseRequestV2,
};
use auction_domain::consignment::purchase::{PurchaseInput, PurchaseRejection};
use auction_domain::consignment::{
    ListingMode as DomainMode, ListingPrice, ListingState, ListingVersion, OrderState,
};
use auction_domain::{CommandId, UserId};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::middleware;
use axum::response::IntoResponse;

use super::ConsignmentState;
use super::input;
use super::mapping;
use super::response::{self, RouteResult};
use crate::errors::api_error;
use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};

/// 购买与公开 v2 目录的已装配能力。端口缺失时保留无数据库骨架语义。
#[derive(Clone, Default)]
pub struct PurchaseCapabilities {
    /// 有效开关；关闭后新购买与公开目录一律 404。
    pub enabled: bool,
    /// 购买用例。
    pub commands: Option<Arc<PurchaseCommandService>>,
    /// 公开 v2 目录与详情。
    pub catalog: Option<Arc<dyn PublicCatalogReadV2>>,
}

/// 路由只借用既有 Public 会话与购买能力。
pub(crate) trait PurchaseState: ConsignmentState {
    /// 当前装配为此进程提供的购买能力。
    fn purchase(&self) -> &PurchaseCapabilities;
}

/// 把购买与公开 v2 目录三条路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: PurchaseState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/consignment/listings", method_strict::get(list_catalog::<S>))
        .route("/api/v2/consignment/listings/{listing_id}", method_strict::get(catalog_detail::<S>))
        .route(
            "/api/v2/consignment/listings/{listing_id}/purchase",
            method_strict::post(purchase::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

async fn purchase<S: PurchaseState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = input::listing_id(path)?;
    let wire: PurchaseRequestV2 = input::decode_write(&parts.headers, body).await?;
    let capabilities = state.purchase();
    let input = PurchaseInput {
        listing_id,
        expected_version: ListingVersion::new(
            u64::try_from(wire.expected_version.get()).map_err(invalid_request)?,
        )
        .map_err(invalid_request)?,
        expected_price: ListingPrice::new(wire.expected_price.get()).map_err(invalid_price)?,
    };
    let buyer = UserId::from_uuid(identity.user_id);
    let command_id = CommandId::from_uuid(wire.command_id);
    let request_id = boundary::request_id(&parts.headers);
    // 不变量 1、D9 §6.3：关闭只截止新承诺，已接受命令的同 ID 重放照常回原结果，所以先只读重放；
    // 账本没有该命令才按开关返回首次禁用的 404，绝不在关闭时受理新购买。
    if !capabilities.enabled {
        let Some(service) = capabilities.commands.as_ref() else {
            return Err(ConsignmentError::Disabled.into());
        };
        let Some(outcome) = service.replay_purchase(buyer, command_id, input, request_id).await?
        else {
            return Err(ConsignmentError::Disabled.into());
        };
        return respond(outcome, listing_id, &wire);
    }
    let service = capabilities.commands.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let outcome = service.purchase(buyer, command_id, input, request_id).await?;
    respond(outcome, listing_id, &wire)
}

/// 把端口结果映射为 HTTP：成交与重放都是 201，确定拒绝按业务码，答非所问按结果未知。
fn respond(
    outcome: PurchaseOutcome,
    listing_id: uuid::Uuid,
    wire: &PurchaseRequestV2,
) -> RouteResult {
    match outcome {
        PurchaseOutcome::Purchased(receipt) => {
            if receipt.listing_id != listing_id {
                return Err(ConsignmentError::OutcomeUnknown.into());
            }
            let dto = receipt_dto(receipt)?;
            // 回执必须回答本请求，否则按结果未知处理，客户端只能原样重放。
            if dto.answers(wire).is_err() {
                return Err(ConsignmentError::OutcomeUnknown.into());
            }
            response::created("购买成功", dto)
        }
        PurchaseOutcome::Rejected(rejection) => rejected(rejection),
    }
}

/// 确定拒绝：码与状态必须与 DTO 闭集一致，否则失败关闭为结果未知。
fn rejected(rejection: PurchaseRejection) -> RouteResult {
    let Ok(code) = mapping::wire_code::<PurchaseRejectionCodeV2>(rejection.code()) else {
        return Err(ConsignmentError::OutcomeUnknown.into());
    };
    let Ok(status) = u16::try_from(code.http_status()) else {
        return Err(ConsignmentError::OutcomeUnknown.into());
    };
    if status != rejection.http_status() {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    let status = StatusCode::from_u16(status).map_err(outcome_unknown)?;
    Ok(api_error(status, rejection.code(), rejection_message(rejection)).into_response())
}

const fn rejection_message(rejection: PurchaseRejection) -> &'static str {
    match rejection {
        PurchaseRejection::ListingNotActive => "挂单已不可购买",
        PurchaseRejection::OperationsHold => "运营止售有效",
        PurchaseRejection::StaleVersion => "挂单版本已变化，请刷新后确认",
        PurchaseRejection::PriceChanged => "标价已变化，请刷新后确认",
        PurchaseRejection::EquipmentCommitted => "装备已被承诺",
        PurchaseRejection::CustodyUnproven => "代管状态未证明",
        PurchaseRejection::SeasonClosed => "寄售季已关闭",
        PurchaseRejection::BotParty => "认证 Bot 不能作为交易当事人",
        PurchaseRejection::SelfPurchase => "不能购买自己的挂单",
        PurchaseRejection::ParticipantIneligible => "当事人不具备购买资格",
        PurchaseRejection::FundingUnavailable => "资金能力暂不可用",
    }
}

/// 成功回执映射；本人回执查询复用同一映射，保证与原写响应逐字段相同。
pub(super) fn receipt_dto(receipt: PurchaseReceipt) -> Result<PurchaseReceiptV2, ConsignmentError> {
    if receipt.order_state != OrderState::AwaitingPayment {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(PurchaseReceiptV2 {
        schema_version: 2,
        operation: PurchaseOperationV2::Purchase,
        disposition: PurchaseDispositionV2::Purchased,
        command_id: receipt.command_id.as_uuid(),
        listing_id: receipt.listing_id,
        listing_version: mapping::version(receipt.listing_version.value())
            .map_err(outcome_unknown)?,
        state: ListingStateV2::SalePending,
        contract_id: receipt.contract_id.as_uuid(),
        contract_version: i64::try_from(receipt.contract_version).map_err(outcome_unknown)?,
        equipment_id: receipt.equipment_id,
        world: mapping::wire_code(receipt.world.as_str()).map_err(outcome_unknown)?,
        price: mapping::price(receipt.price.value()).map_err(outcome_unknown)?,
        order_state: OrderStateV2::AwaitingPayment,
    })
}

async fn list_catalog<S: PurchaseState>(State(state): State<S>, uri: Uri) -> RouteResult {
    let query = input::public_query(&uri)?;
    let capabilities = state.purchase();
    if !capabilities.enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let reader = capabilities.catalog.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let window = query.window;
    let page = reader.list_public_v2(query).await.map_err(read_failure)?;
    response::loaded(mapping::page(page, window, summary_dto)?)
}

async fn catalog_detail<S: PurchaseState>(
    State(state): State<S>,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    input::no_query(&uri)?;
    let listing_id = input::listing_id(path)?;
    let capabilities = state.purchase();
    if !capabilities.enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let reader = capabilities.catalog.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let detail = reader
        .public_detail_v2(listing_id)
        .await
        .map_err(read_failure)?
        .ok_or(ConsignmentError::Business(ListingBusinessRejection::ListingNotFound))?;
    if detail.entry.listing_id != listing_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    response::loaded(detail_dto(detail)?)
}

fn detail_dto(detail: PublicCatalogDetailV2) -> Result<ListingDetailV2, ConsignmentError> {
    let equipment = mapping::equipment(&detail.equipment);
    Ok(ListingDetailV2 { summary: summary_dto(detail.entry)?, equipment })
}

fn summary_dto(entry: PublicCatalogEntryV2) -> Result<ListingSummaryV2, ConsignmentError> {
    let state = match entry.state {
        ListingState::Displayed => PublicListingStateV2::Displayed,
        ListingState::AwaitingDeposit => PublicListingStateV2::AwaitingDeposit,
        ListingState::Available => PublicListingStateV2::Available,
        _ => return Err(ConsignmentError::Unavailable),
    };
    let mode = match entry.mode {
        DomainMode::Direct => ListingMode::Direct,
        DomainMode::Consigned => ListingMode::Consigned,
    };
    let generation = entry
        .generation
        .map(|value| {
            i64::try_from(value)
                .ok()
                .and_then(ListingGeneration::new)
                .ok_or(ConsignmentError::Unavailable)
        })
        .transpose()?;
    Ok(ListingSummaryV2 {
        listing_id: entry.listing_id,
        version: mapping::version(entry.version.value())?,
        mode,
        state,
        world_id: mapping::wire_code(entry.identity.world_id())?,
        external_equipment_id: entry.identity.external_equipment_id().to_owned(),
        name: entry.name,
        category: entry.category,
        level: mapping::level(entry.level),
        price: mapping::price(entry.price.value())?,
        seller_name: entry.seller_name,
        updated_at: entry.updated_at,
        expires_at: entry.expires_at,
        generation,
        effective_reason: None,
        allowed_actions: if entry.purchasable {
            vec![ListingActionV2::Purchase]
        } else {
            Vec::new()
        },
        purchasable: entry.purchasable,
    })
}

fn read_failure(_error: ConsignmentError) -> ConsignmentError {
    ConsignmentError::Unavailable
}

fn invalid_request<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidListingRequest
}

fn invalid_price<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidPrice
}

fn outcome_unknown<T>(_error: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

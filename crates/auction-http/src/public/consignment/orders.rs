//! 本人订单列表与详情处理器。
//!
//! 只向会话用户返回其作为买家或卖家的订单：非本人与不存在同形 404（不变量 7），响应不含预约码、
//! 装备 key、对方账号与原信正文；金额按角色省略，第 6 期订单没有任何买卖双方动作。读取不随寄售
//! 开关关闭（D9 §7：关闭只截止新承诺）。路由挂载由集成车道按本模块的 `mount` 接入。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingWorld, OrderQuery, OrderRole, OwnedOrder,
    OwnedOrderRead,
};
use auction_contracts::{OrderKindV2, OrderRoleV2, OrderV2};
use auction_domain::UserId;
use auction_domain::consignment::{OrderKind, OrderState};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;

use super::ConsignmentState;
use super::input;
use super::mapping;
use super::response::{self, RouteResult};
use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};
use crate::query::parse_public_uuid;

const ORDER_QUERY_KEYS: [&str; 5] = ["role", "world", "state", "page", "page_size"];

/// 全部订单态，用于把查询串里的线协议码解码回领域类型。
const ORDER_STATES: [OrderState; 8] = [
    OrderState::AwaitingPayment,
    OrderState::AwaitingHandoff,
    OrderState::AwaitingPayout,
    OrderState::Refunding,
    OrderState::Withdrawing,
    OrderState::Completed,
    OrderState::Cancelled,
    OrderState::Reconcile,
];

/// 本人订单读取的已装配能力。
#[derive(Clone, Default)]
pub struct OrderCapabilities {
    /// 本人订单读端口；缺失时返回 503。
    pub read: Option<Arc<dyn OwnedOrderRead>>,
}

/// 路由只借用既有 Public 会话与订单读能力。
pub(crate) trait OrderReadState: ConsignmentState {
    /// 当前装配为此进程提供的订单读能力。
    fn orders(&self) -> &OrderCapabilities;
}

/// 把两条本人订单路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: OrderReadState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/me/consignment/orders", method_strict::get(list::<S>))
        .route("/api/v2/me/consignment/orders/{contract_id}", method_strict::get(detail::<S>))
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

fn order_query(uri: &Uri) -> Result<OrderQuery, ConsignmentError> {
    let query = input::listing_query(uri, &ORDER_QUERY_KEYS)?;
    let state = query
        .get("state")
        .map(|raw| {
            ORDER_STATES
                .into_iter()
                .find(|state| state.as_str() == raw)
                .ok_or(ConsignmentError::InvalidQuery)
        })
        .transpose()?;
    let role = match query.get("role").map(String::as_str) {
        None => None,
        Some("buyer") => Some(OrderRole::Buyer),
        Some("seller") => Some(OrderRole::Seller),
        Some(_) => return Err(ConsignmentError::InvalidQuery),
    };
    Ok(OrderQuery {
        window: input::page_window(&query)?,
        role,
        world: input::enum_filter::<ListingWorld>(&query, "world")?,
        state,
    })
}

async fn list<S: OrderReadState>(
    State(state): State<S>,
    headers: HeaderMap,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    let query = order_query(&uri)?;
    let reader = state.orders().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let page = reader
        .list_owned_orders(UserId::from_uuid(identity.user_id), query)
        .await
        .map_err(read_failure)?;
    response::loaded(mapping::page(page, query.window, |order| {
        // 端口按角色筛选；返回了不属于筛选的角色说明读端口出错，整页失败关闭。
        if query.role.is_some_and(|role| role != order.role()) {
            return Err(ConsignmentError::Unavailable);
        }
        order_dto(&order)
    })?)
}

async fn detail<S: OrderReadState>(
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
    let contract_id = parse_public_uuid(&raw).map_err(|()| ConsignmentError::InvalidListingId)?;
    let reader = state.orders().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    // 非本人与不存在同形：端口对两者都返回空。
    let order = reader
        .owned_order(UserId::from_uuid(identity.user_id), contract_id)
        .await
        .map_err(read_failure)?
        .ok_or(ConsignmentError::Business(ListingBusinessRejection::ListingNotFound))?;
    if order.core().contract_id != contract_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    response::loaded(order_dto(&order)?)
}

/// 把本人订单映射为 DTO；金额已在端口按角色省略，DTO 校验在序列化时再核一次。
fn order_dto(order: &OwnedOrder) -> Result<OrderV2, ConsignmentError> {
    let core = order.core();
    let stages = core.stages;
    Ok(OrderV2 {
        contract_id: core.contract_id,
        kind: match core.kind {
            OrderKind::Sale => OrderKindV2::Sale,
            OrderKind::Withdrawal => OrderKindV2::Withdrawal,
        },
        role: match order.role() {
            OrderRole::Buyer => OrderRoleV2::Buyer,
            OrderRole::Seller => OrderRoleV2::Seller,
        },
        listing_id: core.listing_id,
        equipment_id: core.equipment_id,
        world: mapping::wire_code(core.world.as_str())?,
        order_state: mapping::wire_code(stages.order_state.as_str())?,
        delivery_stage: mapping::wire_code(stages.delivery.as_str())?,
        payment_stage: mapping::wire_code(stages.payment.as_str())?,
        payout_stage: mapping::wire_code(stages.payout.as_str())?,
        refund_stage: mapping::wire_code(stages.refund.as_str())?,
        allowed_actions: Vec::new(),
        version: i64::try_from(core.version).map_err(read_failure)?,
        price: core.price.map(|price| mapping::price(price.value())).transpose()?,
        deadline_at: core.deadline_at,
        fee: core.fee,
        seller_amount: core.seller_amount,
    })
}

fn read_failure<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

//! Admin 订单列表、详情与证据分页处理器。
//!
//! 认证沿用既有 Admin 会话守卫；读取不受寄售开关影响。Admin 同时看到买卖双方与完整金额，响应只含
//! ID、阶段码、金额、摘要与时刻，不含预约码、装备 key 与原信正文（不变量 7）。读写路由都由
//! `server/surfaces.rs` 的 Admin 路由表经 `mount` 与 `mount_writes` 接入。
//!
//! 第 7 期在子模块追加：`recheck` 与 `resolve` 两个 Admin 写命令（`mount_writes`），以及恢复段证据的映射
//! （`evidence_recovery`）。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    AdminOrder, AdminOrderQuery, AdminOrderRead, ConsignmentError, ListingBusinessRejection,
    ListingPageWindow, ListingReadPage, ListingWorld, OrderAdminService, OrderEvidence,
};
use auction_contracts::{
    AdminOrderV2, ListingPage, OrderEvidenceItemV2, OrderEvidencePageV2, OrderKindV2,
};
use auction_domain::UserId;
use auction_domain::consignment::{OrderKind, OrderState};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use axum::response::Response;
use serde::Deserialize;
use serde::de::value::StrDeserializer;

use super::AdminConsignmentState;
use super::boundary::{deposit_error as error, ok};
use super::input::{decimal, filter};
use crate::middleware::method_strict::{get, post};
use crate::middleware::response_cache;
use crate::query::{parse_canonical_uuid, parse_query};
use crate::session::authenticated_admin_session;

mod evidence_recovery;
mod recheck;
mod resolve;
#[cfg(test)]
mod tests;

const READ_TIMEOUT: Duration = Duration::from_secs(2);
const LIST_KEYS: [&str; 4] = ["world", "state", "page", "page_size"];
const EVIDENCE_KEYS: [&str; 2] = ["page", "page_size"];

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

/// Admin 订单读取的已装配能力。
#[derive(Clone, Default)]
pub struct AdminOrderCapabilities {
    /// 订单列表、详情与证据读端口；缺失时返回 503。
    pub read: Option<Arc<dyn AdminOrderRead>>,
}

/// Admin 订单路由需要的会话状态与订单读能力。
pub(crate) trait AdminOrderReadState: AdminConsignmentState {
    /// 当前装配为此进程提供的订单读能力。
    fn orders(&self) -> &AdminOrderCapabilities;
}

/// Admin 订单写命令（recheck 与 resolve）的已装配能力。
#[derive(Clone, Default)]
pub struct AdminOrderWriteCapabilities {
    /// 订单复核与处置用例；缺失时返回 503。
    pub admin: Option<Arc<OrderAdminService>>,
}

/// Admin 订单写路由需要的会话状态与订单写能力。
pub(crate) trait AdminOrderWriteState: AdminConsignmentState {
    /// 当前装配为此进程提供的订单写能力。
    fn order_writes(&self) -> &AdminOrderWriteCapabilities;
}

/// 向 Admin 路由器追加 recheck 与 resolve 两条写路径；认证、Origin、CSRF 与正文上限沿用既有 Admin 边界。
pub(crate) fn mount_writes<S: AdminOrderWriteState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route(
            "/api/v2/admin/consignment/orders/{contract_id}/recheck",
            post(recheck::recheck::<S>),
        )
        .route(
            "/api/v2/admin/consignment/orders/{contract_id}/resolve",
            post(resolve::resolve::<S>),
        )
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

/// 向 Admin 路由器追加三条订单读取路径。
pub(crate) fn mount<S: AdminOrderReadState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/admin/consignment/orders", get(list::<S>))
        .route("/api/v2/admin/consignment/orders/{contract_id}", get(detail::<S>))
        .route("/api/v2/admin/consignment/orders/{contract_id}/evidence", get(evidence::<S>))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

fn window(
    query: &std::collections::BTreeMap<&str, String>,
) -> Result<ListingPageWindow, ConsignmentError> {
    let optional = |key| query.get(key).map(String::as_str);
    ListingPageWindow::new(
        optional("page").map(decimal).transpose()?,
        optional("page_size").map(decimal).transpose()?,
    )
}

fn list_query(uri: &Uri) -> Result<AdminOrderQuery, ConsignmentError> {
    let query = parse_query(uri, &LIST_KEYS).map_err(invalid_query)?;
    let state = query
        .get("state")
        .map(|raw| {
            ORDER_STATES
                .into_iter()
                .find(|state| state.as_str() == raw)
                .ok_or(ConsignmentError::InvalidQuery)
        })
        .transpose()?;
    Ok(AdminOrderQuery {
        window: window(&query)?,
        world: filter::<ListingWorld>(query.get("world").map(String::as_str))?,
        state,
    })
}

fn contract_target<'a>(
    uri: &Uri,
    path: Result<Path<String>, PathRejection>,
    allowed: &[&'a str],
) -> Result<(uuid::Uuid, std::collections::BTreeMap<&'a str, String>), ConsignmentError> {
    if uri.path().contains('%') {
        return Err(ConsignmentError::InvalidListingId);
    }
    let Ok(Path(value)) = path else {
        return Err(ConsignmentError::InvalidListingId);
    };
    let contract_id = parse_canonical_uuid(&value).ok_or(ConsignmentError::InvalidListingId)?;
    let query = parse_query(uri, allowed).map_err(invalid_query)?;
    Ok((contract_id, query))
}

fn invalid_query<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidQuery
}

fn unavailable<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

/// 写超时与回执无法确认同为结果未知，调用方只能用原命令 ID 与原正文重试。
fn outcome_unknown<T>(_error: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

/// 查询全部订单；寄售开关不阻断认证后的查询。
pub(super) async fn list<S: AdminOrderReadState>(
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
    let Some(port) = state.orders().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.list_admin_orders(query)).await {
        Ok(Ok(page)) => order_page(page, query.window),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|page| ok("订单列表已加载", page))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 订单详情；不存在统一 404。
pub(super) async fn detail<S: AdminOrderReadState>(
    State(state): State<S>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let contract_id = match contract_target(&uri, path, &[]) {
        Ok((contract_id, _)) => contract_id,
        Err(failure) => return error(&headers, failure),
    };
    let Some(port) = state.orders().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result = match tokio::time::timeout(READ_TIMEOUT, port.admin_order(contract_id)).await {
        Ok(Ok(Some(order))) if order.core.contract_id == contract_id => order_dto(&order),
        Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
        _ => Err(ConsignmentError::Unavailable),
    };
    result
        .and_then(|order| ok("订单详情已加载", order))
        .unwrap_or_else(|failure| error(&headers, failure))
}

/// 订单证据分页；订单不存在统一 404。
pub(super) async fn evidence<S: AdminOrderReadState>(
    State(state): State<S>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authenticated_admin_session(state.admin_state(), &headers).await {
        return *response;
    }
    let (contract_id, query) = match contract_target(&uri, path, &EVIDENCE_KEYS) {
        Ok(target) => target,
        Err(failure) => return error(&headers, failure),
    };
    let window = match window(&query) {
        Ok(window) => window,
        Err(failure) => return error(&headers, failure),
    };
    let Some(port) = state.orders().read.as_deref() else {
        return error(&headers, ConsignmentError::Unavailable);
    };
    let result =
        match tokio::time::timeout(READ_TIMEOUT, port.order_evidence(contract_id, window)).await {
            Ok(Ok(Some(page))) => evidence_page(page, window),
            Ok(Ok(None)) => Err(ListingBusinessRejection::ListingNotFound.into()),
            _ => Err(ConsignmentError::Unavailable),
        };
    result
        .and_then(|page| ok("订单证据已加载", page))
        .unwrap_or_else(|failure| error(&headers, failure))
}

fn page_of<T, U>(
    page: ListingReadPage<T>,
    requested: ListingPageWindow,
    map: impl FnMut(T) -> Result<U, ConsignmentError>,
) -> Result<ListingPage<U>, ConsignmentError> {
    if page.window != requested {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(ListingPage {
        items: page.items.into_iter().map(map).collect::<Result<_, _>>()?,
        total: i64::try_from(page.total).map_err(unavailable)?,
        page: i64::from(requested.page()),
        page_size: i64::from(requested.page_size()),
    })
}

fn order_page(
    page: ListingReadPage<AdminOrder>,
    requested: ListingPageWindow,
) -> Result<ListingPage<AdminOrderV2>, ConsignmentError> {
    page_of(page, requested, |order| order_dto(&order))
}

fn evidence_page(
    page: ListingReadPage<OrderEvidence>,
    requested: ListingPageWindow,
) -> Result<OrderEvidencePageV2, ConsignmentError> {
    page_of(page, requested, evidence_dto)
}

/// DTO 拥有闭集枚举，复用已发布的线协议值；未知值不合成默认状态。
fn wire_code<T: for<'de> Deserialize<'de>>(code: &str) -> Result<T, ConsignmentError> {
    let Ok(value) = T::deserialize(StrDeserializer::<serde::de::value::Error>::new(code)) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(value)
}

fn order_dto(order: &AdminOrder) -> Result<AdminOrderV2, ConsignmentError> {
    let core = &order.core;
    let stages = core.stages;
    Ok(AdminOrderV2 {
        contract_id: core.contract_id,
        kind: match core.kind {
            OrderKind::Sale => OrderKindV2::Sale,
            OrderKind::Withdrawal => OrderKindV2::Withdrawal,
        },
        listing_id: core.listing_id,
        equipment_id: core.equipment_id,
        world: wire_code(core.world.as_str())?,
        order_state: wire_code(stages.order_state.as_str())?,
        delivery_stage: wire_code(stages.delivery.as_str())?,
        payment_stage: wire_code(stages.payment.as_str())?,
        payout_stage: wire_code(stages.payout.as_str())?,
        refund_stage: wire_code(stages.refund.as_str())?,
        version: i64::try_from(core.version).map_err(unavailable)?,
        price: core
            .price
            .map(|price| {
                auction_contracts::ListingPrice::new(price.value())
                    .ok_or(ConsignmentError::Unavailable)
            })
            .transpose()?,
        deadline_at: core.deadline_at,
        fee: core.fee,
        seller_amount: core.seller_amount,
        buyer_user_id: order.buyer_user_id.map(UserId::as_uuid),
        buyer_name: order.buyer_name.clone(),
        seller_user_id: order.seller_user_id.as_uuid(),
        seller_name: order.seller_name.clone(),
        updated_at: core.updated_at,
    })
}

fn positive(value: u64) -> Result<i64, ConsignmentError> {
    i64::try_from(value).map_err(unavailable)
}

fn evidence_dto(item: OrderEvidence) -> Result<OrderEvidenceItemV2, ConsignmentError> {
    Ok(match item {
        OrderEvidence::Round { round_id, round_number, leg, delivery_stage, created_at } => {
            OrderEvidenceItemV2::Round {
                round_id,
                round_number: i64::from(round_number),
                leg: wire_code(leg)?,
                delivery_stage: wire_code(delivery_stage)?,
                created_at,
            }
        }
        OrderEvidence::Attempt { attempt_id, round_id, fence, write_started, delivery_stage } => {
            OrderEvidenceItemV2::Attempt {
                attempt_id,
                round_id,
                fence: positive(fence)?,
                write_started,
                delivery_stage: wire_code(delivery_stage)?,
            }
        }
        OrderEvidence::ReturnTake { intake_id, work_id, original_round_id, fence, state } => {
            OrderEvidenceItemV2::ReturnTake {
                intake_id,
                work_id,
                original_round_id,
                fence: fence.map(positive).transpose()?,
                state: wire_code(state)?,
            }
        }
        OrderEvidence::Observation {
            observation_id,
            category,
            conclusion,
            digest,
            observed_at,
        } => OrderEvidenceItemV2::Observation {
            observation_id,
            category: wire_code(category)?,
            conclusion: wire_code(conclusion)?,
            digest,
            observed_at,
        },
        OrderEvidence::CashUse { purpose, state, amount } => OrderEvidenceItemV2::CashUse {
            purpose: wire_code(purpose)?,
            state: wire_code(state)?,
            amount,
        },
        OrderEvidence::Blocker { blocker, since, resolved } => {
            OrderEvidenceItemV2::Blocker { blocker: wire_code(blocker)?, since, resolved }
        }
        recovery @ (OrderEvidence::Closure { .. }
        | OrderEvidence::Resolution { .. }
        | OrderEvidence::Recheck { .. }) => evidence_recovery::recovery_dto(recovery)?,
    })
}

//! 当前用户的竞价与代理授权读取处理器，以及账户页投影校验和 DTO 映射。

use auction_application::auction::{
    DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_ACCOUNT_OFFSET, MAX_PUBLIC_ACCOUNT_PAGE_SIZE,
    PublicAccountPageRequest, PublicMyBid, PublicMyBidsPage, PublicMyBidsRequest, PublicOwnedProxy,
    PublicProxyBid, PublicProxyTransition,
};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{
    ApiResponse, PublicMyBidsResponse, PublicMyProxyResponse, PublicOwnedBidResponse,
    PublicOwnedEquipmentResponse, PublicOwnedProxyCapabilitiesResponse,
    PublicOwnedProxyQuotaResponse, PublicOwnedProxyResponse, PublicOwnedProxyTransitionResponse,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::public_timestamp;
use crate::query::{page_offset, parse_positive_i64, parse_public_uuid, parse_query};
use crate::session::{authenticated_public_identity, check_public_expected_user, no_store_headers};
use crate::state::PublicApiState;

/// 读取当前用户自己的普通竞价和代理历史，保留旧接口的独立分页字段。
pub(crate) async fn list_my_bids(
    State(state): State<PublicApiState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    if let Err(response) = check_public_expected_user(&headers, identity.user_id) {
        return *response;
    }
    let query = match parse_query(&uri, &["page", "page_size", "bid_page", "proxy_page"]) {
        Ok(query) => query,
        Err(()) => return query_rejection().into_response(),
    };
    let independent = query.contains_key("bid_page") || query.contains_key("proxy_page");
    if independent && query.contains_key("page") {
        return query_rejection().into_response();
    }
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE,
        Some(MAX_PUBLIC_ACCOUNT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return query_rejection().into_response(),
    };
    let bid_page =
        match parse_positive_i64(query.get(if independent { "bid_page" } else { "page" }), 1, None)
        {
            Ok(page) => page,
            Err(()) => return query_rejection().into_response(),
        };
    let proxy_page = if independent {
        match parse_positive_i64(query.get("proxy_page"), 1, None) {
            Ok(page) => page,
            Err(()) => return query_rejection().into_response(),
        }
    } else {
        bid_page
    };
    let Some(bid_offset) = page_offset(bid_page, page_size, Some(MAX_PUBLIC_ACCOUNT_OFFSET)) else {
        return query_rejection().into_response();
    };
    let Some(proxy_offset) = page_offset(proxy_page, page_size, Some(MAX_PUBLIC_ACCOUNT_OFFSET))
    else {
        return query_rejection().into_response();
    };
    let request = PublicMyBidsRequest {
        bids: PublicAccountPageRequest { limit: page_size, offset: bid_offset },
        proxies: PublicAccountPageRequest { limit: page_size, offset: proxy_offset },
    };
    if request.validate().is_err() {
        return query_rejection().into_response();
    }
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    };
    let page = match reader.list_public_user_bids(identity.user_id, request).await {
        Ok(page) => page,
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_account",
                "read_bids",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户竞价暂不可用",
            )
            .into_response();
        }
    };
    if !valid_account_page(&page, identity.user_id, &request) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    }
    let data = PublicMyBidsResponse {
        bids: page.bids.into_iter().map(map_owned_bid).collect(),
        proxies: page.proxies.into_iter().map(map_owned_proxy).collect(),
        bid_total: page.bid_total,
        proxy_total: page.proxy_total,
        page: bid_page,
        bid_page,
        proxy_page,
        page_size,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("账户竞价已加载", data)))
        .into_response()
}

/// 读取当前用户在指定公开 lot 上最新的一份代理授权。
pub(crate) async fn get_my_proxy_authorization(
    State(state): State<PublicApiState>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    if let Err(response) = check_public_expected_user(&headers, identity.user_id) {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权查询不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = parse_public_uuid(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    let Some(lots) = state.lots() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        )
        .into_response();
    };
    match lots.public_lot(lot_uuid).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_error().into_response(),
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_account",
                "read_proxy_lot",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "公开 lot 目录暂不可用",
            )
            .into_response();
        }
    }
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let authorization = match reader.public_user_proxy(lot_uuid, identity.user_id).await {
        Ok(authorization) => authorization,
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_account",
                "read_proxy_authorization",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    if authorization
        .as_ref()
        .is_some_and(|proxy| proxy.lot_id != lot_uuid || proxy.user_id != identity.user_id)
    {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    }
    let data = PublicMyProxyResponse { authorization: authorization.map(map_owned_proxy) };
    let message = if data.authorization.is_some() {
        "代理授权已加载"
    } else {
        "当前批次没有代理授权"
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok(message, data))).into_response()
}

/// 账户页只能包含请求者自己的代理，条数不超过请求上限与端口报告的总数。
fn valid_account_page(
    page: &PublicMyBidsPage,
    user_id: Uuid,
    request: &PublicMyBidsRequest,
) -> bool {
    let Ok(bid_count) = i64::try_from(page.bids.len()) else {
        return false;
    };
    let Ok(proxy_count) = i64::try_from(page.proxies.len()) else {
        return false;
    };
    page.bid_total >= 0
        && page.proxy_total >= 0
        && bid_count <= request.bids.limit
        && proxy_count <= request.proxies.limit
        && bid_count <= page.bid_total
        && proxy_count <= page.proxy_total
        && page.proxies.iter().all(|proxy| proxy.user_id == user_id)
}

fn map_owned_bid(bid: PublicMyBid) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: i64::from(bid.fact_index),
        mechanism: bid.mechanism,
        created_at: public_timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: bid.equipment_name,
            category: bid.equipment_category,
        },
    }
}

fn map_owned_proxy(proxy: PublicOwnedProxy) -> PublicOwnedProxyResponse {
    // wire 在嵌套 bid 中重复装备摘要；只复制该副本，顶层摘要直接接管原字符串。
    let bid =
        proxy.bid.map(|bid| map_proxy_bid(bid, &proxy.equipment_name, &proxy.equipment_category));
    let equipment = PublicOwnedEquipmentResponse {
        name: proxy.equipment_name,
        category: proxy.equipment_category,
    };
    PublicOwnedProxyResponse {
        authorization_id: proxy.authorization_id.to_string(),
        auction_id: proxy.auction_id.to_string(),
        lot_id: proxy.lot_id.to_string(),
        lot_number: proxy.lot_number,
        authorization_sequence: proxy.authorization_sequence,
        usage_number: proxy.usage_number,
        mode: proxy.mode,
        maximum_amount: proxy.maximum_amount,
        quota: PublicOwnedProxyQuotaResponse { used: proxy.quota_used, limit: proxy.quota_limit },
        transitions: proxy.transitions.into_iter().map(map_proxy_transition).collect(),
        status: proxy.status,
        terminal_reason: proxy.terminal_reason,
        state_version: proxy.state_version,
        created_at: public_timestamp(proxy.created_at),
        updated_at: public_timestamp(proxy.updated_at),
        equipment,
        bid,
        invalid_reason: proxy.invalid_reason,
        capabilities: PublicOwnedProxyCapabilitiesResponse {
            can_lower: proxy.capabilities.can_lower,
            waiting: proxy.capabilities.waiting,
            at_maximum: proxy.capabilities.at_maximum,
        },
    }
}

fn map_proxy_bid(
    bid: PublicProxyBid,
    equipment_name: &str,
    equipment_category: &str,
) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: i64::from(bid.fact_index),
        mechanism: bid.mechanism,
        created_at: public_timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: equipment_name.to_owned(),
            category: equipment_category.to_owned(),
        },
    }
}

fn map_proxy_transition(transition: PublicProxyTransition) -> PublicOwnedProxyTransitionResponse {
    PublicOwnedProxyTransitionResponse {
        transition_number: transition.transition_number,
        kind: transition.kind,
        old_maximum_amount: transition.old_maximum_amount,
        new_maximum_amount: transition.new_maximum_amount,
        old_mode: transition.old_mode,
        new_mode: transition.new_mode,
        old_status: transition.old_status,
        new_status: transition.new_status,
        reason: transition.reason,
        resulting_state_version: transition.resulting_state_version,
        created_at: public_timestamp(transition.created_at),
    }
}

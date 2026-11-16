//! Admin 交付列表的处理器。
//!
//! 缺省列表是已发布页面能按闭集解码的投影，不列出结算退件 Take 投递；`include=settlement_returns`
//! 才列出，规则见 `projection` 模块。

use std::net::SocketAddr;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::delivery::DeliveryAdminListRequest;
use auction_contracts::{AdminDeliveryListResponse, AdminDeliveryPaginationResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::projection::DeliveryProjection;
use super::{
    feature_unavailable, map_delivery_error, map_snapshot, parse_optional_filter,
    parse_optional_reference, parse_optional_timestamp,
};
use crate::diagnostics;
use crate::errors::query_rejection;
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 解析精确筛选条件；任一非法值都让整个请求失败关闭，不做部分忽略。
fn delivery_filters(
    query: &std::collections::BTreeMap<&str, String>,
) -> Result<DeliveryAdminListRequest, ()> {
    Ok(DeliveryAdminListRequest {
        recipient: parse_optional_filter(
            query,
            "recipient",
            auction_application::delivery::MAX_DELIVERY_ADMIN_RECIPIENT_LEN,
        )?,
        reference: parse_optional_reference(query, "reference")?,
        template: parse_optional_filter(
            query,
            "template",
            auction_application::delivery::MAX_DELIVERY_ADMIN_TEMPLATE_LEN,
        )?,
        category: query.get("category").cloned().filter(|value| !value.is_empty()),
        world: query.get("world").cloned().filter(|value| !value.is_empty()),
        created_from: parse_optional_timestamp(query, "created_from")?,
        created_to: parse_optional_timestamp(query, "created_to")?,
        ..DeliveryAdminListRequest::default()
    })
}

/// 解析列表查询，返回请求页码与应用层请求。筛选、分页或其他键不合法为 400 `invalid_query`
/// （查询参数无效）；`include` 的取值由 [`DeliveryProjection::parse`] 判定（附加字段无效），任一
/// 非法值都让整个请求失败关闭，不静默退回缺省投影。
fn list_request(uri: &Uri) -> Result<(i64, DeliveryAdminListRequest), Box<Response>> {
    let rejection = || Box::new(query_rejection().into_response());
    let query = parse_query(
        uri,
        &[
            "page",
            "page_size",
            "status",
            "recipient",
            "reference",
            "template",
            "category",
            "world",
            "created_from",
            "created_to",
            "include",
        ],
    )
    .map_err(|()| rejection())?;
    let projection = DeliveryProjection::parse(query.get("include").map(String::as_str))?;
    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| rejection())?;
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(auction_application::delivery::MAX_DELIVERY_ADMIN_PAGE_SIZE),
    )
    .map_err(|()| rejection())?;
    let offset = page_offset(page, page_size, None).ok_or_else(rejection)?;
    let filters = delivery_filters(&query).map_err(|()| rejection())?;
    let request = DeliveryAdminListRequest {
        status: query.get("status").cloned(),
        include_settlement_return_actions: projection.includes_settlement_return_actions(),
        limit: page_size,
        offset,
        ..filters
    };
    if !request.validate() {
        return Err(rejection());
    }
    Ok((page, request))
}

/// 读取 delivery Admin 列表。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let (page, request) = match list_request(&uri) {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    let page_size = request.limit;
    let offset = request.offset;
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let page_result = match port.list(request).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    let entry_count = match i64::try_from(page_result.entries.len()) {
        Ok(count) => count,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_delivery",
                "convert_entry_count",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || page_result.total < 0
        || entry_count > page_size
    {
        return internal_error();
    }
    let deliveries = page_result.entries.into_iter().map(map_snapshot).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "交付列表已加载",
            AdminDeliveryListResponse {
                deliveries,
                pagination: AdminDeliveryPaginationResponse {
                    page,
                    page_size,
                    total: page_result.total,
                },
            },
        )),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::list_request;

    async fn parse(query: &str) -> Result<(i64, bool), (StatusCode, Value)> {
        let uri: Uri = format!("/api/v1/admin/deliveries{query}").parse().expect("测试 URI");
        match list_request(&uri) {
            Ok((page, request)) => Ok((page, request.include_settlement_return_actions)),
            Err(response) => {
                let status = response.status();
                let bytes =
                    to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
                Err((status, serde_json::from_slice(&bytes).expect("错误响应是 JSON")))
            }
        }
    }

    /// 缺省列表不列出结算退件 Take；只有精确的 `include=settlement_returns` 列出它们，并可与筛选、
    /// 分页同用。其他取值为“附加字段无效”，重复出现与未登记的键为“查询参数无效”，都是 400
    /// `invalid_query`，不静默退回缺省投影。
    #[tokio::test]
    async fn include_is_opt_in_and_fails_closed() {
        assert_eq!(parse("").await, Ok((1, false)));
        assert_eq!(parse("?page=2&page_size=20&status=failed").await, Ok((2, false)));
        assert_eq!(parse("?include=settlement_returns").await, Ok((1, true)));
        assert_eq!(
            parse("?page=3&page_size=50&world=isekai&include=settlement_returns").await,
            Ok((3, true))
        );
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=SETTLEMENT_RETURNS", "附加字段无效"),
            ("?include=recovery_equipment_recovered", "附加字段无效"),
            ("?include=settlement_returns,collection_wait", "附加字段无效"),
            ("?include=settlement_returns&include=settlement_returns", "查询参数无效"),
            ("?origin_kind=settlement_return_action", "查询参数无效"),
            ("?include=settlement_returns&status=unknown", "查询参数无效"),
        ] {
            let (status, body) = parse(query).await.expect_err(query);
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }
}

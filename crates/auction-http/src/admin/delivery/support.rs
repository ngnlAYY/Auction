//! 玩家支持只读汇总的 HTTP 适配器。
//!
//! 只接受 `user`（用户名或用户 UUID）与 `reference`（规范 UUID）之一，二者都给或都给空都
//! 失败关闭；不提供写动作，也不把投递 payload、正文或凭据带出安全投影。投递分区与列表、详情
//! 同一投影规则：缺省不含结算退件 Take，`include=settlement_returns` 才包含，见 `projection` 模块。

use std::net::SocketAddr;

use auction_application::delivery::{PlayerSupportRequest, PlayerSupportSelector};
use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::projection::DeliveryProjection;
use super::{
    feature_unavailable, map_delivery_error, map_player_support, parse_optional_filter,
    parse_optional_reference,
};
use crate::errors::query_rejection;
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

/// 解析汇总查询。检索入口不合法、重复或未登记的键为 400 `invalid_query`（查询参数无效）；
/// `include` 的取值由 [`DeliveryProjection::parse`] 判定（附加字段无效），不静默退回缺省投影。
fn support_request(uri: &Uri) -> Result<PlayerSupportRequest, Box<Response>> {
    let rejection = || Box::new(query_rejection().into_response());
    let query = parse_query(uri, &["user", "reference", "include"]).map_err(|()| rejection())?;
    let projection = DeliveryProjection::parse(query.get("include").map(String::as_str))?;
    let user = parse_optional_filter(
        &query,
        "user",
        auction_application::delivery::MAX_PLAYER_SUPPORT_USER_LEN,
    )
    .map_err(|()| rejection())?;
    let reference = parse_optional_reference(&query, "reference").map_err(|()| rejection())?;
    // 两个入口互斥：一次查询只回答一种身份/参考，避免把无关记录混进同一份汇总。
    let selector = match (user, reference) {
        (Some(user), None) => PlayerSupportSelector::User(user),
        (None, Some(reference)) => PlayerSupportSelector::Reference(reference),
        _ => return Err(rejection()),
    };
    let request = PlayerSupportRequest {
        selector,
        include_settlement_return_actions: projection.includes_settlement_return_actions(),
    };
    if !request.validate() {
        return Err(rejection());
    }
    Ok(request)
}

/// 读取玩家支持汇总；响应固定不分页，超过分区上限时由 `truncated` 提示。
pub(crate) async fn player_support(
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
    let request = match support_request(&uri) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let bundle = match port.player_support(request).await {
        Ok(bundle) => bundle,
        Err(error) => return map_delivery_error(error),
    };
    let limit = auction_application::delivery::MAX_PLAYER_SUPPORT_ROWS;
    let count = |length: usize| i64::try_from(length).ok();
    let within_limit = [
        count(bundle.bids.len()),
        count(bundle.deals.len()),
        count(bundle.obligations.len()),
        count(bundle.deliveries.len()),
        count(bundle.notifications.len()),
    ]
    .into_iter()
    .all(|length| length.is_some_and(|length| length >= 0 && length <= limit));
    if !within_limit {
        return internal_error();
    }
    let data = map_player_support(bundle);
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("玩家支持汇总已加载", data)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use auction_application::delivery::PlayerSupportSelector;
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use axum::response::Response;
    use serde_json::Value;
    use uuid::Uuid;

    use super::support_request;

    const REFERENCE: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn uri(query: &str) -> Uri {
        format!("/api/v1/admin/player-support?{query}").parse().expect("合法 URI")
    }

    async fn rejection(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
        (status, serde_json::from_slice(&bytes).expect("错误响应是 JSON"))
    }

    /// 缺省请求不含结算退件 Take；只有精确的 `include=settlement_returns` 才包含，且两种检索入口
    /// 都按同一投影解析。
    #[test]
    fn include_is_opt_in_for_both_selectors() {
        let reference = Uuid::parse_str(REFERENCE).expect("规范 UUID");
        for (query, selector, include) in [
            ("user=Sample", PlayerSupportSelector::User("Sample".to_owned()), false),
            (
                "user=Sample&include=settlement_returns",
                PlayerSupportSelector::User("Sample".to_owned()),
                true,
            ),
            (
                &format!("reference={REFERENCE}") as &str,
                PlayerSupportSelector::Reference(reference),
                false,
            ),
            (
                &format!("include=settlement_returns&reference={REFERENCE}"),
                PlayerSupportSelector::Reference(reference),
                true,
            ),
        ] {
            let request = support_request(&uri(query)).expect(query);
            assert_eq!(request.selector, selector, "{query}");
            assert_eq!(request.include_settlement_return_actions, include, "{query}");
        }
    }

    /// `include` 的其他取值为 400 `invalid_query`（附加字段无效）；重复的 `include`、未登记的键与
    /// 不合法的检索入口为 400 `invalid_query`（查询参数无效）。都不静默退回缺省投影。
    #[tokio::test]
    async fn invalid_include_and_selectors_fail_closed() {
        for include in ["", "SETTLEMENT_RETURNS", "settlement_return_action", "a,b"] {
            let query = format!("user=Sample&include={include}");
            let response = support_request(&uri(&query)).expect_err(&query);
            let (status, body) = rejection(*response).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], "附加字段无效", "{query}");
        }
        for query in [
            "user=Sample&include=settlement_returns&include=settlement_returns",
            "user=Sample&fields=summary",
            &format!("user=Sample&reference={REFERENCE}&include=settlement_returns"),
            "include=settlement_returns",
        ] {
            let response = support_request(&uri(query)).expect_err(query);
            let (status, body) = rejection(*response).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], "查询参数无效", "{query}");
        }
    }
}

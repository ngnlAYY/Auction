//! Admin 投递详情读取：聚合尝试、Sent 证据、领取、处置与来源义务的安全投影。
//!
//! 缺省响应是已发布页面能按闭集解码的投影；结算退件 Take 投递与本版本新增的处置种类只在
//! 扩展投影包含，规则见 `projection` 模块。实见附件只随 `include=sent_proof` 输出。

use std::net::SocketAddr;

use auction_contracts::{AdminDeliveryDetailWithActionsResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::mapping::map_actions;
use super::projection::DeliveryProjection;
use super::{feature_unavailable, map_delivery_error, map_detail};
use crate::errors::{api_error, invalid_query};
use crate::query::parse_query;
use crate::session::{admin_session_guard, authenticated_admin_session, no_store_headers};
use crate::state::AdminApiState;

/// 读取单条 delivery 的完整证据详情；不返回 payload、正文或凭据秘密。
pub(crate) async fn detail(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let (projection, include_sent_proof) = match detail_projection(&uri) {
        Ok(projection) => projection,
        Err(response) => return *response,
    };
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    if projection == DeliveryProjection::DispositionActions {
        let (detail, guard, acknowledge) = match port.detail_with_guard(delivery_id).await {
            Ok(projected) => projected,
            Err(error) => return map_delivery_error(error),
        };
        let detail = match projection.project_detail(map_detail(detail, include_sent_proof)) {
            Ok(detail) => detail,
            Err(response) => return *response,
        };
        let response = AdminDeliveryDetailWithActionsResponse {
            detail,
            actions: map_actions(&guard, acknowledge),
        };
        return (
            StatusCode::OK,
            no_store_headers(),
            Json(ApiResponse::ok("交付详情已加载", response)),
        )
            .into_response();
    }
    let detail = match port.detail(delivery_id).await {
        Ok(detail) => detail,
        Err(error) => return map_delivery_error(error),
    };
    let response = match projection.project_detail(map_detail(detail, false)) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("交付详情已加载", response)))
        .into_response()
}

/// 详情的查询参数只有可选的 `include`。未登记的键或重复出现为“投递详情查询参数无效”，`include`
/// 可选择处置准入或附加 Sent 证据字段。其余取值按 [`DeliveryProjection::parse`] 判定。
fn detail_projection(uri: &Uri) -> Result<(DeliveryProjection, bool), Box<Response>> {
    let mut query = parse_query(uri, &["include"])
        .map_err(|()| Box::new(invalid_query("投递详情查询参数无效")))?;
    match query.remove("include").as_deref() {
        Some("disposition_actions") => Ok((DeliveryProjection::DispositionActions, false)),
        Some("sent_proof") => Ok((DeliveryProjection::DispositionActions, true)),
        include => DeliveryProjection::parse(include).map(|projection| (projection, false)),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::detail_projection;
    use crate::admin::delivery::projection::DeliveryProjection;

    async fn parse(query: &str) -> Result<(DeliveryProjection, bool), (StatusCode, Value)> {
        let uri: Uri = format!("/api/v1/admin/deliveries/x{query}").parse().expect("测试 URI");
        match detail_projection(&uri) {
            Ok(projection) => Ok(projection),
            Err(response) => {
                let status = response.status();
                let bytes =
                    to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
                Err((status, serde_json::from_slice(&bytes).expect("错误响应是 JSON")))
            }
        }
    }

    /// 缺省投影不带参数；扩展投影只接受精确的单项取值。旧的单项取值
    /// `recovery_equipment_recovered`、其他取值与未登记的键都以 400 `invalid_query` 拒绝，不静默
    /// 退回缺省投影。
    #[tokio::test]
    async fn include_is_opt_in_and_fails_closed() {
        assert_eq!(parse("").await, Ok((DeliveryProjection::Published, false)));
        assert_eq!(parse("?").await, Ok((DeliveryProjection::Published, false)));
        assert_eq!(
            parse("?include=settlement_returns").await,
            Ok((DeliveryProjection::SettlementReturns, false))
        );
        assert_eq!(
            parse("?include=disposition_actions").await,
            Ok((DeliveryProjection::DispositionActions, false))
        );
        assert_eq!(
            parse("?include=sent_proof").await,
            Ok((DeliveryProjection::DispositionActions, true))
        );
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=SETTLEMENT_RETURNS", "附加字段无效"),
            ("?include=recovery_equipment_recovered", "附加字段无效"),
            ("?include=settlement_returns,recovery_equipment_recovered", "附加字段无效"),
            ("?include=sent_proof,disposition_actions", "附加字段无效"),
            ("?include=settlement_returns&include=settlement_returns", "投递详情查询参数无效"),
            ("?fields=all", "投递详情查询参数无效"),
        ] {
            let (status, body) = parse(query).await.expect_err(query);
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }
}

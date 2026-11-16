//! Admin 投递详情读取：聚合尝试、Sent 证据、领取、处置与来源义务的安全投影。
//!
//! 缺省响应是已发布页面能按闭集解码的投影；结算退件 Take 投递与本版本新增的处置种类只在
//! `include=settlement_returns` 时输出，规则见 `projection` 模块。

use std::net::SocketAddr;

use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

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
    let projection = match detail_projection(&uri) {
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
    let detail = match port.detail(delivery_id).await {
        Ok(detail) => detail,
        Err(error) => return map_delivery_error(error),
    };
    let response = match projection.project_detail(map_detail(detail)) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("交付详情已加载", response)))
        .into_response()
}

/// 详情的查询参数只有可选的 `include`。未登记的键或重复出现为“投递详情查询参数无效”，`include`
/// 的取值由 [`DeliveryProjection::parse`] 判定，都不静默退回缺省投影。
fn detail_projection(uri: &Uri) -> Result<DeliveryProjection, Box<Response>> {
    let mut query = parse_query(uri, &["include"])
        .map_err(|()| Box::new(invalid_query("投递详情查询参数无效")))?;
    DeliveryProjection::parse(query.remove("include").as_deref())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::detail_projection;
    use crate::admin::delivery::projection::DeliveryProjection;

    async fn parse(query: &str) -> Result<DeliveryProjection, (StatusCode, Value)> {
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

    /// 缺省投影不带参数；只有精确的 `include=settlement_returns` 是扩展投影。旧的单项取值
    /// `recovery_equipment_recovered`、其他取值与未登记的键都以 400 `invalid_query` 拒绝，不静默
    /// 退回缺省投影。
    #[tokio::test]
    async fn include_is_opt_in_and_fails_closed() {
        assert_eq!(parse("").await, Ok(DeliveryProjection::Published));
        assert_eq!(parse("?").await, Ok(DeliveryProjection::Published));
        assert_eq!(
            parse("?include=settlement_returns").await,
            Ok(DeliveryProjection::SettlementReturns)
        );
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=SETTLEMENT_RETURNS", "附加字段无效"),
            ("?include=recovery_equipment_recovered", "附加字段无效"),
            ("?include=settlement_returns,recovery_equipment_recovered", "附加字段无效"),
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

//! Public 注册说明的非敏感配置投影；不访问数据库或游戏服务。

use auction_contracts::{ApiErrorResponse, ApiResponse};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{PublicApiState, api_error, parse_query};

/// 匿名读取启动时注入的 bot 显示名；空名单不代表配置读取失败。
pub(crate) async fn get(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    parse_query(&uri, &[])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let info = state.registration_info().ok_or_else(|| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "configuration_unavailable",
            "注册说明配置暂不可用",
        )
    })?;
    // 在持有 state 时直接序列化借用的安全 DTO，避免每个匿名请求复制全部账号名。
    Ok(Json(ApiResponse::ok("注册说明已加载", info)).into_response())
}

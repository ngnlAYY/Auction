//! 公开 world 列表的处理器。

use auction_application::auction::WorldSnapshot;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{ApiErrorResponse, ApiResponse, WorldStatusResponse, WorldsResponse};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri, header};

use crate::diagnostics;
use crate::mapping::api_error;
use crate::middleware::response_cache::LOW_FREQUENCY_CACHE_CONTROL;
use crate::state::PublicApiState;

/// 世界目录只有各世界的启用标记，只在管理员开关世界时变化：使用低频档并允许边缘保存。
/// 停用一个世界最多延迟一个缓存周期才从选择器里消失；读取与出价的准入仍由各自的服务端
/// 门禁即时判定，不依赖这份目录。
pub(crate) async fn list_worlds(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<
    ([(HeaderName, HeaderValue); 1], Json<ApiResponse<WorldsResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "世界目录不接受查询参数"));
    }

    let Some(reader) = state.worlds() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "世界目录暂不可用",
        ));
    };
    let worlds = reader.list_worlds().await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_worlds",
            "read_worlds",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );

        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "世界目录暂不可用")
    })?;
    let worlds = worlds.into_iter().map(map_world).collect();
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL))],
        Json(ApiResponse::ok("世界状态已加载", WorldsResponse { worlds })),
    ))
}

pub(crate) fn map_world(world: WorldSnapshot) -> (String, WorldStatusResponse) {
    (world.id, WorldStatusResponse { enabled: world.enabled })
}

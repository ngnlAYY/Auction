//! 图片读取与安全头：只接收公开拍卖身份及闭集计时器，不接受自定义正文或任意时间。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use super::{
    super::{PublicApiState, api_error, not_found_error, parse_public_uuid, parse_query},
    svg::{TimerKind, render},
};
use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

/// 只由成功的图片 handler 签发，避免全站或错误响应继承图片专用安全策略。
#[derive(Clone, Copy)]
struct CountdownImage;

/// GET/HEAD 动态 SVG；每次只读同一场的公开时间快照，不访问论坛或第三方。
pub(crate) async fn get(
    State(state): State<PublicApiState>,
    Path((auction_id, timer)): Path<(String, String)>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    parse_query(&uri, &[])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let kind = TimerKind::from_file(&timer).ok_or_else(not_found_error)?;
    let reader = state.auctions().ok_or_else(unavailable)?;
    let snapshot = reader
        .public_auction_countdown(auction_id)
        .await
        .map_err(|diagnostic_cause| {
            diagnostics::warn(
                "public_countdown",
                "read_snapshot",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?
        .ok_or_else(not_found_error)?;
    if snapshot.auction_id != auction_id || snapshot.validate().is_err() {
        return Err(unavailable());
    }
    let body = render(kind, &snapshot).map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_countdown",
            "render_countdown",
            diagnostics::invalid_state(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        unavailable()
    })?;
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml; charset=utf-8"));
    // 缓存策略由路由组的 short_lived 中间件签发；倒计时随截止时间变化，只允许秒级复用。
    response.extensions_mut().insert(CountdownImage);
    Ok(response)
}

/// 通用边界写完默认头后，仅对已签发图片收紧 CSP；允许论坛作为跨源图片加载。
pub(crate) fn apply_security_headers(response: &mut Response) {
    if response.extensions().get::<CountdownImage>().is_none() {
        return;
    }
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
    ));
    headers.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
}

fn unavailable() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "拍卖倒计时暂不可用")
}

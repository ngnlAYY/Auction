//! 图片读取与安全头：只接收公开拍卖身份及闭集计时器，不接受自定义正文或任意时间。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;
use crate::middleware::response_cache::LOW_FREQUENCY_CACHE_CONTROL;

use super::{
    super::{PublicApiState, api_error, not_found_error, parse_public_uuid, parse_query},
    svg::{TimerKind, render},
};
use auction_application::auction::PublicAuctionCountdown;
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
    let finished = timer_finished(kind, &snapshot);
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
    // 仍在倒数（或尚未排期）的图片由路由组 short_lived 签发秒级缓存；计时目标已经过去的图片
    // 固定显示结束态，旧论坛帖仍会反复加载，改用低频档让浏览器与 CDN 复用。截止时间若被
    // 事后改动，最迟在低频档的再验证窗口内更新。
    if finished {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL));
    }
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

/// 计时目标已经过去：图片固定显示结束态。尚未排期的拍卖计时器不算结束。
fn timer_finished(kind: TimerKind, snapshot: &PublicAuctionCountdown) -> bool {
    kind.deadline(snapshot).is_some_and(|deadline| deadline <= snapshot.observed_at)
}

fn unavailable() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "拍卖倒计时暂不可用")
}

#[cfg(test)]
mod tests {
    use auction_application::auction::PublicAuctionCountdown;
    use chrono::{Duration, TimeZone, Utc};
    use uuid::Uuid;

    use super::{TimerKind, timer_finished};

    fn snapshot(scheduled_end_in: Option<i64>, intake_in: i64) -> PublicAuctionCountdown {
        let observed_at = Utc.timestamp_opt(1_800_000_000, 0).single().expect("valid time");
        PublicAuctionCountdown {
            auction_id: Uuid::from_u128(7),
            intake_closes_at: observed_at + Duration::seconds(intake_in),
            scheduled_end_at: scheduled_end_in
                .map(|seconds| observed_at + Duration::seconds(seconds)),
            observed_at,
        }
    }

    #[test]
    fn only_timers_past_their_deadline_get_the_long_cache() {
        // 进行中：清单计时器已截止，拍卖计时器仍在倒数。
        let live = snapshot(Some(3_600), -60);
        assert!(timer_finished(TimerKind::List, &live));
        assert!(!timer_finished(TimerKind::Auction, &live));
        // 截止时刻本身即结束态。
        assert!(timer_finished(TimerKind::Auction, &snapshot(Some(0), -60)));
        // 预览期尚未排期的拍卖计时器随时可能排期，保持秒级缓存。
        let preview = snapshot(None, 600);
        assert!(!timer_finished(TimerKind::Auction, &preview));
        assert!(!timer_finished(TimerKind::List, &preview));
    }
}

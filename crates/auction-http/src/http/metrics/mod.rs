//! 业务 listener 与三角色附加出口共用的 Prometheus 契约。
//! 该路由只读取装配根注入的 `ProcessMetrics`，不读取业务 state、数据库或敏感配置；未启用
//! 指标、带 query 的路径和未注册的变体都 fail closed 为 404。转发请求的访问收紧由全局
//! boundary 在 request-id 与方法错误之前执行；本模块持有只检查头存在性的判定。

use auction_observability::{METRICS_ROUTE, MetricsError, ProcessMetrics};
use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

use super::state::{AdminApiState, PublicApiState};

mod egress;

pub use egress::serve_metrics;

/// Public `/metrics` scrape handler。
pub(crate) async fn public(
    State(state): State<PublicApiState>,
    method: Method,
    uri: Uri,
) -> Response {
    render(state.metrics(), method, uri)
}

/// Admin `/metrics` scrape handler。
pub(crate) async fn admin(
    State(state): State<AdminApiState>,
    method: Method,
    uri: Uri,
) -> Response {
    render(state.metrics(), method, uri)
}

/// 任意来源的转发痕迹都拒绝；空值、重复值和非法值也属于存在，绝不解析或记录值。
pub(crate) fn has_forwarding_headers(headers: &HeaderMap) -> bool {
    // CDN 专用来源头待 W0.12 核实实际代理链后再补充；不能据此宣称能识别全部代理。
    headers.keys().any(|name| {
        matches!(name.as_str(), "forwarded" | "x-real-ip" | "via")
            || name.as_str().starts_with("x-forwarded-")
    })
}

fn render(metrics: Option<&ProcessMetrics>, method: Method, uri: Uri) -> Response {
    // any 路由不自动追加 Allow；转发拒绝保持空 404，直连沿用原 GET/HEAD 和 405 契约。
    if !matches!(method, Method::GET | Method::HEAD) {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "GET,HEAD")]).into_response();
    }
    if uri.path() != METRICS_ROUTE || uri.query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(metrics) = metrics else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match metrics.encode() {
        Ok(body) => {
            ([(header::CONTENT_TYPE, ProcessMetrics::content_type())], body).into_response()
        }
        Err(MetricsError::Disabled) => StatusCode::NOT_FOUND.into_response(),
        Err(MetricsError::Registry(_) | MetricsError::Encode(_)) => {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

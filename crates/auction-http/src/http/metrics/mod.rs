//! Public/Admin 进程本地 Prometheus scrape 路由。
//!
//! 该路由只读取装配根注入的 `ProcessMetrics`，不读取业务 state、数据库或敏感配置；未启用
//! 指标、带 query 的路径和未注册的变体都 fail closed 为 404。

use auction_observability::{METRICS_ROUTE, MetricsError, ProcessMetrics};
use axum::{
    extract::State,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

use super::state::{AdminApiState, PublicApiState};

/// Public `/metrics` scrape handler。
pub(crate) async fn public(State(state): State<PublicApiState>, uri: Uri) -> Response {
    render(state.metrics(), uri)
}

/// Admin `/metrics` scrape handler。
pub(crate) async fn admin(State(state): State<AdminApiState>, uri: Uri) -> Response {
    render(state.metrics(), uri)
}

fn render(metrics: Option<&ProcessMetrics>, uri: Uri) -> Response {
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

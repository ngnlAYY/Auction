//! Public 与 Admin 的 HTTP 表面适配器。
//!
//! 这里只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；竞价、结算和状态机规则
//! 留在领域与应用层。Public 与 Admin 是不同 surface，各自拥有独立的路由与前端产物。

mod web_assets;

use auction_contracts::HealthResponse;
use axum::{Json, Router, routing::get};

/// 公开端路由。
///
/// 健康检查路径沿用已冻结的旧契约；其余路径交给内嵌的公开端 SPA 产物处理。
///
/// `version` 由装配根传入自身的 `CARGO_PKG_VERSION`：契约 crate 内展开该宏只会得到契约
/// 自己的版本，而健康检查要报告的是运行进程的版本。
pub fn public_router<S>(version: &'static str) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let health = move || health_response("public-api", version);

    Router::<S>::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .fallback(web_assets::serve_public_asset)
}

/// 管理端路由。
///
/// 管理端产物与公开端完全隔离，公开端二进制不包含管理端资源。
pub fn admin_router<S>(version: &'static str) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let health = move || health_response("admin-api", version);

    Router::<S>::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .fallback(web_assets::serve_admin_asset)
}

async fn health_response(service: &'static str, version: &'static str) -> Json<HealthResponse> {
    Json(HealthResponse::ok(service, version))
}

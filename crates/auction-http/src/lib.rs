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
pub fn public_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::<S>::new()
        .route("/health", get(public_health))
        .route("/api/v1/health", get(public_health))
        .fallback(web_assets::serve_public_asset)
}

/// 管理端路由。
///
/// 管理端产物与公开端完全隔离，公开端二进制不包含管理端资源。
pub fn admin_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::<S>::new()
        .route("/health", get(admin_health))
        .route("/api/v1/health", get(admin_health))
        .fallback(web_assets::serve_admin_asset)
}

async fn public_health() -> Json<HealthResponse> {
    Json(HealthResponse::ok("public-api"))
}

async fn admin_health() -> Json<HealthResponse> {
    Json(HealthResponse::ok("admin-api"))
}

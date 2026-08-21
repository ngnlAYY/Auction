use auction_contracts::HealthResponse;
use axum::{Json, Router, routing::get};

pub fn public_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::<S>::new()
        .route("/health", get(public_health))
        .route("/api/v1/health", get(public_health))
}

pub fn admin_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::<S>::new()
        .route("/health", get(admin_health))
        .route("/api/v1/health", get(admin_health))
}

async fn public_health() -> Json<HealthResponse> {
    Json(HealthResponse::ok("public-api"))
}

async fn admin_health() -> Json<HealthResponse> {
    Json(HealthResponse::ok("admin-api"))
}

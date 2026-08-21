use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub service: String,
    pub status: &'static str,
    pub version: &'static str,
}

impl HealthResponse {
    pub fn ok(service: impl Into<String>) -> Self {
        Self { service: service.into(), status: "ok", version: env!("CARGO_PKG_VERSION") }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorResponse {
    pub code: &'static str,
    pub message: String,
    pub request_id: Option<String>,
}

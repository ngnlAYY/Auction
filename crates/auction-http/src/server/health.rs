//! Public/Admin 健康响应与依赖 readiness 探针。

use auction_contracts::{HealthLiveData, HealthProbeResponse, HealthReadyData, HealthResponse};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::state::{AdminApiState, PublicApiState};

pub(super) async fn health_response(
    service: &'static str,
    version: &'static str,
) -> Json<HealthResponse> {
    Json(HealthResponse::ok(service, version))
}

/// 返回不访问外部依赖的存活探针；两端共用，进程管理器据此判断是否需要重启。
pub(super) async fn health_live() -> Json<HealthProbeResponse<HealthLiveData>> {
    Json(HealthProbeResponse {
        code: "live".to_owned(),
        message: "进程存活".to_owned(),
        data: HealthLiveData { live: true },
    })
}

/// 返回装配根注入的最终判定（默认一秒缓存）；缺少端口时保持未就绪。
pub(super) async fn public_health_ready(State(state): State<PublicApiState>) -> Response {
    health_ready_response(probe_ready(state.readiness()).await)
}

/// 返回 Admin 最终判定；缓存只影响探测新鲜度，不改变 200/503 或 no-store 契约。
pub(super) async fn admin_health_ready(State(state): State<AdminApiState>) -> Response {
    health_ready_response(probe_ready(state.readiness()).await)
}

async fn probe_ready(
    readiness: Option<&dyn auction_application::operations::ProcessReadinessPort>,
) -> bool {
    match readiness {
        Some(readiness) => readiness.check().await.is_ok(),
        None => false,
    }
}

fn health_ready_response(ready: bool) -> Response {
    let (status, code, message) = if ready {
        (StatusCode::OK, "ready", "进程已就绪")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready", "进程尚未就绪")
    };
    (
        status,
        Json(HealthProbeResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: HealthReadyData { ready },
        }),
    )
        .into_response()
}

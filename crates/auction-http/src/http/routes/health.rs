//! Public/Admin 健康响应与依赖 readiness 探针。

use super::*;

pub(super) async fn health_response(
    service: &'static str,
    version: &'static str,
) -> Json<HealthResponse> {
    Json(HealthResponse::ok(service, version))
}

/// 返回不访问外部依赖的存活探针；该路径用于进程管理器判断是否需要重启。
pub(super) async fn public_health_live() -> Json<HealthProbeResponse<HealthLiveData>> {
    Json(HealthProbeResponse {
        code: "live".to_owned(),
        message: "进程存活".to_owned(),
        data: HealthLiveData { live: true },
    })
}

/// 返回 Public 的依赖 readiness；缺少数据库端口时保持未就绪而不伪造成功。
pub(super) async fn public_health_ready(State(state): State<PublicApiState>) -> Response {
    let ready = match state.readiness() {
        Some(readiness) => readiness.check().await.is_ok(),
        None => false,
    };
    health_ready_response(ready)
}

/// 返回不访问外部依赖的 Admin 存活探针。
pub(super) async fn admin_health_live() -> Json<HealthProbeResponse<HealthLiveData>> {
    Json(HealthProbeResponse {
        code: "live".to_owned(),
        message: "进程存活".to_owned(),
        data: HealthLiveData { live: true },
    })
}

/// 返回 Admin 的依赖 readiness；数据库连接或迁移不完整时使用 503。
pub(super) async fn admin_health_ready(State(state): State<AdminApiState>) -> Response {
    let ready = match state.readiness() {
        Some(readiness) => readiness.check().await.is_ok(),
        None => false,
    };
    health_ready_response(ready)
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

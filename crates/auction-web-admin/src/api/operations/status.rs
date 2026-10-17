//! Admin 登录外壳 readiness HTTP 客户端。
//! 静态 Status 路由不发请求；登录外壳继续读取 `/health/ready`。其 503 是合法的未就绪结果，
//! 不能当成网络解析错误。

use auction_contracts::{HealthProbeResponse, HealthReadyData};

use super::{invalid_response_error, send_get};

/// 单个健康探针在管理端的有限状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminHealthProbeState {
    Healthy,
    NotReady,
    Unavailable,
}

/// 管理端全局 readiness 状态，对齐旧版登录壳层的可访问性提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminReadinessState {
    Checking,
    Ready,
    Degraded,
}

/// 读取管理端全局 readiness；网络错误交给页面映射为 degraded，不向浏览器暴露下游细节。
pub(crate) async fn fetch_admin_readiness() -> Result<bool, String> {
    match fetch_ready().await? {
        AdminHealthProbeState::Healthy => Ok(true),
        AdminHealthProbeState::NotReady | AdminHealthProbeState::Unavailable => Ok(false),
    }
}

async fn fetch_ready() -> Result<AdminHealthProbeState, String> {
    let response = send_get("/health/ready").await?;
    let payload: HealthProbeResponse<HealthReadyData> =
        response.json().await.map_err(invalid_response_error)?;
    if response.ok() && payload.data.ready {
        Ok(AdminHealthProbeState::Healthy)
    } else if !response.ok() && !payload.data.ready {
        Ok(AdminHealthProbeState::NotReady)
    } else {
        Ok(AdminHealthProbeState::Unavailable)
    }
}

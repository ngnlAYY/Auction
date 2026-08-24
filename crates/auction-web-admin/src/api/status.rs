//! Admin 服务状态 HTTP 客户端。
//!
//! 页面只读取已有的 `/health/live` 与 `/health/ready` 探针，不把业务 dashboard 或 Worker
//! control 的安全投影混入进程健康判断。`ready` 的 503 是合法的未就绪结果，不能当成网络解析错误。

use gloo_net::http::Request;
use serde::Deserialize;

/// 单个健康探针在管理端的有限状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminHealthProbeState {
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

/// Admin 状态页使用的两个进程探针结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdminHealthStatus {
    pub live: AdminHealthProbeState,
    pub ready: AdminHealthProbeState,
}

#[derive(Debug, Deserialize)]
struct HealthEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct LiveData {
    live: bool,
}

#[derive(Debug, Deserialize)]
struct ReadyData {
    ready: bool,
}

/// 读取 Admin 进程的存活与就绪探针。
pub(crate) async fn fetch_admin_health() -> Result<AdminHealthStatus, String> {
    let live = fetch_live().await?;
    let ready = fetch_ready().await?;
    Ok(AdminHealthStatus { live, ready })
}

/// 读取管理端全局 readiness；网络错误交给页面映射为 degraded，不向浏览器暴露下游细节。
pub(crate) async fn fetch_admin_readiness() -> Result<bool, String> {
    match fetch_ready().await? {
        AdminHealthProbeState::Healthy => Ok(true),
        AdminHealthProbeState::NotReady | AdminHealthProbeState::Unavailable => Ok(false),
    }
}

async fn fetch_live() -> Result<AdminHealthProbeState, String> {
    let response = Request::get("/health/live").send().await.map_err(|error| error.to_string())?;
    let payload: HealthEnvelope<LiveData> =
        response.json().await.map_err(|error| error.to_string())?;
    if response.ok() && payload.data.live {
        Ok(AdminHealthProbeState::Healthy)
    } else {
        Ok(AdminHealthProbeState::Unavailable)
    }
}

async fn fetch_ready() -> Result<AdminHealthProbeState, String> {
    let response = Request::get("/health/ready").send().await.map_err(|error| error.to_string())?;
    let payload: HealthEnvelope<ReadyData> =
        response.json().await.map_err(|error| error.to_string())?;
    if response.ok() && payload.data.ready {
        Ok(AdminHealthProbeState::Healthy)
    } else if !response.ok() && !payload.data.ready {
        Ok(AdminHealthProbeState::NotReady)
    } else {
        Ok(AdminHealthProbeState::Unavailable)
    }
}

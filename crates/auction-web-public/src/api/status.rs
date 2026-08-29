//! Public 外壳 readiness HTTP 客户端。
//!
//! 旧版页头只在挂载时读取一次 `/health/ready`；HTTP 未就绪和网络/解码失败均由外壳映射为
//! `unavailable`，这里不泄露后端错误正文，也不建立轮询状态。

use auction_contracts::{HealthProbeResponse, HealthReadyData};
use gloo_net::http::Request;

use super::send_request;

/// 读取 Public 进程 readiness，并保留旧客户端的 JSON Accept 头与 10 秒请求上限。
pub(crate) async fn fetch_public_readiness() -> Result<bool, String> {
    let response = send_request(|abort_signal| {
        Request::get("/health/ready")
            .header("Accept", "application/json")
            .abort_signal(abort_signal)
            .build()
            .map_err(|error| error.to_string())
    })
    .await?;
    let response_ok = response.ok();
    let payload: HealthProbeResponse<HealthReadyData> =
        response.json().await.map_err(|_| "invalid_response".to_owned())?;
    Ok(response_ok && payload.data.ready)
}

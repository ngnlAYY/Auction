//! Public 外壳 readiness HTTP 客户端。
//! 外壳只在启动门禁与页面恢复可见时读取 `/health/ready`；HTTP 未就绪和网络/解码失败均映射为
//! `unavailable`，这里不泄露后端错误正文，读取生命周期仍由页面 owner 管理。

use auction_contracts::{HealthProbeResponse, HealthReadyData};
use auction_web_shared::{BrowserErrorClass, report_browser_error};
use gloo_net::http::Request;

use super::{TransportError, send_request};

/// 读取 Public 进程 readiness，并保留旧客户端的 JSON Accept 头与 10 秒请求上限。
pub(crate) async fn fetch_public_readiness() -> Result<bool, String> {
    #[cfg(test)]
    if let Some(result) = crate::surface::PageReadFixture::response(()) {
        return result;
    }
    let response = send_request(|abort_signal| {
        Request::get("/health/ready")
            .header("Accept", "application/json")
            .abort_signal(abort_signal)
            .build()
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| error.code().to_owned())?;
    let response_ok = response.ok();
    let payload: HealthProbeResponse<HealthReadyData> = response.json().await.map_err(|_| {
        report_browser_error(BrowserErrorClass::DtoDecode);
        "invalid_response".to_owned()
    })?;
    Ok(response_ok && payload.data.ready)
}

#[cfg(test)]
mod tests {
    use auction_contracts::{HealthProbeResponse, HealthReadyData};

    #[test]
    fn readiness_decodes_backend_contract_fixtures() {
        for (body, code, ready) in [
            (
                include_str!("../../../auction-contracts/tests/fixtures/public/readiness.json"),
                "ready",
                true,
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/readiness-unavailable.json"
                ),
                "not_ready",
                false,
            ),
        ] {
            let response: HealthProbeResponse<HealthReadyData> =
                serde_json::from_str(body).expect("就绪探针");
            assert_eq!(response.code, code);
            assert_eq!(response.data.ready, ready);
        }
    }
}

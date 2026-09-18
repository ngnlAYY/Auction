//! 一次性本机初始化接口；不会复用常规 Admin 会话或 CSRF API。

use auction_contracts::{SetupErrorResponse, SetupRequest, SetupResponse};
use gloo_net::http::Request;

/// 令牌仅在本次请求 header 发送，成功响应只消费稳定阶段值。
pub(crate) async fn submit_first_run(
    token: &str,
    form: &SetupRequest,
    complete: bool,
) -> Result<(), String> {
    let path = if complete { "/api/v1/setup/complete" } else { "/api/v1/setup/validate" };
    let request = Request::post(path)
        .header("x-setup-token", token)
        .json(form)
        .map_err(|_| "invalid_input".to_owned())?;
    let response = request.send().await.map_err(|_| "network".to_owned())?;
    if !response.ok() {
        return Err(response
            .json::<SetupErrorResponse>()
            .await
            .map(|body| body.code)
            .unwrap_or_else(|_| "setup_unknown".to_owned()));
    }
    let body: SetupResponse = response.json().await.map_err(|_| "setup_unknown".to_owned())?;
    let expected = if complete { "completed" } else { "validated" };
    if body.status == expected { Ok(()) } else { Err("setup_unknown".to_owned()) }
}

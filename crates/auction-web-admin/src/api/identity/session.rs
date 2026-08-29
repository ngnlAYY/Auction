//! Admin 会话 HTTP 适配。
//!
//! 浏览器只持有服务端返回的 actor、CSRF token 和过期时间；Admin secret 只用于一次 Basic
//! Auth 请求，不进入组件之外的状态，也不写入日志或 DTO。

use auction_contracts::{AdminSessionResponse, ApiResponse};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gloo_net::http::Request;
use serde_json::Value;

use super::{invalid_response_error, response_error, response_failure, send_get, send_request};

/// 已认证 Admin 会话的浏览器状态；`authenticated = false` 不会构造该类型。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminSession {
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

/// 按旧版会话协议验证认证成功投影；匿名成功正文必须失败关闭，不能进入登录态分支。
pub(super) fn decode_admin_session(data: AdminSessionResponse) -> Result<AdminSession, String> {
    if !data.authenticated
        || data.actor.is_empty()
        || data.csrf_token.is_empty()
        || !valid_session_time(&data.expires_at)
        || !valid_session_time(&data.absolute_expires_at)
    {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminSession {
        actor: data.actor,
        csrf_token: data.csrf_token,
        expires_at: data.expires_at,
        absolute_expires_at: data.absolute_expires_at,
    })
}

fn valid_session_time(value: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        return js_sys::Date::parse(value).is_finite();
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::DateTime::parse_from_rfc3339(value).is_ok()
    }
}

/// 读取当前 Admin 会话；401 只表示尚未登录或会话已失效，不视为网络故障。
pub(crate) async fn fetch_admin_session() -> Result<Option<AdminSession>, String> {
    let response = send_get("/api/v1/admin/session").await?;
    if response.status() == 401 {
        let failure = response_failure(response).await;
        return if failure.code == "unauthenticated" { Ok(None) } else { Err(failure.code) };
    }
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_admin_session(payload.data).map(Some)
}

/// 使用一次 Basic Auth 登录 Admin；服务端负责设置 HttpOnly session Cookie。
pub(crate) async fn login_admin(actor: String, secret: String) -> Result<AdminSession, String> {
    let credentials = format!("{actor}:{secret}");
    let authorization = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/session")
            .abort_signal(abort_signal)
            .header("authorization", &authorization)
            .build()
            .map_err(super::invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_admin_session(payload.data)
}

/// 使用当前 CSRF token 注销 Admin，会话 Cookie 由服务端清除。
pub(crate) async fn logout_admin(csrf_token: String) -> Result<(), String> {
    let response = send_request(|abort_signal| {
        Request::delete("/api/v1/admin/session")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .build()
            .map_err(super::invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<Value> = response.json().await.map_err(invalid_response_error)?;
    if !payload.data.is_null() {
        return Err("invalid_response".to_owned());
    }
    Ok(())
}

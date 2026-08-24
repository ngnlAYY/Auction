//! Admin 会话 HTTP 适配。
//!
//! 浏览器只持有服务端返回的 actor、CSRF token 和过期时间；Admin secret 只用于一次 Basic
//! Auth 请求，不进入组件之外的状态，也不写入日志或 DTO。

use auction_contracts::{AdminSessionResponse, ApiResponse};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gloo_net::http::Request;

/// 已认证 Admin 会话的浏览器状态；`authenticated = false` 不会构造该类型。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminSession {
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

fn into_session(data: AdminSessionResponse) -> Option<AdminSession> {
    data.authenticated.then_some(AdminSession {
        actor: data.actor,
        csrf_token: data.csrf_token,
        expires_at: data.expires_at,
        absolute_expires_at: data.absolute_expires_at,
    })
}

/// 读取当前 Admin 会话；401 只表示尚未登录或会话已失效，不视为网络故障。
pub(crate) async fn fetch_admin_session() -> Result<Option<AdminSession>, String> {
    let response =
        Request::get("/api/v1/admin/session").send().await.map_err(|error| error.to_string())?;
    if response.status() == 401 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(into_session(payload.data))
}

/// 使用一次 Basic Auth 登录 Admin；服务端负责设置 HttpOnly session Cookie。
pub(crate) async fn login_admin(actor: String, secret: String) -> Result<AdminSession, String> {
    let credentials = format!("{actor}:{secret}");
    let authorization = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));
    let response = Request::post("/api/v1/admin/session")
        .header("authorization", &authorization)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    into_session(payload.data).ok_or_else(|| "Admin 登录响应未包含有效会话".to_owned())
}

/// 使用当前 CSRF token 注销 Admin，会话 Cookie 由服务端清除。
pub(crate) async fn logout_admin(csrf_token: String) -> Result<(), String> {
    let response = Request::delete("/api/v1/admin/session")
        .header("x-csrf-token", &csrf_token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    Ok(())
}

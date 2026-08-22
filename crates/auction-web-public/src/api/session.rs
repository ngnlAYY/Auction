//! Public 会话 HTTP DTO 与同源请求。
//!
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
//! 组件只持有安全身份投影。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PublicSession {
    pub authenticated: bool,
    pub user_id: String,
    pub username: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
struct CurrentSessionData {
    authenticated: bool,
    user_id: String,
    username: String,
    expires_at: String,
    absolute_expires_at: String,
}

impl From<CurrentSessionData> for PublicSession {
    fn from(data: CurrentSessionData) -> Self {
        Self {
            authenticated: data.authenticated,
            user_id: data.user_id,
            username: data.username,
            expires_at: data.expires_at,
            absolute_expires_at: data.absolute_expires_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct PublicLoginRequest {
    key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PublicLoginData {
    user_id: String,
    username: String,
    expires_at: String,
    absolute_expires_at: String,
}

pub(crate) async fn fetch_public_session() -> Result<Option<PublicSession>, String> {
    let response =
        Request::get("/api/v1/session").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<Option<CurrentSessionData>> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.map(Into::into))
}

pub(crate) async fn login_public(key: String) -> Result<PublicSession, String> {
    let request = Request::post("/api/v1/session")
        .json(&PublicLoginRequest { key })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PublicLoginData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(PublicSession {
        authenticated: true,
        user_id: payload.data.user_id,
        username: payload.data.username,
        expires_at: payload.data.expires_at,
        absolute_expires_at: payload.data.absolute_expires_at,
    })
}

pub(crate) async fn logout_public() -> Result<(), String> {
    let response =
        Request::delete("/api/v1/session").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    Ok(())
}

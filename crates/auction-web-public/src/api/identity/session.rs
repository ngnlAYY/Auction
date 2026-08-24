//! Public 会话 HTTP DTO 与同源请求。
//!
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
//! 组件只持有安全身份投影。

pub(crate) use auction_contracts::PublicSessionCurrentResponse as PublicSession;
use auction_contracts::{ApiErrorResponse, ApiResponse, PublicLoginRequest, PublicLoginResponse};
use gloo_net::http::{Request, Response};
use serde_json::Value;

use super::{observe_response_date, send_get};

/// Public 登录失败的稳定投影；界面只根据错误码和安全公开原因选择本地化文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicSessionApiError {
    pub(crate) status: Option<u16>,
    pub(crate) code: Option<String>,
    pub(crate) public_reason: Option<String>,
}

impl PublicSessionApiError {
    /// 构造浏览器本地校验错误，不把用户输入或底层网络错误带入页面。
    pub(crate) fn local(code: &'static str) -> Self {
        Self { status: None, code: Some(code.to_owned()), public_reason: None }
    }

    fn transport() -> Self {
        Self { status: None, code: None, public_reason: None }
    }
}

pub(crate) async fn fetch_public_session() -> Result<Option<PublicSession>, String> {
    let response = send_get("/api/v1/session").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<Option<PublicSession>> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn login_public(key: String) -> Result<PublicSession, PublicSessionApiError> {
    let request = Request::post("/api/v1/session")
        .json(&PublicLoginRequest { key })
        .map_err(|_| PublicSessionApiError::transport())?;
    let response = request.send().await.map_err(|_| PublicSessionApiError::transport())?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(parse_api_error(response).await);
    }
    let payload: ApiResponse<PublicLoginResponse> =
        response.json().await.map_err(|_| PublicSessionApiError::transport())?;
    Ok(PublicSession {
        authenticated: true,
        user_id: payload.data.user_id,
        username: payload.data.username,
        expires_at: payload.data.expires_at,
        absolute_expires_at: payload.data.absolute_expires_at,
    })
}

async fn parse_api_error(response: Response) -> PublicSessionApiError {
    let status = Some(response.status());
    let body = response.text().await.unwrap_or_default();
    let Ok(payload) = serde_json::from_str::<ApiErrorResponse>(&body) else {
        return PublicSessionApiError { status, ..PublicSessionApiError::transport() };
    };
    let public_reason = if status == Some(403) && payload.code == "account_blacklisted" {
        payload
            .data
            .as_ref()
            .and_then(|data| data.get("public_reason"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| {
                !reason.is_empty() && reason.len() <= 1024 && !reason.chars().any(char::is_control)
            })
            .map(ToOwned::to_owned)
    } else {
        None
    };
    PublicSessionApiError { status, code: Some(payload.code), public_reason }
}

pub(crate) async fn logout_public() -> Result<(), String> {
    let response =
        Request::delete("/api/v1/session").send().await.map_err(|error| error.to_string())?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    Ok(())
}

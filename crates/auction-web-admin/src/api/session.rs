//! Admin 会话 HTTP 适配。
//! 浏览器只持有服务端返回的 actor、CSRF token 和过期时间；Admin secret 只用于一次 Basic
//! Auth 请求，不进入组件之外的状态，也不写入日志或 DTO。

use auction_contracts::{AdminSessionResponse, ApiResponse};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use dioxus::{
    core::Runtime,
    prelude::{Memo, Signal, WritableExt, try_consume_context},
};
use gloo_net::http::Request;
use serde_json::Value;

use super::{invalid_response_error, response_error, send_get, send_request};

/// 当前认证外壳拥有的会话检查触发器；API 只发出重读请求，不据网络错误推断已登出。
#[derive(Clone, Copy)]
pub(crate) struct AdminSessionCheckTrigger(pub(crate) Signal<u64>);

/// 认证 owner 提供的实时写门禁；临时会话检查失败不会销毁旧草稿，但必须停止新请求。
#[derive(Clone, Copy)]
pub(crate) struct AdminSessionWritePermission(pub(crate) Memo<bool>);

/// API 与页面命令宿主共用门禁；缺少认证上下文或已卸载时失败关闭。
pub(crate) fn admin_session_writes_allowed() -> bool {
    let Some(runtime) = Runtime::try_current() else { return false };
    if runtime.try_current_scope_id().is_none() {
        return false;
    }
    try_consume_context::<AdminSessionWritePermission>().is_some_and(|permission| (permission.0)())
}

/// 业务 API 明确返回未认证后重新检查会话，统一退出旧登录态或展示会话读取错误。
pub(super) fn request_admin_session_check() {
    let Some(runtime) = Runtime::try_current() else { return };
    if runtime.try_current_scope_id().is_none() {
        return;
    }
    let Some(AdminSessionCheckTrigger(mut generation)) = try_consume_context() else { return };
    if let Ok(mut current) = generation.try_write() {
        *current = current.saturating_add(1);
    }
}

/// 已认证 Admin 会话的浏览器状态；`authenticated = false` 不会构造该类型。
/// 调试格式保留会话元数据，但不能输出 CSRF token。
#[derive(Clone, PartialEq)]
pub(crate) struct AdminSession {
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

impl std::fmt::Debug for AdminSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSession")
            .field("actor", &self.actor)
            .field("csrf_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("absolute_expires_at", &self.absolute_expires_at)
            .finish()
    }
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
        js_sys::Date::parse(value).is_finite()
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
        // 会话读取自身的 401 直接完成本次检查，不能再次触发自己而形成无限重读。
        return Ok(None);
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

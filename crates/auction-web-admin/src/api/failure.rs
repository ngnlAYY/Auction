//! Admin 同源错误 envelope 的解码与稳定失败码投影。
//! 服务端正文的自然语言不进入界面；页面只消费稳定 `code`，再经 Fluent 映射。HTTP 状态
//! 单独保留，供 404、409 等需改变交互流程的页面判断。

use auction_web_shared::{BrowserErrorClass, report_browser_error};
use gloo_net::http::Response;
use serde::Deserialize;

use super::session;

/// Admin 浏览器请求的稳定失败投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminApiFailure {
    pub status: Option<u16>,
    pub code: String,
}

/// Admin 错误 envelope 的浏览器侧兼容模型。
/// 旧版要求 `code`、`message`、`data` 三字段存在但允许扩展字段与任意错误码；错误数据不进入
/// 页面，仍以不透明 JSON 保留，仅为兼容既有 envelope 形状。
#[derive(Debug, Deserialize)]
struct AdminApiErrorResponse {
    code: String,
    message: String,
    data: serde_json::Value,
}

impl AdminApiErrorResponse {
    fn into_code(self) -> String {
        let Self { code, message, data } = self;
        // 错误正文和不透明 data 只用于完成旧 wire 解码，不能进入浏览器状态或界面。
        let _ = (message, data);
        code
    }
}

impl AdminApiFailure {
    pub(crate) fn client(code: impl ToString) -> Self {
        Self { status: None, code: code.to_string() }
    }

    pub(crate) const fn is_conflict(&self) -> bool {
        matches!(self.status, Some(409))
    }
}

/// 将同源错误 envelope 缩减为稳定错误码；正文无效时只保留 HTTP 状态兜底。
pub(crate) async fn response_failure(response: Response) -> AdminApiFailure {
    let failure = read_response_failure(response).await;
    if failure.status == Some(401) && failure.code == "unauthenticated" {
        session::request_admin_session_check();
    }
    failure
}

/// 读取错误 envelope 而不触发认证副作用，供会话检查自身消费确定的未认证结果。
async fn read_response_failure(response: Response) -> AdminApiFailure {
    let status = response.status();
    let code = match response.json::<AdminApiErrorResponse>().await {
        Ok(payload) => payload.into_code(),
        Err(_) => {
            report_browser_error(BrowserErrorClass::DtoDecode);
            format!("http_{status}")
        }
    };
    AdminApiFailure { status: Some(status), code }
}

/// `Result<_, String>` API 只返回稳定错误码，兼容现有页面状态而不泄漏底层错误正文。
pub(crate) async fn response_error(response: Response) -> String {
    response_failure(response).await.code
}

/// 请求构造失败只暴露稳定业务 code；底层构建错误不进入页面。
pub(crate) fn invalid_request_error(_error: impl ToString) -> String {
    "invalid_request".to_owned()
}

/// 响应解码失败时上报 DTO 解码类别，页面只看到稳定 code。
pub(crate) fn invalid_response_error(_error: impl ToString) -> String {
    report_browser_error(BrowserErrorClass::DtoDecode);
    "invalid_response".to_owned()
}

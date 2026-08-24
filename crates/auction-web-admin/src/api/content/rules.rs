//! Admin 规则文档 API 客户端。
//!
//! 浏览器只持有规则安全快照；发布命令的版本 fence 与 command_id 由服务端最终校验。

use auction_contracts::{AdminRulesCandidateRequest, AdminRulesPublishRequest};
pub(crate) use auction_contracts::{
    AdminRulesCandidateResponse as AdminRulesCandidate, AdminRulesResponse as AdminRules,
};
use gloo_net::http::Request;

use super::{ApiResponse, fetch_csrf_token};

/// Admin 规则请求失败时保留 HTTP 状态，页面可以把版本冲突和普通网络错误区分开。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminRulesApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl AdminRulesApiError {
    /// 判断服务端是否拒绝了过期版本，调用方应先重新加载权威快照。
    pub(crate) const fn is_conflict(&self) -> bool {
        matches!(self.status, Some(409))
    }
}

pub(crate) async fn fetch_admin_rules(locale: String) -> Result<AdminRules, AdminRulesApiError> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("lang", locale.trim());
    let url = format!("/api/v1/admin/rules?{}", query.finish());
    let response = Request::get(&url).send().await.map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRules> = response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

pub(crate) async fn validate_admin_rules(
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRulesCandidate, AdminRulesApiError> {
    let request = AdminRulesCandidateRequest { locale, title, markdown };
    let csrf_token = fetch_csrf_token().await.map_err(message_error)?;
    let response = Request::post("/api/v1/admin/rules")
        .header("x-csrf-token", &csrf_token)
        .json(&request)
        .map_err(request_error)?
        .send()
        .await
        .map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRulesCandidate> =
        response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

pub(crate) async fn publish_admin_rules(
    command_id: uuid::Uuid,
    expected_revision: i64,
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRules, AdminRulesApiError> {
    let request =
        AdminRulesPublishRequest { command_id, expected_revision, locale, title, markdown };
    let csrf_token = fetch_csrf_token().await.map_err(message_error)?;
    let response = Request::put("/api/v1/admin/rules")
        .header("x-csrf-token", &csrf_token)
        .json(&request)
        .map_err(request_error)?
        .send()
        .await
        .map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRules> = response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

fn request_error(error: impl ToString) -> AdminRulesApiError {
    message_error(error.to_string())
}

fn response_body_error(error: impl ToString) -> AdminRulesApiError {
    message_error(error.to_string())
}

fn response_error(status: u16) -> AdminRulesApiError {
    AdminRulesApiError { status: Some(status), message: format!("HTTP {status}") }
}

fn message_error(message: String) -> AdminRulesApiError {
    AdminRulesApiError { status: None, message }
}

//! Admin 站点文档（规则、关于）API 客户端。
//! 浏览器只持有文档安全快照；发布命令的版本 fence 与 command_id 由服务端最终校验。
//! 两种文档的请求与响应形状相同，只有接口路径不同。

use auction_contracts::{AdminRulesCandidateRequest, AdminRulesPublishRequest};
pub(crate) use auction_contracts::{
    AdminRulesCandidateResponse as AdminRulesCandidate, AdminRulesResponse as AdminRules,
};
use gloo_net::http::Request;

use super::{
    AdminApiFailure, ApiResponse, MAX_JSON_SAFE_INTEGER, fetch_csrf_token, invalid_request_error,
    invalid_response_error, response_failure, send_get, send_request, valid_admin_time,
    valid_trimmed_text,
};

// 结构闭集由 contracts 的 `AdminRulesResponse` 与 `AdminRulesCandidateResponse`
//（`deny_unknown_fields`）负责；本文件只保留 locale/title/发布时间等值域谓词与请求回显校验。

/// Admin 规则请求失败时保留 HTTP 状态，页面可以把版本冲突和普通网络错误区分开。
pub(crate) type AdminRulesApiError = AdminApiFailure;

/// 管理端可编辑的站点文档种类：决定接口路径，并隔离各自的草稿与未决命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminDocumentKind {
    Rules,
    About,
}

impl AdminDocumentKind {
    fn api_path(self) -> &'static str {
        match self {
            Self::Rules => "/api/v1/admin/rules",
            Self::About => "/api/v1/admin/about",
        }
    }

    /// 未决命令账本里的实体 key；一种文档同时只允许一个未决发布。
    pub(crate) fn pending_key(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::About => "about",
        }
    }
}

pub(crate) async fn fetch_admin_rules(
    kind: AdminDocumentKind,
    locale: String,
) -> Result<AdminRules, AdminRulesApiError> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("lang", locale.trim());
    let url = format!("{}?{}", kind.api_path(), query.finish());
    let response = send_get(&url).await.map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRules> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let rules = decode_rules(payload.data).map_err(invalid_rules_response)?;
    if rules.locale != locale.trim() {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(rules)
}

pub(crate) async fn validate_admin_rules(
    kind: AdminDocumentKind,
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRulesCandidate, AdminRulesApiError> {
    let request = AdminRulesCandidateRequest { locale, title, markdown };
    let csrf_token =
        fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let response = send_request(|abort_signal| {
        Request::post(kind.api_path())
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRulesCandidate> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let candidate = decode_candidate(payload.data).map_err(invalid_rules_response)?;
    if candidate.locale != request.locale
        || candidate.title != request.title.trim()
        || candidate.markdown != request.markdown
    {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(candidate)
}

pub(crate) async fn publish_admin_rules(
    kind: AdminDocumentKind,
    command_id: uuid::Uuid,
    expected_revision: i64,
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRules, AdminRulesApiError> {
    let request =
        AdminRulesPublishRequest { command_id, expected_revision, locale, title, markdown };
    let csrf_token =
        fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let response = send_request(|abort_signal| {
        Request::put(kind.api_path())
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRules> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let rules = decode_rules(payload.data).map_err(invalid_rules_response)?;
    if rules.locale != request.locale
        || rules.title != request.title
        || rules.markdown != request.markdown
        || Some(rules.revision) != request.expected_revision.checked_add(1)
    {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(rules)
}

fn decode_candidate(value: AdminRulesCandidate) -> Result<AdminRulesCandidate, &'static str> {
    if !valid_locale(&value.locale) || !valid_trimmed_text(&value.title) {
        return Err("rules_candidate");
    }
    Ok(value)
}

fn decode_rules(value: AdminRules) -> Result<AdminRules, &'static str> {
    if !valid_locale(&value.locale)
        || !valid_trimmed_text(&value.title)
        || !valid_trimmed_text(&value.published_by)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.revision)
        || !valid_admin_time(&value.published_at)
    {
        return Err("rules");
    }
    Ok(value)
}

fn valid_locale(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 35
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn invalid_rules_response(_error: &'static str) -> AdminRulesApiError {
    AdminApiFailure::client("invalid_response")
}

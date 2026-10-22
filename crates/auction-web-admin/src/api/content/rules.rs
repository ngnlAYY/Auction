//! Admin 规则文档 API 客户端。
//! 浏览器只持有规则安全快照；发布命令的版本 fence 与 command_id 由服务端最终校验。

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

// 本地 Strict 镜像已删除：contracts 的 `AdminRulesResponse` 与 `AdminRulesCandidateResponse`
// 已带 `deny_unknown_fields`；本文件只保留 locale/title/发布时间等值域谓词与请求回显校验。

/// Admin 规则请求失败时保留 HTTP 状态，页面可以把版本冲突和普通网络错误区分开。
pub(crate) type AdminRulesApiError = AdminApiFailure;

pub(crate) async fn fetch_admin_rules(locale: String) -> Result<AdminRules, AdminRulesApiError> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("lang", locale.trim());
    let url = format!("/api/v1/admin/rules?{}", query.finish());
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
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRulesCandidate, AdminRulesApiError> {
    let request = AdminRulesCandidateRequest { locale, title, markdown };
    let csrf_token =
        fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/rules")
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
        Request::put("/api/v1/admin/rules")
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
        && value.chars().all(|character| character.is_alphanumeric() || character == '-')
}

fn invalid_rules_response(_error: &'static str) -> AdminRulesApiError {
    AdminApiFailure::client("invalid_response")
}

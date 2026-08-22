//! Admin 规则文档 API 客户端。
//!
//! 浏览器只持有规则安全快照；发布命令的版本 fence 与 command_id 由服务端最终校验。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiResponse, fetch_csrf_token};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRules {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub published_by: String,
    pub revision: i64,
    pub published_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct AdminRulesCandidateRequest {
    locale: String,
    title: String,
    markdown: String,
}

#[derive(Debug, Clone, Serialize)]
struct AdminRulesPublishRequest {
    command_id: Uuid,
    expected_revision: i64,
    locale: String,
    title: String,
    markdown: String,
}

pub(crate) async fn fetch_admin_rules() -> Result<AdminRules, String> {
    let response =
        Request::get("/api/v1/admin/rules").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRules> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn validate_admin_rules(
    locale: String,
    title: String,
    markdown: String,
) -> Result<(), String> {
    let request = AdminRulesCandidateRequest { locale, title, markdown };
    let csrf_token = fetch_csrf_token().await?;
    let response = Request::post("/api/v1/admin/rules")
        .header("x-csrf-token", &csrf_token)
        .json(&request)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() { Ok(()) } else { Err(format!("HTTP {}", response.status())) }
}

pub(crate) async fn publish_admin_rules(
    expected_revision: i64,
    locale: String,
    title: String,
    markdown: String,
) -> Result<AdminRules, String> {
    let request = AdminRulesPublishRequest {
        command_id: Uuid::new_v4(),
        expected_revision,
        locale,
        title,
        markdown,
    };
    let csrf_token = fetch_csrf_token().await?;
    let response = Request::put("/api/v1/admin/rules")
        .header("x-csrf-token", &csrf_token)
        .json(&request)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRules> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

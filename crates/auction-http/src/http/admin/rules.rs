//! Admin 规则文档 HTTP 适配器。
//! GET 读取安全快照，POST 只做候选规范化校验，PUT 通过应用端口完成版本 CAS、幂等发布和审计。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::{
    AdminRulesCandidate, AdminRulesDocument, AdminRulesError, AdminRulesPublishCommand,
    AdminRulesRequest, AdminRulesTransactionError, ApplicationError, RequestHash,
};
use auction_contracts::{
    AdminRulesCandidateRequest, AdminRulesCandidateResponse, AdminRulesPublishRequest,
    AdminRulesResponse, ApiErrorResponseWithData, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

const DEFAULT_LOCALE: &str = "en";

/// 读取 Admin 规则文档；缺失的非默认语言回退到英文。
pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(&uri, &["lang"]) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let locale = query
        .get("lang")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOCALE.to_owned());
    let Some(port) = state.admin_rules() else {
        return feature_unavailable();
    };
    let request = AdminRulesRequest { locale: locale.clone() };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    let mut document = match port.get(request).await {
        Ok(document) => document,
        Err(error) => return map_error(error),
    };
    if document.is_none() && locale != DEFAULT_LOCALE {
        document = match port.get(AdminRulesRequest { locale: DEFAULT_LOCALE.to_owned() }).await {
            Ok(document) => document,
            Err(error) => return map_error(error),
        };
    }
    let Some(document) = document else {
        return super::not_found_error().into_response();
    };
    if document.validate().is_err() {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("规则已加载", map_document(document))),
    )
        .into_response()
}

/// 校验并规范化 Admin 规则候选，不执行持久化。
pub(crate) async fn validate(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Err(response) = super::authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminRulesCandidateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let candidate =
        AdminRulesCandidate { locale: input.locale, title: input.title, markdown: input.markdown }
            .normalize();
    if let Err(error) = candidate.validate() {
        return map_error(ApplicationError::from(error));
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok(
            "规则候选已验证",
            AdminRulesCandidateResponse {
                locale: candidate.locale,
                title: candidate.title,
                markdown: candidate.markdown,
            },
        )),
    )
        .into_response()
}

/// 发布 Admin 规则文档。
pub(crate) async fn publish(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminRulesPublishRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let request_hash =
        match RequestHash::from_request("admin_rules_publish", input.command_id, &input) {
            Ok(hash) => hash,
            Err(diagnostic_cause) => {
                diagnostics::error(
                    "admin_rules",
                    "hash_publication",
                    DiagnosticError::diagnostic_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                return super::internal_error();
            }
        };
    let Some(port) = state.admin_rules() else {
        return feature_unavailable();
    };
    let result = match port
        .publish(AdminRulesPublishCommand {
            command_id: CommandId::from_uuid(input.command_id),
            expected_revision: input.expected_revision,
            locale: input.locale,
            title: input.title,
            markdown: input.markdown,
            actor_id: identity.actor,
            request_id: super::request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if result.validate().is_err() {
        return super::internal_error();
    }
    state.invalidate_public_rules_cache(&result.locale).await;
    let status = if input.expected_revision == 0 { StatusCode::CREATED } else { StatusCode::OK };
    let message =
        if status == StatusCode::CREATED { "规则已创建并发布" } else { "规则已发布" };
    (status, super::no_store_headers(), axum::Json(ApiResponse::ok(message, map_document(result))))
        .into_response()
}

fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "规则命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

fn map_document(document: AdminRulesDocument) -> AdminRulesResponse {
    AdminRulesResponse {
        locale: document.locale,
        title: document.title,
        markdown: document.markdown,
        published_by: document.published_by,
        revision: document.revision,
        published_at: super::timestamp(document.published_at),
    }
}

fn feature_unavailable() -> Response {
    super::api_error(StatusCode::NOT_IMPLEMENTED, "feature_not_supported", "Admin 规则内容暂不可用")
        .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminRules(AdminRulesError::InvalidLocale) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_locale", "语言参数无效")
                .into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::InvalidDocument)
        | ApplicationError::AdminRules(AdminRulesError::InvalidCommand) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_rule_document", "规则文档参数无效")
                .into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::RevisionConflict { current_revision }) => (
            StatusCode::CONFLICT,
            axum::Json(ApiErrorResponseWithData {
                code: "state_conflict".to_owned(),
                message: "规则版本已变化".to_owned(),
                data: serde_json::json!({ "current_revision": current_revision }),
                request_id: None,
            }),
        )
            .into_response(),
        ApplicationError::AdminRulesTransaction(AdminRulesTransactionError::RevisionConflict {
            current_revision,
        }) => (
            StatusCode::CONFLICT,
            axum::Json(ApiErrorResponseWithData {
                code: "state_conflict".to_owned(),
                message: "规则版本已变化".to_owned(),
                data: serde_json::json!({ "current_revision": current_revision }),
                request_id: None,
            }),
        )
            .into_response(),
        ApplicationError::AdminRulesTransaction(AdminRulesTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "规则内容暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_rules",
        "respond_publication_failure",
        diagnostic_kind,
        response,
    )
}

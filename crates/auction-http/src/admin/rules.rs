//! Admin 站点文档（规则、关于）HTTP 适配器。
//! GET 读取安全快照，POST 只做候选规范化校验，PUT 通过应用端口完成版本 CAS、幂等发布和审计。
//! 两种文档共用同一组处理逻辑，只在路由层绑定各自的文档种类。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminRulesCandidate, AdminRulesDocument, AdminRulesError, AdminRulesPublishCommand,
    AdminRulesRequest, AdminRulesTransactionError,
};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::public::SiteDocumentKind;
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
use crate::diagnostics;

const DEFAULT_LOCALE: &str = "en";

/// 读取 Admin 规则文档；所选语言尚未建立时返回 404。
pub(crate) async fn get(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    read_document(SiteDocumentKind::Rules, state, address, headers, uri).await
}

/// 校验并规范化 Admin 规则候选，不执行持久化。
pub(crate) async fn validate(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    validate_candidate(state, address, headers, uri, body).await
}

/// 发布 Admin 规则文档。
pub(crate) async fn publish(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    publish_document(SiteDocumentKind::Rules, state, address, headers, uri, body).await
}

/// 读取 Admin “关于”文档；所选语言尚未建立时返回 404。
pub(crate) async fn get_about(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    read_document(SiteDocumentKind::About, state, address, headers, uri).await
}

/// 校验并规范化 Admin “关于”候选，不执行持久化。
pub(crate) async fn validate_about(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    validate_candidate(state, address, headers, uri, body).await
}

/// 发布 Admin “关于”文档。
pub(crate) async fn publish_about(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    publish_document(SiteDocumentKind::About, state, address, headers, uri, body).await
}

/// 发布命令的摘要域：两种文档各用一个，规则沿用既有值，已有账本记录仍可重放。
fn publish_hash_domain(kind: SiteDocumentKind) -> &'static str {
    match kind {
        SiteDocumentKind::Rules => "admin_rules_publish",
        SiteDocumentKind::About => "admin_about_publish",
    }
}

async fn read_document(
    kind: SiteDocumentKind,
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
        return super::feature_unavailable("Admin 文档内容暂不可用");
    };
    let request = AdminRulesRequest { kind, locale };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    // 管理端读取不回退到默认语言：编辑器必须能区分“该语言尚未建立”（404，从版本 0 新建）
    // 与“已有文档”。回退会把英文版当成所选语言的基线，新建其余语言时版本与正文都对不上。
    // 公开端读取的语言回退不受影响。
    let document = match port.get(request).await {
        Ok(document) => document,
        Err(error) => return map_error(error),
    };
    let Some(document) = document else {
        return super::not_found_error().into_response();
    };
    if document.validate().is_err() {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("文档已加载", map_document(document))),
    )
        .into_response()
}

/// 候选校验与文档种类无关：两种文档的语言、标题与正文边界相同。
async fn validate_candidate(
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
            "文档候选已验证",
            AdminRulesCandidateResponse {
                locale: candidate.locale,
                title: candidate.title,
                markdown: candidate.markdown,
            },
        )),
    )
        .into_response()
}

async fn publish_document(
    kind: SiteDocumentKind,
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
        match RequestHash::from_request(publish_hash_domain(kind), input.command_id, &input) {
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
        return super::feature_unavailable("Admin 文档内容暂不可用");
    };
    let result = match port
        .publish(AdminRulesPublishCommand {
            command_id: CommandId::from_uuid(input.command_id),
            kind,
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
    let status = if input.expected_revision == 0 { StatusCode::CREATED } else { StatusCode::OK };
    let message =
        if status == StatusCode::CREATED { "文档已创建并发布" } else { "文档已发布" };
    (status, super::no_store_headers(), axum::Json(ApiResponse::ok(message, map_document(result))))
        .into_response()
}

fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "文档命令不接受查询参数")
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

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminRules(AdminRulesError::InvalidLocale) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_locale", "语言参数无效")
                .into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::InvalidDocument)
        | ApplicationError::AdminRules(AdminRulesError::InvalidCommand) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_rule_document", "文档参数无效")
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
                message: "文档版本已变化".to_owned(),
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
                message: "文档版本已变化".to_owned(),
                data: serde_json::json!({ "current_revision": current_revision }),
                request_id: None,
            }),
        )
            .into_response(),
        ApplicationError::AdminRulesTransaction(AdminRulesTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => {
            super::feature_unavailable("Admin 文档内容暂不可用")
        }
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "文档内容暂不可用",
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

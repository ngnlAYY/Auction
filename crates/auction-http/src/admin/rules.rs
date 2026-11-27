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
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error, feature_unavailable,
    not_found_error, query_rejection,
};
use crate::mapping::timestamp;
use crate::middleware::boundary::request_id;
use crate::middleware::strict_json;
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session, internal_error,
    no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_LOCALE: &str = "en";

/// 规则与关于共用的请求体上限；只在鉴权后由解码头强制执行。
const DOCUMENT_BODY_LIMIT: usize = 1 << 20;

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
    body: Body,
) -> Response {
    validate_candidate(state, address, headers, uri, body).await
}

/// 发布 Admin 规则文档。
pub(crate) async fn publish(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
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
    body: Body,
) -> Response {
    validate_candidate(state, address, headers, uri, body).await
}

/// 发布 Admin “关于”文档。
pub(crate) async fn publish_about(
    state: State<AdminApiState>,
    address: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
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
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match parse_query(&uri, &["lang"]) {
        Ok(query) => query,
        Err(()) => return query_rejection().into_response(),
    };
    let locale = query
        .get("lang")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOCALE.to_owned());
    let Some(port) = state.admin_rules() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
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
        return not_found_error().into_response();
    };
    if document.validate().is_err() {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
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
    body: Body,
) -> Response {
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminRulesCandidateRequest =
        match strict_json::decode_after_guard(&headers, body, DOCUMENT_BODY_LIMIT).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let candidate =
        AdminRulesCandidate { locale: input.locale, title: input.title, markdown: input.markdown }
            .normalize();
    if let Err(error) = candidate.validate() {
        return map_error(ApplicationError::from(error));
    }
    (
        StatusCode::OK,
        no_store_headers(),
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
    body: Body,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminRulesPublishRequest =
        match strict_json::decode_after_guard(&headers, body, DOCUMENT_BODY_LIMIT).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
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
                return internal_error();
            }
        };
    let Some(port) = state.admin_rules() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
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
            request_id: request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if result.validate().is_err() {
        return internal_error();
    }
    let status = if input.expected_revision == 0 { StatusCode::CREATED } else { StatusCode::OK };
    let message =
        if status == StatusCode::CREATED { "文档已创建并发布" } else { "文档已发布" };
    (status, no_store_headers(), axum::Json(ApiResponse::ok(message, map_document(result))))
        .into_response()
}

fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "文档命令不接受查询参数")
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
        published_at: timestamp(document.published_at),
    }
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminRules(AdminRulesError::InvalidLocale) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_locale", "语言参数无效").into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::InvalidDocument)
        | ApplicationError::AdminRules(AdminRulesError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_rule_document", "文档参数无效")
                .into_response()
        }
        ApplicationError::AdminRules(AdminRulesError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
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
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(CAPABILITY_NOT_PROVIDED),
        ApplicationError::Serialization(_) => internal_error(),
        _ => api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "文档内容暂不可用")
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminRulesCandidate, AdminRulesDocument, AdminRulesPort, AdminRulesPublishCommand,
        AdminRulesRequest,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::public::SiteDocumentKind;
    use auction_contracts::ApiErrorResponse;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    use super::map_error;
    use crate::server::contract_fixtures::{AdminFixturePorts, admin_get, assert_http, time};

    #[tokio::test]
    async fn unsupported_publication_locale_keeps_the_invalid_locale_response() {
        let candidate = AdminRulesCandidate {
            locale: "fr".to_owned(),
            title: "Titre".to_owned(),
            markdown: String::new(),
        };
        let error = candidate.validate().expect_err("Public 不支持 fr");
        let response = map_error(error.into());
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: ApiErrorResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error.code, "invalid_locale");
    }

    struct FixtureDocuments;

    impl AdminRulesPort for FixtureDocuments {
        fn get(
            &self,
            request: AdminRulesRequest,
        ) -> ApplicationFuture<'_, Option<AdminRulesDocument>> {
            let (title, markdown) = match (request.kind, request.locale.as_str()) {
                (SiteDocumentKind::Rules, "en") => {
                    ("Auction rules", "# Auction rules\n\nExample document.")
                }
                (SiteDocumentKind::Rules, "fr") => ("Règles", ""),
                (SiteDocumentKind::About, "zh-CN") => ("关于拍卖", "# 关于拍卖"),
                other => panic!("非预期的文档请求：{other:?}"),
            };
            Box::pin(async move {
                Ok(Some(AdminRulesDocument {
                    locale: request.locale,
                    title: title.to_owned(),
                    markdown: markdown.to_owned(),
                    published_by: "fixture-admin".to_owned(),
                    revision: 6,
                    published_at: time(),
                }))
            })
        }

        fn publish(
            &self,
            _command: AdminRulesPublishCommand,
        ) -> ApplicationFuture<'_, AdminRulesDocument> {
            panic!("文档读取样本不发布文档")
        }
    }

    #[tokio::test]
    async fn admin_rules_and_about_contract_fixtures() {
        let state =
            AdminFixturePorts { rules: Some(Arc::new(FixtureDocuments)), ..Default::default() }
                .state();
        for (name, url) in [
            ("admin/rules", "/api/v1/admin/rules"),
            ("admin/rules-legacy-locale", "/api/v1/admin/rules?lang=fr"),
            ("admin/about", "/api/v1/admin/about?lang=zh-CN"),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), url).await).await;
        }
    }

    #[tokio::test]
    async fn rules_version_error_contract_fixture() {
        use auction_application::admin::AdminRulesError;

        assert_http(
            "admin/error-document-state-conflict",
            StatusCode::CONFLICT,
            map_error(AdminRulesError::RevisionConflict { current_revision: 6 }.into()),
        )
        .await;
    }
}

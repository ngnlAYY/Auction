//! Public 站点文档（规则、关于）HTTP 适配器。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use auction_application::public::{
    PUBLIC_RULES_DEFAULT_LOCALE, PublicRulesDocument, PublicRulesRequest, SiteDocumentKind,
};
use auction_contracts::{ApiErrorResponse, ApiResponse, PublicRulesResponse};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
};

use super::{PublicApiState, api_error, parse_query, public_timestamp};

type DocumentResult = Result<
    (HeaderMap, Json<ApiResponse<PublicRulesResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
>;

/// 读取指定语言的公开规则；缺失的非默认语言回退到英文。
pub(crate) async fn get(state: State<PublicApiState>, uri: Uri) -> DocumentResult {
    read_document(SiteDocumentKind::Rules, state, uri).await
}

/// 读取指定语言的“关于”文档；缺失的非默认语言回退到英文。
pub(crate) async fn get_about(state: State<PublicApiState>, uri: Uri) -> DocumentResult {
    read_document(SiteDocumentKind::About, state, uri).await
}

async fn read_document(
    kind: SiteDocumentKind,
    State(state): State<PublicApiState>,
    uri: Uri,
) -> DocumentResult {
    let query = parse_query(&uri, &["lang"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let requested_locale = query
        .get("lang")
        .filter(|locale| !locale.is_empty())
        .cloned()
        .unwrap_or_else(|| PUBLIC_RULES_DEFAULT_LOCALE.to_owned());
    let request = PublicRulesRequest { kind, locale: requested_locale.clone() };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_locale", "语言参数无效"))?;

    let Some(reader) = state.rules() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "规则内容暂不可用",
        ));
    };
    let mut document = reader.get_public_rules(request).await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_rules",
            "read_requested_locale",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );

        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "规则内容暂不可用")
    })?;
    if document.is_none() && requested_locale != PUBLIC_RULES_DEFAULT_LOCALE {
        document = reader
            .get_public_rules(PublicRulesRequest {
                kind,
                locale: PUBLIC_RULES_DEFAULT_LOCALE.to_owned(),
            })
            .await
            .map_err(|diagnostic_cause| {
                diagnostics::warn(
                    "public_rules",
                    "read_fallback_locale",
                    DiagnosticError::diagnostic_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "规则内容暂不可用",
                )
            })?;
    }
    let Some(document) = document else {
        return Err(match kind {
            // 规则是站点运行的必备内容，缺失按服务不可用报告（既有契约）。
            SiteDocumentKind::Rules => {
                api_error(StatusCode::SERVICE_UNAVAILABLE, "rules_unavailable", "规则内容暂不可用")
            }
            // “关于”是可选内容：尚未发布是确定的空状态，不是故障。
            SiteDocumentKind::About => {
                api_error(StatusCode::NOT_FOUND, "about_unpublished", "关于内容尚未发布")
            }
        });
    };
    if document.validate().is_err() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "规则内容暂不可用",
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, Json(ApiResponse::ok("文档已加载", map_document(document)))))
}

fn map_document(document: PublicRulesDocument) -> PublicRulesResponse {
    PublicRulesResponse {
        locale: document.locale,
        title: document.title,
        markdown: document.markdown,
        revision: document.revision,
        published_at: public_timestamp(document.published_at),
    }
}

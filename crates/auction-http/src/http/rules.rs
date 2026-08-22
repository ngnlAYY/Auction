//! Public 规则文档 HTTP 适配器。

use auction_application::{PUBLIC_RULES_DEFAULT_LOCALE, PublicRulesDocument, PublicRulesRequest};
use auction_contracts::{ApiErrorResponse, ApiResponse, PublicRulesResponse};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
};

use super::{PublicApiState, api_error, parse_query, timestamp};

/// 读取指定语言的公开规则；缺失的非默认语言回退到英文。
pub(crate) async fn get(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<(HeaderMap, Json<ApiResponse<PublicRulesResponse>>), (StatusCode, Json<ApiErrorResponse>)>
{
    let query = parse_query(&uri, &["lang"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let requested_locale = query
        .get("lang")
        .filter(|locale| !locale.is_empty())
        .cloned()
        .unwrap_or_else(|| PUBLIC_RULES_DEFAULT_LOCALE.to_owned());
    let request = PublicRulesRequest { locale: requested_locale.clone() };
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
    let mut document = reader.get_public_rules(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "规则内容暂不可用")
    })?;
    if document.is_none() && requested_locale != PUBLIC_RULES_DEFAULT_LOCALE {
        document = reader
            .get_public_rules(PublicRulesRequest { locale: PUBLIC_RULES_DEFAULT_LOCALE.to_owned() })
            .await
            .map_err(|_| {
                api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "规则内容暂不可用",
                )
            })?;
    }
    let Some(document) = document else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "rules_unavailable",
            "规则内容暂不可用",
        ));
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
    Ok((headers, Json(ApiResponse::ok("规则已加载", map_document(document)))))
}

fn map_document(document: PublicRulesDocument) -> PublicRulesResponse {
    PublicRulesResponse {
        locale: document.locale,
        title: document.title,
        markdown: document.markdown,
        revision: document.revision,
        published_at: timestamp(document.published_at),
    }
}

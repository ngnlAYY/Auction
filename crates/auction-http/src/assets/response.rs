//! 静态资源响应构造、条件请求与页面重定向及错误。

use std::borrow::Cow;

use auction_contracts::ApiErrorResponse;
use auction_time::Timestamp;
use axum::Json;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::{EmbeddedFile, RustEmbed};

use super::cache::{CachePolicy, StaticCachePolicy};
use super::negotiation::{AssetEncoding, negotiate_encoding, static_method};
use super::routes::{Surface, browser_route};
use super::{
    ADMIN_NOT_FOUND_HTML, ASSETS_MISSING_BODY, IMF_FIXDATE, OBSOLETE_HTTP_DATES,
    PUBLIC_NOT_FOUND_HTML,
};
use crate::middleware::boundary::request_id;
use crate::middleware::response_cache::etag_matches;

const VARY_ACCEPT_ENCODING: &str = "Accept-Encoding";

/// 路径与方法检查通过后，才按实际副本协商并构造静态响应。
pub(super) fn negotiated_asset_response<A: RustEmbed>(
    file: EmbeddedFile,
    relative_path: &str,
    request: &Request,
    cache_policy: CachePolicy,
    preload_links: Option<HeaderValue>,
) -> Response {
    let compressed = A::get(&format!("{relative_path}.br"));
    let selection = negotiate_encoding(request.headers(), compressed.is_some());
    // 压缩副本沿用原文的媒体类型，验证器与长度取实际发出的表示。
    let content_type = file.metadata.mimetype().to_owned();
    let (file, encoding) = match (selection, compressed) {
        (Some(AssetEncoding::Brotli), Some(compressed)) => (compressed, Some("br")),
        (Some(AssetEncoding::Identity), _) => (file, None),
        _ => {
            return (
                StatusCode::NOT_ACCEPTABLE,
                [
                    (header::VARY, HeaderValue::from_static(VARY_ACCEPT_ENCODING)),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
            )
                .into_response();
        }
    };
    let mut response = asset_response(
        file,
        request.method(),
        request.headers(),
        cache_policy,
        &content_type,
        encoding,
    );
    // 即使只有原文，缓存的 200 与 304 也不能被拒绝原文的请求复用。
    response.headers_mut().insert(header::VARY, HeaderValue::from_static(VARY_ACCEPT_ENCODING));
    if let Some(preload_links) = preload_links {
        response.headers_mut().insert(header::LINK, preload_links);
    }
    response
}

pub(super) fn asset_response(
    file: EmbeddedFile,
    method: &Method,
    request_headers: &HeaderMap,
    cache_policy: CachePolicy,
    content_type: &str,
    encoding: Option<&'static str>,
) -> Response {
    // HTML 不是可共享的静态缓存候选，即使构建产物把它放在 assets/ 下。
    let cache_policy = if content_type.eq_ignore_ascii_case("text/html") {
        CachePolicy::NoCache
    } else {
        cache_policy
    };
    let content_length = file.data.len();
    let content_type = HeaderValue::from_str(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    // 同秒重建或回滚会让稳定 URL 的构建时间与内容变化脱钩，仅保留指纹资源的日期验证器。
    let last_modified = if matches!(cache_policy, CachePolicy::Immutable) {
        file.metadata
            .last_modified()
            .and_then(|seconds| Timestamp::from_timestamp(i64::try_from(seconds).ok()?, 0))
    } else {
        None
    };
    // 对实际发送的表示使用内容摘要，压缩副本与原文各自再验证。
    let etag = HeaderValue::from_str(&format!(
        "\"{}\"",
        auction_application::core::lower_hex(&file.metadata.sha256_hash())
    ))
    .ok();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_policy.value()));
    if let Some(etag) = etag {
        headers.insert(header::ETAG, etag);
    }
    if let Some(encoding) = encoding {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    if let Some(last_modified) = last_modified {
        headers.insert(
            header::LAST_MODIFIED,
            HeaderValue::from_str(&last_modified.strftime(IMF_FIXDATE).to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT")),
        );
    }
    // If-None-Match 优先，陈旧 ETag 不得被相同日期覆盖（RFC 9110 §13.1.3）。
    let revalidated = if request_headers.contains_key(header::IF_NONE_MATCH) {
        etag_not_modified(request_headers, &headers)
    } else {
        last_modified.is_some_and(|last_modified| not_modified(request_headers, last_modified))
    };
    if revalidated {
        // RFC 9110 §15.4.5 要求 304 在响应头结束。保留对应 200 表示的长度（§8.6），
        // 让真正的空正文在 HTTP/2 的 HEADERS 上结束流，同时防止 axum 补出长度 0。
        let mut response = (StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response();
        response.extensions_mut().insert(StaticCachePolicy(cache_policy));
        return response;
    }

    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        // release 构建中数据借用自二进制静态段，这里避免多余复制。
        match file.data {
            Cow::Borrowed(bytes) => Body::from(bytes),
            Cow::Owned(bytes) => Body::from(bytes),
        }
    };
    let mut response = (StatusCode::OK, headers, body).into_response();
    response.extensions_mut().insert(StaticCachePolicy(cache_policy));
    response
}

/// `If-None-Match` 是列表字段，允许重复出现；比较语义（精确、弱比较与 `*`）与 API 响应缓存
/// 共用同一实现。日期不再为它兜底，经代理弱化的 `W/"…"` 也必须在这里命中。
pub(super) fn etag_not_modified(request_headers: &HeaderMap, response_headers: &HeaderMap) -> bool {
    let Some(etag) = response_headers.get(header::ETAG) else {
        return false;
    };
    request_headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .any(|candidates| etag_matches(candidates, etag))
}

/// 指纹资源沿用日期验证器同秒匹配的行为。
pub(super) fn not_modified(headers: &HeaderMap, last_modified: Timestamp) -> bool {
    headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_http_date)
        .is_some_and(|requested| last_modified.timestamp() == requested.timestamp())
}

/// 无法解析（含星期与日期不符）的日期按 RFC 9110 §13.1.3 忽略，响应退回完整内容。
pub(super) fn parse_http_date(value: &str) -> Option<Timestamp> {
    std::iter::once(IMF_FIXDATE)
        .chain(OBSOLETE_HTTP_DATES)
        .find_map(|format| Timestamp::strptime_utc(format, value))
}

/// 只把同一 surface 白名单中的页面重定向到去掉一个尾斜杠后的规范路径。
pub(super) fn canonical_page_redirect(
    surface: Surface,
    method: &Method,
    uri: &Uri,
) -> Option<Response> {
    if !static_method(method) {
        return None;
    }
    let canonical = uri.path().strip_suffix('/')?;
    // 双斜杠去掉一个后会等于根路径，不能把它当成白名单页面的别名。
    if canonical == "/" || !browser_route(surface, canonical) {
        return None;
    }
    let location = match uri.query() {
        Some(query) => format!("{canonical}?{query}"),
        None => canonical.to_owned(),
    };
    let headers = [
        (header::LOCATION, HeaderValue::from_str(&location).ok()?),
        (header::CACHE_CONTROL, HeaderValue::from_static(CachePolicy::NoCache.value())),
    ];
    Some((StatusCode::PERMANENT_REDIRECT, headers).into_response())
}

pub(super) fn method_not_allowed(request_headers: &HeaderMap) -> Response {
    let mut response = (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ApiErrorResponse {
            code: "method_not_allowed".to_owned(),
            message: "请求方法不受支持".to_owned(),
            data: None,
            request_id: request_id(request_headers),
        }),
    )
        .into_response();
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn json_not_found(request_headers: &HeaderMap) -> Response {
    let mut response = (
        StatusCode::NOT_FOUND,
        Json(ApiErrorResponse {
            code: "not_found".to_owned(),
            message: "请求的资源不存在".to_owned(),
            data: None,
            request_id: request_id(request_headers),
        }),
    )
        .into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn not_found(
    surface: Surface,
    method: &Method,
    request_headers: &HeaderMap,
) -> Response {
    if !accepts_html(request_headers) {
        return json_not_found(request_headers);
    }
    let document = match surface {
        Surface::Public => PUBLIC_NOT_FOUND_HTML,
        Surface::Admin => ADMIN_NOT_FOUND_HTML,
    };
    let body = if *method == Method::HEAD { Body::empty() } else { Body::from(document) };
    let mut response = (StatusCode::NOT_FOUND, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&document.len().to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    response
}

pub(super) fn accepts_html(headers: &HeaderMap) -> bool {
    headers.get_all(header::ACCEPT).iter().filter_map(|value| value.to_str().ok()).any(|value| {
        value.split(',').any(|item| {
            let mut parts = item.split(';');
            let media_type = parts.next().map_or_default(str::trim);
            let quality = parts
                .filter_map(|part| part.trim().split_once('='))
                .find_map(|(name, value)| {
                    name.trim().eq_ignore_ascii_case("q").then_some(value.trim())
                })
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            (media_type.eq_ignore_ascii_case("text/html")
                || media_type.eq_ignore_ascii_case("application/xhtml+xml"))
                && quality > 0.0
        })
    })
}

/// 批准的浏览器路由缺少前端产物时说明可执行的恢复动作，而不是 panic。
pub(super) fn assets_missing(method: &Method) -> Response {
    let body =
        if *method == Method::HEAD { Body::empty() } else { Body::from(ASSETS_MISSING_BODY) };
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&ASSETS_MISSING_BODY.len().to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    response
}

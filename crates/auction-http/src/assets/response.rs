//! 静态资源响应构造、条件请求与 404/405/503 页面。

use std::borrow::Cow;

use auction_contracts::ApiErrorResponse;
use auction_time::Timestamp;
use axum::Json;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::EmbeddedFile;

use super::cache::{CachePolicy, StaticCachePolicy};
use super::routes::Surface;
use super::{
    ADMIN_NOT_FOUND_HTML, ASSETS_MISSING_BODY, IMF_FIXDATE, OBSOLETE_HTTP_DATES,
    PUBLIC_NOT_FOUND_HTML,
};
use crate::middleware::boundary::request_id;
use crate::middleware::response_cache::etag_matches;

pub(super) fn asset_response(
    file: EmbeddedFile,
    method: &Method,
    request_headers: &HeaderMap,
    cache_policy: CachePolicy,
    content_type: &str,
    encoding: Option<&'static str>,
) -> Response {
    // HTML 不是可共享的静态缓存候选，即使构建产物把它放在 assets/ 下。
    let cache_policy = if file.metadata.mimetype().eq_ignore_ascii_case("text/html") {
        CachePolicy::NoCache
    } else {
        cache_policy
    };
    let content_length = file.data.len();
    let content_type = HeaderValue::from_str(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let last_modified = file
        .metadata
        .last_modified()
        .and_then(|seconds| Timestamp::from_timestamp(i64::try_from(seconds).ok()?, 0));
    // 内容摘要作为强 ETag；嵌入文件 mtime 可能随构建环境变化，ETag 让再验证稳定命中。
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
        // 压缩表示依赖请求头协商；有副本的资源两个方向都必须声明 Vary。
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
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
    // 不变量：请求带 `If-None-Match` 时只按内容 ETag 判定，`If-Modified-Since` 仅在它缺席时参与
    // （RFC 9110 §13.1.3）。`Last-Modified` 取自构建时的提交时间：同一提交重建或回滚到更早的
    // 提交后内容已变而时间没有前进，两者按“或”合并会让浏览器一直停在另一版入口文档上。
    // 只有日期验证器时也只认原样回传的同一时刻（见 `not_modified`）。
    let revalidated = if request_headers.contains_key(header::IF_NONE_MATCH) {
        etag_not_modified(request_headers, &headers)
    } else {
        last_modified.is_some_and(|last_modified| not_modified(request_headers, last_modified))
    };
    if revalidated {
        headers.remove(header::CONTENT_LENGTH);
        let mut response = (StatusCode::NOT_MODIFIED, headers).into_response();
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

/// 只有 `If-Modified-Since` 时，仅当它与本资源的 `Last-Modified` 是同一秒才判为未修改。
/// Cloudflare 改写 HTML 时会剥离入口文档的 ETag，浏览器与 CDN 回源时只剩日期验证器；回滚到
/// 更早的提交后 `Last-Modified` 倒退，按 RFC 的“不晚于”判定会把回滚后的入口回成 304，客户端
/// 继续停在已撤下的新版。浏览器与 CDN 都原样回传收到的 `Last-Modified`，稳态再验证仍得 304。
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
            let media_type = parts.next().map(str::trim).unwrap_or_default();
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

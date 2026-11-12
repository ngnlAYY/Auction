//! 前端静态产物内嵌、显式浏览器路由与 404 分派。
//! 两个装配根各自提供 `RustEmbed` 类型，资源目录相对其 `CARGO_MANIFEST_DIR`，互不引用。
//! 必须保留 `allow_missing = true`，让未运行 `dx bundle` 的工作区仍可编译；缺失页面返回 503。
//! release 内嵌资源，dev 从磁盘读取；本 crate 仅拥有通用响应和 surface 路由规则。

mod routes;

use std::{
    borrow::Cow,
    time::{Duration, SystemTime},
};

use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::EmbeddedFile;

use routes::Surface;

/// SPA 入口文档；只有 routes 模块批准的浏览器路径可以回退到它。
const INDEX_HTML: &str = "index.html";
const ASSETS_MISSING_BODY: &str =
    "web assets are not included in this binary; run `just build-release`";
const PUBLIC_NOT_FOUND_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>404</title></head><body><main><h1>404</h1></main></body></html>";
const ADMIN_NOT_FOUND_HTML: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>404</title></head><body><main><h1>404</h1></main></body></html>";

/// 处理公开端的静态产物与显式浏览器路由。
pub(crate) async fn serve_public_asset<A: rust_embed::RustEmbed>(request: Request) -> Response {
    serve::<A>(Surface::Public, request, false)
}

/// 处理管理端的静态产物与显式浏览器路由。
pub(crate) async fn serve_admin_asset<A: rust_embed::RustEmbed>(request: Request) -> Response {
    serve::<A>(Surface::Admin, request, false)
}

/// 只向一次性初始化进程提供 Admin 的 `/setup` 页面和必要静态资源。
pub async fn serve_admin_setup_asset<A: rust_embed::RustEmbed>(request: Request) -> Response {
    let mut response = serve::<A>(Surface::Admin, request, true);
    // 首次设置不属于正式 CDN 表面，连静态资源也不允许边缘保存。
    response.extensions_mut().remove::<StaticCachePolicy>();
    response.headers_mut().insert("cdn-cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("cloudflare-cdn-cache-control", HeaderValue::from_static("no-store"));
    response
}

/// 按“保留路径 → 精确静态文件 → 批准浏览器路由 → 404”顺序封闭分派请求。
fn serve<A: rust_embed::RustEmbed>(
    surface: Surface,
    request: Request,
    setup_only: bool,
) -> Response {
    let method = request.method();
    let path = request.uri().path();
    let request_headers = request.headers();

    if setup_only
        && path != "/setup"
        && path != "/favicon.ico"
        && path != "/index.html"
        && !path.starts_with("/assets/")
    {
        return not_found(surface, method, request_headers);
    }

    if path == "/favicon.ico" {
        if !static_method(method) {
            return method_not_allowed(request_headers);
        }
        return match favicon_file::<A>() {
            Some(file) => {
                let content_type = file.metadata.mimetype().to_owned();
                asset_response(
                    file,
                    method,
                    request_headers,
                    CachePolicy::NoCache,
                    &content_type,
                    None,
                )
            }
            None => not_found(surface, method, request_headers),
        };
    }

    if reserved_json_path(path) {
        return json_not_found(request_headers);
    }

    if let Some(relative_path) = exact_asset_path(path)
        && let Some(file) = A::get(relative_path)
    {
        if !static_method(method) {
            return method_not_allowed(request_headers);
        }
        // 构建期生成的高等级 brotli 副本按 Accept-Encoding 协商；没有副本的资源不受影响。
        // Content-Type 始终取原始表示，副本只是同一资源的压缩编码。
        let content_type = file.metadata.mimetype().to_owned();
        let compressed = A::get(&format!("{relative_path}.br"));
        let vary = compressed.is_some();
        let (file, encoding) = match compressed {
            Some(compressed) if accepts_brotli(request_headers) => (compressed, Some("br")),
            _ => (file, None),
        };
        let mut response = asset_response(
            file,
            method,
            request_headers,
            exact_asset_cache(relative_path),
            &content_type,
            encoding,
        );
        if vary {
            response
                .headers_mut()
                .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        }
        return response;
    }

    if (setup_only && path == "/setup") || (!setup_only && routes::browser_route(surface, path)) {
        if !static_method(method) {
            return method_not_allowed(request_headers);
        }
        return match A::get(INDEX_HTML) {
            Some(index) => {
                let content_type = index.metadata.mimetype().to_owned();
                asset_response(
                    index,
                    method,
                    request_headers,
                    CachePolicy::NoStore,
                    &content_type,
                    None,
                )
            }
            None => assets_missing(method),
        };
    }

    not_found(surface, method, request_headers)
}

/// 精确静态分派不能依赖后端规范化路径。RustEmbed 的开发态读取允许符号链接，若先传入
/// `..` 或反斜线，既可能越出资源目录，也可能让路径前缀与实际文件的缓存策略不一致。
fn exact_asset_path(path: &str) -> Option<&str> {
    let relative_path = path.strip_prefix('/')?;
    if relative_path.contains('\\')
        || relative_path.ends_with(".br")
        || relative_path
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(relative_path)
}

fn static_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// 解析 Accept-Encoding：任一 `br` 项的 q 值大于 0 即接受；非法 q 值按接受处理（宽容）。
fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers.get_all(header::ACCEPT_ENCODING).iter().filter_map(|value| value.to_str().ok()).any(
        |value| {
            value.split(',').any(|item| {
                let mut parts = item.split(';');
                let coding = parts.next().unwrap_or_default().trim();
                if !coding.eq_ignore_ascii_case("br") {
                    return false;
                }
                parts.all(|parameter| {
                    let parameter = parameter.trim();
                    match parameter.strip_prefix("q=").or_else(|| parameter.strip_prefix("Q=")) {
                        Some(weight) => {
                            weight.trim().parse::<f32>().map(|weight| weight > 0.0).unwrap_or(true)
                        }
                        None => true,
                    }
                })
            })
        },
    )
}

fn reserved_json_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/health" || path.starts_with("/health/")
}

#[derive(Clone, Copy)]
enum CachePolicy {
    NoCache,
    NoStore,
    Immutable,
}

impl CachePolicy {
    const fn value(self) -> &'static str {
        match self {
            Self::NoCache => "no-cache",
            Self::NoStore => "no-store",
            Self::Immutable => "public, max-age=31536000, immutable",
        }
    }
}

// 只有本模块的精确静态响应可以签发此标记；API 的 Cache-Control 不能启用 CDN 缓存。
#[derive(Clone, Copy)]
struct StaticCachePolicy(CachePolicy);

/// 返回静态 handler 明确批准的 CDN 策略，其余响应一律禁止边缘保存。
pub(crate) fn cdn_cache_policy(response: &Response) -> HeaderValue {
    if let Some(policy) = response.extensions().get::<StaticCachePolicy>() {
        return HeaderValue::from_static(policy.0.value());
    }
    // 动态响应：仅当处理器明确声明 no-cache 时允许边缘保存并强制再验证；
    // no-store、缺失头与带 max-age 的动态读取继续禁止边缘保存。
    let no_cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("no-cache"));
    if no_cache {
        HeaderValue::from_static("no-cache")
    } else {
        HeaderValue::from_static("no-store")
    }
}

fn exact_asset_cache(relative_path: &str) -> CachePolicy {
    if relative_path == INDEX_HTML {
        // 入口文档必须再验证，但允许浏览器/CDN 保存，避免每次导航都完整回源。
        return CachePolicy::NoCache;
    }
    // Dioxus 只给 assets/ 下的 CSS 等资源加内容指纹；顶层 wasm/*.js 与 *.wasm 文件名稳定，
    // 必须每次再验证，不能把全部非 index 文件都标为 immutable。
    if relative_path.starts_with("assets/") { CachePolicy::Immutable } else { CachePolicy::NoCache }
}

fn favicon_file<A: rust_embed::RustEmbed>() -> Option<EmbeddedFile> {
    let path = A::iter().find(|path| {
        let path = path.as_ref();
        path.starts_with("assets/favicon-") && path.ends_with(".svg")
    })?;
    A::get(path.as_ref())
}

fn asset_response(
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
        .and_then(|seconds| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)));
    // 内容摘要作为强 ETag；嵌入文件 mtime 可能随构建环境变化，ETag 让再验证稳定命中。
    let etag = HeaderValue::from_str(&format!(
        "\"{}\"",
        auction_application::core::lower_hex(&file.metadata.sha256_hash())
    ))
    .ok();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_policy.value()));
    if let Some(etag) = etag.clone() {
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
            HeaderValue::from_str(&httpdate::fmt_http_date(last_modified))
                .unwrap_or_else(|_| HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT")),
        );
    }
    if etag_not_modified(request_headers, &headers)
        || last_modified.is_some_and(|last_modified| not_modified(request_headers, last_modified))
    {
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

fn etag_not_modified(request_headers: &HeaderMap, response_headers: &HeaderMap) -> bool {
    let Some(etag) = response_headers.get(header::ETAG).and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    request_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
}

fn not_modified(headers: &HeaderMap, last_modified: SystemTime) -> bool {
    headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value).ok())
        .is_some_and(|requested| last_modified <= requested)
}

fn method_not_allowed(request_headers: &HeaderMap) -> Response {
    let mut response = (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ApiErrorResponse {
            code: "method_not_allowed".to_owned(),
            message: "请求方法不受支持".to_owned(),
            data: None,
            request_id: crate::mapping::request_id(request_headers),
        }),
    )
        .into_response();
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn json_not_found(request_headers: &HeaderMap) -> Response {
    let mut response = (
        StatusCode::NOT_FOUND,
        Json(ApiErrorResponse {
            code: "not_found".to_owned(),
            message: "请求的资源不存在".to_owned(),
            data: None,
            request_id: crate::mapping::request_id(request_headers),
        }),
    )
        .into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn not_found(surface: Surface, method: &Method, request_headers: &HeaderMap) -> Response {
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

fn accepts_html(headers: &HeaderMap) -> bool {
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
fn assets_missing(method: &Method) -> Response {
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

#[cfg(test)]
mod tests {
    use super::{accepts_brotli, serve_public_asset};
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::response::Response;

    #[derive(rust_embed::Embed)]
    #[folder = "tests/assets-fixture"]
    struct TestAssets;

    fn request(uri: &str, accept_encoding: Option<&str>) -> Request {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(value) = accept_encoding {
            builder = builder.header(header::ACCEPT_ENCODING, value);
        }
        builder.body(Body::empty()).expect("request")
    }

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 4096).await.expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    }

    #[test]
    fn accepts_brotli_parses_quality() {
        let cases = [
            (Some("br"), true),
            (Some("gzip, br;q=0.5"), true),
            (Some("br;q=0"), false),
            (Some("br;Q=0.000"), false),
            (Some("gzip, deflate"), false),
            (Some(""), false),
            (None, false),
        ];
        for (value, expected) in cases {
            let headers = match value {
                Some(value) => {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        header::ACCEPT_ENCODING,
                        HeaderValue::from_str(value).expect("header"),
                    );
                    headers
                }
                None => HeaderMap::new(),
            };
            assert_eq!(accepts_brotli(&headers), expected, "value={value:?}");
        }
    }

    #[tokio::test]
    async fn brotli_copy_is_served_when_accepted() {
        let response =
            serve_public_asset::<TestAssets>(request("/assets/app.css", Some("gzip, br"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_ENCODING)
                .expect("encoding")
                .to_str()
                .expect("ascii"),
            "br"
        );
        assert_eq!(
            response.headers().get(header::VARY).expect("vary").to_str().expect("ascii"),
            "Accept-Encoding"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).expect("type").to_str().expect("ascii"),
            "text/css"
        );
        assert_eq!(body_text(response).await, "compressed-css\n");
    }

    #[tokio::test]
    async fn raw_copy_is_served_with_vary_and_zero_quality_is_respected() {
        for accept in [None, Some("br;q=0")] {
            let response =
                serve_public_asset::<TestAssets>(request("/assets/app.css", accept)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
            assert_eq!(
                response.headers().get(header::VARY).expect("vary").to_str().expect("ascii"),
                "Accept-Encoding"
            );
            assert_eq!(body_text(response).await, "body{color:red}\n");
        }
    }

    #[tokio::test]
    async fn resource_without_copy_has_no_vary_or_encoding() {
        let response =
            serve_public_asset::<TestAssets>(request("/assets/app.js", Some("br"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(response.headers().get(header::VARY).is_none());
    }

    #[tokio::test]
    async fn direct_br_path_is_rejected() {
        let response =
            serve_public_asset::<TestAssets>(request("/assets/app.css.br", Some("br"))).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

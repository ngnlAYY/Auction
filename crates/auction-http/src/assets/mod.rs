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

use crate::middleware::response_cache::etag_matches;
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
                    CachePolicy::Favicon,
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
                let preload_links = entry_preload_links(&index.data);
                let mut response = asset_response(
                    index,
                    method,
                    request_headers,
                    CachePolicy::NoCache,
                    &content_type,
                    None,
                );
                if let Some(preload_links) = preload_links {
                    response.headers_mut().insert(header::LINK, preload_links);
                }
                response
            }
            None => assets_missing(method),
        };
    }

    not_found(surface, method, request_headers)
}

/// 把入口文档引用的样式表与预加载资源写成 `Link` 响应头。
///
/// 入口文档每次都要回源再验证；前置 CDN 可以据这个头在等待源站期间先发 103 Early Hints，
/// 浏览器提前取回带指纹的 WASM、脚本与样式。没有 Early Hints 时它与文档内的 `<link>` 等价，
/// 浏览器按 URL 与请求模式去重，不会重复下载。只转写 `assets/` 下、字符受限的同源路径，
/// 属性沿用文档里的写法，保证预加载与真正的请求匹配。
fn entry_preload_links(document: &[u8]) -> Option<HeaderValue> {
    let document = std::str::from_utf8(document).ok()?;
    let mut links = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find("<link ") {
        let tag = &rest[start + "<link ".len()..];
        let Some(end) = tag.find('>') else {
            break;
        };
        let attributes = &tag[..end];
        rest = &tag[end..];
        let Some(path) = link_attribute(attributes, "href").and_then(fingerprinted_asset_path)
        else {
            continue;
        };
        let destination = match link_attribute(attributes, "rel") {
            Some("stylesheet") => "style",
            Some("preload") => match link_attribute(attributes, "as") {
                Some(destination @ ("script" | "fetch" | "style" | "font")) => destination,
                _ => continue,
            },
            _ => continue,
        };
        let mut link = format!("<{path}>; rel=preload; as={destination}");
        if let Some(media_type) = link_attribute(attributes, "type").filter(|value| {
            value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/+-.".contains(&byte))
        }) && destination != "style"
        {
            link.push_str(&format!("; type=\"{media_type}\""));
        }
        if attributes
            .split_ascii_whitespace()
            .any(|attribute| attribute == "crossorigin" || attribute.starts_with("crossorigin="))
        {
            link.push_str("; crossorigin");
        }
        links.push(link);
    }
    if links.is_empty() {
        return None;
    }
    HeaderValue::from_str(&links.join(", ")).ok()
}

/// 读取 `name="value"` 形式的属性值；dx 生成的入口文档只使用双引号。
fn link_attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    attributes.split_ascii_whitespace().find_map(|attribute| {
        let value = attribute.strip_prefix(name)?.strip_prefix("=\"")?;
        value.strip_suffix('"')
    })
}

/// 只接受 `assets/` 下的带指纹文件，并去掉 dx 写入的 `/./` 前缀。
fn fingerprinted_asset_path(href: &str) -> Option<String> {
    let relative = href.strip_prefix("/./").or_else(|| href.strip_prefix('/'))?;
    let name = relative.strip_prefix("assets/")?;
    (!name.is_empty()
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
    .then(|| format!("/{relative}"))
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
    Immutable,
    /// 稳定文件名的站点图标：允许浏览器与 CDN 保存一天，过期后按 ETag 再验证。
    Favicon,
}

impl CachePolicy {
    const fn value(self) -> &'static str {
        match self {
            Self::NoCache => "no-cache",
            Self::Immutable => "public, max-age=31536000, immutable",
            Self::Favicon => "public, max-age=86400",
        }
    }
}

// 只有本模块的精确静态响应可以签发此标记；API 的 Cache-Control 不能启用 CDN 缓存。
#[derive(Clone, Copy)]
struct StaticCachePolicy(CachePolicy);

/// 返回静态 handler 明确批准的 CDN 策略，其余响应按 `Cache-Control` 语义映射。
pub(crate) fn cdn_cache_policy(response: &Response) -> HeaderValue {
    if let Some(policy) = response.extensions().get::<StaticCachePolicy>() {
        return HeaderValue::from_static(policy.0.value());
    }
    shared_cache_policy(response.headers()).unwrap_or_else(|| HeaderValue::from_static("no-store"))
}

/// 动态响应只有可被共享缓存保存的公开读取才允许边缘保存：
/// `no-store`/`private` 永不放宽，`no-cache` 保留强制再验证，`public` 且带有效
/// `max-age`/`s-maxage` 的读取获得边缘缓存资格，其余（缺失头、无公开标记或无新鲜度）
/// 一律禁止边缘保存。
///
/// 边缘指令按 CDN 语义重写，不原样复制：新鲜度取 `s-maxage`（缺省取 `max-age`）并统一写成
/// `max-age`，`stale-while-revalidate` 与 `stale-if-error` 照搬。Cloudflare 在指令含
/// `s-maxage` 时不提供“过期后先回旧值、后台刷新”，原样复制会让这两个窗口在边缘失效。
fn shared_cache_policy(headers: &HeaderMap) -> Option<HeaderValue> {
    let mut eligible = None;
    let mut no_cache = false;
    for value in headers.get_all(header::CACHE_CONTROL) {
        let Ok(text) = value.to_str() else {
            return None;
        };
        let mut is_public = false;
        let mut malformed_freshness = false;
        let mut max_age = None;
        let mut shared_max_age = None;
        let mut stale_while_revalidate = None;
        let mut stale_if_error = None;
        for directive in text.split(',') {
            let directive = directive.trim();
            let (name, argument) = match directive.split_once('=') {
                Some((name, argument)) => (name.trim(), Some(argument.trim())),
                None => (directive, None),
            };
            if name.eq_ignore_ascii_case("no-store") || name.eq_ignore_ascii_case("private") {
                return None;
            }
            let seconds = argument.and_then(delta_seconds);
            if name.eq_ignore_ascii_case("no-cache") {
                no_cache = true;
            } else if name.eq_ignore_ascii_case("public") {
                is_public = true;
            } else if name.eq_ignore_ascii_case("max-age") {
                malformed_freshness |= seconds.is_none();
                max_age = seconds;
            } else if name.eq_ignore_ascii_case("s-maxage") {
                malformed_freshness |= seconds.is_none();
                shared_max_age = seconds;
            } else if name.eq_ignore_ascii_case("stale-while-revalidate") {
                stale_while_revalidate = seconds;
            } else if name.eq_ignore_ascii_case("stale-if-error") {
                stale_if_error = seconds;
            }
        }
        // 新鲜度写坏的响应不猜测意图，按无资格处理。
        let freshness = if malformed_freshness { None } else { shared_max_age.or(max_age) };
        if let (true, Some(freshness), None) = (is_public, freshness, &eligible) {
            let mut policy = format!("public, max-age={freshness}");
            for (name, seconds) in [
                ("stale-while-revalidate", stale_while_revalidate),
                ("stale-if-error", stale_if_error),
            ] {
                if let Some(seconds) = seconds {
                    policy.push_str(&format!(", {name}={seconds}"));
                }
            }
            eligible = HeaderValue::from_str(&policy).ok();
        }
    }
    if no_cache {
        return Some(HeaderValue::from_static("no-cache"));
    }
    eligible
}

/// 只接受非负十进制秒数；`max-age` 历史上可能带引号，两种写法都按有效新鲜度处理。
fn delta_seconds(value: &str) -> Option<&str> {
    let value = value.trim_matches('"');
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then_some(value)
}

fn exact_asset_cache(relative_path: &str) -> CachePolicy {
    if relative_path == INDEX_HTML {
        // 入口文档必须再验证，但允许浏览器/CDN 保存，避免每次导航都完整回源。
        return CachePolicy::NoCache;
    }
    // Dioxus 把 JS、WASM、CSS 等构建资源全部放进 assets/ 并加内容指纹；该目录之外的文件名稳定，
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
    // 不变量：请求带 `If-None-Match` 时只按内容 ETag 判定，`If-Modified-Since` 仅在它缺席时参与
    // （RFC 9110 §13.1.3）。`Last-Modified` 取自构建时的提交时间：同一提交重建或回滚到更早的
    // 提交后内容已变而时间没有前进，两者按“或”合并会让浏览器一直停在另一版入口文档上。
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
fn etag_not_modified(request_headers: &HeaderMap, response_headers: &HeaderMap) -> bool {
    let Some(etag) = response_headers.get(header::ETAG) else {
        return false;
    };
    request_headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .any(|candidates| etag_matches(candidates, etag))
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
    use super::{accepts_brotli, cdn_cache_policy, entry_preload_links, serve_public_asset};
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::response::{IntoResponse, Response};

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

    /// 取一次完整响应的再验证头；夹具文件必须同时带 ETag 与 Last-Modified。
    async fn validators(uri: &str) -> (HeaderValue, HeaderValue) {
        let response = serve_public_asset::<TestAssets>(request(uri, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers().get(header::ETAG).cloned().expect("etag");
        let last_modified =
            response.headers().get(header::LAST_MODIFIED).cloned().expect("last-modified");
        (etag, last_modified)
    }

    fn conditional(uri: &str, headers: &[(header::HeaderName, &HeaderValue)]) -> Request {
        let mut request = request(uri, None);
        for (name, value) in headers {
            request.headers_mut().append(name.clone(), (*value).clone());
        }
        request
    }

    #[tokio::test]
    async fn matching_etag_revalidates_without_body() {
        let (etag, _) = validators("/").await;
        let response =
            serve_public_asset::<TestAssets>(conditional("/", &[(header::IF_NONE_MATCH, &etag)]))
                .await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers().get(header::ETAG), Some(&etag));
        assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
        assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
        assert!(body_text(response).await.is_empty());
    }

    #[tokio::test]
    async fn weak_wildcard_and_repeated_validators_revalidate() {
        let (etag, _) = validators("/").await;
        let weak =
            HeaderValue::from_str(&format!("W/{}", etag.to_str().expect("etag text"))).unwrap();
        let wildcard = HeaderValue::from_static("*");
        let stale = HeaderValue::from_static("\"stale\"");
        for candidate in [&weak, &wildcard] {
            let response = serve_public_asset::<TestAssets>(conditional(
                "/",
                &[(header::IF_NONE_MATCH, candidate)],
            ))
            .await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        }
        // 列表字段允许拆成多行；任一行命中即可。
        let response = serve_public_asset::<TestAssets>(conditional(
            "/",
            &[(header::IF_NONE_MATCH, &stale), (header::IF_NONE_MATCH, &etag)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    /// 回归：同一提交重建或回滚后 `Last-Modified` 不前进，旧 ETag 不能被日期判成未修改。
    #[tokio::test]
    async fn stale_etag_is_not_rescued_by_if_modified_since() {
        let (_, last_modified) = validators("/").await;
        let stale = HeaderValue::from_static("\"stale\"");
        let response = serve_public_asset::<TestAssets>(conditional(
            "/",
            &[(header::IF_NONE_MATCH, &stale), (header::IF_MODIFIED_SINCE, &last_modified)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("fixture"));
    }

    #[tokio::test]
    async fn if_modified_since_alone_still_revalidates() {
        let (_, last_modified) = validators("/").await;
        let response = serve_public_asset::<TestAssets>(conditional(
            "/",
            &[(header::IF_MODIFIED_SINCE, &last_modified)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

        let epoch = HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT");
        let response = serve_public_asset::<TestAssets>(conditional(
            "/",
            &[(header::IF_MODIFIED_SINCE, &epoch)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::OK);
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

    fn dynamic_response(values: &[&'static str]) -> Response {
        let mut response = (StatusCode::OK, "ok").into_response();
        for value in values {
            response.headers_mut().append(header::CACHE_CONTROL, HeaderValue::from_static(value));
        }
        response
    }

    fn policy(values: &[&'static str]) -> String {
        cdn_cache_policy(&dynamic_response(values)).to_str().expect("ascii").to_owned()
    }

    #[test]
    fn cdn_policy_follows_dynamic_cache_control_semantics() {
        for (values, expected) in [
            (&["no-store"][..], "no-store"),
            (&["no-cache"], "no-cache"),
            // 边缘新鲜度取 s-maxage 并统一写成 max-age；两个 stale 窗口照搬。
            (
                &["public, max-age=300, s-maxage=300, stale-while-revalidate=60"],
                "public, max-age=300, stale-while-revalidate=60",
            ),
            (&["public, max-age=10, s-maxage=600"], "public, max-age=600"),
            (&["public, s-maxage=30"], "public, max-age=30"),
            (
                &["public, max-age=\"60\", stale-if-error=3600"],
                "public, max-age=60, stale-if-error=3600",
            ),
            // 进行中场次的目录读取只给浏览器：边缘不得保存。
            (&["private, max-age=2, stale-while-revalidate=5"], "no-store"),
            // 缺少 public、缺少新鲜度或非法的 max-age 都不能获得边缘缓存资格。
            (&["max-age=30"], "no-store"),
            (&["public"], "no-store"),
            (&["public, max-age=abc"], "no-store"),
            (&["public, max-age=30, s-maxage=abc"], "no-store"),
            (&["private, max-age=30"], "no-store"),
            (&[], "no-store"),
        ] {
            assert_eq!(policy(values), expected, "values={values:?}");
        }
    }

    #[test]
    fn cdn_policy_never_relaxes_no_store_across_headers() {
        assert_eq!(policy(&["public, max-age=30", "no-store"]), "no-store");
        assert_eq!(policy(&["public, max-age=30", "private"]), "no-store");
        // no-cache 比可缓存的新鲜度更严格，边缘必须再验证。
        assert_eq!(policy(&["public, max-age=30", "no-cache"]), "no-cache");
    }

    #[test]
    fn entry_document_links_become_preload_headers() {
        // 与 dx 生成的入口文档同构：样式表、WASM 与脚本预加载、站点图标和模块脚本。
        let document = concat!(
            "<link rel=\"icon\" href=\"/favicon.ico\">",
            "<link rel=\"stylesheet\" href=\"/./assets/public-dxh1.css\" type=\"text/css\">",
            "<link rel=\"preload\" as=\"fetch\" type=\"application/wasm\" ",
            "href=\"/./assets/app_bg-dxh2.wasm\" crossorigin>",
            "<link rel=\"preload\" as=\"script\" href=\"/./assets/app-dxh3.js\" crossorigin>",
            "<script type=\"module\" async src=\"/./assets/app-dxh3.js\"></script>",
        );
        assert_eq!(
            entry_preload_links(document.as_bytes()).expect("links").to_str().expect("ascii"),
            concat!(
                "</assets/public-dxh1.css>; rel=preload; as=style, ",
                "</assets/app_bg-dxh2.wasm>; rel=preload; as=fetch; ",
                "type=\"application/wasm\"; crossorigin, ",
                "</assets/app-dxh3.js>; rel=preload; as=script; crossorigin",
            )
        );
    }

    #[test]
    fn entry_document_links_ignore_everything_outside_fingerprinted_assets() {
        for document in [
            // 站外地址、assets 之外的路径、含头注入字符的路径与未知预加载类型都不转写。
            "<link rel=\"stylesheet\" href=\"https://cdn.example/x.css\">",
            "<link rel=\"stylesheet\" href=\"/other/x.css\">",
            "<link rel=\"stylesheet\" href=\"/assets/x.css>;\">",
            "<link rel=\"preload\" as=\"document\" href=\"/assets/x.html\">",
            "<link rel=\"icon\" href=\"/assets/favicon-dxh1.svg\">",
            "<link rel=\"stylesheet\" href=\"/assets/x.css\"",
            "",
        ] {
            assert!(entry_preload_links(document.as_bytes()).is_none(), "document={document}");
        }
    }

    #[tokio::test]
    async fn cdn_policy_prefers_static_cache_extension() {
        let response = serve_public_asset::<TestAssets>(request("/assets/app.css", None)).await;
        assert_eq!(
            cdn_cache_policy(&response).to_str().expect("ascii"),
            "public, max-age=31536000, immutable"
        );
    }
}

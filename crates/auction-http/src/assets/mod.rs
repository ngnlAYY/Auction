//! 前端静态产物内嵌、显式浏览器路由与 404 分派。
//! 两个装配根各自提供 `RustEmbed` 类型，资源目录相对其 `CARGO_MANIFEST_DIR`，互不引用。
//! 必须保留 `allow_missing = true`，让未运行 `dx bundle` 的工作区仍可编译；缺失页面返回 503。
//! release 内嵌资源，dev 从磁盘读取；本 crate 仅拥有通用响应和 surface 路由规则。

mod cache;
mod negotiation;
mod response;
mod routes;
#[cfg(test)]
mod wire_tests;

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    response::Response,
};

use self::cache::{CachePolicy, StaticCachePolicy, exact_asset_cache, favicon_file};
use self::negotiation::{
    accepts_brotli, entry_preload_links, exact_asset_path, reserved_json_path, static_method,
};
use self::response::{
    asset_response, assets_missing, canonical_page_redirect, json_not_found, method_not_allowed,
    not_found,
};
use self::routes::Surface;

pub(crate) use cache::cdn_cache_policy;

/// HTTP-date 的首选格式 IMF-fixdate（RFC 9110 §5.6.7）；响应只生成这一种。
const IMF_FIXDATE: &str = "%a, %d %b %Y %H:%M:%S GMT";
/// 接收方还须接受两种过时格式：RFC 850 与 asctime。
const OBSOLETE_HTTP_DATES: [&str; 2] = ["%A, %d-%b-%y %H:%M:%S GMT", "%a %b %e %H:%M:%S %Y"];

/// SPA 入口文档；只有 routes 模块批准的浏览器路径可以回退到它。
const INDEX_HTML: &str = "index.html";
const ASSETS_MISSING_BODY: &str =
    "web assets are not included in this binary; run `mise run build-release`";
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

    if !setup_only && let Some(response) = canonical_page_redirect(surface, method, request.uri()) {
        return response;
    }
    not_found(surface, method, request_headers)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Version, header};
    use axum::response::{IntoResponse, Response};
    use rust_embed::{EmbeddedFile, Metadata};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use super::cache::{CachePolicy, cdn_cache_policy, exact_asset_cache};
    use super::negotiation::{accepts_brotli, entry_preload_links};
    use super::response::{asset_response, parse_http_date};
    use super::routes::Surface;
    use super::{IMF_FIXDATE, serve_admin_asset, serve_public_asset};

    #[derive(rust_embed::Embed)]
    #[folder = "tests/assets-fixture"]
    pub(super) struct TestAssets;

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

    #[test]
    fn http_dates_format_as_imf_fixdate_and_parse_all_three_rfc_9110_forms() {
        let expected = auction_time::Timestamp::from_timestamp(784_111_777, 0).unwrap();
        assert_eq!(expected.strftime(IMF_FIXDATE).to_string(), "Sun, 06 Nov 1994 08:49:37 GMT");
        for value in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(parse_http_date(value), Some(expected), "{value}");
        }
        for value in [
            "",
            "784111777",
            "Sun, 06 Nov 1994 08:49:37",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 08:49:37 +0000",
            "Mon, 06 Nov 1994 08:49:37 GMT",
            "Sun, 31 Feb 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "1994-11-06T08:49:37Z",
        ] {
            assert_eq!(parse_http_date(value), None, "{value}");
        }
    }

    async fn etag(uri: &str) -> HeaderValue {
        let response = serve_public_asset::<TestAssets>(request(uri, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        response.headers().get(header::ETAG).cloned().expect("etag")
    }

    fn embedded_at_build_second(data: &'static [u8], mime: &'static str) -> EmbeddedFile {
        EmbeddedFile {
            data: Cow::Borrowed(data),
            metadata: Metadata::__rust_embed_new(
                Sha256::digest(data).into(),
                Some(784_111_777),
                None,
                mime,
            ),
        }
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
        let app = Router::new().fallback(serve_public_asset::<TestAssets>);
        for (uri, encoding) in [("/", None), ("/assets/app.css", Some("br"))] {
            let mut request = request(uri, encoding);
            *request.version_mut() = Version::HTTP_2;
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut revalidate =
                conditional(uri, &[(header::IF_NONE_MATCH, &response.headers()[header::ETAG])]);
            *revalidate.version_mut() = Version::HTTP_2;
            if let Some(encoding) = encoding {
                revalidate
                    .headers_mut()
                    .insert(header::ACCEPT_ENCODING, HeaderValue::from_static(encoding));
            }
            let length = response.headers()[header::CONTENT_LENGTH].to_str().unwrap();
            let length: usize = length.parse().unwrap();
            assert!(length > 0);
            let revalidated = app.clone().oneshot(revalidate).await.unwrap();
            assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
            for header in
                [header::ETAG, header::CACHE_CONTROL, header::VARY, header::CONTENT_LENGTH]
            {
                assert_eq!(revalidated.headers().get(&header), response.headers().get(&header));
            }
            assert!(body_text(revalidated).await.is_empty());
            assert_eq!(body_text(response).await.len(), length);
        }
    }

    #[tokio::test]
    async fn weak_wildcard_and_repeated_validators_revalidate() {
        let etag = etag("/").await;
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

    /// 构建秒值相同也必须返回当前入口；同内容仍可按 ETag 再验证。
    #[tokio::test]
    async fn entry_rebuild_in_same_second_requires_current_etag() {
        let respond = |document, headers: &HeaderMap| {
            asset_response(
                embedded_at_build_second(document, "text/html"),
                &Method::GET,
                headers,
                CachePolicy::NoCache,
                "text/html",
                None,
            )
        };
        let mut headers = HeaderMap::new();
        let before = respond(b"first entry", &headers);
        headers.insert(
            header::IF_MODIFIED_SINCE,
            HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
        );
        let after = respond(b"second entry", &headers);
        assert_eq!(after.status(), StatusCode::OK);
        assert!(before.headers().get(header::LAST_MODIFIED).is_none());
        assert!(after.headers().get(header::LAST_MODIFIED).is_none());
        let current_etag = after.headers()[header::ETAG].clone();
        let old_etag = before.headers()[header::ETAG].clone();
        assert_ne!(current_etag, old_etag);
        assert_eq!(body_text(after).await, "second entry");

        headers.insert(header::IF_NONE_MATCH, old_etag);
        assert_eq!(respond(b"second entry", &headers).status(), StatusCode::OK);
        headers.insert(header::IF_NONE_MATCH, current_etag);
        assert_eq!(respond(b"second entry", &headers).status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn only_fingerprinted_assets_can_revalidate_by_date() {
        let mut headers = HeaderMap::new();
        let date = HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT");
        headers.insert(header::IF_MODIFIED_SINCE, date.clone());
        for (policy, status, last_modified) in [
            (
                exact_asset_cache("assets/style-dxh01234567.css"),
                StatusCode::NOT_MODIFIED,
                Some(&date),
            ),
            (exact_asset_cache("assets/app.css"), StatusCode::OK, None),
            (exact_asset_cache("robots.txt"), StatusCode::OK, None),
            (CachePolicy::Favicon, StatusCode::OK, None),
        ] {
            let response = asset_response(
                embedded_at_build_second(b"asset", "text/css"),
                &Method::GET,
                &headers,
                policy,
                "text/css",
                None,
            );
            assert_eq!(response.status(), status);
            assert_eq!(response.headers().get(header::LAST_MODIFIED), last_modified);
        }
    }

    /// CDN 只转发旧日期时，回滚也必须返回当前入口。
    #[tokio::test]
    async fn later_if_modified_since_after_rollback_returns_the_current_entry() {
        let later = HeaderValue::from_static("Fri, 31 Dec 2100 23:59:59 GMT");
        let response = serve_public_asset::<TestAssets>(conditional(
            "/",
            &[(header::IF_MODIFIED_SINCE, &later)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("fixture"));
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

    #[tokio::test]
    async fn trailing_slash_redirects_to_each_surfaces_canonical_page() {
        for (surface, method, source, location) in [
            (Surface::Public, Method::GET, "/rules/?lang=ja&x=%2f&x=", "/rules?lang=ja&x=%2f&x="),
            (Surface::Public, Method::HEAD, "/me/", "/me"),
            (Surface::Public, Method::GET, "/lots/example/?", "/lots/example?"),
            (Surface::Admin, Method::GET, "/dashboard/", "/dashboard"),
            (
                Surface::Admin,
                Method::HEAD,
                "/auctions/example/lots/?page=2",
                "/auctions/example/lots?page=2",
            ),
        ] {
            let response = page_request(surface, method, source).await;
            assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{source}");
            assert_eq!(response.headers()[header::LOCATION], location);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            assert_eq!(cdn_cache_policy(&response), "no-cache");
            assert!(body_text(response).await.is_empty());
        }
    }

    #[tokio::test]
    async fn only_page_get_and_head_with_one_trailing_slash_are_redirected() {
        for surface in [Surface::Public, Surface::Admin] {
            for (method, path, status) in [
                (Method::GET, "/", StatusCode::OK),
                (Method::GET, "/assets/app.css", StatusCode::OK),
                (Method::GET, "/assets/app.css/", StatusCode::NOT_FOUND),
                (Method::GET, "//", StatusCode::NOT_FOUND),
                (Method::GET, "//host/", StatusCode::NOT_FOUND),
                (Method::GET, "/rules//", StatusCode::NOT_FOUND),
                (Method::GET, "/lots//example/", StatusCode::NOT_FOUND),
                (Method::GET, "/lots/../", StatusCode::NOT_FOUND),
                (Method::GET, "/lots/%2F/", StatusCode::NOT_FOUND),
                (Method::GET, "/deliveries/%2f/", StatusCode::NOT_FOUND),
                (Method::GET, "/unknown/", StatusCode::NOT_FOUND),
                (Method::GET, "/api/v1/unknown/?lang=ja", StatusCode::NOT_FOUND),
                (Method::HEAD, "/health/ready/", StatusCode::NOT_FOUND),
                (Method::POST, "/rules/", StatusCode::NOT_FOUND),
            ] {
                let response = page_request(surface, method, path).await;
                assert_eq!(response.status(), status, "{path}");
                assert!(!response.headers().contains_key(header::LOCATION), "{path}");
                if path.starts_with("/api/") || path.starts_with("/health/") {
                    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
                }
            }
        }
        for (surface, path) in [(Surface::Public, "/dashboard/"), (Surface::Admin, "/me/")] {
            let response = page_request(surface, Method::GET, path).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(!response.headers().contains_key(header::LOCATION));
        }
    }

    async fn page_request(surface: Surface, method: Method, path: &str) -> Response {
        let mut request = request(path, None);
        *request.method_mut() = method;
        match surface {
            Surface::Public => serve_public_asset::<TestAssets>(request).await,
            Surface::Admin => serve_admin_asset::<TestAssets>(request).await,
        }
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

    /// 处理器使用的两个公开缓存档位在边缘的实际新鲜度；改动档位时这里同步失败。
    #[test]
    fn public_cache_tiers_keep_their_edge_freshness() {
        use crate::middleware::response_cache::{
            LOW_FREQUENCY_CACHE_CONTROL, SETTLED_CACHE_CONTROL,
        };

        // 低频内容：5 分钟新鲜，之后一小时内先回旧副本、后台刷新。
        assert_eq!(
            policy(&[LOW_FREQUENCY_CACHE_CONTROL]),
            "public, max-age=300, stale-while-revalidate=3600"
        );
        // 已结算内容：一天新鲜，之后七天内先回旧副本、后台刷新。
        assert_eq!(
            policy(&[SETTLED_CACHE_CONTROL]),
            "public, max-age=86400, stale-while-revalidate=604800"
        );
    }

    #[test]
    fn cdn_policy_follows_dynamic_cache_control_semantics() {
        for (values, expected) in [
            (&["no-store"][..], "no-store"),
            (&["no-cache"], "no-cache"),
            // 边缘新鲜度取 s-maxage 并统一写成 max-age；两个 stale 窗口照搬。
            (
                &["public, max-age=300, s-maxage=300, stale-while-revalidate=3600"],
                "public, max-age=300, stale-while-revalidate=3600",
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
        assert_eq!(cdn_cache_policy(&response), "no-cache");

        let fingerprinted = asset_response(
            embedded_at_build_second(b"body{color:red}", "text/css"),
            &Method::GET,
            &HeaderMap::new(),
            exact_asset_cache("assets/app-dxh0123456789abcdef.css"),
            "text/css",
            None,
        );
        assert_eq!(cdn_cache_policy(&fingerprinted), "public, max-age=31536000, immutable");
    }
}

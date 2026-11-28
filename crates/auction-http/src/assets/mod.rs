//! 前端静态产物内嵌、显式浏览器路由与 404 分派。
//! 两个装配根各自提供 `RustEmbed` 类型，资源目录相对其 `CARGO_MANIFEST_DIR`，互不引用。
//! 必须保留 `allow_missing = true`，让未运行 `dx bundle` 的工作区仍可编译；缺失页面返回 503。
//! release 内嵌资源，dev 从磁盘读取；本 crate 仅拥有通用响应和 surface 路由规则。

mod cache;
mod independent;
mod mount;
mod negotiation;
mod pages;
mod response;
mod routes;
mod source;
#[cfg(test)]
mod wire_tests;

use std::sync::Arc;

use axum::{extract::Request, http::HeaderValue, response::Response};

use self::cache::{CachePolicy, StaticCachePolicy, exact_asset_cache};
use self::independent::{EntryClass, entry_class};
use self::negotiation::{entry_preload_links, exact_asset_path, reserved_json_path, static_method};
use self::response::{
    assets_missing, canonical_page_redirect, json_not_found, method_not_allowed,
    negotiated_asset_response, not_found,
};
use self::routes::Surface;
use crate::WebScriptPolicy;

pub use mount::WebMountError;
pub use pages::SurfacePages;

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

/// 处理公开端的静态产物与显式浏览器路由；缓存判定按本 surface 的实际技术栈。
#[cfg(test)]
pub(crate) async fn serve_public_asset<A: rust_embed::RustEmbed + 'static>(
    request: Request,
    web_script_policy: WebScriptPolicy,
) -> Response {
    serve::<A>(Surface::Public, request, false, web_script_policy)
}

/// 处理管理端的静态产物与显式浏览器路由；缓存判定按本 surface 的实际技术栈。
#[cfg(test)]
pub(crate) async fn serve_admin_asset<A: rust_embed::RustEmbed + 'static>(
    request: Request,
    web_script_policy: WebScriptPolicy,
) -> Response {
    serve::<A>(Surface::Admin, request, false, web_script_policy)
}

/// 公开端的页面分派：默认入口与独立入口各自绑定构建号与脚本策略。
pub(crate) async fn serve_public_pages(pages: Arc<SurfacePages>, request: Request) -> Response {
    serve_pages(Surface::Public, request, false, &pages)
}

/// 管理端的页面分派；首次设置页不经此入口。
pub(crate) async fn serve_admin_pages(pages: Arc<SurfacePages>, request: Request) -> Response {
    serve_pages(Surface::Admin, request, false, &pages)
}

/// 只向一次性初始化进程提供 Admin 的 `/setup` 页面和必要静态资源。
///
/// 首次设置页属于 Admin surface，CSP、入口 `Link` 与静态资源指纹判定都必须使用装配根传入的
/// 同一策略，否则 Solid 产物会拿到 Dioxus 的缓存与预加载规则。永远只用内嵌页面，不经挂载。
pub async fn serve_admin_setup_asset<A: rust_embed::RustEmbed + 'static>(
    request: Request,
    web_script_policy: WebScriptPolicy,
) -> Response {
    let mut response = serve::<A>(Surface::Admin, request, true, web_script_policy);
    // 首次设置不属于正式 CDN 表面：契约要求全部响应（页面、图标与静态资源）都禁止缓存，
    // 因此浏览器 `Cache-Control` 也一并覆盖，保留的静态标记不能放宽。
    response.extensions_mut().remove::<StaticCachePolicy>();
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert("cdn-cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("cloudflare-cdn-cache-control", HeaderValue::from_static("no-store"));
    response
}

/// 纯内嵌分派：与挂载功能出现之前的行为字节一致。
fn serve<A: rust_embed::RustEmbed + 'static>(
    surface: Surface,
    request: Request,
    setup_only: bool,
    web_script_policy: WebScriptPolicy,
) -> Response {
    serve_pages(surface, request, setup_only, &SurfacePages::embedded::<A>(web_script_policy))
}

/// 按“保留路径 → 精确静态文件 → 批准浏览器路由 → 404”顺序封闭分派请求。
///
/// 不变量：页面字节、构建号与脚本策略来自同一个 `PageBuild`；批准的 `/consignment` 前缀路由
/// 取独立入口，其余页面取默认入口。首次设置只会传入纯内嵌的页面集合。
fn serve_pages(
    surface: Surface,
    request: Request,
    setup_only: bool,
    pages: &SurfacePages,
) -> Response {
    let method = request.method();
    let path = request.uri().path();
    let request_headers = request.headers();

    if reserved_json_path(path) {
        return json_not_found(request_headers);
    }

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
        return match pages.find_favicon() {
            Some((build, relative_path, file)) => negotiated_asset_response(
                build,
                file,
                &relative_path,
                &request,
                CachePolicy::Favicon,
                None,
            ),
            None => not_found(surface, method, request_headers),
        };
    }

    if let Some(relative_path) = exact_asset_path(path)
        && let Some((build, file)) = pages.find(relative_path)
    {
        if !static_method(method) {
            return method_not_allowed(request_headers);
        }
        return negotiated_asset_response(
            build,
            file,
            relative_path,
            &request,
            exact_asset_cache(relative_path, build.script_policy()),
            None,
        );
    }

    if (setup_only && path == "/setup") || (!setup_only && routes::browser_route(surface, path)) {
        if !static_method(method) {
            return method_not_allowed(request_headers);
        }
        let class = if setup_only { EntryClass::Default } else { entry_class(surface, path) };
        let build = pages.entry(class);
        return match build.get(INDEX_HTML) {
            Some(index) => {
                let preload_links = entry_preload_links(&index.data, build.script_policy());
                negotiated_asset_response(
                    build,
                    index,
                    INDEX_HTML,
                    &request,
                    CachePolicy::NoCache,
                    preload_links,
                )
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
    use super::negotiation::entry_preload_links;
    use super::response::{asset_response, parse_http_date};
    use super::routes::Surface;
    use super::{
        IMF_FIXDATE, serve, serve_admin_asset, serve_admin_setup_asset, serve_public_asset,
    };
    use crate::{
        AdminApiState, PublicApiState, WebScriptPolicy, admin_router_with_web_script_policy,
        public_router_with_web_script_policy,
    };

    #[derive(rust_embed::Embed)]
    #[folder = "tests/assets-fixture"]
    pub(super) struct TestAssets;

    async fn serve_public_fixture(request: Request) -> Response {
        serve_public_asset::<TestAssets>(request, WebScriptPolicy::Dioxus).await
    }

    async fn serve_admin_fixture(request: Request) -> Response {
        serve_admin_asset::<TestAssets>(request, WebScriptPolicy::Dioxus).await
    }

    const DIOXUS_INDEX: &[u8] = concat!(
        "<link rel=\"stylesheet\" href=\"/./assets/public-dxh1.css\" type=\"text/css\">",
        "<link rel=\"preload\" as=\"fetch\" type=\"application/wasm\" ",
        "href=\"/./assets/app_bg-dxh2.wasm\" crossorigin>",
        "<link rel=\"preload\" as=\"script\" href=\"/./assets/app-dxh3.js\" crossorigin>",
        "<script type=\"module\" async src=\"/./assets/app-dxh3.js\"></script>",
    )
    .as_bytes();
    const SOLID_INDEX: &[u8] = concat!(
        "<link rel=\"icon\" href=\"/favicon.ico\">\n",
        "<script data-cfasync=\"false\" type=\"module\" crossorigin ",
        "src=\"/assets/index-B2UikXgQ.js\"></script>\n",
        "<link rel=\"modulepreload\" crossorigin ",
        "href=\"/assets/rolldown-runtime-DK3Fl9T5.js\">\n",
        "<link rel=\"stylesheet\" crossorigin href=\"/assets/index-ZX7NKpK-.css\">\n",
    )
    .as_bytes();
    const DIOXUS_LINK_INDEX: &[u8] = concat!(
        "<script type=\"module\" src=\"/assets/script-only-dxh12345678.js\"></script>",
        "<link rel=\"preload\" as=\"fetch\" href=\"/assets/shared-dxh12345678.js\">",
        "<script type=\"module\" src=\"/assets/entry-dxh12345678.js\"></script>",
        "<link rel=\"preload\" as=\"script\" href=\"/assets/shared-dxh12345678.js\">",
        "<link rel=\"preload\" as=\"script\" href=\"/assets/entry-dxh12345678.js\">",
    )
    .as_bytes();
    const SOLID_FAVICON: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

    fn fixture_file(data: &'static [u8], mime: &'static str) -> EmbeddedFile {
        EmbeddedFile {
            data: Cow::Borrowed(data),
            metadata: Metadata::__rust_embed_new(Sha256::digest(data).into(), None, None, mime),
        }
    }

    struct DioxusAssets;

    impl rust_embed::RustEmbed for DioxusAssets {
        fn get(path: &str) -> Option<EmbeddedFile> {
            Some(fixture_file(
                match path {
                    "index.html" => DIOXUS_INDEX,
                    "assets/public-dxh1.css" | "assets/index-B2UikXgQ.js" | "assets/app.css" => {
                        b"body{color:red}"
                    }
                    "assets/app_bg-dxh2.wasm"
                    | "assets/app-dxh3.js"
                    | "assets/app-dxh01234567.js" => b"asset",
                    "assets/favicon-dxh01234567.svg" => SOLID_FAVICON,
                    _ => return None,
                },
                if path.ends_with(".html") {
                    "text/html"
                } else if path.ends_with(".css") {
                    "text/css"
                } else if path.ends_with(".svg") {
                    "image/svg+xml"
                } else {
                    "application/octet-stream"
                },
            ))
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> + 'static {
            [
                "index.html",
                "assets/public-dxh1.css",
                "assets/app_bg-dxh2.wasm",
                "assets/app-dxh3.js",
                "assets/app-dxh01234567.js",
                "assets/index-B2UikXgQ.js",
                "assets/app.css",
                "assets/favicon-dxh01234567.svg",
            ]
            .into_iter()
            .map(Cow::Borrowed)
        }
    }

    struct DioxusLinkAssets;

    impl rust_embed::RustEmbed for DioxusLinkAssets {
        fn get(path: &str) -> Option<EmbeddedFile> {
            (path == "index.html").then(|| fixture_file(DIOXUS_LINK_INDEX, "text/html"))
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> + 'static {
            ["index.html"].into_iter().map(Cow::Borrowed)
        }
    }

    struct SolidAssets;

    impl rust_embed::RustEmbed for SolidAssets {
        fn get(path: &str) -> Option<EmbeddedFile> {
            Some(fixture_file(
                match path {
                    "index.html" => SOLID_INDEX,
                    "assets/index-B2UikXgQ.js"
                    | "assets/rolldown-runtime-DK3Fl9T5.js"
                    | "assets/app-dxh01234567.js"
                    | "assets/chunks/index-B2UikXgQ.js"
                    | "assets/chunks/index.js" => b"asset",
                    "assets/index-ZX7NKpK-.css" | "assets/app.css" => b"body{color:red}",
                    "assets/favicon-B2UikXgQ.svg" => SOLID_FAVICON,
                    _ => return None,
                },
                if path.ends_with(".html") {
                    "text/html"
                } else if path.ends_with(".css") {
                    "text/css"
                } else if path.ends_with(".svg") {
                    "image/svg+xml"
                } else {
                    "application/octet-stream"
                },
            ))
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> + 'static {
            [
                "index.html",
                "assets/index-B2UikXgQ.js",
                "assets/rolldown-runtime-DK3Fl9T5.js",
                "assets/index-ZX7NKpK-.css",
                "assets/app-dxh01234567.js",
                "assets/chunks/index-B2UikXgQ.js",
                "assets/chunks/index.js",
                "assets/app.css",
                "assets/favicon-B2UikXgQ.svg",
            ]
            .into_iter()
            .map(Cow::Borrowed)
        }
    }

    struct NoAssets;

    impl rust_embed::RustEmbed for NoAssets {
        fn get(_path: &str) -> Option<EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            std::iter::empty()
        }
    }

    fn public_app<A: rust_embed::RustEmbed + Send + Sync + 'static>(
        web_script_policy: WebScriptPolicy,
    ) -> Router {
        let state = PublicApiState::default();
        let boundary = state.http_boundary();
        let gate = state.anonymous_read_gate();
        public_router_with_web_script_policy::<A>(
            env!("CARGO_PKG_VERSION"),
            boundary,
            gate,
            web_script_policy,
        )
        .with_state(state)
    }

    fn admin_app<A: rust_embed::RustEmbed + Send + Sync + 'static>(
        web_script_policy: WebScriptPolicy,
    ) -> Router {
        let state = AdminApiState::default();
        let boundary = state.http_boundary();
        admin_router_with_web_script_policy::<A>(
            env!("CARGO_PKG_VERSION"),
            boundary,
            web_script_policy,
        )
        .with_state(state)
    }

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

    const PUBLIC_CONSIGNMENT_PAGES: [&str; 5] = [
        "/consignment",
        "/consignment/new",
        "/consignment/me",
        "/consignment/me/0f8fad5b-d9cb-469f-a165-70867728950e",
        "/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e",
    ];
    const ADMIN_CONSIGNMENT_PAGES: [&str; 2] =
        ["/consignment", "/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e"];
    const ADMIN_CONSIGNMENT_API: [(Method, &str); 3] = [
        (Method::GET, "/api/v1/admin/consignment/listings"),
        (Method::GET, "/api/v1/admin/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e"),
        (
            Method::POST,
            "/api/v1/admin/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e/delist",
        ),
    ];

    #[tokio::test]
    async fn consignment_browser_paths_serve_each_embedded_entry_or_report_missing_assets() {
        for (surface, paths) in [
            (Surface::Public, PUBLIC_CONSIGNMENT_PAGES.as_slice()),
            (Surface::Admin, ADMIN_CONSIGNMENT_PAGES.as_slice()),
        ] {
            for &path in paths {
                let solid = serve::<SolidAssets>(
                    surface,
                    request(path, None),
                    false,
                    WebScriptPolicy::Solid,
                );
                assert_eq!(solid.status(), StatusCode::OK, "{path}");
                assert_eq!(solid.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
                assert_eq!(body_text(solid).await.as_bytes(), SOLID_INDEX, "{path}");

                let dioxus = serve::<DioxusAssets>(
                    surface,
                    request(path, None),
                    false,
                    WebScriptPolicy::Dioxus,
                );
                assert_eq!(dioxus.status(), StatusCode::OK, "{path}");
                assert_eq!(body_text(dioxus).await.as_bytes(), DIOXUS_INDEX, "{path}");

                let missing =
                    serve::<NoAssets>(surface, request(path, None), false, WebScriptPolicy::Solid);
                assert_eq!(missing.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
            }
        }
    }

    #[tokio::test]
    async fn consignment_browser_paths_reject_noncanonical_ids_and_other_surfaces() {
        for surface in [Surface::Public, Surface::Admin] {
            for prefix in ["/consignment/listings/", "/consignment/me/"] {
                for invalid in [
                    "",
                    "invalid",
                    "00000000-0000-0000-0000-000000000000",
                    "0F8FAD5B-D9CB-469F-A165-70867728950E",
                    "0f8fad5bd9cb469fa16570867728950e",
                    "%30f8fad5b-d9cb-469f-a165-70867728950e",
                    "0f8fad5b-d9cb-469f-a165-70867728950e/extra",
                ] {
                    // 已有页面尾斜杠规则另作验证，空后缀只测试没有对应精确路径的前缀。
                    if invalid.is_empty() && prefix == "/consignment/me/" {
                        continue;
                    }
                    let path = format!("{prefix}{invalid}");
                    let response = serve::<SolidAssets>(
                        surface,
                        request(&path, None),
                        false,
                        WebScriptPolicy::Solid,
                    );
                    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
                }
            }
            let response = serve::<SolidAssets>(
                surface,
                request("/Consignment", None),
                false,
                WebScriptPolicy::Solid,
            );
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        for path in ["/consignment/new", "/consignment/me", PUBLIC_CONSIGNMENT_PAGES[3]] {
            let response =
                serve_admin_asset::<SolidAssets>(request(path, None), WebScriptPolicy::Solid).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn setup_asset_fallback_excludes_formal_admin_consignment() {
        let setup =
            serve_admin_setup_asset::<SolidAssets>(request("/setup", None), WebScriptPolicy::Solid)
                .await;
        assert_eq!(setup.status(), StatusCode::OK);
        // 契约：首次设置的全部响应禁止缓存，包括页面、图标与带指纹静态资源。
        for path in ["/setup", "/favicon.ico", "/assets/index-B2UikXgQ.js"] {
            let cached =
                serve_admin_setup_asset::<SolidAssets>(request(path, None), WebScriptPolicy::Solid)
                    .await;
            assert_eq!(cached.status(), StatusCode::OK, "{path}");
            assert_eq!(cached.headers()[header::CACHE_CONTROL], "no-store", "{path}");
        }
        for path in ADMIN_CONSIGNMENT_PAGES {
            let page =
                serve_admin_setup_asset::<SolidAssets>(request(path, None), WebScriptPolicy::Solid)
                    .await;
            assert_eq!(page.status(), StatusCode::NOT_FOUND, "{path}");
        }
        for (method, path) in ADMIN_CONSIGNMENT_API {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .expect("设置入口请求");
            let api = serve_admin_setup_asset::<SolidAssets>(request, WebScriptPolicy::Solid).await;
            assert_eq!(api.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(api.headers()[header::CONTENT_TYPE], "application/json", "{path}");
        }
    }

    #[tokio::test]
    async fn brotli_copy_is_served_when_accepted() {
        let response = serve_public_fixture(request("/assets/app.css", Some("gzip, br"))).await;
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
            let response = serve_public_fixture(request("/assets/app.css", accept)).await;
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
    async fn wildcard_selects_brotli_when_identity_is_excluded() {
        let wildcard = HeaderValue::from_static("*;q=1");
        let identity = HeaderValue::from_static("identity;q=0");
        let response = serve_public_fixture(conditional(
            "/assets/app.css",
            &[(header::ACCEPT_ENCODING, &wildcard), (header::ACCEPT_ENCODING, &identity)],
        ))
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING),
            Some(&HeaderValue::from_static("br"))
        );
        assert_eq!(body_text(response).await, "compressed-css\n");
    }

    #[tokio::test]
    async fn existing_assets_with_no_acceptable_representation_return_406() {
        for (path, accept) in [
            ("/assets/app.css", "*;q=1, br;q=0, identity;q=0"),
            ("/assets/app.js", "br, identity;q=0"),
            ("/", "*;q=0"),
        ] {
            let response = serve_public_fixture(request(path, Some(accept))).await;
            assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{path}");
            assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(cdn_cache_policy(&response), "no-store");
            assert!(!response.headers().contains_key(header::ETAG));
            assert!(body_text(response).await.is_empty());
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
        let response = serve_public_fixture(request(uri, None)).await;
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
        let app = Router::new().fallback(serve_public_fixture);
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
            let response =
                serve_public_fixture(conditional("/", &[(header::IF_NONE_MATCH, candidate)])).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        }
        // 列表字段允许拆成多行；任一行命中即可。
        let response = serve_public_fixture(conditional(
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
                embedded_at_build_second(document, "text/html").into(),
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
                exact_asset_cache("assets/style-dxh01234567.css", WebScriptPolicy::Dioxus),
                StatusCode::NOT_MODIFIED,
                Some(&date),
            ),
            (exact_asset_cache("assets/app.css", WebScriptPolicy::Dioxus), StatusCode::OK, None),
            (exact_asset_cache("robots.txt", WebScriptPolicy::Dioxus), StatusCode::OK, None),
            (CachePolicy::Favicon, StatusCode::OK, None),
        ] {
            let response = asset_response(
                embedded_at_build_second(b"asset", "text/css").into(),
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

    #[test]
    fn solid_immutable_names_match_the_build_fingerprint_pattern() {
        const IMMUTABLE: &str = "public, max-age=31536000, immutable";
        for (path, immutable) in [
            ("assets/index-B2UikXgQ.js", true),
            ("assets/rolldown-runtime-DK3Fl9T5.js", true),
            ("assets/a-b_c-D2E3F4G5.css", true),
            ("assets/a-________.js", true),
            ("assets/chunks/index-B2UikXgQ.js", true),
            ("assets/index-B2UikXg.js", false),
            ("assets/-B2UikXgQ.js", false),
            ("assets/index-B2UikXgQ", false),
            ("assets/index-B2UikXgQ!.js", false),
            ("assets/index-B2UikXgQ.js.map", false),
            ("assets/chunks/index.js", false),
            ("index-B2UikXgQ.js", false),
            ("assets/app-dxh01234567.js", false),
        ] {
            assert_eq!(
                exact_asset_cache(path, WebScriptPolicy::Solid).value() == IMMUTABLE,
                immutable,
                "{path}"
            );
        }
    }

    /// CDN 只转发旧日期时，回滚也必须返回当前入口。
    #[tokio::test]
    async fn later_if_modified_since_after_rollback_returns_the_current_entry() {
        let later = HeaderValue::from_static("Fri, 31 Dec 2100 23:59:59 GMT");
        let response =
            serve_public_fixture(conditional("/", &[(header::IF_MODIFIED_SINCE, &later)])).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("fixture"));
    }

    #[tokio::test]
    async fn resource_without_copy_varies_without_content_encoding() {
        let response = serve_public_fixture(request("/assets/app.js", Some("br"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
    }

    #[tokio::test]
    async fn direct_br_path_is_rejected() {
        let response = serve_public_fixture(request("/assets/app.css.br", Some("br"))).await;
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
            Surface::Public => serve_public_fixture(request).await,
            Surface::Admin => serve_admin_fixture(request).await,
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
            entry_preload_links(document.as_bytes(), WebScriptPolicy::Dioxus)
                .expect("links")
                .to_str()
                .expect("ascii"),
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
            assert!(
                entry_preload_links(document.as_bytes(), WebScriptPolicy::Dioxus).is_none(),
                "document={document}"
            );
        }
    }

    #[test]
    fn solid_entry_document_preserves_modulepreload_and_entry_module() {
        assert_eq!(
            entry_preload_links(SOLID_INDEX, WebScriptPolicy::Solid)
                .expect("links")
                .to_str()
                .expect("ascii"),
            concat!(
                "</assets/index-B2UikXgQ.js>; rel=modulepreload; crossorigin, ",
                "</assets/rolldown-runtime-DK3Fl9T5.js>; rel=modulepreload; crossorigin, ",
                "</assets/index-ZX7NKpK-.css>; rel=preload; as=style; crossorigin",
            )
        );
    }

    #[tokio::test]
    async fn entry_aliases_add_links_but_exact_index_documents_do_not() {
        let dioxus = public_app::<DioxusAssets>(WebScriptPolicy::Dioxus);
        let response = dioxus.clone().oneshot(request("/", None)).await.expect("dioxus entry");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::LINK],
            concat!(
                "</assets/public-dxh1.css>; rel=preload; as=style, ",
                "</assets/app_bg-dxh2.wasm>; rel=preload; as=fetch; ",
                "type=\"application/wasm\"; crossorigin, ",
                "</assets/app-dxh3.js>; rel=preload; as=script; crossorigin",
            )
        );
        let exact = dioxus.oneshot(request("/index.html", None)).await.expect("dioxus index");
        assert_eq!(exact.status(), StatusCode::OK);
        assert!(!exact.headers().contains_key(header::LINK));

        let solid = public_app::<SolidAssets>(WebScriptPolicy::Solid);
        let response = solid.clone().oneshot(request("/", None)).await.expect("solid entry");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            response.headers()[header::LINK],
            concat!(
                "</assets/index-B2UikXgQ.js>; rel=modulepreload; crossorigin, ",
                "</assets/rolldown-runtime-DK3Fl9T5.js>; rel=modulepreload; crossorigin, ",
                "</assets/index-ZX7NKpK-.css>; rel=preload; as=style; crossorigin",
            )
        );
        let exact = solid.oneshot(request("/index.html", None)).await.expect("solid index");
        assert_eq!(exact.status(), StatusCode::OK);
        assert!(!exact.headers().contains_key(header::LINK));
    }

    /// Admin 的 Solid 入口与 Public 共用同一份预加载转写规则，不能按 surface 写死 Dioxus。
    #[tokio::test]
    async fn admin_entry_aliases_use_solid_modulepreload() {
        let solid = admin_app::<SolidAssets>(WebScriptPolicy::Solid);
        let response = solid.clone().oneshot(request("/", None)).await.expect("admin solid entry");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            response.headers()[header::LINK],
            concat!(
                "</assets/index-B2UikXgQ.js>; rel=modulepreload; crossorigin, ",
                "</assets/rolldown-runtime-DK3Fl9T5.js>; rel=modulepreload; crossorigin, ",
                "</assets/index-ZX7NKpK-.css>; rel=preload; as=style; crossorigin",
            )
        );
        let exact = solid.oneshot(request("/index.html", None)).await.expect("admin solid index");
        assert_eq!(exact.status(), StatusCode::OK);
        assert!(!exact.headers().contains_key(header::LINK));
    }

    /// Dioxus 必须保持旧的 `<link>` 逐条转写：同一 URL 的不同 `as` 都保留，入口脚本不参与。
    #[tokio::test]
    async fn dioxus_entry_links_keep_the_legacy_link_semantics() {
        let response = public_app::<DioxusLinkAssets>(WebScriptPolicy::Dioxus)
            .oneshot(request("/", None))
            .await
            .expect("dioxus link entry");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::LINK],
            concat!(
                "</assets/shared-dxh12345678.js>; rel=preload; as=fetch, ",
                "</assets/shared-dxh12345678.js>; rel=preload; as=script, ",
                "</assets/entry-dxh12345678.js>; rel=preload; as=script",
            )
        );
    }

    async fn assert_asset_cache<A: rust_embed::RustEmbed + Send + Sync + 'static>(
        web_script_policy: WebScriptPolicy,
        immutable: &str,
        not_immutable: &[&str],
    ) {
        let app = public_app::<A>(web_script_policy);
        let response =
            app.clone().oneshot(request(immutable, None)).await.expect("immutable asset");
        assert_eq!(response.status(), StatusCode::OK, "{immutable}");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable",
            "{immutable}"
        );
        for path in not_immutable {
            let response = app.clone().oneshot(request(path, None)).await.expect("stable asset");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
        }
    }

    /// 不可变判定必须按装配根传入的技术栈；另一套产物的指纹命名不能获得长期缓存。
    #[tokio::test]
    async fn immutable_cache_follows_the_surface_web_script_policy() {
        assert_asset_cache::<SolidAssets>(
            WebScriptPolicy::Solid,
            "/assets/index-B2UikXgQ.js",
            &["/assets/app-dxh01234567.js", "/assets/app.css", "/index.html"],
        )
        .await;
        assert_asset_cache::<SolidAssets>(
            WebScriptPolicy::Solid,
            "/assets/chunks/index-B2UikXgQ.js",
            &["/assets/chunks/index.js"],
        )
        .await;
        assert_asset_cache::<DioxusAssets>(
            WebScriptPolicy::Dioxus,
            "/assets/app-dxh01234567.js",
            &["/assets/index-B2UikXgQ.js", "/assets/app.css", "/index.html"],
        )
        .await;
    }

    #[tokio::test]
    async fn admin_immutable_cache_follows_the_supplied_web_script_policy() {
        let app = admin_app::<SolidAssets>(WebScriptPolicy::Solid);
        let response = app
            .clone()
            .oneshot(request("/assets/index-B2UikXgQ.js", None))
            .await
            .expect("admin solid asset");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let response = app
            .oneshot(request("/assets/app-dxh01234567.js", None))
            .await
            .expect("admin dioxus-named asset");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    }

    #[tokio::test]
    async fn solid_fingerprinted_favicon_is_found_by_the_stable_alias() {
        let response = public_app::<SolidAssets>(WebScriptPolicy::Solid)
            .oneshot(request("/favicon.ico", None))
            .await
            .expect("favicon");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=86400");
        let body = to_bytes(response.into_body(), 4096).await.expect("favicon body");
        assert_eq!(body.as_ref(), SOLID_FAVICON);
    }

    #[tokio::test]
    async fn cdn_policy_prefers_static_cache_extension() {
        let response = serve_public_fixture(request("/assets/app.css", None)).await;
        assert_eq!(cdn_cache_policy(&response), "no-cache");

        let fingerprinted = asset_response(
            embedded_at_build_second(b"body{color:red}", "text/css").into(),
            &Method::GET,
            &HeaderMap::new(),
            exact_asset_cache("assets/app-dxh0123456789abcdef.css", WebScriptPolicy::Dioxus),
            "text/css",
            None,
        );
        assert_eq!(cdn_cache_policy(&fingerprinted), "public, max-age=31536000, immutable");
    }
}

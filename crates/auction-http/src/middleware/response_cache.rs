//! 已冻结 HTTP 路由的响应缓存策略中间件。

use axum::{
    body::{Body, HttpBody, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

/// 公开目录读取允许浏览器与共享缓存保存的时长；过期后仍允许先返回旧副本再后台再验证。
const PUBLIC_CATALOG_CACHE_CONTROL: &str =
    "public, max-age=2, s-maxage=2, stale-while-revalidate=5";
/// 已结算批次与已固化结算明细：内容不再变化，允许浏览器与 CDN 长期保存。
pub(crate) const SETTLED_CACHE_CONTROL: &str =
    "public, max-age=86400, s-maxage=86400, stale-while-revalidate=604800";
/// 历史列表仍可能追加新结算记录，使用分钟级缓存而不是 immutable。
pub(crate) const HISTORY_CACHE_CONTROL: &str =
    "public, max-age=300, s-maxage=300, stale-while-revalidate=60";
/// 计算内容 ETag 时缓冲正文的硬上限；超过上限的响应跳过 ETag，不阻止正常返回。
const MAX_ETAG_BODY_BYTES: usize = 2 << 20;

/// 公开目录读取的短期缓存策略。
/// 只标记成功响应，错误仍由边界默认拒绝存储；处理器显式给出的 `no-store` 绝不被放宽。
/// 成功正文在缓冲上限内附带内容 ETag，客户端再验证命中时直接返回 304。
pub(crate) async fn public_catalog(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    // 条件请求头是列表字段，允许重复出现；逐个保留后再统一比较。
    let if_none_match =
        request.headers().get_all(header::IF_NONE_MATCH).iter().cloned().collect::<Vec<_>>();
    let mut response = next.run(request).await;
    if !response.status().is_success() || has_cache_directive(response.headers(), "no-store") {
        return response;
    }
    if !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC_CATALOG_CACHE_CONTROL));
    }
    with_content_etag(response, method, &if_none_match).await
}

/// 倒计时 SVG 等秒级动态图片的短期缓存策略；只标记成功响应，错误仍由边界默认拒绝存储。
pub(crate) async fn short_lived(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response.status().is_success() && !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
    }
    response
}

/// 会话、身份化读取、mutation 与全部 Admin 投影不得被客户端或共享缓存保存。
pub(crate) async fn no_store(request: Request, next: Next) -> Response {
    with_policy(request, next, HeaderValue::from_static("no-store")).await
}

async fn with_policy(request: Request, next: Next, policy: HeaderValue) -> Response {
    let mut response = next.run(request).await;
    // no-store 分组始终兜住敏感响应；处理器已显式禁止存储时保持原策略。
    if policy == "no-store" || !has_cache_directive(response.headers(), "no-store") {
        response.headers_mut().insert(header::CACHE_CONTROL, policy);
    }
    response
}

/// 对允许缓存的成功正文生成强 ETag，并处理 `If-None-Match` 再验证。
/// 只处理 GET：HEAD 没有可摘要的表示正文；已有 ETag 的响应由原 owner 负责再验证。
async fn with_content_etag(
    response: Response,
    method: Method,
    if_none_match: &[HeaderValue],
) -> Response {
    if method != Method::GET || response.headers().contains_key(header::ETAG) {
        return response;
    }
    // 只为已知大小的正文计算摘要；流式响应没有稳定正文，无法生成内容 ETag。
    let Some(body_bytes) = response.body().size_hint().exact() else {
        return response;
    };
    if body_bytes > MAX_ETAG_BODY_BYTES as u64 {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_ETAG_BODY_BYTES).await else {
        // 已知大小的正文在读取时出错：无法恢复原响应，明确失败而不是发送截断正文。
        let mut failure = (StatusCode::INTERNAL_SERVER_ERROR, "响应正文不可用").into_response();
        failure.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return failure;
    };
    let etag = content_etag(&bytes);
    parts.headers.insert(header::ETAG, etag.clone());
    if if_none_match.iter().any(|value| etag_matches(value, &etag)) {
        // 304 保留 Cache-Control、ETag 等缓存头，只去掉正文与 Content-Length。
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.status = StatusCode::NOT_MODIFIED;
        return Response::from_parts(parts, not_modified_body());
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// 304 使用未知长度的空流正文：axum 的路由层会为已知大小的空正文补 `Content-Length: 0`，
/// 而 304 的 Content-Length 只能等于对应 200 表示的字节数，不能凭空写 0；未知长度让补全
/// 不发生，hyper 对 304 也不会发送正文或长度头。
fn not_modified_body() -> Body {
    Body::from_stream(futures_util::stream::empty::<Result<axum::body::Bytes, axum::Error>>())
}

/// 强 ETag 使用带双引号的 SHA-256 十六进制摘要，与静态资源 ETag 保持同一格式。
fn content_etag(bytes: &[u8]) -> HeaderValue {
    let digest = Sha256::digest(bytes);
    let hash = auction_application::core::lower_hex(&digest);
    HeaderValue::from_str(&format!("\"{hash}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("\"\""))
}

/// `If-None-Match` 按弱比较匹配；`*` 匹配任何现有表示。静态产物的再验证复用同一比较。
pub(crate) fn etag_matches(header: &HeaderValue, etag: &HeaderValue) -> bool {
    let (Ok(candidates), Ok(etag)) = (header.to_str(), etag.to_str()) else {
        return false;
    };
    candidates.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*"
            || candidate == etag
            || candidate.strip_prefix("W/").is_some_and(|weak| weak == etag)
    })
}

fn has_cache_directive(headers: &HeaderMap, expected: &str) -> bool {
    headers.get_all(header::CACHE_CONTROL).iter().filter_map(|value| value.to_str().ok()).any(
        |value| value.split(',').any(|directive| directive.trim().eq_ignore_ascii_case(expected)),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
        middleware,
        response::{IntoResponse, Response},
        routing::get,
    };
    use serde_json::json;
    use tower::ServiceExt;

    const PUBLIC_TTL: &str = "public, max-age=2, s-maxage=2, stale-while-revalidate=5";

    async fn ok_json() -> Json<serde_json::Value> {
        Json(json!({"ok": true}))
    }

    async fn unavailable() -> impl IntoResponse {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    }

    async fn already_no_store() -> Response {
        ([(header::CACHE_CONTROL, "no-store")], "ok").into_response()
    }

    async fn existing_etag() -> Response {
        ([(header::ETAG, "\"handler-etag\"")], "ok").into_response()
    }

    async fn oversized() -> Response {
        (StatusCode::OK, vec![b'x'; super::MAX_ETAG_BODY_BYTES + 1]).into_response()
    }

    fn app() -> Router {
        Router::new()
            .route("/json", get(ok_json))
            .route("/unavailable", get(unavailable))
            .route("/stored", get(already_no_store))
            .route("/existing-etag", get(existing_etag))
            .route("/oversized", get(oversized))
            .route_layer(middleware::from_fn(super::public_catalog))
    }

    async fn send(request: Request<Body>) -> Response {
        app().oneshot(request).await.expect("response")
    }

    fn get_request(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).expect("request")
    }

    fn get_with_etag(uri: &str, etag: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(header::IF_NONE_MATCH, etag)
            .body(Body::empty())
            .expect("request")
    }

    fn header_value(response: &Response, name: header::HeaderName) -> Option<String> {
        response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
    }

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    }

    #[tokio::test]
    async fn successful_json_gets_short_ttl_and_content_etag() {
        let response = send(get_request("/json")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_value(&response, header::CACHE_CONTROL).as_deref(), Some(PUBLIC_TTL));
        let etag = header_value(&response, header::ETAG).expect("etag");
        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert_eq!(etag.len(), 66, "SHA-256 十六进制加双引号");
    }

    #[tokio::test]
    async fn conditional_request_returns_empty_304_and_keeps_cache_headers() {
        let first = send(get_request("/json")).await;
        let etag = header_value(&first, header::ETAG).expect("etag");
        assert_eq!(body_text(first).await, r#"{"ok":true}"#);

        let response = send(get_with_etag("/json", &etag)).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_value(&response, header::CACHE_CONTROL).as_deref(), Some(PUBLIC_TTL));
        assert_eq!(header_value(&response, header::ETAG).as_deref(), Some(etag.as_str()));
        assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
        assert_eq!(body_text(response).await, "");
    }

    #[tokio::test]
    async fn wildcard_and_weak_validators_match() {
        let first = send(get_request("/json")).await;
        let etag = header_value(&first, header::ETAG).expect("etag");
        drop(first);

        for candidate in ["*".to_owned(), format!("W/{etag}")] {
            let response = send(get_with_etag("/json", &candidate)).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "candidate={candidate}");
        }
        // 不匹配的验证器仍返回完整正文。
        let response = send(get_with_etag("/json", "\"other\"")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, r#"{"ok":true}"#);

        // 重复的 If-None-Match 头字段都要参与比较。
        let repeated = Request::builder()
            .uri("/json")
            .header(header::IF_NONE_MATCH, "\"other\"")
            .header(header::IF_NONE_MATCH, etag.as_str())
            .body(Body::empty())
            .expect("request");
        assert_eq!(send(repeated).await.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn no_store_response_is_never_buffered_or_tagged() {
        let response = send(get_request("/stored")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_value(&response, header::CACHE_CONTROL).as_deref(), Some("no-store"));
        assert!(response.headers().get(header::ETAG).is_none());
    }

    #[tokio::test]
    async fn oversized_response_skips_etag() {
        let response = send(get_request("/oversized")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_value(&response, header::CACHE_CONTROL).as_deref(), Some(PUBLIC_TTL));
        assert!(response.headers().get(header::ETAG).is_none());
    }

    #[tokio::test]
    async fn existing_etag_is_not_replaced() {
        let response = send(get_with_etag("/existing-etag", "\"handler-etag\"")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_value(&response, header::ETAG).as_deref(), Some("\"handler-etag\""));
        assert_eq!(body_text(response).await, "ok");
    }

    #[tokio::test]
    async fn error_response_gets_no_cache_policy_from_catalog_layer() {
        let response = send(get_request("/unavailable")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(header::CACHE_CONTROL).is_none());
        assert!(response.headers().get(header::ETAG).is_none());
    }

    #[tokio::test]
    async fn short_lived_sets_seconds_ttl_only_for_success() {
        let app = Router::new()
            .route("/ok", get(|| async { "ok" }))
            .route("/unavailable", get(unavailable))
            .route("/stored", get(already_no_store))
            .route_layer(middleware::from_fn(super::short_lived));
        let request = || Request::builder().uri("/ok").body(Body::empty()).expect("request");
        let response = app.clone().oneshot(request()).await.expect("response");
        assert_eq!(
            header_value(&response, header::CACHE_CONTROL).as_deref(),
            Some("public, max-age=5")
        );
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/unavailable").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert!(response.headers().get(header::CACHE_CONTROL).is_none());
        let response = app
            .oneshot(Request::builder().uri("/stored").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(header_value(&response, header::CACHE_CONTROL).as_deref(), Some("no-store"));
    }
}

//! 已冻结 HTTP 路由的响应缓存策略中间件。

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// 公开读取的短期缓存策略；只标记成功响应，错误仍由边界默认拒绝存储。
pub(crate) async fn short_lived(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response.status().is_success() && !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
    }
    response
}

/// 公开、非身份化读取必须每次向权威服务再验证。
pub(crate) async fn no_cache(request: Request, next: Next) -> Response {
    with_policy(request, next, HeaderValue::from_static("no-cache")).await
}

/// 会话、身份化读取、mutation 与全部 Admin 投影不得被客户端或共享缓存保存。
pub(crate) async fn no_store(request: Request, next: Next) -> Response {
    with_policy(request, next, HeaderValue::from_static("no-store")).await
}

async fn with_policy(request: Request, next: Next, policy: HeaderValue) -> Response {
    let mut response = next.run(request).await;
    // no-store 分组始终兜住敏感响应；no-cache 分组不能放宽 handler 的禁止存储策略。
    if policy == "no-store"
        || !response.headers().get_all(header::CACHE_CONTROL).iter().any(|value| {
            value.to_str().is_ok_and(|value| {
                value.split(',').any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
            })
        })
    {
        response.headers_mut().insert(header::CACHE_CONTROL, policy);
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        middleware,
        response::{IntoResponse, Response},
        routing::get,
    };
    use tower::ServiceExt;

    async fn ok() -> &'static str {
        "ok"
    }

    async fn unavailable() -> impl IntoResponse {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    }

    async fn already_no_store() -> Response {
        ([(header::CACHE_CONTROL, "no-store")], "ok").into_response()
    }

    fn app() -> Router {
        Router::new()
            .route("/ok", get(ok))
            .route("/unavailable", get(unavailable))
            .route("/stored", get(already_no_store))
            .route_layer(middleware::from_fn(super::short_lived))
    }

    async fn get_headers(uri: &str) -> (StatusCode, Option<String>) {
        let response = app()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        (status, cache)
    }

    #[tokio::test]
    async fn short_lived_sets_seconds_ttl_only_for_success() {
        assert_eq!(
            get_headers("/ok").await,
            (StatusCode::OK, Some("public, max-age=5".to_owned()))
        );
        assert_eq!(get_headers("/unavailable").await, (StatusCode::SERVICE_UNAVAILABLE, None));
    }

    #[tokio::test]
    async fn short_lived_never_relaxes_a_handler_no_store() {
        assert_eq!(get_headers("/stored").await, (StatusCode::OK, Some("no-store".to_owned())));
    }
}

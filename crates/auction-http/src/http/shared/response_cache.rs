//! 已冻结 HTTP 路由的响应缓存策略中间件。

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

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
    response.headers_mut().insert(header::CACHE_CONTROL, policy);
    response
}

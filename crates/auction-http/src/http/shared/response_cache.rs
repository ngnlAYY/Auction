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
    // no-store 分组始终兜住敏感响应；no-cache 分组不能放宽 handler 的禁止存储策略。
    let handler_forbids_storage =
        response.headers().get_all(header::CACHE_CONTROL).iter().any(|value| {
            value.to_str().is_ok_and(|value| {
                value.split(',').any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
            })
        });
    if policy == "no-store" || !handler_forbids_storage {
        response.headers_mut().insert(header::CACHE_CONTROL, policy);
    }
    response
}

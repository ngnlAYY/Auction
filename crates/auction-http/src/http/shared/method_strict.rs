//! 旧版业务路由的方法严格性边界。
//!
//! Axum 的 `get` 会隐式接受 `HEAD`，默认 405 也没有旧版 JSON 外壳。业务路由通过本模块的
//! method builder 或 `MethodRouterExt` 显式注册 HEAD/fallback，健康检查、指标和静态资源继续由
//! 各自 owner 决定方法语义。

use std::convert::Infallible;

use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    handler::Handler,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};

/// 给含多种方法的业务路由补齐旧版 HEAD 与 405 契约。
pub(crate) trait MethodRouterExt<S> {
    fn strict_methods(self, allow: &'static str) -> Self;
}

impl<S> MethodRouterExt<S> for MethodRouter<S, Infallible>
where
    S: Clone + Send + Sync + 'static,
{
    fn strict_methods(self, allow: &'static str) -> Self {
        self.head(move || async move { method_not_allowed(allow) })
            .fallback(move || async move { method_not_allowed(allow) })
    }
}

pub(crate) fn get<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    axum::routing::get(handler).strict_methods("GET")
}

pub(crate) fn post<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    axum::routing::post(handler).strict_methods("POST")
}

pub(crate) fn put<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    axum::routing::put(handler).strict_methods("PUT")
}

pub(crate) fn patch<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    axum::routing::patch(handler).strict_methods("PATCH")
}

fn method_not_allowed(allow: &'static str) -> Response {
    let mut response = (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ApiErrorResponse {
            code: "method_not_allowed".to_owned(),
            message: "请求方法不受支持".to_owned(),
            data: None,
            request_id: None,
        }),
    )
        .into_response();
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
    response
}

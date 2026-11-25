//! 受限权利请求的查询、路径、正文与会话 Cookie 读取，不回显请求原文。

use auction_application::consignment::{BrowserBinding, RightsStoreError};
use auction_domain::consignment::rights::parse_session_token;
use axum::body::Body;
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::http::{HeaderMap, Uri};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::cookie::{BrowserCookie, read_session_cookie};
use super::response::RightsRouteError;
use crate::public::consignment::input;
use crate::public::consignment::response::RouteError;
use crate::query::parse_public_uuid;

/// 受限权利接口没有查询参数；任何非空查询失败关闭。
pub(super) fn no_query(uri: &Uri) -> Result<(), RightsRouteError> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(RightsStoreError::InvalidRequest.into());
    }
    Ok(())
}

/// 严格解码 JSON 正文；形状类拒绝统一为 `invalid_request`，不区分是哪个字段。
pub(super) async fn decode<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, RightsRouteError> {
    input::decode_write(headers, body).await.map_err(|error| match error {
        RouteError::Consignment(_) => RightsStoreError::InvalidRequest.into(),
        other => RightsRouteError::Route(other),
    })
}

/// 路径里的命令 ID；规范小写非零 UUID 以外一律无效。
pub(super) fn command_id(
    path: Result<Path<String>, PathRejection>,
) -> Result<Uuid, RightsRouteError> {
    let Ok(Path(raw)) = path else {
        return Err(RightsStoreError::InvalidRequest.into());
    };
    parse_public_uuid(&raw).map_err(|()| RightsStoreError::InvalidRequest.into())
}

/// 已认证请求需要的两枚 Cookie：缺任何一枚都按未认证处理。
pub(super) struct SessionCookies {
    /// 会话令牌文本；格式由用例严格解析。
    pub(super) session_token: String,
    /// 浏览器绑定。
    pub(super) browser: BrowserBinding,
}

/// 读取会话与绑定 Cookie；缺失、重复或绑定不规范一律 `Unauthenticated`（401）。
pub(super) fn session_cookies(headers: &HeaderMap) -> Result<SessionCookies, RightsRouteError> {
    let session_token = read_session_cookie(headers).ok_or(RightsStoreError::Unauthenticated)?;
    // 格式错误的令牌与缺失同样是未认证，且先于正文解码判定。
    if parse_session_token(&session_token).is_err() {
        return Err(RightsStoreError::Unauthenticated.into());
    }
    let browser =
        BrowserCookie::from_headers(headers).ok_or(RightsStoreError::Unauthenticated)?.binding();
    Ok(SessionCookies { session_token, browser })
}

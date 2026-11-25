//! Public v2 受限权利处理器：找回、挑战、核验、当前权利与续办。
//!
//! 权利人没有登录会话，故不经 `private_identity`；浏览器绑定来自 `hv_right_browser`，会话来自
//! `hv_right`。处理器只做来源守卫、严格输入解码、用例调用与 DTO 映射；真实、虚拟与未匹配目标的
//! 同形性由存储事务保证，这里不对目标是否真实做任何分支（不变量 7）。路由不读寄售激活开关：
//! 软关闭只截止新承诺，持有人续办旧权利照常（D9 §7）。挂载由集成车道按本模块导出的 `mount` 接入。

use std::net::SocketAddr;
use std::sync::Arc;

use auction_application::consignment::{
    ChallengeRightInput, ClaimRightInput, LocateRightInput, RightsService, RightsStoreError,
    VerifyRightInput,
};
use auction_contracts::{
    ChallengeRightRequest, ClaimRightRequest, LocateRightRequest, VerifyRightRequest,
};
use auction_domain::CommandId;
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};

use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};
use crate::session::session_guard;
use crate::state::PublicApiState;
use cookie::BrowserCookie;
use response::{RightsRouteError, with_browser_cookie};

mod cookie;
mod input;
mod response;
#[cfg(test)]
mod tests;

/// 进程内已装配的受限权利用例；缺失时保留无数据库骨架语义，各路由返回 503。
#[derive(Clone, Default)]
pub struct RightsCapabilities {
    /// 受限权利用例服务。
    pub service: Option<Arc<RightsService>>,
}

/// 路由只借用既有 Public 来源守卫与受限权利能力。
pub(crate) trait RightsState: Clone + Send + Sync + 'static {
    /// 既有来源检查与变更限流所用的状态。
    fn public_state(&self) -> &PublicApiState;

    /// 当前装配为此进程提供的受限权利能力。
    fn rights(&self) -> &RightsCapabilities;
}

/// 将六条受限权利路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: RightsState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/consignment/rights/locators", method_strict::post(locate::<S>))
        .route("/api/v2/consignment/rights/challenges", method_strict::post(challenge::<S>))
        .route("/api/v2/consignment/rights/verify", method_strict::post(verify::<S>))
        .route("/api/v2/consignment/rights/current", method_strict::get(current::<S>))
        .route("/api/v2/consignment/rights/current/claim", method_strict::post(claim::<S>))
        .route(
            "/api/v2/consignment/rights/current/commands/{command_id}",
            method_strict::get(command_receipt::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

fn service<S: RightsState>(state: &S) -> Result<&RightsService, RightsRouteError> {
    state
        .rights()
        .service
        .as_deref()
        .ok_or_else(|| RightsRouteError::Rights(RightsStoreError::Unavailable))
}

/// 变更请求的来源检查与限流；与目标无关，先于 Cookie 与正文。
fn mutation_guard<S: RightsState>(
    state: &S,
    headers: &HeaderMap,
    address: SocketAddr,
) -> Result<(), RightsRouteError> {
    match session_guard(state.public_state(), headers, address, true, false) {
        Some(response) => Err(Box::new(response).into()),
        None => Ok(()),
    }
}

/// 浏览器发起的三个命令共用的前置：读取或签发绑定、拆出正文。
fn browser_for(headers: &HeaderMap) -> BrowserCookie {
    BrowserCookie::from_headers(headers).unwrap_or_else(BrowserCookie::mint)
}

async fn locate<S: RightsState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Err(error) = mutation_guard(&state, &parts.headers, address) {
        return error.into_response();
    }
    let browser = browser_for(&parts.headers);
    let result = async {
        input::no_query(&parts.uri)?;
        let input: LocateRightRequest = input::decode(&parts.headers, body).await?;
        let issued = service(&state)?
            .locate(LocateRightInput {
                command_id: input.command_id,
                browser: browser.binding(),
                client_ip: address.ip(),
                game_identity: input.game_identity,
                world: response::domain_world(input.world),
            })
            .await?;
        response::challenge_accepted(issued)
    }
    .await;
    with_browser_cookie(result, &browser)
}

async fn challenge<S: RightsState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Err(error) = mutation_guard(&state, &parts.headers, address) {
        return error.into_response();
    }
    let browser = browser_for(&parts.headers);
    let result = async {
        input::no_query(&parts.uri)?;
        let input: ChallengeRightRequest = input::decode(&parts.headers, body).await?;
        let issued = service(&state)?
            .challenge(ChallengeRightInput {
                command_id: input.command_id,
                browser: browser.binding(),
                client_ip: address.ip(),
                right_locator: input.right_locator,
            })
            .await?;
        response::challenge_accepted(issued)
    }
    .await;
    with_browser_cookie(result, &browser)
}

async fn verify<S: RightsState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Err(error) = mutation_guard(&state, &parts.headers, address) {
        return error.into_response();
    }
    let browser = browser_for(&parts.headers);
    let result = async {
        input::no_query(&parts.uri)?;
        let input: VerifyRightRequest = input::decode(&parts.headers, body).await?;
        let grant = service(&state)?
            .verify(VerifyRightInput {
                command_id: input.command_id,
                browser: browser.binding(),
                challenge_id: input.challenge_id,
                code: input.code,
            })
            .await?;
        response::verified(grant)
    }
    .await;
    with_browser_cookie(result, &browser)
}

async fn current<S: RightsState>(State(state): State<S>, headers: HeaderMap, uri: Uri) -> Response {
    let result = async {
        let cookies = input::session_cookies(&headers)?;
        input::no_query(&uri)?;
        let view = service(&state)?
            .current(&cookies.session_token, cookies.browser)
            .await?
            .ok_or(RightsStoreError::Unauthenticated)?;
        response::current(view)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn claim<S: RightsState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let result = async {
        mutation_guard(&state, &parts.headers, address)?;
        // 认证先于命令仲裁与正文解码：没有有效 Cookie 的请求不会触及用例。
        let cookies = input::session_cookies(&parts.headers)?;
        input::no_query(&parts.uri)?;
        let input: ClaimRightRequest = input::decode(&parts.headers, body).await?;
        let Ok(expected_version) = u64::try_from(input.expected_version) else {
            return Err(RightsStoreError::InvalidRequest.into());
        };
        let outcome = service(&state)?
            .claim(ClaimRightInput {
                command_id: input.command_id,
                browser: cookies.browser,
                session_token: cookies.session_token,
                expected_version,
                action: response::domain_action(input.action).as_str().to_owned(),
            })
            .await?;
        response::claim(CommandId::from_uuid(input.command_id), outcome)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn command_receipt<S: RightsState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> Response {
    let result = async {
        let cookies = input::session_cookies(&headers)?;
        input::no_query(&uri)?;
        let command_id = input::command_id(path)?;
        let service = service(&state)?;
        // 先确认会话有效：未知、过期或浏览器不符的会话是 401，不能与“回执不存在”的 404 混淆。
        service
            .current(&cookies.session_token, cookies.browser)
            .await?
            .ok_or(RightsStoreError::Unauthenticated)?;
        let receipt = service
            .command_receipt(&cookies.session_token, cookies.browser, command_id)
            .await?
            .ok_or(RightsRouteError::ReceiptNotFound)?;
        response::receipt(receipt)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

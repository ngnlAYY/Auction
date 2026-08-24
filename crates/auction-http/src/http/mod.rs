//! Public 与 Admin 的 HTTP 表面适配器。
//!
//! 这里只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；竞价、结算和状态机规则
//! 留在领域与应用层。Public 与 Admin 是不同 surface，各自拥有独立的路由与前端产物。

#[path = "admin/auctions.rs"]
mod admin_auctions;
#[path = "admin/audit.rs"]
mod admin_audit;
#[path = "admin/credentials.rs"]
mod admin_credentials;
#[path = "admin/dashboard/mod.rs"]
mod admin_dashboard;
#[path = "admin/forum.rs"]
mod admin_forum;
#[path = "admin/intake.rs"]
mod admin_intake;
#[path = "admin/lot_update/mod.rs"]
mod admin_lot_update;
#[path = "admin/lots.rs"]
mod admin_lots;
#[path = "admin/mail/mod.rs"]
mod admin_mail;
#[path = "admin/proxy_events.rs"]
mod admin_proxy_events;
#[path = "admin/renames.rs"]
mod admin_renames;
#[path = "admin/rules.rs"]
mod admin_rules;
#[path = "admin/runtime_config.rs"]
mod admin_runtime_config;
#[path = "admin/users.rs"]
mod admin_users;
#[path = "public/blacklist.rs"]
mod blacklist;
#[path = "shared/boundary.rs"]
mod boundary;
#[path = "public/compensation.rs"]
mod compensation;
#[path = "admin/delivery.rs"]
mod delivery_admin;
#[path = "public/forum.rs"]
mod forum;
mod metrics;
#[path = "public/realtime.rs"]
mod realtime;
#[path = "admin/realtime.rs"]
mod realtime_admin;
#[path = "shared/realtime_hub/mod.rs"]
mod realtime_hub;
#[path = "shared/response_cache.rs"]
mod response_cache;
#[path = "public/rules.rs"]
mod rules;
mod security;
#[path = "shared/server.rs"]
mod server;
#[path = "shared/state.rs"]
mod state;

use std::{collections::BTreeMap, net::SocketAddr, time::Instant};

use crate::assets as web_assets;

use auction_application::{
    AdminLotUpdateError, AdminSessionIdentity, ApplicationError, AuctionLifecycleAction,
    AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand, CommandLedgerError,
    CompetitionTransactionError, CreateProxyCommand, CurrentAuctionSnapshot,
    DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE, LowerProxyCommand, MAX_PUBLIC_ACCOUNT_OFFSET,
    MAX_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_BID_LOG_OFFSET, MAX_PUBLIC_BID_LOG_PAGE_SIZE,
    MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_LOT_OFFSET, MAX_PUBLIC_LOT_PAGE_SIZE,
    MAX_PUBLIC_PRICE_HISTORY_OFFSET, MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE, PublicAccountPageRequest,
    PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage, PublicAuctionProjection,
    PublicAuctionSummary, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
    PublicBidLogPage, PublicEquipment, PublicFinalRestriction, PublicLiveLotSummary,
    PublicLotDetail, PublicLotListRequest, PublicLotPage, PublicMyBid, PublicMyBidsPage,
    PublicMyBidsRequest, PublicOwnedProxy, PublicPreviewLotDetail, PublicPreviewLotSummary,
    PublicPriceHistoryCursor, PublicPriceHistoryEntry, PublicPriceHistoryListRequest,
    PublicPriceHistoryPage, PublicPriceHistoryStats, PublicProxyBid, PublicProxyTransition,
    PublicSessionIdentity, RecoveryCommandError, RecoveryConfirmationAction,
    RecoveryConfirmationCommand, RecoveryConfirmationResult, RecoveryReport, RequestHash,
    SessionError, SettlementReport, WorldSnapshot, allowed_recovery_commands,
};
use auction_contracts::{
    AdminAuctionLifecycleRequest, AdminAuctionLifecycleResponse, AdminRecoveryActionResponse,
    AdminRecoveryConfirmationRequest, AdminRecoveryConfirmationResponse,
    AdminRecoveryConfirmationResultResponse, AdminRecoveryReportResponse, AdminRecoveryResponse,
    AdminSessionResponse, AdminSettlementAdjustmentResponse, AdminSettlementAuctionResponse,
    AdminSettlementLotResponse, AdminSettlementReportResponse, AdminSettlementSummaryResponse,
    ApiErrorResponse, ApiErrorResponseWithData, ApiResponse, AuctionListResponse,
    AuctionSummaryResponse, CreateProxyAuthorizationRequest, CurrentAuctionItemResponse,
    CurrentAuctionResponse, HealthLiveData, HealthProbeResponse, HealthReadyData, HealthResponse,
    LowerProxyAuthorizationRequest, PlaceBidRequest, PlaceBidResponse, PlaceBidResultResponse,
    PublicAccountBlockedResponse, PublicBidLogEntryResponse, PublicBidLogListResponse,
    PublicEquipmentResponse, PublicEquipmentStatResponse, PublicFinalRestrictionResponse,
    PublicLiveLotResponse, PublicLoginRequest, PublicLoginResponse, PublicLotDetailItemResponse,
    PublicLotEnvelopeResponse, PublicLotListItemsResponse, PublicMyBidsResponse,
    PublicMyProxyResponse, PublicOwnedBidResponse, PublicOwnedEquipmentResponse,
    PublicOwnedProxyCapabilitiesResponse, PublicOwnedProxyQuotaResponse, PublicOwnedProxyResponse,
    PublicOwnedProxyTransitionResponse, PublicPreviewLotDetailResponse, PublicPreviewLotResponse,
    PublicPriceHistoryEntryResponse, PublicPriceHistoryListResponse,
    PublicPriceHistoryStatsResponse, PublicProxyCommandAuthorizationResponse,
    PublicProxyCommandBidResponse, PublicProxyCommandResponse, PublicSessionCurrentResponse,
    WorldStatusResponse, WorldsResponse,
};
use auction_domain::{
    BidAmount, CommandId, CompetitionPlan, DomainError, LotId, ProxyAuthorization,
    ProxyAuthorizationId, ProxyMode, ProxyStatus, SettlementId, UserId,
};
use auction_observability::{BusinessEvent, BusinessOutcome};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, TimeZone, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::form_urlencoded;
use uuid::Uuid;

pub use boundary::HttpBoundary;
pub use realtime_hub::{RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription};
pub use security::{AdminHttpSecurity, HttpSecurityOptions, PublicHttpSecurity};
pub use server::serve_http;
pub use state::{AdminApiState, PublicApiState};

/// 公开端路由。
///
/// 健康检查路径沿用已冻结的旧契约；其余路径交给内嵌的公开端 SPA 产物处理。
///
/// `version` 由装配根传入自身的 `CARGO_PKG_VERSION`：契约 crate 内展开该宏只会得到契约
/// 自己的版本，而健康检查要报告的是运行进程的版本。
pub fn public_router<A>(version: &'static str, boundary: HttpBoundary) -> Router<PublicApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    let health = move || health_response("public-api", version);

    public_feature_routes(
        Router::<PublicApiState>::new()
            .route("/health", get(health))
            .route("/api/v1/health", get(health))
            .route("/health/live", get(public_health_live))
            .route("/health/ready", get(public_health_ready))
            .route("/metrics", get(metrics::public)),
    )
    .fallback(web_assets::serve_public_asset::<A>)
    .layer(middleware::from_fn_with_state(boundary, boundary::enforce))
}

/// 管理端路由。
///
/// 管理端与公开端由各自装配根传入不同资源类型，避免任一二进制引用另一端产物。
pub fn admin_router<A>(version: &'static str, boundary: HttpBoundary) -> Router<AdminApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    let health = move || health_response("admin-api", version);

    admin_feature_routes(
        Router::<AdminApiState>::new()
            .route("/health", get(health))
            .route("/api/v1/health", get(health))
            .route("/health/live", get(admin_health_live))
            .route("/health/ready", get(admin_health_ready))
            .route("/metrics", get(metrics::admin)),
    )
    .fallback(web_assets::serve_admin_asset::<A>)
    .layer(middleware::from_fn_with_state(boundary, boundary::enforce))
}

async fn health_response(service: &'static str, version: &'static str) -> Json<HealthResponse> {
    Json(HealthResponse::ok(service, version))
}

/// 返回不访问外部依赖的存活探针；该路径用于进程管理器判断是否需要重启。
async fn public_health_live() -> Json<HealthProbeResponse<HealthLiveData>> {
    Json(HealthProbeResponse {
        code: "live".to_owned(),
        message: "进程存活".to_owned(),
        data: HealthLiveData { live: true },
    })
}

/// 返回 Public 的依赖 readiness；缺少数据库端口时保持未就绪而不伪造成功。
async fn public_health_ready(State(state): State<PublicApiState>) -> Response {
    let ready = match state.readiness() {
        Some(readiness) => readiness.check().await.is_ok(),
        None => false,
    };
    health_ready_response(ready)
}

/// 返回不访问外部依赖的 Admin 存活探针。
async fn admin_health_live() -> Json<HealthProbeResponse<HealthLiveData>> {
    Json(HealthProbeResponse {
        code: "live".to_owned(),
        message: "进程存活".to_owned(),
        data: HealthLiveData { live: true },
    })
}

/// 返回 Admin 的依赖 readiness；数据库连接或迁移不完整时使用 503。
async fn admin_health_ready(State(state): State<AdminApiState>) -> Response {
    let ready = match state.readiness() {
        Some(readiness) => readiness.check().await.is_ok(),
        None => false,
    };
    health_ready_response(ready)
}

fn health_ready_response(ready: bool) -> Response {
    let (status, code, message) = if ready {
        (StatusCode::OK, "ready", "进程已就绪")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready", "进程尚未就绪")
    };
    (
        status,
        Json(HealthProbeResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: HealthReadyData { ready },
        }),
    )
        .into_response()
}

/// 注册已冻结的 Public 路径。
///
/// 路由只负责绑定各自的 HTTP owner；未装配的窄端口仍由 handler 明确返回能力不可用，
/// 不用伪造的成功 payload 掩盖部署缺口。
fn public_feature_routes(router: Router<PublicApiState>) -> Router<PublicApiState> {
    let no_cache_routes = Router::<PublicApiState>::new()
        .route("/api/v1/worlds", get(list_worlds))
        .route("/api/v1/rules", get(rules::get))
        .route("/api/v1/blacklist", get(blacklist::list))
        .route("/api/v1/compensation-pool", get(compensation::pool))
        .route("/api/v1/compensation-pool/{auction_id}/movements", get(compensation::movements))
        .route("/api/v1/auctions", get(list_auctions))
        .route("/api/v1/auctions/current", get(current_auction))
        .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
        .route("/api/v1/auctions/{auction_id}/bid-log", get(list_bid_log))
        .route("/api/v1/lots/{lot_id}", get(lot_detail))
        .route("/api/v1/price-history", get(list_price_history))
        .route("/api/v1/auctions/{auction_id}/forum", get(forum::get))
        .route_layer(middleware::from_fn(response_cache::no_cache));
    let no_store_routes = Router::<PublicApiState>::new()
        .route("/api/v1/events", get(realtime::list))
        .route(
            "/api/v1/session",
            get(current_session)
                .post(login_session)
                .delete(logout_session)
                .layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route("/api/v1/me/bids", get(list_my_bids))
        .route("/api/v1/lots/{lot_id}/proxy-authorizations/mine", get(get_my_proxy_authorization))
        .route(
            "/api/v1/lots/{lot_id}/proxy-authorizations",
            post(create_proxy_authorization).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower",
            post(lower_proxy_authorization).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route("/api/v1/lots/{lot_id}/bids", post(place_bid).layer(DefaultBodyLimit::max(4 << 10)))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router
        .merge(no_cache_routes)
        .merge(no_store_routes)
        .route("/api/v1/ws", get(realtime::websocket))
}

/// 注册已冻结的 Admin 路径。
fn admin_feature_routes(router: Router<AdminApiState>) -> Router<AdminApiState> {
    let routes = Router::<AdminApiState>::new()
        .route(
            "/api/v1/admin/session",
            get(current_admin_session)
                .post(login_admin_session)
                .delete(logout_admin_session)
                .layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route("/api/v1/admin/dashboard", get(admin_dashboard::get))
        .route(
            "/api/v1/admin/auctions",
            get(admin_auctions::list)
                .post(admin_auctions::create)
                .layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/worlds/{world_id}/auction-intake/items",
            post(admin_intake::add_item).layer(DefaultBodyLimit::max(72 << 10)),
        )
        .route(
            "/api/v1/admin/auction-forum/preview",
            post(admin_forum::preview).layer(DefaultBodyLimit::max(2 << 20)),
        )
        .route("/api/v1/admin/auctions/{auction_id}/lots", get(admin_lots::list))
        .route(
            "/api/v1/admin/auctions/{auction_id}/settlement-report",
            get(admin_settlement_report),
        )
        .route("/api/v1/admin/auctions/{auction_id}/recovery-report", get(admin_recovery_report))
        .route(
            "/api/v1/admin/auctions/{auction_id}/preview",
            post(admin_preview_auction).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/auctions/{auction_id}/open",
            post(admin_open_auction).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/auctions/{auction_id}/cancel",
            post(admin_cancel_auction).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/auctions/{auction_id}/final-phase",
            post(admin_final_phase_auction).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/lots/{lot_id}",
            patch(admin_lot_update::update).layer(DefaultBodyLimit::max(8 << 10)),
        )
        .route(
            "/api/v1/admin/recoveries/{recovery_id}/confirm",
            post(admin_confirm_recovery).layer(DefaultBodyLimit::max(8 << 10)),
        )
        .route("/api/v1/admin/lots/{lot_id}/proxy-events", get(admin_proxy_events::list))
        .route("/api/v1/admin/mail", get(admin_mail::state))
        .route("/api/v1/admin/mail/intake", get(admin_mail::list_intakes))
        .route("/api/v1/admin/mail/intake/{intake_id}", get(admin_mail::detail_intake))
        .route(
            "/api/v1/admin/mail/polling",
            put(admin_mail::set_paused).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/mail/polls:trigger",
            post(admin_mail::trigger_poll).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route("/api/v1/admin/realtime/lag", get(realtime_admin::list_lag))
        .route("/api/v1/admin/realtime/failures", get(realtime_admin::list_failures))
        .route("/api/v1/admin/realtime/failures/{failure_id}", get(realtime_admin::get_failure))
        .route(
            "/api/v1/admin/realtime/failures/{failure_id}/replay",
            post(realtime_admin::replay).layer(DefaultBodyLimit::max(64 << 10)),
        )
        .route(
            "/api/v1/admin/rules",
            get(admin_rules::get)
                .post(admin_rules::validate)
                .put(admin_rules::publish)
                .layer(DefaultBodyLimit::max(1 << 20)),
        )
        .route(
            "/api/v1/admin/config",
            get(admin_runtime_config::get)
                .put(admin_runtime_config::update)
                .layer(DefaultBodyLimit::max(4 << 20)),
        )
        .route(
            "/api/v1/admin/config:validate",
            post(admin_runtime_config::validate).layer(DefaultBodyLimit::max(4 << 20)),
        )
        .route(
            "/api/v1/admin/users",
            get(admin_users::list)
                .post(admin_users::create)
                .patch(admin_users::mutate)
                .layer(DefaultBodyLimit::max(16 << 10)),
        )
        .route("/api/v1/admin/renames", get(admin_renames::list))
        .route(
            "/api/v1/admin/renames/{rename_id}/revert",
            post(admin_users::revert_rename).layer(DefaultBodyLimit::max(8 << 10)),
        )
        .route(
            "/api/v1/admin/users/{user_id}/blacklist",
            put(admin_users::blacklist)
                .delete(admin_users::unblacklist)
                .layer(DefaultBodyLimit::max(8 << 10)),
        )
        .route(
            "/api/v1/admin/credentials",
            get(admin_credentials::list)
                .post(admin_credentials::create)
                .patch(admin_credentials::mutate)
                .delete(admin_credentials::delete)
                .layer(DefaultBodyLimit::max(128 << 10)),
        )
        .route("/api/v1/admin/audit", get(admin_audit::list))
        .route("/api/v1/admin/deliveries", get(delivery_admin::list))
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/release",
            post(delivery_admin::release).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/retry",
            post(delivery_admin::retry).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/reconcile",
            post(delivery_admin::reconcile).layer(DefaultBodyLimit::max(4 << 10)),
        );
    router.merge(routes.route_layer(middleware::from_fn(response_cache::no_store)))
}

const ADMIN_SESSION_COOKIE: &str = "auction_admin_session";
const ADMIN_CSRF_COOKIE: &str = "auction_admin_csrf";
const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

async fn login_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some((actor, secret)) = read_admin_basic_auth(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
            .into_response();
    };
    let now = Utc::now();
    match session.login(actor, secret, now).await {
        Ok(result) => {
            let mut response_headers = no_store_headers();
            let Ok(session_cookie) = admin_cookie(
                ADMIN_SESSION_COOKIE,
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                true,
            ) else {
                return internal_error();
            };
            let Ok(csrf_cookie) = admin_cookie(
                ADMIN_CSRF_COOKIE,
                &result.csrf_token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                false,
            ) else {
                return internal_error();
            };
            response_headers.append(header::SET_COOKIE, session_cookie);
            response_headers.append(header::SET_COOKIE, csrf_cookie);
            let data = AdminSessionResponse {
                authenticated: true,
                actor: result.identity.actor,
                csrf_token: result.csrf_token,
                expires_at: timestamp(result.identity.expires_at),
                absolute_expires_at: timestamp(result.identity.absolute_expires_at),
            };
            (StatusCode::OK, response_headers, Json(ApiResponse::ok("登录成功", data)))
                .into_response()
        }
        Err(error) => map_admin_session_error(error),
    }
}

async fn current_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let (raw_csrf, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let data = AdminSessionResponse {
        authenticated: true,
        actor: identity.actor,
        csrf_token: raw_csrf,
        expires_at: timestamp(identity.expires_at),
        absolute_expires_at: timestamp(identity.absolute_expires_at),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("会话有效", data))).into_response()
}

async fn logout_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response();
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_admin_cookie(&headers, ADMIN_SESSION_COOKIE) else {
        return authentication_error();
    };
    let now = Utc::now();
    if let Err(error) = session.current(raw_token.clone(), raw_csrf, now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    if let Err(error) = session.logout(raw_token, now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_admin_cookies(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}

async fn admin_preview_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Preview,
    )
    .await
}

async fn admin_open_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Open,
    )
    .await
}

async fn admin_cancel_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Cancel,
    )
    .await
}

async fn admin_final_phase_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::FinalPhase,
    )
    .await
}

async fn execute_admin_lifecycle(
    state: AdminApiState,
    address: SocketAddr,
    auction_id: String,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    action: AuctionLifecycleAction,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "生命周期命令不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminAuctionLifecycleRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    if input.expected_state_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_state_version", "拍卖状态版本无效")
            .into_response();
    }
    if action == AuctionLifecycleAction::Cancel {
        let Some(reason) = input.reason.as_deref() else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_cancel_reason",
                "取消拍卖必须提供原因",
            )
            .into_response();
        };
        if reason.trim().is_empty() || reason.len() > 1024 || reason.chars().any(char::is_control) {
            return api_error(StatusCode::BAD_REQUEST, "invalid_cancel_reason", "取消原因无效")
                .into_response();
        }
    } else if input.reason.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_lifecycle_reason",
            "非取消生命周期命令不能携带原因",
        )
        .into_response();
    }
    let request_hash = match RequestHash::from_request(action.command_kind(), auction_uuid, &input)
    {
        Ok(hash) => hash,
        Err(_) => return internal_error(),
    };
    let Some(lifecycle) = state.lifecycle() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖生命周期暂不可用",
        )
        .into_response();
    };
    let command = AuctionLifecycleCommand {
        command_id: CommandId::from_uuid(input.command_id),
        auction_id: auction_domain::AuctionId::from_uuid(auction_uuid),
        action,
        expected_state_version: Some(
            u64::try_from(input.expected_state_version).unwrap_or_default(),
        ),
        cancel_reason: input.reason,
        request_hash,
        request_id: request_id(&headers),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match lifecycle.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_admin_lifecycle_error(error),
    };
    if result.command_id.as_uuid() != input.command_id
        || result.auction_id.as_uuid() != auction_uuid
        || result.state_version == 0
    {
        return internal_error();
    }
    let data = AdminAuctionLifecycleResponse {
        command_id: input.command_id,
        auction_id: auction_uuid.to_string(),
        status: admin_auction_status_name(result.status).to_owned(),
        state_version: result.state_version,
        updated_at: timestamp(result.updated_at),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("拍卖生命周期已更新", data)))
        .into_response()
}

/// 读取 PostgreSQL 完整档的结算报告；Lite 档由应用端口明确返回能力缺失。
async fn admin_settlement_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "结算报告不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some(reader) = state.settlement_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "结算报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.settlement_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "结算报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "结算报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_settlement_report(&report, auction_uuid) {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("结算报告已加载", map_settlement_report(report))),
    )
        .into_response()
}

/// 读取 PostgreSQL 完整档的 recovery 安全投影；内部 claim/lease 字段永不进入 HTTP。
async fn admin_recovery_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复报告不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some(reader) = state.recovery_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.recovery_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "恢复报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "恢复报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_recovery_report(&report, auction_uuid) {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("恢复报告已加载", map_recovery_report(report))),
    )
        .into_response()
}

/// 以版本 fence 确认一个已解决 custody 证据的 recovery case。
async fn admin_confirm_recovery(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(recovery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复确认不接受查询参数")
            .into_response();
    }
    let Ok(recovery_uuid) = Uuid::parse_str(&recovery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_id", "恢复 ID 无效")
            .into_response();
    };
    if recovery_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_id", "恢复 ID 无效")
            .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminRecoveryConfirmationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() || input.expected_state_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    }
    let Some(action) = RecoveryConfirmationAction::parse(&input.action) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    };
    let request_hash = match RequestHash::from_request("confirm_recovery", recovery_uuid, &input) {
        Ok(hash) => hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.recovery_commands() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let command = RecoveryConfirmationCommand {
        command_id: CommandId::from_uuid(input.command_id),
        recovery_id: SettlementId::from_uuid(recovery_uuid),
        expected_state_version: input.expected_state_version,
        action,
        reason: input.reason,
        request_hash,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match commands.confirm_recovery(command).await {
        Ok(result) => result,
        Err(error) => return map_recovery_command_error(error),
    };
    if result.recovery_id.as_uuid() != recovery_uuid
        || result.state_version <= input.expected_state_version
    {
        return internal_error();
    }
    let data = AdminRecoveryConfirmationResponse { recovery: map_recovery_confirmation(result) };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("恢复确认已提交", data)))
        .into_response()
}

const PUBLIC_SESSION_COOKIE: &str = "auction_public_session";

async fn login_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let input: PublicLoginRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let now = Utc::now();
    match session.login(input.key.into_bytes(), now).await {
        Ok(result) => {
            let mut response_headers = no_store_headers();
            let Ok(cookie) = public_session_cookie(
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
            ) else {
                return internal_error();
            };
            response_headers.insert(header::SET_COOKIE, cookie);
            let data = PublicLoginResponse {
                user_id: result.identity.user_id.to_string(),
                username: result.identity.username,
                status: "active".to_owned(),
                expires_at: timestamp(result.identity.expires_at),
                absolute_expires_at: timestamp(result.identity.absolute_expires_at),
            };
            (StatusCode::OK, response_headers, Json(ApiResponse::ok("登录成功", data)))
                .into_response()
        }
        Err(error) => map_login_error(error),
    }
}

async fn current_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return current_session_response(&state, None, true);
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => current_session_response(&state, Some(identity), false),
        Err(error) if is_unauthenticated(&error) => current_session_response(&state, None, true),
        Err(_) => session_unavailable(),
    }
}

async fn logout_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return authentication_error();
    };
    let now = Utc::now();
    if let Err(error) = session.current(raw_token.clone(), now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    if session.logout(raw_token, now).await.is_err() {
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}

/// 读取当前用户自己的普通竞价和代理历史，保留旧接口的独立分页字段。
async fn list_my_bids(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let query = match parse_query(&uri, &["page", "page_size", "bid_page", "proxy_page"]) {
        Ok(query) => query,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let independent = query.contains_key("bid_page") || query.contains_key("proxy_page");
    if independent && query.contains_key("page") {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE,
        Some(MAX_PUBLIC_ACCOUNT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let bid_page =
        match parse_positive_i64(query.get(if independent { "bid_page" } else { "page" }), 1, None)
        {
            Ok(page) => page,
            Err(_) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        };
    let proxy_page = if independent {
        match parse_positive_i64(query.get("proxy_page"), 1, None) {
            Ok(page) => page,
            Err(_) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        }
    } else {
        bid_page
    };
    let Some(bid_offset) = page_offset(bid_page, page_size) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };
    let Some(proxy_offset) = page_offset(proxy_page, page_size) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };
    let request = PublicMyBidsRequest {
        bids: PublicAccountPageRequest { limit: page_size, offset: bid_offset },
        proxies: PublicAccountPageRequest { limit: page_size, offset: proxy_offset },
    };
    if request.validate().is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    };
    let page = match reader.list_public_user_bids(identity.user_id, request).await {
        Ok(page) => page,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户竞价暂不可用",
            )
            .into_response();
        }
    };
    if !valid_account_page(&page, identity.user_id, &request) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    }
    let data = PublicMyBidsResponse {
        bids: page.bids.into_iter().map(map_owned_bid).collect(),
        proxies: page.proxies.into_iter().map(map_owned_proxy).collect(),
        bid_total: page.bid_total,
        proxy_total: page.proxy_total,
        page: bid_page,
        bid_page,
        proxy_page,
        page_size,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("账户竞价已加载", data)))
        .into_response()
}

/// 读取当前用户在指定公开 lot 上最新的一份代理授权。
async fn get_my_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权查询不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let Some(lots) = state.lots() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        )
        .into_response();
    };
    match lots.public_lot(lot_uuid).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_error().into_response(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "公开 lot 目录暂不可用",
            )
            .into_response();
        }
    }
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let authorization = match reader.public_user_proxy(lot_uuid, identity.user_id).await {
        Ok(authorization) => authorization,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    if authorization
        .as_ref()
        .is_some_and(|proxy| proxy.lot_id != lot_uuid || proxy.user_id != identity.user_id)
    {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    }
    let data = PublicMyProxyResponse { authorization: authorization.map(map_owned_proxy) };
    let message = if data.authorization.is_some() {
        "代理授权已加载"
    } else {
        "当前批次没有代理授权"
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok(message, data))).into_response()
}

/// Public 普通竞价命令：认证、请求摘要和事务结果都在 HTTP 边界完成稳定映射。
async fn place_bid(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "出价不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }

    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let input: PlaceBidRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let amount = match BidAmount::new(input.amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("place_bid", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let command = auction_application::PlaceBidCommand {
        command_id,
        lot_id,
        bidder_id,
        amount,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.place_bid(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Bid,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(&state, BusinessEvent::Bid, BusinessOutcome::Success, command_started);
    let Some(current_price) = plan.final_lot.current_price else {
        return internal_error();
    };
    let Ok(state_version) = i64::try_from(plan.final_lot.state_version) else {
        return internal_error();
    };
    if plan.final_lot.lot_id != lot_id
        || plan.final_lot.leader_id.is_none()
        || !plan.public_bids.iter().any(|bid| {
            bid.command_id == command_id && bid.lot_id == lot_id && bid.bidder_id == bidder_id
        })
    {
        return internal_error();
    }
    let data = PlaceBidResponse {
        bid: PlaceBidResultResponse {
            lot_id: lot_uuid.to_string(),
            current_price: current_price.value(),
            effective_end_at: timestamp(plan.final_lot.effective_end_at),
            state_version,
            leading: plan.final_lot.leader_id == Some(bidder_id),
        },
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("出价已接受", data))).into_response()
}

/// 创建代理授权命令；命令账本和代理快照提交后返回旧契约要求的完整命令投影。
async fn create_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let input: CreateProxyAuthorizationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let mode = match input.mode.as_str() {
        "" => None,
        "fast" => Some(ProxyMode::Fast),
        "slow" => Some(ProxyMode::Slow),
        _ => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_proxy_mode", "代理模式无效")
                .into_response();
        }
    };
    let maximum_amount = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("create_proxy", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let command = CreateProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        maximum_amount,
        mode,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.create_proxy(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Proxy,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(
        &state,
        BusinessEvent::Proxy,
        BusinessOutcome::Success,
        command_started,
    );
    let Some(expected_state) = plan.proxy_states.iter().find(|state| {
        state.lot_id == lot_id
            && state.user_id == bidder_id
            && state.creation_command_id == command_id
    }) else {
        return internal_error();
    };
    let Some(account) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let proxy = match account
        .public_user_proxy_by_id(lot_uuid, expected_state.id.as_uuid(), identity.user_id)
        .await
    {
        Ok(Some(proxy)) if valid_command_proxy_identity(&proxy, expected_state) => proxy,
        Ok(_) => return internal_error(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    let Some(data) = map_proxy_command_response(proxy, &plan, expected_state, bidder_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权已创建", data)))
        .into_response()
}

/// 降低代理授权上限命令；命令账本保证重试不会重复追加转换事实，并返回完整命令投影。
async fn lower_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path((lot_id, authorization_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    let Ok(authorization_uuid) = Uuid::parse_str(&authorization_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_authorization_id", "代理授权 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() || authorization_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let input: LowerProxyAuthorizationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let new_maximum = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("lower_proxy", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let authorization_id = ProxyAuthorizationId::from_uuid(authorization_uuid);
    let command = LowerProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        authorization_id,
        new_maximum,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.lower_proxy(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Proxy,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(
        &state,
        BusinessEvent::Proxy,
        BusinessOutcome::Success,
        command_started,
    );
    let Some(expected_state) = plan.proxy_states.iter().find(|state| {
        state.id == authorization_id && state.lot_id == lot_id && state.user_id == bidder_id
    }) else {
        return internal_error();
    };
    if plan.final_lot.lot_id != lot_id
        || !plan.proxy_transitions.iter().any(|transition| {
            transition.authorization_id == authorization_id && transition.command_id == command_id
        })
    {
        return internal_error();
    }
    let Some(account) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let proxy = match account
        .public_user_proxy_by_id(lot_uuid, expected_state.id.as_uuid(), identity.user_id)
        .await
    {
        Ok(Some(proxy)) if valid_command_proxy_identity(&proxy, expected_state) => proxy,
        Ok(_) => return internal_error(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    let Some(data) = map_proxy_command_response(proxy, &plan, expected_state, bidder_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权上限已降低", data)))
        .into_response()
}

fn session_guard(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else {
        state.security().allow_mutation(address.ip())
    };
    let retry_after = retry_after?;
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response_headers.insert(header::RETRY_AFTER, value);
    }
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            response_headers,
            Json(ApiErrorResponse {
                code: "rate_limited".to_owned(),
                message: "请求过于频繁".to_owned(),
                data: None,
                request_id: None,
            }),
        )
            .into_response(),
    )
}

fn admin_session_guard(
    state: &AdminApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else {
        state.security().allow_mutation(address.ip())
    };
    let retry_after = retry_after?;
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response_headers.insert(header::RETRY_AFTER, value);
    }
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            response_headers,
            Json(ApiErrorResponse {
                code: "rate_limited".to_owned(),
                message: "请求过于频繁".to_owned(),
                data: None,
                request_id: None,
            }),
        )
            .into_response(),
    )
}

async fn authenticated_admin_session(
    state: &AdminApiState,
    headers: &HeaderMap,
) -> Result<(String, AdminSessionIdentity), Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(session_unavailable()));
    };
    let Some(raw_token) = read_admin_cookie(headers, ADMIN_SESSION_COOKIE) else {
        return Err(Box::new(authentication_error()));
    };
    let Some(raw_csrf) = read_admin_cookie(headers, ADMIN_CSRF_COOKIE) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, raw_csrf.clone(), Utc::now()).await {
        Ok(identity) => Ok((raw_csrf, identity)),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(_) => Err(Box::new(session_unavailable())),
    }
}

fn current_session_response(
    state: &PublicApiState,
    identity: Option<PublicSessionIdentity>,
    clear_cookie: bool,
) -> Response {
    let mut response_headers = no_store_headers();
    if clear_cookie {
        append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    }
    let data = identity.map(|identity| PublicSessionCurrentResponse {
        authenticated: true,
        user_id: identity.user_id.to_string(),
        username: identity.username,
        expires_at: timestamp(identity.expires_at),
        absolute_expires_at: timestamp(identity.absolute_expires_at),
    });
    let message = if data.is_some() { "会话有效" } else { "当前未登录" };
    (StatusCode::OK, response_headers, Json(ApiResponse::ok(message, data))).into_response()
}

fn map_login_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidLoginKey) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_key", "登录凭据格式无效").into_response()
        }
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::AccountInactive { public_reason }) => {
            let Some(public_reason) = public_reason else {
                return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                    .into_response();
            };
            (
                StatusCode::FORBIDDEN,
                Json(ApiErrorResponseWithData {
                    code: "account_blacklisted".to_owned(),
                    message: "账号已被封禁".to_owned(),
                    data: PublicAccountBlockedResponse { public_reason },
                    request_id: None,
                }),
            )
                .into_response()
        }
        ApplicationError::Session(SessionError::SessionLimit) => {
            api_error(StatusCode::CONFLICT, "session_limit", "并发会话数量已达上限").into_response()
        }
        _ => session_unavailable(),
    }
}

fn map_admin_session_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::Unauthenticated) => authentication_error(),
        ApplicationError::Session(SessionError::InvalidConfiguration)
        | ApplicationError::Session(SessionError::TokenGeneration) => session_unavailable(),
        _ => session_unavailable(),
    }
}

fn map_admin_lifecycle_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Domain(DomainError::InvalidTransition { .. })
        | ApplicationError::Domain(DomainError::InvalidSchedule(_))
        | ApplicationError::Domain(DomainError::CancellationCutoff) => {
            api_error(StatusCode::CONFLICT, "invalid_lifecycle", "拍卖当前不允许该生命周期操作")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            api_error(
                StatusCode::CONFLICT,
                "state_version_conflict",
                "拍卖状态已变化，请刷新后重试",
            )
            .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::Serialization(_) => internal_error(),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖生命周期暂不可用",
        )
        .into_response(),
    }
}

fn is_unauthenticated(error: &ApplicationError) -> bool {
    matches!(error, ApplicationError::Session(SessionError::Unauthenticated))
}

fn admin_auction_status_name(status: auction_domain::AuctionStatus) -> &'static str {
    match status {
        auction_domain::AuctionStatus::Draft => "draft",
        auction_domain::AuctionStatus::Preview => "preview",
        auction_domain::AuctionStatus::Live => "live",
        auction_domain::AuctionStatus::Final => "final",
        auction_domain::AuctionStatus::Settling => "settling",
        auction_domain::AuctionStatus::Settled => "settled",
        auction_domain::AuctionStatus::Cancelled => "cancelled",
    }
}

async fn authenticated_public_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
) -> Result<PublicSessionIdentity, Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(session_unavailable()));
    };
    let Some(raw_token) = read_public_session_cookie(headers) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => Ok(identity),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(_) => Err(Box::new(session_unavailable())),
    }
}

fn observe_public_business(
    state: &PublicApiState,
    event: BusinessEvent,
    outcome: BusinessOutcome,
    started: Instant,
) {
    let Some(metrics) = state.metrics() else {
        return;
    };
    let recorder = metrics.business();
    recorder.observe(event, outcome);
    recorder.observe_duration(event, started.elapsed());
}

fn business_outcome_for_competition_error(error: &ApplicationError) -> BusinessOutcome {
    match error {
        ApplicationError::Domain(_)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::NotFound
            | CompetitionTransactionError::Conflict
            | CompetitionTransactionError::StaleState
            | CompetitionTransactionError::InvalidState
            | CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict),
        ) => BusinessOutcome::Rejected,
        _ => BusinessOutcome::Error,
    }
}

fn map_competition_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidAmount { .. }) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效").into_response()
        }
        ApplicationError::Domain(DomainError::BidTooLow { .. }) => {
            api_error(StatusCode::CONFLICT, "bid_too_low", "出价未达到当前最低价").into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyMaximum) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_maximum", "代理上限当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyDisabled) => {
            api_error(StatusCode::CONFLICT, "proxy_disabled", "当前拍卖未启用代理竞价")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyOutbidLimit) => {
            api_error(StatusCode::CONFLICT, "proxy_outbid_limit", "代理重复进场次数已用尽")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxySlowFinal) => {
            api_error(StatusCode::CONFLICT, "proxy_slow_final", "最终阶段不允许使用慢速代理")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyTransition { .. }) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_state", "代理授权当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::AuctionClosed)
        | ApplicationError::Domain(DomainError::InvalidBidState(_)) => {
            api_error(StatusCode::CONFLICT, "auction_not_open", "当前拍卖不接受竞价")
                .into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::Serialization(_) => internal_error(),
        ApplicationError::CapabilityUnavailable(_)
        | ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::Domain(_)
        | ApplicationError::Repository(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "竞价服务暂不可用")
                .into_response()
        }
    }
}

fn map_recovery_command_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "恢复记录不存在").into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::StaleState) => {
            api_error(StatusCode::CONFLICT, "stale_recovery_state", "恢复状态版本已变化")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable) => {
            api_error(StatusCode::CONFLICT, "recovery_not_confirmable", "恢复状态不允许当前确认")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::EvidenceUnresolved) => {
            api_error(StatusCode::CONFLICT, "recovery_evidence_unresolved", "恢复证据尚未解决")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response(),
        ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::Serialization(_)
        | ApplicationError::Repository(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::Domain(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "恢复确认服务暂不可用",
        )
        .into_response(),
    }
}

fn authentication_error() -> Response {
    api_error(StatusCode::UNAUTHORIZED, "unauthenticated", "会话无效或已过期").into_response()
}

fn session_unavailable() -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "会话服务暂不可用")
        .into_response()
}

fn internal_error() -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "服务内部错误").into_response()
}

fn no_store_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers
}

fn read_public_session_cookie(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for pair in value.split(';') {
            let (name, cookie_value) = pair.trim().split_once('=')?;
            if name != PUBLIC_SESSION_COOKIE {
                continue;
            }
            if found.is_some() || cookie_value.is_empty() {
                return None;
            }
            found = Some(cookie_value.to_owned());
        }
    }
    found
}

fn read_admin_basic_auth(headers: &HeaderMap) -> Option<(String, Vec<u8>)> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") || encoded.is_empty() {
        return None;
    }
    let decoded = STANDARD.decode(encoded).ok()?;
    let separator = decoded.iter().position(|byte| *byte == b':')?;
    let actor = std::str::from_utf8(&decoded[..separator]).ok()?.to_owned();
    let secret = decoded.get(separator + 1..)?.to_vec();
    Some((actor, secret))
}

fn read_admin_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for pair in value.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (cookie_name, cookie_value) = pair.split_once('=')?;
            if cookie_name != name {
                continue;
            }
            if found.is_some() || cookie_value.is_empty() {
                return None;
            }
            found = Some(cookie_value.to_owned());
        }
    }
    found
}

fn valid_admin_csrf_header(headers: &HeaderMap, cookie: &str) -> bool {
    let mut values = headers.get_all(ADMIN_CSRF_HEADER).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    values.next().is_none() && constant_time_equal(cookie, value)
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    let left_hash = Sha256::digest(left.as_bytes());
    let right_hash = Sha256::digest(right.as_bytes());
    left_hash.as_slice().ct_eq(right_hash.as_slice()).unwrap_u8() == 1
}

fn admin_cookie(
    name: &str,
    token: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    secure: bool,
    http_only: bool,
) -> Result<HeaderValue, ()> {
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{name}={token}; Path=/; Expires={}; Max-Age={max_age}; SameSite=Strict",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if http_only {
        value.push_str("; HttpOnly");
    }
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}

fn append_clear_cookie(headers: &mut HeaderMap, secure: bool) {
    let mut value = String::from(
        "auction_public_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; HttpOnly; SameSite=Lax",
    );
    if secure {
        value.push_str("; Secure");
    }
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(header::SET_COOKIE, value);
    }
}

fn append_clear_admin_cookies(headers: &mut HeaderMap, secure: bool) {
    for (name, http_only) in [(ADMIN_SESSION_COOKIE, true), (ADMIN_CSRF_COOKIE, false)] {
        let mut value = format!(
            "{name}=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; SameSite=Strict"
        );
        if http_only {
            value.push_str("; HttpOnly");
        }
        if secure {
            value.push_str("; Secure");
        }
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.append(header::SET_COOKIE, value);
        }
    }
}

fn public_session_cookie(
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    secure: bool,
) -> Result<HeaderValue, ()> {
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{PUBLIC_SESSION_COOKIE}={token}; Path=/; Expires={}; Max-Age={max_age}; HttpOnly; SameSite=Lax",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}

async fn list_worlds(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<WorldsResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "世界目录不接受查询参数"));
    }

    let Some(reader) = state.worlds() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "世界目录暂不可用",
        ));
    };
    let worlds = reader.list_worlds().await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "世界目录暂不可用")
    })?;
    let worlds = worlds.into_iter().map(map_world).collect();
    Ok(Json(ApiResponse::ok("世界状态已加载", WorldsResponse { worlds })))
}

fn map_world(world: WorldSnapshot) -> (String, WorldStatusResponse) {
    (world.id, WorldStatusResponse { enabled: world.enabled })
}

async fn list_auctions(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<AuctionListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["world", "status", "page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query.get("world").filter(|value| !value.is_empty()).cloned();
    if world_id.as_deref().is_some_and(|world| world.len() > 64 || world != world.trim()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let status = query
        .get("status")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("settled");
    if !matches!(status, "live" | "final" | "settled") {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, Some(200))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor = query
        .get("cursor")
        .map(|raw| decode_public_cursor(raw))
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    if cursor.is_some() && page != 1 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page.checked_sub(1)
            .and_then(|page| page.checked_mul(page_size))
            .filter(|offset| *offset <= 10_000)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };

    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    };
    let result = reader
        .list_public_auctions(PublicAuctionListRequest {
            world_id,
            status: status.to_owned(),
            limit: page_size,
            offset,
            cursor: cursor.clone(),
        })
        .await
        .map_err(|_| {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "拍卖目录暂不可用")
        })?;
    if result.limit != page_size || result.offset != offset {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    }

    let response = map_auction_page(result, page, query.get("cursor").cloned());
    Ok(Json(ApiResponse::ok("拍卖列表已加载", response)))
}

/// 公开竞价日志。编号页与 keyset cursor 页沿用旧系统互斥的分页字段。
async fn list_bid_log(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicBidLogListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = Uuid::parse_str(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(&uri, &["world", "lot", "page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = match query.get("world") {
        None => None,
        Some(world) if world.is_empty() || world.len() > 64 || world != world.trim() => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
        }
        Some(world) => Some(world.clone()),
    };
    let lot_id = match query.get("lot") {
        None => None,
        Some(lot) if lot.is_empty() => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
        }
        Some(lot) => Some(
            Uuid::parse_str(lot)
                .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?,
        ),
    };
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 20, Some(MAX_PUBLIC_BID_LOG_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor_raw = query.get("cursor").cloned();
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_bid_log_cursor)
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    if cursor.is_some() && page != 1 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page.checked_sub(1)
            .and_then(|page| page.checked_mul(page_size))
            .filter(|offset| *offset <= MAX_PUBLIC_BID_LOG_OFFSET)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };
    let request = PublicBidLogListRequest {
        auction_id,
        world_id,
        lot_id,
        limit: page_size,
        offset,
        cursor: cursor.clone(),
    };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.bid_log() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    };
    let page_result = reader.list_public_bid_log(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开竞价日志暂不可用")
    })?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.entries.iter().any(|entry| {
            entry.auction_id != auction_id || lot_id.is_some_and(|lot_id| entry.lot_id != lot_id)
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    }
    let response = map_bid_log_page(page_result, page, cursor_raw);
    Ok(Json(ApiResponse::ok("竞价日志已加载", response)))
}

/// 公开成交价格历史。过滤器和分页边界保持旧 Public 契约，cursor 页不计算统计聚合。
async fn list_price_history(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicPriceHistoryListResponse>>, (StatusCode, Json<ApiErrorResponse>)>
{
    let query = parse_query(
        &uri,
        &[
            "name",
            "world",
            "from",
            "to",
            "min_price",
            "max_price",
            "buyer",
            "seller",
            "page",
            "page_size",
            "cursor",
        ],
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let name_terms = parse_history_name_terms(query.get("name"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = parse_optional_history_text(query.get("world"), 64)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let closed_from = parse_history_date(query.get("from"), false)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let closed_before = parse_history_date(query.get("to"), true)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let minimum_price = parse_history_price(query.get("min_price"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let maximum_price = parse_history_price(query.get("max_price"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let buyer = parse_optional_history_text(query.get("buyer"), 256)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let seller = parse_optional_history_text(query.get("seller"), 256)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor_raw = query.get("cursor").cloned();
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_price_history_cursor)
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    if cursor.is_some() && page != 1 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page.checked_sub(1)
            .and_then(|page| page.checked_mul(page_size))
            .filter(|offset| *offset <= MAX_PUBLIC_PRICE_HISTORY_OFFSET)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };
    let request = PublicPriceHistoryListRequest {
        name_terms,
        world_id,
        closed_from,
        closed_before,
        minimum_price,
        maximum_price,
        buyer,
        seller,
        limit: page_size,
        offset,
        cursor,
    };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.price_history() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    };
    let page_result = reader.list_public_price_history(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开价格历史暂不可用")
    })?;
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_some() != page_result.stats.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.entries.iter().any(|entry| {
            entry.lot_id.is_nil()
                || entry.auction_id.is_nil()
                || entry.world_id.is_empty()
                || entry.equipment_name.is_empty()
                || entry.equipment_category.is_empty()
                || entry.buyer_name.is_empty()
                || entry.seller_name.is_empty()
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    }
    let response = map_price_history_page(page_result, page, cursor_raw);
    Ok(Json(ApiResponse::ok("价格历史已加载", response)))
}

/// 公开 lot 列表。preview 与 live 使用互斥的数组形状，保持旧系统的 JSON 契约。
async fn list_lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = Uuid::parse_str(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(&uri, &["category", "page", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, Some(MAX_PUBLIC_LOT_PAGE_SIZE))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_LOT_OFFSET)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let category = query.get("category").cloned();
    let request = PublicLotListRequest { category, limit: page_size, offset };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.lots() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    };
    let page_result = reader.list_public_lots(auction_id, request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开 lot 目录暂不可用")
    })?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.projection.auction_id != auction_id
        || page_result.limit != page_size
        || page_result.offset != offset
        || page_result.category_counts.len() != 8
        || page_result.projection.projection_kind == "preview" && !page_result.live.is_empty()
        || page_result.projection.projection_kind != "preview" && !page_result.preview.is_empty()
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    }
    let response = map_lot_page(page_result, page);
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

/// 公开 lot 详情只返回仍属于当前公开 projection 的对象。
async fn lot_detail(
    State(state): State<PublicApiState>,
    Path(lot_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "lot 详情不接受查询参数"));
    }
    let lot_id = Uuid::parse_str(&lot_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效"))?;
    let Some(reader) = state.lots() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    };
    let detail = reader.public_lot(lot_id).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开 lot 目录暂不可用")
    })?;
    let Some(detail) = detail else {
        return Err(not_found_error());
    };
    if !valid_lot_detail(&detail, lot_id) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    }
    let response = map_lot_detail(detail);
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

async fn current_auction(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<CurrentAuctionResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["world"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query
        .get("world")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| "main".to_owned());
    if world_id.len() > 64 || world_id != world_id.trim() {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "当前拍卖暂不可用",
        ));
    };
    let auction = reader.current_auction(world_id).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "当前拍卖暂不可用")
    })?;
    Ok(Json(ApiResponse::ok(
        if auction.is_some() { "当前拍卖已加载" } else { "当前没有进行中的拍卖" },
        CurrentAuctionResponse { auction: auction.map(map_current_auction) },
    )))
}

fn api_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> (StatusCode, Json<ApiErrorResponse>) {
    (
        status,
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id: None,
        }),
    )
}

pub(crate) fn request_id(headers: &HeaderMap) -> Option<String> {
    boundary::request_id(headers)
}

fn parse_query(uri: &Uri, allowed: &[&str]) -> Result<BTreeMap<String, String>, ()> {
    let mut values = BTreeMap::new();
    let Some(query) = uri.query() else {
        return Ok(values);
    };
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if !allowed.iter().any(|allowed_key| *allowed_key == key)
            || values.insert(key.into_owned(), value.into_owned()).is_some()
        {
            return Err(());
        }
    }
    Ok(values)
}

fn parse_positive_i64(
    value: Option<&String>,
    default: i64,
    maximum: Option<i64>,
) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_empty() {
        return Err(());
    }
    let parsed = value.parse::<i64>().map_err(|_| ())?;
    if parsed < 1 || maximum.is_some_and(|maximum| parsed > maximum) {
        return Err(());
    }
    Ok(parsed)
}

fn page_offset(page: i64, page_size: i64) -> Option<i64> {
    page.checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_ACCOUNT_OFFSET)
}

fn valid_account_page(
    page: &PublicMyBidsPage,
    user_id: Uuid,
    request: &PublicMyBidsRequest,
) -> bool {
    page.bid_total >= 0
        && page.proxy_total >= 0
        && page.bids.len() as i64 <= request.bids.limit
        && page.proxies.len() as i64 <= request.proxies.limit
        && page.bids.len() as i64 <= page.bid_total
        && page.proxies.len() as i64 <= page.proxy_total
        && page.proxies.iter().all(|proxy| proxy.user_id == user_id)
}

fn parse_history_name_terms(value: Option<&String>) -> Result<Vec<String>, ()> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.len() > 1024 {
        return Err(());
    }
    let terms = value
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if terms.len() > MAX_PUBLIC_HISTORY_NAME_TERMS {
        return Err(());
    }
    Ok(terms)
}

fn parse_optional_history_text(
    value: Option<&String>,
    maximum: usize,
) -> Result<Option<String>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > maximum || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value.clone()))
}

fn parse_history_date(
    value: Option<&String>,
    exclusive_next_day: bool,
) -> Result<Option<DateTime<Utc>>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value != value.trim() {
        return Err(());
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ())?;
    if date.format("%Y-%m-%d").to_string() != *value {
        return Err(());
    }
    let midnight = date.and_hms_opt(0, 0, 0).ok_or(())?;
    let timestamp = Utc.from_utc_datetime(&midnight);
    if exclusive_next_day {
        timestamp.checked_add_signed(Duration::days(1)).map(Some).ok_or(())
    } else {
        Ok(Some(timestamp))
    }
}

fn parse_history_price(value: Option<&String>) -> Result<Option<i64>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return if value.is_empty() { Ok(None) } else { Err(()) };
    }
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed > i64::MAX as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(Some(parsed as i64))
}

fn map_auction_page(
    page: PublicAuctionPage,
    page_number: i64,
    cursor: Option<String>,
) -> AuctionListResponse {
    let auctions = page.entries.into_iter().map(map_auction_summary).collect();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_cursor);
    let has_more = cursor.as_ref().map(|_| page.has_more);
    AuctionListResponse {
        auctions,
        page: cursor.is_none().then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor,
        has_more,
    }
}

fn map_bid_log_page(
    page: PublicBidLogPage,
    page_number: i64,
    cursor: Option<String>,
) -> PublicBidLogListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_bid_log_cursor);
    PublicBidLogListResponse {
        bids: page.entries.into_iter().map(map_bid_log_entry).collect(),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

fn map_bid_log_entry(entry: PublicBidLogEntry) -> PublicBidLogEntryResponse {
    PublicBidLogEntryResponse {
        id: entry.id.to_string(),
        auction_id: entry.auction_id.to_string(),
        lot_id: entry.lot_id.to_string(),
        lot_number: entry.lot_number,
        amount: entry.amount,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        bidder_name: entry.bidder_name,
        valid: entry.valid,
        accepted_state_version: entry.accepted_state_version,
        fact_index: entry.fact_index,
        mechanism: entry.mechanism,
        created_at: timestamp(entry.created_at),
    }
}

fn map_owned_bid(bid: PublicMyBid) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: bid.fact_index,
        mechanism: bid.mechanism,
        created_at: timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: bid.equipment_name,
            category: bid.equipment_category,
        },
    }
}

fn map_owned_proxy(proxy: PublicOwnedProxy) -> PublicOwnedProxyResponse {
    let equipment = PublicOwnedEquipmentResponse {
        name: proxy.equipment_name.clone(),
        category: proxy.equipment_category.clone(),
    };
    PublicOwnedProxyResponse {
        authorization_id: proxy.authorization_id.to_string(),
        auction_id: proxy.auction_id.to_string(),
        lot_id: proxy.lot_id.to_string(),
        lot_number: proxy.lot_number,
        authorization_sequence: proxy.authorization_sequence,
        usage_number: proxy.usage_number,
        mode: proxy.mode,
        maximum_amount: proxy.maximum_amount,
        quota: PublicOwnedProxyQuotaResponse { used: proxy.quota_used, limit: proxy.quota_limit },
        transitions: proxy.transitions.into_iter().map(map_proxy_transition).collect(),
        status: proxy.status,
        terminal_reason: proxy.terminal_reason,
        state_version: proxy.state_version,
        created_at: timestamp(proxy.created_at),
        updated_at: timestamp(proxy.updated_at),
        equipment,
        bid: proxy
            .bid
            .map(|bid| map_proxy_bid(bid, &proxy.equipment_name, &proxy.equipment_category)),
        invalid_reason: proxy.invalid_reason,
        capabilities: PublicOwnedProxyCapabilitiesResponse {
            can_lower: proxy.capabilities.can_lower,
            waiting: proxy.capabilities.waiting,
            at_maximum: proxy.capabilities.at_maximum,
        },
    }
}

/// 校验命令结果所需的历史账户投影身份和追加式转换链。
///
/// 当前授权行可能已经被后续命令推进，但按授权 ID 读取的转换历史仍包含原命令版本；命令
/// 映射器会根据领域计划截取到原状态版本，避免把后续状态或后续转换混入重放结果。
fn valid_command_proxy_identity(proxy: &PublicOwnedProxy, expected: &ProxyAuthorization) -> bool {
    let Ok(expected_state_version) = i64::try_from(expected.state_version) else {
        return false;
    };
    proxy.authorization_id == expected.id.as_uuid()
        && proxy.lot_id == expected.lot_id.as_uuid()
        && proxy.user_id == expected.user_id.as_uuid()
        && proxy.quota_limit >= 0
        && proxy.transitions.len() >= usize::try_from(expected_state_version).unwrap_or(usize::MAX)
}

/// 把已提交的代理快照和本次竞价计划组合成旧系统要求的命令响应。
fn map_proxy_command_response(
    proxy: PublicOwnedProxy,
    plan: &CompetitionPlan,
    expected: &ProxyAuthorization,
    bidder_id: UserId,
) -> Option<PublicProxyCommandResponse> {
    let expected_state_version = i64::try_from(expected.state_version).ok()?;
    let transition_count = usize::try_from(expected_state_version).ok()?;
    if proxy.transitions.len() < transition_count {
        return None;
    }
    let state_version = i64::try_from(plan.final_lot.state_version).ok()?;
    let bid = match plan.public_bids.as_slice() {
        [] => None,
        [bid] => {
            if bid.lot_id != plan.final_lot.lot_id
                || bid.amount.value() != plan.final_lot.current_price?.value()
                || bid.state_version != plan.final_lot.state_version
            {
                return None;
            }
            Some(PublicProxyCommandBidResponse {
                lot_id: bid.lot_id.to_string(),
                current_price: bid.amount.value(),
                effective_end_at: timestamp(plan.final_lot.effective_end_at),
                state_version,
                leading: plan.final_lot.leader_id == Some(bidder_id),
            })
        }
        _ => return None,
    };

    let status = match expected.status {
        ProxyStatus::Active | ProxyStatus::FinalFast => "active",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    };
    let mode = match expected.mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    };
    let terminal_reason = if status == "active" {
        None
    } else {
        proxy
            .transitions
            .get(transition_count.saturating_sub(1))
            .and_then(|transition| transition.reason.clone())
    };
    let transitions =
        proxy.transitions.into_iter().take(transition_count).map(map_proxy_transition).collect();
    Some(PublicProxyCommandResponse {
        authorization: PublicProxyCommandAuthorizationResponse {
            authorization_id: expected.id.to_string(),
            lot_id: expected.lot_id.to_string(),
            authorization_sequence: i64::try_from(expected.authorization_sequence).ok()?,
            usage_number: i64::try_from(expected.usage_number).ok()?,
            mode: mode.to_owned(),
            maximum_amount: expected.maximum_amount.value(),
            quota: PublicOwnedProxyQuotaResponse {
                used: i64::try_from(expected.usage_number).ok()?,
                limit: proxy.quota_limit,
            },
            transitions,
            status: status.to_owned(),
            terminal_reason,
            state_version: expected_state_version,
            bid,
        },
    })
}

fn map_proxy_bid(
    bid: PublicProxyBid,
    equipment_name: &str,
    equipment_category: &str,
) -> PublicOwnedBidResponse {
    PublicOwnedBidResponse {
        id: bid.id.to_string(),
        auction_id: bid.auction_id.to_string(),
        lot_id: bid.lot_id.to_string(),
        lot_number: bid.lot_number,
        amount: bid.amount,
        accepted_state_version: bid.accepted_state_version,
        fact_index: bid.fact_index,
        mechanism: bid.mechanism,
        created_at: timestamp(bid.created_at),
        leading: bid.leading,
        equipment: PublicOwnedEquipmentResponse {
            name: equipment_name.to_owned(),
            category: equipment_category.to_owned(),
        },
    }
}

fn map_proxy_transition(transition: PublicProxyTransition) -> PublicOwnedProxyTransitionResponse {
    PublicOwnedProxyTransitionResponse {
        transition_number: transition.transition_number,
        kind: transition.kind,
        old_maximum_amount: transition.old_maximum_amount,
        new_maximum_amount: transition.new_maximum_amount,
        old_mode: transition.old_mode,
        new_mode: transition.new_mode,
        old_status: transition.old_status,
        new_status: transition.new_status,
        reason: transition.reason,
        resulting_state_version: transition.resulting_state_version,
        created_at: timestamp(transition.created_at),
    }
}

fn map_price_history_page(
    page: PublicPriceHistoryPage,
    page_number: i64,
    cursor: Option<String>,
) -> PublicPriceHistoryListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_price_history_cursor);
    PublicPriceHistoryListResponse {
        rows: page.entries.into_iter().map(map_price_history_entry).collect(),
        stats: page.stats.map(map_price_history_stats),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

fn map_price_history_entry(entry: PublicPriceHistoryEntry) -> PublicPriceHistoryEntryResponse {
    PublicPriceHistoryEntryResponse {
        lot_id: entry.lot_id.to_string(),
        auction_id: entry.auction_id.to_string(),
        world_id: entry.world_id,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.lot_number,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        equipment_level: entry.equipment_level,
        buyer_name: entry.buyer_name,
        seller_name: entry.seller_name,
        price: entry.price,
        closed_at: timestamp(entry.closed_at),
    }
}

fn map_price_history_stats(stats: PublicPriceHistoryStats) -> PublicPriceHistoryStatsResponse {
    PublicPriceHistoryStatsResponse {
        count: stats.count,
        min: stats.minimum,
        max: stats.maximum,
        median: stats.median,
    }
}

fn map_settlement_report(report: SettlementReport) -> AdminSettlementReportResponse {
    let auction = report.auction;
    AdminSettlementReportResponse {
        auction: AdminSettlementAuctionResponse {
            id: auction.id.to_string(),
            world_id: auction.world_id,
            sequence: auction.sequence,
            status: auction.status,
            preview_scheduled_at: timestamp(auction.preview_scheduled_at),
            scheduled_open_at: timestamp(auction.scheduled_open_at),
            preview_opened_at: auction.preview_opened_at.map(timestamp),
            preview_manual_open: auction.preview_manual_open,
            preview_revision: auction.preview_revision,
            opened_at: auction.opened_at.map(timestamp),
            final_phase_at: auction.final_phase_at.map(timestamp),
            settled_at: auction.settled_at.map(timestamp),
            cancelled_at: auction.cancelled_at.map(timestamp),
            cancelled_reason: auction.cancelled_reason,
            cancelled_by: auction.cancelled_by,
            state_version: auction.state_version,
            scheduled_end_at: auction.scheduled_end_at.map(timestamp),
            effective_end_at: auction.effective_end_at.map(timestamp),
            created_at: timestamp(auction.created_at),
            updated_at: timestamp(auction.updated_at),
        },
        settlement: report.settlement.map(|settlement| AdminSettlementSummaryResponse {
            opening_balance: settlement.opening_balance,
            tax_income: settlement.tax_income,
            discount_demand: settlement.discount_demand,
            compensation_out: settlement.compensation_out,
            closing_balance: settlement.closing_balance,
            movement_count: settlement.movement_count,
            settled_at: timestamp(settlement.settled_at),
        }),
        lots: report
            .lots
            .into_iter()
            .map(|lot| AdminSettlementLotResponse {
                id: lot.id.to_string(),
                lot_number: lot.lot_number,
                status: lot.status,
                equipment_name: lot.equipment_name,
                seller: lot.seller,
                winner: lot.winner,
                final_price: lot.final_price,
                buyer_amount: lot.buyer_amount,
                seller_amount: lot.seller_amount,
                tax_amount: lot.tax_amount,
                compensation_amount: lot.compensation_amount,
                adjustment: AdminSettlementAdjustmentResponse { policy: lot.adjustment.policy },
            })
            .collect(),
    }
}

fn valid_settlement_report(report: &SettlementReport, auction_id: Uuid) -> bool {
    let auction = &report.auction;
    if auction.id.as_uuid() != auction_id
        || auction.sequence <= 0
        || auction.state_version <= 0
        || auction.world_id.is_empty()
        || auction.status.is_empty()
        || auction.preview_revision < 0
        || auction.created_at > auction.updated_at
    {
        return false;
    }
    match &report.settlement {
        None => auction.status != "settled" && report.lots.is_empty(),
        Some(settlement) => {
            if auction.status != "settled"
                || auction.settled_at != Some(settlement.settled_at)
                || settlement.opening_balance < 0
                || settlement.tax_income < 0
                || settlement.discount_demand < 0
                || settlement.compensation_out < 0
                || settlement.closing_balance < 0
                || settlement.movement_count < 2
                || report.lots.is_empty()
            {
                return false;
            }
            report.lots.iter().all(|lot| {
                !lot.id.is_nil()
                    && lot.lot_number > 0
                    && matches!(lot.status.as_str(), "sold" | "unsold")
                    && !lot.equipment_name.is_empty()
                    && lot.final_price >= 0
                    && lot.buyer_amount >= 0
                    && lot.seller_amount >= 0
                    && lot.tax_amount >= 0
                    && lot.compensation_amount >= 0
                    && lot.adjustment.policy.as_ref().is_none_or(|policy| {
                        !policy.is_empty() && policy.len() <= 64 && policy == policy.trim()
                    })
            })
        }
    }
}

fn map_recovery_report(report: RecoveryReport) -> AdminRecoveryReportResponse {
    AdminRecoveryReportResponse {
        auction_id: report.auction_id.to_string(),
        recoveries: report
            .recoveries
            .into_iter()
            .map(|recovery| AdminRecoveryResponse {
                id: recovery.id.to_string(),
                auction_id: recovery.auction_id.to_string(),
                lot_id: recovery.lot_id.to_string(),
                lot_number: recovery.lot_number,
                world_id: recovery.world_id,
                equipment_name: recovery.equipment_name,
                stage: recovery.stage,
                resolution: recovery.resolution,
                state_version: recovery.state_version,
                payment_deadline_at: timestamp(recovery.payment_deadline_at),
                reminder_count: recovery.reminder_count,
                reminder_limit: recovery.reminder_limit,
                custody_proof_kind: recovery.custody_proof_kind,
                custody_proof_observed_at: recovery.custody_proof_observed_at.map(timestamp),
                blocking_code: recovery.blocking_code,
                manual_reconcile_delivery_id: recovery
                    .manual_reconcile_delivery_id
                    .map(|id| id.to_string()),
                relisted_auction_id: recovery.relisted_auction_id.map(|id| id.to_string()),
                relisted_lot_id: recovery.relisted_lot_id.map(|id| id.to_string()),
                actions: recovery
                    .actions
                    .into_iter()
                    .map(|action| AdminRecoveryActionResponse {
                        id: action.id.to_string(),
                        kind: action.kind,
                        state: action.state,
                        delivery_id: action.delivery_id.map(|id| id.to_string()),
                        failure_code: action.failure_code,
                        proof_kind: action.proof_kind,
                        proof_observed_at: action.proof_observed_at.map(timestamp),
                        updated_at: timestamp(action.updated_at),
                    })
                    .collect(),
                allowed_commands: recovery.allowed_commands,
                updated_at: timestamp(recovery.updated_at),
            })
            .collect(),
    }
}

fn map_recovery_confirmation(
    result: RecoveryConfirmationResult,
) -> AdminRecoveryConfirmationResultResponse {
    AdminRecoveryConfirmationResultResponse {
        recovery_id: result.recovery_id.to_string(),
        action: result.action.as_str().to_owned(),
        stage: result.stage,
        resolution: result.resolution,
        state_version: result.state_version,
        relisted_auction_id: result.relisted_auction_id.map(|id| id.to_string()),
        relisted_lot_id: result.relisted_lot_id.map(|id| id.to_string()),
    }
}

fn valid_recovery_report(report: &RecoveryReport, auction_id: Uuid) -> bool {
    if report.auction_id.as_uuid() != auction_id {
        return false;
    }
    let mut recovery_ids = std::collections::BTreeSet::new();
    let mut lot_ids = std::collections::BTreeSet::new();
    for recovery in &report.recoveries {
        if recovery.id.is_nil()
            || recovery.auction_id.as_uuid() != auction_id
            || recovery.lot_id.is_nil()
            || recovery.lot_number <= 0
            || !matches!(recovery.world_id.as_str(), "main" | "isekai")
            || recovery.equipment_name.is_empty()
            || !matches!(
                recovery.stage.as_str(),
                "awaiting_payment"
                    | "payment_overdue"
                    | "delivering_equipment"
                    | "reclaiming"
                    | "relist_pending"
                    | "refunding"
                    | "completed"
            )
            || recovery.resolution.as_ref().is_some_and(|resolution| {
                !matches!(resolution.as_str(), "reclaimed" | "cancelled" | "equipment_recovered")
            })
            || recovery.state_version <= 0
            || recovery.payment_deadline_at.timestamp() <= 0
            || recovery.reminder_limit != 1
            || recovery.reminder_count < 0
            || recovery.reminder_count > recovery.reminder_limit
            || recovery.updated_at.timestamp() <= 0
            || recovery.blocking_code.as_ref().is_some_and(|code| code.is_empty())
            || recovery.manual_reconcile_delivery_id.is_some_and(SettlementId::is_nil)
            || recovery.relisted_auction_id.is_some_and(auction_domain::AuctionId::is_nil)
            || recovery.relisted_lot_id.is_some_and(auction_domain::LotId::is_nil)
            || recovery.relisted_auction_id.is_some() != recovery.relisted_lot_id.is_some()
            || !recovery_ids.insert(recovery.id.as_uuid())
            || !lot_ids.insert(recovery.lot_id.as_uuid())
        {
            return false;
        }
        let mut action_ids = std::collections::BTreeSet::new();
        for action in &recovery.actions {
            if action.id.is_nil()
                || !matches!(
                    action.kind.as_str(),
                    "payment_reminder"
                        | "reclaim_equipment_take"
                        | "cancel_carrier_take"
                        | "recover_equipment_take"
                        | "buyer_blacklist"
                )
                || !matches!(
                    action.state.as_str(),
                    "pending"
                        | "prepared"
                        | "enqueued"
                        | "satisfied"
                        | "failed"
                        | "manual_reconcile"
                )
                || action.delivery_id.is_some_and(SettlementId::is_nil)
                || action.failure_code.as_ref().is_some_and(|code| code.is_empty())
                || action.proof_kind.as_ref().is_some_and(String::is_empty)
                || action.updated_at.timestamp() <= 0
                || !action_ids.insert(action.id.as_uuid())
            {
                return false;
            }
        }
        if recovery.allowed_commands != allowed_recovery_commands(recovery)
            || recovery.allowed_commands.len() > 1
            || recovery.allowed_commands.iter().any(|command| {
                !matches!(command.as_str(), "reclaimed" | "cancelled" | "equipment_recovered")
            })
        {
            return false;
        }
    }
    true
}

fn map_auction_summary(summary: PublicAuctionSummary) -> AuctionSummaryResponse {
    AuctionSummaryResponse {
        id: summary.id.to_string(),
        world_id: summary.world_id,
        sequence: summary.sequence,
        status: summary.status,
        scheduled_open_at: timestamp(summary.scheduled_open_at),
        opened_at: summary.opened_at.map(timestamp),
        final_phase_at: summary.final_phase_at.map(timestamp),
        effective_end_at: summary.effective_end_at.map(timestamp),
        settled_at: summary.settled_at.map(timestamp),
        lot_count: summary.lot_count,
        sold_count: summary.sold_count,
        unsold_count: summary.unsold_count,
        sold_amount: summary.sold_amount,
        state_version: summary.state_version,
    }
}

fn map_current_auction(auction: CurrentAuctionSnapshot) -> CurrentAuctionItemResponse {
    CurrentAuctionItemResponse {
        id: auction.id.to_string(),
        world: auction.world_id,
        number: auction.sequence,
        status: auction.status,
        opens_at: timestamp(auction.opens_at),
        scheduled_ends_at: auction.scheduled_ends_at.map(timestamp),
        presentation_mode: auction.presentation_mode,
        bidding_open: auction.bidding_open,
        preview_revision: auction.preview_revision,
    }
}

fn map_lot_page(page: PublicLotPage, page_number: i64) -> PublicLotEnvelopeResponse {
    let projection = page.projection;
    let lots = if projection.projection_kind == "preview" {
        PublicLotListItemsResponse::Preview(page.preview.into_iter().map(map_preview_lot).collect())
    } else {
        PublicLotListItemsResponse::Live(
            page.live.into_iter().map(|lot| map_live_lot(lot, None)).collect(),
        )
    };
    let response = map_lot_envelope(projection);
    PublicLotEnvelopeResponse {
        lots: Some(lots),
        total: Some(page.total),
        page: Some(page_number),
        page_size: Some(page.limit),
        category_counts: Some(page.category_counts),
        ..response
    }
}

fn map_lot_detail(detail: PublicLotDetail) -> PublicLotEnvelopeResponse {
    let response = map_lot_envelope(detail.projection);
    let lot = match (detail.preview, detail.live) {
        (Some(preview), None) => PublicLotDetailItemResponse::Preview(map_preview_detail(preview)),
        (None, Some(live)) => PublicLotDetailItemResponse::Live(map_live_lot(
            live.summary,
            Some(live.equipment_attributes),
        )),
        (None, None) | (Some(_), Some(_)) => {
            // 存储端口的 projection 与分支不一致时由边界校验拦截；这里没有可安全序列化的 fallback。
            return PublicLotEnvelopeResponse { lot: None, ..response };
        }
    };
    PublicLotEnvelopeResponse { lot: Some(lot), ..response }
}

fn map_lot_envelope(projection: PublicAuctionProjection) -> PublicLotEnvelopeResponse {
    PublicLotEnvelopeResponse {
        auction_id: projection.auction_id.to_string(),
        world: projection.world_id,
        number: projection.sequence,
        auction_status: projection.auction_status,
        projection_kind: projection.projection_kind,
        presentation_mode: projection.presentation_mode,
        bidding_open: projection.bidding_open,
        opens_at: timestamp(projection.opens_at),
        scheduled_ends_at: projection.scheduled_ends_at.map(timestamp),
        preview_revision: projection.preview_revision,
        final_restriction: projection.final_restriction.map(map_final_restriction),
        lots: None,
        lot: None,
        total: None,
        page: None,
        page_size: None,
        category_counts: None,
    }
}

fn map_final_restriction(restriction: PublicFinalRestriction) -> PublicFinalRestrictionResponse {
    PublicFinalRestrictionResponse {
        new_slow_proxy_allowed: restriction.new_slow_proxy_allowed,
        existing_slow_proxy_converted: restriction.existing_slow_proxy_converted,
    }
}

fn map_live_lot(
    lot: PublicLiveLotSummary,
    attributes: Option<serde_json::Value>,
) -> PublicLiveLotResponse {
    PublicLiveLotResponse {
        id: lot.id.to_string(),
        auction_id: lot.auction_id.to_string(),
        lot_number: lot.lot_number,
        status: lot.status,
        start_price: lot.start_price,
        current_price: lot.current_price,
        next_bid: lot.next_bid,
        has_leader: lot.has_leader,
        leader_name: lot.leader_name,
        seller_name: lot.seller_name,
        effective_end_at: lot.effective_end_at.map(timestamp),
        state_version: lot.state_version,
        equipment: map_equipment(lot.equipment, attributes),
    }
}

fn map_preview_lot(lot: PublicPreviewLotSummary) -> PublicPreviewLotResponse {
    PublicPreviewLotResponse {
        id: lot.id.to_string(),
        auction_id: lot.auction_id.to_string(),
        code: lot.code,
        status: lot.status,
        seller_name: lot.seller_name,
        start_price: lot.start_price,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        equipment_level: lot.equipment_level,
        avg_roll: lot.avg_roll,
        game_equipment_url: lot.game_equipment_url,
    }
}

fn map_preview_detail(detail: PublicPreviewLotDetail) -> PublicPreviewLotDetailResponse {
    PublicPreviewLotDetailResponse {
        summary: map_preview_lot(detail.summary),
        equipment: map_equipment(detail.equipment, None),
    }
}

fn map_equipment(
    equipment: PublicEquipment,
    attributes: Option<serde_json::Value>,
) -> PublicEquipmentResponse {
    let stats = equipment
        .stats
        .into_iter()
        .map(|(name, stat)| {
            (
                name,
                PublicEquipmentStatResponse { value: stat.value, base: stat.base, roll: stat.roll },
            )
        })
        .collect();
    PublicEquipmentResponse {
        name: equipment.name,
        category: equipment.category,
        level: equipment.level,
        info_summary: equipment.info_summary,
        stats,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url,
        attributes,
    }
}

fn valid_lot_detail(detail: &PublicLotDetail, lot_id: Uuid) -> bool {
    if detail.projection.auction_id.is_nil()
        || detail.projection.projection_kind == "preview" && detail.live.is_some()
        || detail.projection.projection_kind != "preview" && detail.preview.is_some()
    {
        return false;
    }
    match (&detail.preview, &detail.live) {
        (Some(preview), None) => preview.summary.id == lot_id,
        (None, Some(live)) => live.summary.id == lot_id,
        _ => false,
    }
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn encode_public_cursor(cursor: &PublicAuctionCursor) -> String {
    match cursor {
        PublicAuctionCursor::Start => "start".to_owned(),
        PublicAuctionCursor::Boundary { sequence, id } => {
            let mut payload = [0_u8; 26];
            payload[0] = 1;
            payload[1] = 3;
            payload[2..10].copy_from_slice(&sequence.to_be_bytes());
            payload[10..].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

fn decode_public_cursor(raw: &str) -> Result<PublicAuctionCursor, ()> {
    if raw == "start" {
        return Ok(PublicAuctionCursor::Start);
    }
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 26
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 3
    {
        return Err(());
    }
    let sequence = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[10..]).map_err(|_| ())?;
    if sequence <= 0 || id.is_nil() {
        return Err(());
    }
    Ok(PublicAuctionCursor::Boundary { sequence, id })
}

fn encode_public_bid_log_cursor(cursor: &PublicBidLogCursor) -> String {
    match cursor {
        PublicBidLogCursor::Start => "start".to_owned(),
        PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        } => {
            let mut payload = [0_u8; 54];
            payload[0] = 1;
            payload[1] = 4;
            payload[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
            payload[10..26].copy_from_slice(lot_id.as_bytes());
            payload[26..34].copy_from_slice(&accepted_state_version.to_be_bytes());
            payload[34..38].copy_from_slice(&fact_index.to_be_bytes());
            payload[38..54].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

fn decode_public_bid_log_cursor(raw: &str) -> Result<PublicBidLogCursor, ()> {
    if raw == "start" {
        return Ok(PublicBidLogCursor::Start);
    }
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 54
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 4
    {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let created_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let lot_id = Uuid::from_slice(&decoded[10..26]).map_err(|_| ())?;
    let accepted_state_version = i64::from_be_bytes(decoded[26..34].try_into().map_err(|_| ())?);
    let fact_index = i32::from_be_bytes(decoded[34..38].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[38..54]).map_err(|_| ())?;
    if timestamp_micros == 0
        || lot_id.is_nil()
        || accepted_state_version <= 0
        || fact_index <= 0
        || id.is_nil()
    {
        return Err(());
    }
    Ok(PublicBidLogCursor::Boundary { created_at, lot_id, accepted_state_version, fact_index, id })
}

fn encode_public_price_history_cursor(cursor: &PublicPriceHistoryCursor) -> String {
    match cursor {
        PublicPriceHistoryCursor::Start => "start".to_owned(),
        PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id } => {
            let mut payload = [0_u8; 38];
            payload[0] = 1;
            payload[1] = 5;
            payload[2..10].copy_from_slice(&closed_at.timestamp_micros().to_be_bytes());
            payload[10..18].copy_from_slice(&auction_sequence.to_be_bytes());
            payload[18..22].copy_from_slice(&(*lot_number as i32).to_be_bytes());
            payload[22..38].copy_from_slice(lot_id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

fn decode_public_price_history_cursor(raw: &str) -> Result<PublicPriceHistoryCursor, ()> {
    if raw == "start" {
        return Ok(PublicPriceHistoryCursor::Start);
    }
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 38
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 5
    {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let closed_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let auction_sequence = i64::from_be_bytes(decoded[10..18].try_into().map_err(|_| ())?);
    let lot_number = i64::from(i32::from_be_bytes(decoded[18..22].try_into().map_err(|_| ())?));
    let lot_id = Uuid::from_slice(&decoded[22..38]).map_err(|_| ())?;
    if timestamp_micros == 0 || auction_sequence <= 0 || lot_number <= 0 || lot_id.is_nil() {
        return Err(());
    }
    Ok(PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id })
}

fn not_found_error() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::NOT_FOUND, "not_found", "请求的资源不存在")
}

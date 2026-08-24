//! Public 与 Admin 的 HTTP 表面适配器。
//!
//! 这里只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；竞价、结算和状态机规则
//! 留在领域与应用层。Public 与 Admin 是不同 surface，各自拥有独立的路由与前端产物。

#[path = "../admin/auctions.rs"]
mod admin_auctions;
#[path = "../admin/audit.rs"]
mod admin_audit;
#[path = "../admin/credentials.rs"]
mod admin_credentials;
#[path = "../admin/dashboard/mod.rs"]
mod admin_dashboard;
#[path = "../admin/forum.rs"]
mod admin_forum;
#[path = "../admin/intake.rs"]
mod admin_intake;
#[path = "../admin/lot_update/mod.rs"]
mod admin_lot_update;
#[path = "../admin/lots.rs"]
mod admin_lots;
#[path = "../admin/mail/mod.rs"]
mod admin_mail;
#[path = "../admin/proxy_events.rs"]
mod admin_proxy_events;
#[path = "../admin/renames.rs"]
mod admin_renames;
#[path = "../admin/rules.rs"]
mod admin_rules;
#[path = "../admin/runtime_config.rs"]
mod admin_runtime_config;
#[path = "../admin/users.rs"]
mod admin_users;
#[path = "../public/blacklist.rs"]
mod blacklist;
#[path = "../shared/boundary.rs"]
mod boundary;
#[path = "../public/compensation.rs"]
mod compensation;
#[path = "../admin/delivery.rs"]
mod delivery_admin;
#[path = "../public/forum.rs"]
mod forum;
#[path = "../metrics/mod.rs"]
mod metrics;
#[path = "../public/realtime.rs"]
mod realtime;
#[path = "../admin/realtime.rs"]
mod realtime_admin;
#[path = "../shared/realtime_hub/mod.rs"]
mod realtime_hub;
#[path = "../shared/response_cache.rs"]
mod response_cache;
#[path = "../public/rules.rs"]
mod rules;
#[path = "../security/mod.rs"]
mod security;
#[path = "../shared/server.rs"]
mod server;
#[path = "../shared/state/mod.rs"]
mod state;
#[cfg(target_os = "linux")]
#[path = "../shared/unix_listener.rs"]
mod unix_listener;

mod admin;
mod mapping;
mod public;
mod reads;
mod session;

pub(crate) use admin::*;
pub(crate) use mapping::*;
pub(crate) use public::*;
pub(crate) use reads::*;
pub(crate) use session::*;

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
#[cfg(target_os = "linux")]
pub use unix_listener::{UnixHttpListener, UnixListenerOptions};

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

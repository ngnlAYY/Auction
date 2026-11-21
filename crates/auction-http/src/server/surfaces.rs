//! Public/Admin 路由表与静态资源 fallback 装配。

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{Router, middleware};

use super::{health, metrics};
use crate::admin::alerts as admin_alerts;
use crate::admin::runtime_switches as admin_runtime_switches;
use crate::admin::{
    admin_cancel_auction, admin_confirm_recovery, admin_early_settlement_auction,
    admin_final_phase_auction, admin_open_auction, admin_preview_auction, admin_recovery_report,
    admin_settlement_report, current_admin_session, login_admin_session, logout_admin_session,
};
use crate::assets as web_assets;
use crate::middleware::boundary::HttpBoundary;
use crate::middleware::security::{AnonymousReadGate, WebScriptPolicy};
use crate::middleware::{boundary, method_strict, read_gate, response_cache, strict_json};
use crate::public::browser_errors;
use crate::public::{
    create_proxy_authorization, current_session, get_my_proxy_authorization,
    get_session_preference, get_watch_preference, list_lot_watches, list_my_bids, login_session,
    logout_session, lower_proxy_authorization, place_bid, refresh_session_cookie, set_lot_watch,
    set_session_preference, set_watch_preference,
};
use crate::reads::{
    auction_summary, current_auction, list_auctions, list_auctions_v2, list_bid_log, list_lots,
    list_price_history, list_worlds, lot_detail,
};
use crate::state::{AdminApiState, PublicApiState};
use crate::{admin as admin_handlers, public as public_handlers};

/// 公开端路由。
/// 健康检查路径沿用已冻结的旧契约；其余路径交给内嵌的公开端 SPA 产物处理。
/// `version` 由装配根传入自身的 `CARGO_PKG_VERSION`：契约 crate 内展开该宏只会得到契约
/// 自己的版本，而健康检查要报告的是运行进程的版本。
///
/// 测试专用的兼容入口：正式构建只导出要求显式策略的构造函数。
#[cfg(test)]
pub fn public_router<A>(
    version: &'static str,
    boundary: HttpBoundary,
    anonymous_read_gate: AnonymousReadGate,
) -> Router<PublicApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    public_router_with_web_script_policy::<A>(
        version,
        boundary,
        anonymous_read_gate,
        WebScriptPolicy::Dioxus,
    )
}

/// 公开端路由，并显式选择本 surface 的浏览器脚本策略。
pub fn public_router_with_web_script_policy<A>(
    version: &'static str,
    boundary: HttpBoundary,
    anonymous_read_gate: AnonymousReadGate,
    web_script_policy: WebScriptPolicy,
) -> Router<PublicApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    let health = move || health::health_response("public-api", version);

    public_feature_routes(
        Router::<PublicApiState>::new()
            .route("/health", get(health))
            .route("/api/v1/health", get(health))
            .route("/health/live", get(health::health_live))
            .route("/health/ready", get(health::public_health_ready))
            .route("/metrics", axum::routing::any(metrics::public)),
        anonymous_read_gate,
    )
    .fallback(move |request| web_assets::serve_public_asset::<A>(request, web_script_policy))
    .layer(middleware::from_fn_with_state(
        boundary.with_web_script_policy(web_script_policy),
        boundary::enforce,
    ))
}

/// 管理端路由。
/// 管理端与公开端由各自装配根传入不同资源类型，避免任一二进制引用另一端产物。
///
/// 测试专用的兼容入口：正式构建只导出要求显式策略的构造函数。
#[cfg(test)]
pub fn admin_router<A>(version: &'static str, boundary: HttpBoundary) -> Router<AdminApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    admin_router_with_web_script_policy::<A>(version, boundary, WebScriptPolicy::Dioxus)
}

/// 管理端路由，并显式选择本 surface 的浏览器脚本策略。
pub fn admin_router_with_web_script_policy<A>(
    version: &'static str,
    boundary: HttpBoundary,
    web_script_policy: WebScriptPolicy,
) -> Router<AdminApiState>
where
    A: rust_embed::RustEmbed + Send + Sync + 'static,
{
    let health = move || health::health_response("admin-api", version);

    admin_feature_routes(
        Router::<AdminApiState>::new()
            .route("/health", get(health))
            .route("/api/v1/health", get(health))
            .route("/health/live", get(health::health_live))
            .route("/health/ready", get(health::admin_health_ready))
            .route("/metrics", axum::routing::any(metrics::admin)),
    )
    .fallback(move |request| web_assets::serve_admin_asset::<A>(request, web_script_policy))
    .layer(middleware::from_fn_with_state(
        boundary.with_web_script_policy(web_script_policy),
        boundary::enforce,
    ))
}

/// 注册已冻结的 Public 路径。
/// 路由只负责绑定各自的 HTTP owner；未装配的窄端口仍由 handler 明确返回能力不可用，
/// 不用伪造的成功 payload 掩盖部署缺口。
fn public_feature_routes(
    router: Router<PublicApiState>,
    anonymous_read_gate: AnonymousReadGate,
) -> Router<PublicApiState> {
    use method_strict::{MethodRouterExt as _, get, post, put};

    // 公开目录读取与身份无关：默认只让浏览器保存 2 秒，处理器按内容的变化频率显式给出更长的
    // 档位（低频、已结算）；成功响应附带内容 ETag 并支持 304。
    let public_catalog_routes = Router::<PublicApiState>::new()
        .route("/api/v1/worlds", get(list_worlds))
        .route("/api/v1/registration-info", get(public_handlers::registration::get))
        .route("/api/v1/rules", get(public_handlers::rules::get))
        .route("/api/v1/about", get(public_handlers::rules::get_about))
        .route("/api/v1/blacklist", get(public_handlers::blacklist::list))
        .route("/api/v1/compensation-pool", get(public_handlers::compensation::pool))
        .route(
            "/api/v1/compensation-pool/{auction_id}/movements",
            get(public_handlers::compensation::movements),
        )
        .route(
            "/api/v1/compensation-pool/{auction_id}/lots",
            get(public_handlers::compensation::lots),
        )
        .route("/api/v1/auctions", get(list_auctions))
        .route("/api/v2/auctions", get(list_auctions_v2))
        .route("/api/v1/auctions/current", get(current_auction))
        .route("/api/v1/auctions/{auction_id}/summary", get(auction_summary))
        .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
        .route("/api/v1/lots/{lot_id}", get(lot_detail))
        .route("/api/v1/auctions/{auction_id}/bid-log", get(list_bid_log))
        .route("/api/v1/auctions/{auction_id}/fulfillment", get(public_handlers::fulfillment::list))
        .route("/api/v1/price-history", get(list_price_history))
        .route_layer(middleware::from_fn(response_cache::public_catalog))
        // 匿名读闸门独立于登录/mutation 限流；写路由不挂载该层。
        .route_layer(middleware::from_fn_with_state(
            anonymous_read_gate.clone(),
            read_gate::enforce,
        ));
    // 匿名读但包含实时事件、论坛镜像或运行状态观测的投影：保持 no-store，并只由读闸门约束并发与
    // 来源额度。论坛同步开关取自 Worker 的最新观测，心跳缺失时要立刻反映，不进任何缓存。
    let anonymous_read_routes = Router::<PublicApiState>::new()
        .route("/api/v1/forum/sync-status", get(public_handlers::forum::sync_status))
        .route("/api/v1/auctions/{auction_id}/forum", get(public_handlers::forum::get))
        .route("/api/v1/events", get(public_handlers::realtime::list))
        .route_layer(middleware::from_fn(response_cache::no_store))
        .route_layer(middleware::from_fn_with_state(
            anonymous_read_gate.clone(),
            read_gate::enforce,
        ));
    // 动态图片显式提供 HEAD；倒计时只做秒级缓存，其余 JSON GET 不隐式扩展方法闭集。
    let countdown_routes = Router::<PublicApiState>::new()
        .route(
            "/api/v1/auctions/{auction_id}/countdown/{timer}",
            method_strict::get_head(public_handlers::countdown::get),
        )
        .route_layer(middleware::from_fn(response_cache::short_lived))
        .route_layer(middleware::from_fn_with_state(anonymous_read_gate, read_gate::enforce));
    let no_store_routes = Router::<PublicApiState>::new()
        .route("/api/v1/browser-errors", post(browser_errors::public))
        .route(
            "/api/v1/session",
            axum::routing::get(current_session)
                .post(login_session)
                .put(refresh_session_cookie)
                .delete(logout_session)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("DELETE, GET, POST, PUT"),
        )
        .route("/api/v1/me/bids", get(list_my_bids))
        .route("/api/v1/me/lot-watches", get(list_lot_watches))
        .route(
            "/api/v1/me/lot-watch-preference",
            axum::routing::get(get_watch_preference)
                .put(set_watch_preference)
                .layer(DefaultBodyLimit::max(4 << 10))
                .strict_methods("GET, PUT"),
        )
        .route(
            "/api/v1/me/session-preference",
            axum::routing::get(get_session_preference)
                .put(set_session_preference)
                .layer(DefaultBodyLimit::max(4 << 10))
                .strict_methods("GET, PUT"),
        )
        .route(
            "/api/v1/lots/{lot_id}/watch",
            put(set_lot_watch).layer(DefaultBodyLimit::max(4 << 10)),
        )
        .route("/api/v1/lots/{lot_id}/proxy-authorizations/mine", get(get_my_proxy_authorization))
        .route("/api/v1/lots/{lot_id}/proxy-authorizations", post(create_proxy_authorization))
        .route(
            "/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower",
            post(lower_proxy_authorization),
        )
        .route("/api/v1/lots/{lot_id}/bids", post(place_bid))
        .route_layer(middleware::from_fn(response_cache::no_store));
    let routes = public_catalog_routes
        .merge(anonymous_read_routes)
        .merge(countdown_routes)
        .merge(no_store_routes)
        .route(
            "/api/v1/ws",
            // HTTP/1.1 使用 GET Upgrade，HTTP/2 使用扩展 CONNECT；两者共用会话与范围门禁。
            axum::routing::get(public_handlers::realtime::websocket)
                .connect(public_handlers::realtime::websocket)
                .strict_methods("CONNECT, GET"),
        );
    // 不变量 1、6、7：写入口保留寄售自身的守卫、严格解码和原命令仲裁。
    public_handlers::consignment::mount(
        router.merge(routes.route_layer(middleware::from_fn(boundary::normalize_api_rejection))),
    )
}

/// 注册已冻结的 Admin 路径。
fn admin_feature_routes(router: Router<AdminApiState>) -> Router<AdminApiState> {
    use method_strict::{MethodRouterExt as _, get, patch, post, put};

    let routes = Router::<AdminApiState>::new()
        .route("/api/v1/browser-errors", post(browser_errors::admin))
        .route(
            "/api/v1/admin/session",
            axum::routing::get(current_admin_session)
                .post(login_admin_session)
                .delete(logout_admin_session)
                .layer(DefaultBodyLimit::max(4 << 10))
                .strict_methods("DELETE, GET, POST"),
        )
        .route("/api/v1/admin/session/activity", post(admin_handlers::renew_admin_session))
        .route("/api/v1/admin/dashboard", get(admin_handlers::dashboard::get))
        .route("/api/v1/admin/worlds", get(admin_handlers::worlds::list))
        .route("/api/v1/admin/worlds/{world_id}", patch(admin_handlers::worlds::set_enabled))
        .route(
            "/api/v1/admin/auctions",
            axum::routing::get(admin_handlers::auctions::list)
                .post(admin_handlers::auctions::create)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("GET, POST"),
        )
        .route(
            "/api/v1/admin/worlds/{world_id}/auction-intake/items",
            post(admin_handlers::intake::add_item).layer(DefaultBodyLimit::max(72 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(72 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/forum/sync-status", get(admin_handlers::forum::sync_status))
        .route("/api/v1/admin/auctions/{auction_id}/forum", get(admin_handlers::forum::get))
        .route("/api/v1/admin/auctions/{auction_id}/lots", get(admin_handlers::lots::list))
        .route(
            "/api/v1/admin/auctions/{auction_id}/settlement-report",
            get(admin_settlement_report),
        )
        .route("/api/v1/admin/settlement-catalog", get(admin_handlers::settlement_catalog::list))
        .route("/api/v1/admin/auctions/{auction_id}/recovery-report", get(admin_recovery_report))
        .route("/api/v1/admin/auctions/{auction_id}/preview", post(admin_preview_auction))
        .route(
            "/api/v1/admin/auctions/{auction_id}/schedule",
            patch(admin_handlers::auctions::update_schedule)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::patch(4 << 10),
                    strict_json::enforce,
                )),
        )
        .route("/api/v1/admin/auctions/{auction_id}/open", post(admin_open_auction))
        .route("/api/v1/admin/auctions/{auction_id}/cancel", post(admin_cancel_auction))
        .route("/api/v1/admin/auctions/{auction_id}/final-phase", post(admin_final_phase_auction))
        .route(
            "/api/v1/admin/auctions/{auction_id}/early-settlement",
            post(admin_early_settlement_auction),
        )
        .route(
            "/api/v1/admin/lots/{lot_id}",
            patch(admin_handlers::lot_update::update).layer(DefaultBodyLimit::max(8 << 10)).layer(
                middleware::from_fn_with_state(strict_json::patch(8 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/recoveries/{recovery_id}/confirm",
            post(admin_confirm_recovery).layer(DefaultBodyLimit::max(8 << 10)),
        )
        .route("/api/v1/admin/lots/{lot_id}/proxy-events", get(admin_handlers::proxy_events::list))
        .route("/api/v1/admin/mail", get(admin_handlers::mail::state))
        .route("/api/v1/admin/mail/intake", get(admin_handlers::mail::list_intakes))
        .route("/api/v1/admin/mail/intake/{intake_id}", get(admin_handlers::mail::detail_intake))
        .route(
            "/api/v1/admin/mail/polling",
            put(admin_handlers::mail::set_paused).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::put(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/mail/polls:trigger",
            post(admin_handlers::mail::trigger_poll).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/realtime/events", get(admin_handlers::realtime::list_events))
        .route("/api/v1/admin/realtime/lag", get(admin_handlers::realtime::list_lag))
        .route("/api/v1/admin/realtime/failures", get(admin_handlers::realtime::list_failures))
        .route(
            "/api/v1/admin/realtime/failures/{failure_id}/replay",
            post(admin_handlers::realtime::replay).layer(DefaultBodyLimit::max(64 << 10)).layer(
                middleware::from_fn_with_state(
                    strict_json::post_with_invalid_json(64 << 10, "invalid_replay", "重放参数无效"),
                    strict_json::enforce,
                ),
            ),
        )
        .route(
            "/api/v1/admin/rules",
            axum::routing::get(admin_handlers::rules::get)
                .post(admin_handlers::rules::validate)
                .put(admin_handlers::rules::publish)
                .layer(DefaultBodyLimit::max(1 << 20))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_put(1 << 20),
                    strict_json::enforce,
                ))
                .strict_methods("GET, POST, PUT"),
        )
        // “关于”与规则共用同一套文档发布机制，路径、请求体上限与方法闭集保持一致。
        .route(
            "/api/v1/admin/about",
            axum::routing::get(admin_handlers::rules::get_about)
                .post(admin_handlers::rules::validate_about)
                .put(admin_handlers::rules::publish_about)
                .layer(DefaultBodyLimit::max(1 << 20))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_put(1 << 20),
                    strict_json::enforce,
                ))
                .strict_methods("GET, POST, PUT"),
        )
        .route(
            "/api/v1/admin/runtime-switches",
            axum::routing::get(admin_runtime_switches::get).strict_methods("GET"),
        )
        .route(
            "/api/v1/admin/runtime-switches/external-access-paused",
            post(admin_runtime_switches::pause_external_access)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/mail-templates",
            axum::routing::get(admin_handlers::mail_templates::list).strict_methods("GET"),
        )
        // 单个模板正文上限 64 KiB；请求体另含命令 ID 与版本，放宽到 96 KiB。
        .route(
            "/api/v1/admin/mail-templates/{template_key}",
            put(admin_handlers::mail_templates::update)
                .layer(DefaultBodyLimit::max(96 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::put(96 << 10),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/config",
            axum::routing::get(admin_handlers::runtime_config::get)
                .put(admin_handlers::runtime_config::update)
                .layer(DefaultBodyLimit::max(4 << 20))
                .layer(middleware::from_fn_with_state(
                    strict_json::put(4 << 20),
                    strict_json::enforce,
                ))
                .strict_methods("GET, PUT"),
        )
        .route(
            "/api/v1/admin/config:validate",
            post(admin_handlers::runtime_config::validate)
                .layer(DefaultBodyLimit::max(4 << 20))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 20),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/users",
            axum::routing::get(admin_handlers::users::list)
                .post(admin_handlers::users::create)
                .patch(admin_handlers::users::mutate)
                .layer(DefaultBodyLimit::max(16 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_patch(16 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("GET, PATCH, POST"),
        )
        .route("/api/v1/admin/renames", get(admin_handlers::renames::list))
        .route(
            "/api/v1/admin/renames/{rename_id}/revert",
            post(admin_handlers::users::revert_rename).layer(DefaultBodyLimit::max(8 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(8 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/users/{user_id}/blacklist",
            axum::routing::put(admin_handlers::users::blacklist)
                .delete(admin_handlers::users::unblacklist)
                .layer(DefaultBodyLimit::max(8 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::put_delete(8 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("DELETE, PUT"),
        )
        .route(
            "/api/v1/admin/credentials",
            axum::routing::get(admin_handlers::credentials::list)
                .post(admin_handlers::credentials::create)
                .patch(admin_handlers::credentials::mutate)
                .delete(admin_handlers::credentials::delete)
                .layer(DefaultBodyLimit::max(128 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_patch_delete(128 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("DELETE, GET, PATCH, POST"),
        )
        .route(
            "/api/v1/admin/credentials/{credential_id}/check",
            post(admin_handlers::credentials::check).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/alerts", get(admin_alerts::list))
        .route("/api/v1/admin/alerts/webhooks", get(admin_alerts::webhooks))
        .route("/api/v1/admin/pending-counts", get(admin_alerts::pending_counts))
        .route("/api/v1/admin/audit", get(admin_handlers::audit::list))
        .route(
            "/api/v1/admin/alerts/{alert_id}/acknowledge",
            post(admin_alerts::acknowledge).layer(DefaultBodyLimit::max(13 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(13 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/alerts/{alert_id}/silence",
            post(admin_alerts::silence).layer(DefaultBodyLimit::max(13 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(13 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/settlement-returns", get(admin_handlers::settlement_returns::list))
        .route(
            "/api/v1/admin/settlement-returns/{return_id}",
            get(admin_handlers::settlement_returns::detail),
        )
        .route(
            "/api/v1/admin/settlement-returns/{return_id}/recheck",
            post(admin_handlers::settlement_returns::recheck)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/settlement-returns/{return_id}/handle",
            post(admin_handlers::settlement_returns::handle)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                )),
        )
        .route("/api/v1/admin/deliveries", get(admin_handlers::delivery::list))
        .route("/api/v1/admin/player-support", get(admin_handlers::delivery::player_support))
        .route("/api/v1/admin/deliveries/{delivery_id}", get(admin_handlers::delivery::detail))
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/sent-proof/preview",
            post(admin_handlers::delivery::sent_proof_preview)
                .layer(DefaultBodyLimit::max(16 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(16 << 10),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/sent-proof/confirm",
            post(admin_handlers::delivery::sent_proof_confirm)
                .layer(DefaultBodyLimit::max(16 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(16 << 10),
                    strict_json::enforce,
                )),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/release",
            post(admin_handlers::delivery::release).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/retry",
            post(admin_handlers::delivery::retry).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/reconcile",
            post(admin_handlers::delivery::reconcile).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/acknowledge",
            post(admin_handlers::delivery::acknowledge)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                )),
        );
    let router = router.merge(
        routes
            .route_layer(middleware::from_fn(response_cache::no_store))
            .route_layer(middleware::from_fn(boundary::normalize_api_rejection)),
    );
    // 不变量 1、6、8：只有正式 Admin 路由接入下架命令，并沿用既有会话与审计边界。
    admin_handlers::consignment::mount(router)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use auction_application::auction::{
        AuctionReader, CurrentAuctionSnapshot, PublicAccountReader, PublicAuctionCountdown,
        PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage, PublicAuctionProjection,
        PublicEquipment, PublicLiveLotDetail, PublicLiveLotSummary, PublicLotDetail,
        PublicLotListRequest, PublicLotPage, PublicLotReader, PublicMyBid, PublicMyBidsPage,
        PublicMyBidsRequest, PublicOwnedProxy, PublicProxyBid, PublicProxyCapabilities,
        PublicProxyTransition, PublicRealtimeAuctionStatus, WorldReader, WorldSnapshot,
    };
    use auction_application::consignment::{
        AdminListingDetail, AdminListingQuery, AdminListingRead, AdminListingRecord,
        ConsignmentFuture, ListingReadPage,
    };
    use auction_application::core::{ApplicationError, ApplicationFuture};
    use auction_application::forum::{EquipmentGroup, PublicForumReader, PublicForumThread};
    use auction_application::public::{
        LotWatch, LotWatchError, LotWatchFuture, LotWatchPort, LotWatchPreference,
        LotWatchPreferencePort, PublicBlacklistEntry, PublicBlacklistPage, PublicBlacklistReader,
        PublicBlacklistRequest, PublicRulesDocument, PublicRulesReader, PublicRulesRequest,
        PublicSessionPreference, SetLotWatchCommand, SetLotWatchPreferenceCommand,
        SetLotWatchResult, SiteDocumentKind,
    };
    use auction_application::public::{
        PublicAuctionSummaryReader, PublicLoginResult, PublicSessionIdentity, PublicSessionPort,
        PublicSessionPreferenceResult, SetPublicSessionPreferenceCommand, SettledAuctionSummary,
    };
    use auction_application::realtime::{
        PublicRealtimeEvent, PublicRealtimePage, PublicRealtimeReader, PublicRealtimeRequest,
    };
    use auction_contracts::ApiErrorResponse;
    use auction_domain::EquipmentId;
    use auction_domain::consignment::FeeBps;
    use auction_time::{Duration, Timestamp};
    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{HeaderValue, Method, Request, StatusCode, header},
        response::Response,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{
        admin_router_with_web_script_policy, public_feature_routes, public_router,
        public_router_with_web_script_policy,
    };
    use crate::mapping::encode_public_cursor;
    use crate::middleware::response_cache::SETTLED_CACHE_CONTROL;
    use crate::middleware::security::PublicHttpSecurity;
    use crate::server::contract_fixtures::{
        AUCTION_ID, admin_get, admin_request, admin_state, assert_http, response_json,
    };
    use crate::state::{AdminApiState, PublicApiState};
    use crate::{AdminConsignmentCapabilities, ConsignmentCapabilities, WebScriptPolicy};

    fn app() -> Router {
        let gate = PublicHttpSecurity::default().anonymous_read_gate();
        public_feature_routes(Router::<PublicApiState>::new(), gate)
            .with_state(PublicApiState::default())
    }

    async fn response(uri: &str) -> axum::response::Response {
        app()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response")
    }

    struct NoAssets;

    impl rust_embed::RustEmbed for NoAssets {
        fn get(_path: &str) -> Option<rust_embed::EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            std::iter::empty()
        }
    }

    #[tokio::test]
    async fn public_consignment_status_and_catalog_use_config_without_database() {
        for (state, enabled, fee_bps, catalog_status, catalog_code) in [
            (PublicApiState::default(), false, 500, StatusCode::NOT_FOUND, "consignment_disabled"),
            (
                PublicApiState::default().with_consignment(
                    ConsignmentCapabilities::without_database(
                        false,
                        FeeBps::new(615).expect("费率"),
                    ),
                ),
                false,
                615,
                StatusCode::NOT_FOUND,
                "consignment_disabled",
            ),
            (
                PublicApiState::default().with_consignment(
                    ConsignmentCapabilities::without_database(
                        true,
                        FeeBps::new(735).expect("费率"),
                    ),
                ),
                true,
                735,
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            ),
        ] {
            let app = public_router::<NoAssets>(
                "fixture",
                state.http_boundary(),
                state.anonymous_read_gate(),
            )
            .with_state(state);
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/consignment/status")
                        .header("x-request-id", "fixture-consignment")
                        .body(Body::empty())
                        .expect("能力请求"),
                )
                .await
                .expect("能力响应");
            assert_eq!(response.headers()["x-request-id"], "fixture-consignment");
            assert_eq!(
                consignment_json(StatusCode::OK, response).await["data"],
                serde_json::json!({
                    "enabled": enabled,
                    "registration_ready": false,
                    "registration_modes": [],
                    "registration_worlds": [],
                    "accepts_mail": false,
                    "purchasable": false,
                    "fee_bps": fee_bps,
                    "min_price": 1,
                    "max_price": 1_000_000_000,
                }),
            );

            for path in [
                "/api/v1/consignment/listings",
                "/api/v1/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e",
            ] {
                let response = app
                    .clone()
                    .oneshot(Request::builder().uri(path).body(Body::empty()).expect("目录请求"))
                    .await
                    .expect("目录响应");
                assert_eq!(consignment_json(catalog_status, response).await["code"], catalog_code);
            }

            let response = app
                .oneshot(
                    Request::builder().uri("/consignment").body(Body::empty()).expect("入口请求"),
                )
                .await
                .expect("入口响应");
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "enabled={enabled}");
        }
    }

    async fn consignment_json(status: StatusCode, response: Response) -> serde_json::Value {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["cdn-cache-control"], "no-store");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        response_json(status, response).await
    }

    #[tokio::test]
    async fn public_consignment_private_routes_require_ports_even_when_disabled() {
        for enabled in [false, true] {
            let state = public_state_with_session().with_consignment(
                ConsignmentCapabilities::without_database(enabled, FeeBps::DEFAULT_CONSIGNED),
            );
            let app = public_router::<NoAssets>(
                "fixture",
                state.http_boundary(),
                state.anonymous_read_gate(),
            )
            .with_state(state);
            for (method, path, body) in [
                (Method::GET, "/api/v1/me/consignment/listings", ""),
                (
                    Method::GET,
                    "/api/v1/me/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e",
                    "",
                ),
                (
                    Method::GET,
                    "/api/v1/me/consignment/commands/0f8fad5b-d9cb-469f-a165-70867728950e",
                    "",
                ),
                (
                    Method::POST,
                    "/api/v1/consignment/listings",
                    r#"{"command_id":"0f8fad5b-d9cb-469f-a165-70867728950e","mode":"direct","equipment_url":"fixture-placeholder","price":1,"expected_fee_bps":0}"#,
                ),
                (
                    Method::PATCH,
                    "/api/v1/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e/price",
                    r#"{"command_id":"0f8fad5b-d9cb-469f-a165-70867728950e","expected_version":1,"price":1}"#,
                ),
                (
                    Method::POST,
                    "/api/v1/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e/withdraw",
                    r#"{"command_id":"0f8fad5b-d9cb-469f-a165-70867728950e","expected_version":1}"#,
                ),
            ] {
                let mut request = Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, "auction_public_session=fixture-placeholder")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("本人请求");
                request
                    .extensions_mut()
                    .insert(ConnectInfo(std::net::SocketAddr::from(([192, 0, 2, 21], 3000))));
                let response = app.clone().oneshot(request).await.expect("本人响应");
                assert_eq!(
                    consignment_json(StatusCode::SERVICE_UNAVAILABLE, response).await["code"],
                    "consignment_unavailable",
                    "{path}, enabled={enabled}",
                );
            }
        }
    }

    struct EmptyAdminConsignment;

    impl AdminListingRead for EmptyAdminConsignment {
        fn list_admin(
            &self,
            query: AdminListingQuery,
        ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>> {
            Box::pin(async move {
                Ok(ListingReadPage { items: Vec::new(), total: 0, window: query.listings.window })
            })
        }

        fn admin_detail(
            &self,
            _listing_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<AdminListingDetail>> {
            Box::pin(async { Ok(None) })
        }
    }

    #[tokio::test]
    async fn formal_admin_consignment_uses_injected_read_port_and_keeps_missing_ports_unavailable()
    {
        let path = "/api/v1/admin/consignment/listings";
        let response = admin_get(admin_state(), path).await;
        assert_eq!(
            consignment_json(StatusCode::SERVICE_UNAVAILABLE, response).await["code"],
            "consignment_unavailable",
        );

        let state = admin_state().with_consignment(AdminConsignmentCapabilities {
            read: Some(Arc::new(EmptyAdminConsignment)),
            ..AdminConsignmentCapabilities::default()
        });
        let response =
            admin_get(state.clone(), "/api/v1/admin/consignment/listings?page=2&page_size=37")
                .await;
        assert_eq!(
            consignment_json(StatusCode::OK, response).await["data"],
            serde_json::json!({"items":[],"total":0,"page":2,"page_size":37}),
        );

        let response = admin_get(
            state.clone(),
            "/api/v1/admin/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e",
        )
        .await;
        assert_eq!(
            consignment_json(StatusCode::NOT_FOUND, response).await["code"],
            "listing_not_found"
        );

        let response = admin_request(
            state,
            Method::POST,
            "/api/v1/admin/consignment/listings/0f8fad5b-d9cb-469f-a165-70867728950e/delist",
            Body::from(r#"{"command_id":"0f8fad5b-d9cb-469f-a165-70867728950e","expected_version":1,"reason":"测试下架"}"#),
        ).await;
        assert_eq!(
            consignment_json(StatusCode::SERVICE_UNAVAILABLE, response).await["code"],
            "consignment_unavailable",
        );
    }

    #[tokio::test]
    async fn consignment_routes_are_not_exposed_on_the_other_surface() {
        let public = PublicApiState::default();
        let app = public_router::<NoAssets>(
            "fixture",
            public.http_boundary(),
            public.anonymous_read_gate(),
        )
        .with_state(public);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/consignment/listings")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .expect("Public 请求"),
            )
            .await
            .expect("Public 响应");
        assert_eq!(consignment_json(StatusCode::NOT_FOUND, response).await["code"], "not_found");

        let response = admin_get(admin_state(), "/api/v1/consignment/status").await;
        assert_eq!(consignment_json(StatusCode::NOT_FOUND, response).await["code"], "not_found");
    }

    /// 只提供倒计时快照，用于验证 SVG 覆盖发生在通用脚本策略之后。
    struct CountdownAuctions(Uuid);

    impl AuctionReader for CountdownAuctions {
        fn list_public_auctions(
            &self,
            _request: PublicAuctionListRequest,
        ) -> ApplicationFuture<'_, PublicAuctionPage> {
            Box::pin(async {
                Err(ApplicationError::CapabilityUnavailable("测试不提供目录"))
            })
        }

        fn current_auction(
            &self,
            _world_id: String,
        ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>> {
            Box::pin(async { Ok(None) })
        }

        fn public_auction_countdown(
            &self,
            auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicAuctionCountdown>> {
            let expected = self.0;
            Box::pin(async move {
                let observed_at = Timestamp::from_timestamp(1_800_000_000, 0).expect("valid time");
                Ok((auction_id == expected).then(|| PublicAuctionCountdown {
                    auction_id,
                    intake_closes_at: observed_at + Duration::seconds(3_600),
                    scheduled_end_at: Some(observed_at + Duration::seconds(7_200)),
                    observed_at,
                }))
            })
        }

        fn public_realtime_auction_status(
            &self,
            _auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicRealtimeAuctionStatus>> {
            Box::pin(async { Ok(None) })
        }
    }

    const COUNTDOWN_SVG_CSP: &str = "default-src 'none'; script-src 'none'; style-src \
                                     'unsafe-inline'; object-src 'none'; base-uri 'none'; \
                                     frame-ancestors 'none'";
    const DIOXUS_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-eval' \
                              'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                              object-src 'none'; base-uri 'self'; form-action 'self'; \
                              frame-ancestors 'none'";
    const SOLID_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' \
                             'unsafe-inline'; object-src 'none'; base-uri 'self'; \
                             form-action 'self'; frame-ancestors 'none'";

    fn csp(response: &Response) -> &str {
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .expect("csp")
    }

    async fn public_health(web_script_policy: WebScriptPolicy) -> Response {
        let state = PublicApiState::default();
        let boundary = state.http_boundary();
        let gate = state.anonymous_read_gate();
        public_router_with_web_script_policy::<NoAssets>(
            env!("CARGO_PKG_VERSION"),
            boundary,
            gate,
            web_script_policy,
        )
        .with_state(state)
        .oneshot(Request::builder().uri("/health").body(Body::empty()).expect("request"))
        .await
        .expect("response")
    }

    async fn admin_health(web_script_policy: WebScriptPolicy) -> Response {
        let state = AdminApiState::default();
        let boundary = state.http_boundary();
        admin_router_with_web_script_policy::<NoAssets>(
            env!("CARGO_PKG_VERSION"),
            boundary,
            web_script_policy,
        )
        .with_state(state)
        .oneshot(Request::builder().uri("/health").body(Body::empty()).expect("request"))
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn health_responses_use_the_route_script_policy() {
        for (web_script_policy, expected) in
            [(WebScriptPolicy::Dioxus, DIOXUS_CSP), (WebScriptPolicy::Solid, SOLID_CSP)]
        {
            let public = public_health(web_script_policy).await;
            assert_eq!(public.status(), StatusCode::OK, "public {web_script_policy:?}");
            assert_eq!(csp(&public), expected, "public {web_script_policy:?}");

            let admin = admin_health(web_script_policy).await;
            assert_eq!(admin.status(), StatusCode::OK, "admin {web_script_policy:?}");
            assert_eq!(csp(&admin), expected, "admin {web_script_policy:?}");
        }
    }

    #[tokio::test]
    async fn invalid_request_id_uses_the_route_script_policy() {
        for (web_script_policy, expected) in
            [(WebScriptPolicy::Dioxus, DIOXUS_CSP), (WebScriptPolicy::Solid, SOLID_CSP)]
        {
            let state = PublicApiState::default();
            let boundary = state.http_boundary();
            let gate = state.anonymous_read_gate();
            let app = public_router_with_web_script_policy::<NoAssets>(
                env!("CARGO_PKG_VERSION"),
                boundary,
                gate,
                web_script_policy,
            )
            .with_state(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .header("x-request-id", "bad id")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(csp(&response), expected, "{web_script_policy:?}");
        }
    }

    #[tokio::test]
    async fn countdown_svg_overrides_each_surface_script_policy_last() {
        let auction_id = Uuid::from_u128(0x018f_4c8e_7b2a_7c3d_8e4f_0123_4567_89ab);
        for web_script_policy in [WebScriptPolicy::Dioxus, WebScriptPolicy::Solid] {
            let state = PublicApiState::new(
                None,
                None,
                None,
                None,
                None,
                Some(Arc::new(CountdownAuctions(auction_id))),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                PublicHttpSecurity::default(),
            );
            let gate = state.anonymous_read_gate();
            let boundary = state.http_boundary();
            let app = public_router_with_web_script_policy::<NoAssets>(
                env!("CARGO_PKG_VERSION"),
                boundary,
                gate,
                web_script_policy,
            )
            .with_state(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/auctions/{auction_id}/countdown/list.svg"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .and_then(|value| value.to_str().ok()),
                Some(COUNTDOWN_SVG_CSP)
            );
            assert_eq!(
                response
                    .headers()
                    .get("cross-origin-resource-policy")
                    .and_then(|value| value.to_str().ok()),
                Some("cross-origin")
            );
        }
    }

    #[tokio::test]
    async fn auction_summary_route_reports_missing_storage_without_the_state_port() {
        let response =
            summary_response(&format!("{AUCTION_ID}/summary"), PublicApiState::default(), None)
                .await;
        assert_no_summary_cache(&response);
        let body = response_json(StatusCode::SERVICE_UNAVAILABLE, response).await;
        assert_eq!(body["code"], "storage_unavailable");
    }

    struct SummaryReader {
        status: Option<&'static str>,
        effective_end_at: Option<Timestamp>,
    }

    impl PublicAuctionSummaryReader for SummaryReader {
        fn read_settled_auction_summary(
            &self,
            auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<SettledAuctionSummary>> {
            assert_eq!(auction_id, AUCTION_ID);
            Box::pin(async move {
                Ok((self.status == Some("settled")).then(|| SettledAuctionSummary {
                    id: auction_id,
                    world_id: "main".to_owned(),
                    sequence: 5,
                    effective_end_at: self.effective_end_at,
                }))
            })
        }
    }

    async fn summary_response(
        path: &str,
        state: PublicApiState,
        validator: Option<HeaderValue>,
    ) -> Response {
        let app = public_router::<NoAssets>(
            "fixture",
            state.http_boundary(),
            state.anonymous_read_gate(),
        )
        .with_state(state);
        let mut request =
            Request::get(format!("/api/v1/auctions/{path}")).body(Body::empty()).unwrap();
        if let Some(validator) = validator {
            request.headers_mut().insert(header::IF_NONE_MATCH, validator);
        }
        app.oneshot(request).await.unwrap()
    }

    fn assert_no_summary_cache(response: &Response) {
        for name in ["cache-control", "cdn-cache-control", "cloudflare-cdn-cache-control"] {
            assert_eq!(response.headers()[name], "no-store");
        }
        assert!(!response.headers().contains_key(header::ETAG));
    }

    #[tokio::test]
    async fn settled_auction_summary_matches_the_fixture_and_supports_revalidation() {
        const CDN_CACHE: &str = "public, max-age=86400, stale-while-revalidate=604800";
        let path = format!("{AUCTION_ID}/summary");
        let reader = SummaryReader {
            status: Some("settled"),
            effective_end_at: Some(
                Timestamp::from_timestamp(1_763_200_000, 0).unwrap() + Duration::days(2),
            ),
        };
        let state = PublicApiState::default().with_auction_summary(Arc::new(reader));
        let response = summary_response(&path, state.clone(), None).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], SETTLED_CACHE_CONTROL);
        assert_eq!(response.headers()["cdn-cache-control"], CDN_CACHE);
        assert_eq!(response.headers()["cloudflare-cdn-cache-control"], CDN_CACHE);
        let etag = response.headers()[header::ETAG].clone();
        assert_http("public/auction-summary", StatusCode::OK, response).await;

        let response = summary_response(&path, state, Some(etag.clone())).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], etag);
        assert_eq!(response.headers()[header::CACHE_CONTROL], SETTLED_CACHE_CONTROL);
        assert_eq!(response.headers()["cdn-cache-control"], CDN_CACHE);
        assert_eq!(response.headers()["cloudflare-cdn-cache-control"], CDN_CACHE);
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
        assert!(to_bytes(response.into_body(), usize::MAX).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn auction_summary_preserves_a_null_effective_end() {
        let reader = SummaryReader { status: Some("settled"), effective_end_at: None };
        let state = PublicApiState::default().with_auction_summary(Arc::new(reader));
        let response = summary_response(&format!("{AUCTION_ID}/summary"), state, None).await;
        let body = response_json(StatusCode::OK, response).await;
        assert_eq!(body["data"].get("effective_end_at"), Some(&serde_json::Value::Null));
    }

    #[tokio::test]
    async fn auction_summary_does_not_cache_unsettled_or_missing_auctions() {
        for status in [Some("live"), None] {
            let reader = SummaryReader { status, effective_end_at: None };
            let state = PublicApiState::default().with_auction_summary(Arc::new(reader));
            let response = summary_response(
                &format!("{AUCTION_ID}/summary"),
                state,
                Some(HeaderValue::from_static("*")),
            )
            .await;
            assert_no_summary_cache(&response);
            let actual = response_json(StatusCode::NOT_FOUND, response).await;
            assert_eq!(
                actual,
                serde_json::json!({
                    "code": "not_found",
                    "message": "请求的资源不存在",
                    "data": null,
                    "request_id": null,
                })
            );
        }
    }

    #[tokio::test]
    async fn auction_summary_rejects_invalid_ids_and_query_keys_before_reading() {
        for (path, code) in [
            ("bad/summary".to_owned(), "invalid_auction_id"),
            ("00000000-0000-0000-0000-000000000000/summary".to_owned(), "invalid_auction_id"),
            (format!("{}/summary", AUCTION_ID.to_string().to_uppercase()), "invalid_auction_id"),
            ("%FF/summary".to_owned(), "invalid_query"),
            (format!("{AUCTION_ID}/summary?world=main"), "invalid_query"),
        ] {
            let response = summary_response(&path, PublicApiState::default(), None).await;
            assert_no_summary_cache(&response);
            let body = response_json(StatusCode::BAD_REQUEST, response).await;
            assert_eq!(body["code"], code);
        }
    }

    #[tokio::test]
    async fn lots_routes_are_in_the_public_catalog_cache_group() {
        let id = "018f4c8e-7b2a-7c3d-8e4f-0123456789ab";
        for uri in [
            format!("/api/v1/auctions/{id}/lots"),
            format!("/api/v1/lots/{id}"),
            format!("/api/v1/auctions/{id}/fulfillment"),
        ] {
            let response = response(&uri).await;
            // 未注入端口时返回依赖不可用；失败响应不由目录缓存层写头。
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "uri={uri}");
            assert!(response.headers().get(header::CACHE_CONTROL).is_none(), "uri={uri}");
        }
        // 注册说明同在目录缓存组：失败响应不带缓存头，交给边界默认拒绝存储。
        let registration = response("/api/v1/registration-info").await;
        assert_eq!(registration.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(registration.headers().get(header::CACHE_CONTROL).is_none());
        // 对照：仍在 no-store 分组的运行状态观测即使失败也显式禁止存储。
        let sync_status = response("/api/v1/forum/sync-status").await;
        assert_eq!(sync_status.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            sync_status.headers().get(header::CACHE_CONTROL).and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
    }

    async fn rejection(uri: &str) -> (StatusCode, ApiErrorResponse) {
        let response = response(uri).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        (status, serde_json::from_slice(&body).expect("error envelope"))
    }

    #[tokio::test]
    async fn catalog_page_rejections_keep_each_route_error_shell() {
        // 未知参数、非正页码、页码乘法溢出与越过偏移上限都在读取端口之前以同一外壳拒绝。
        for uri in [
            "/api/v1/auctions?unknown=1",
            "/api/v1/price-history?page=0",
            "/api/v1/auctions?page=9223372036854775807",
            "/api/v1/auctions?page=502&page_size=20",
        ] {
            let (status, body) = rejection(uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "uri={uri}");
            assert_eq!(
                (body.code.as_str(), body.message.as_str()),
                ("invalid_query", "查询参数无效")
            );
        }
        // 自带文案的分页拒绝保留各自的 code 与 message。
        let (status, body) = rejection("/api/v1/compensation-pool?page=9223372036854775807").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!((body.code.as_str(), body.message.as_str()), ("invalid_query", "分页参数无效"));
        let (status, body) = rejection("/api/v1/blacklist?page=9223372036854775807").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            (body.code.as_str(), body.message.as_str()),
            ("invalid_pagination", "分页参数无效")
        );
        // 对照：恰好落在偏移上限上的页码通过参数校验，未装配端口时才返回依赖不可用。
        let boundary = response("/api/v1/auctions?page=501&page_size=20").await;
        assert_eq!(boundary.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn realtime_events_only_replay_a_single_auction_stream() {
        // 公开回放只有单拍卖流：`scope`（含 `scope=auctions`）与缺失拍卖都按未知/非法查询拒绝。
        let auction_id = "1f0c0000-0000-4000-8000-000000000002";
        for uri in [
            "/api/v1/events?scope=auctions".to_owned(),
            format!("/api/v1/events?auction_id={auction_id}&scope=auctions"),
            "/api/v1/events?after=0".to_owned(),
        ] {
            let (status, body) = rejection(&uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "uri={uri}");
            assert_eq!(
                (body.code.as_str(), body.message.as_str()),
                ("invalid_query", "查询参数无效")
            );
        }
        // 对照：合法单拍卖目标通过参数校验，未装配实时端口时才返回依赖不可用。
        let accepted = response(&format!("/api/v1/events?auction_id={auction_id}")).await;
        assert_eq!(accepted.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// 只认证任意 Cookie 的会话替身，让 `/api/v1/me/bids` 走到查询解析；其余方法不会被调用。
    struct AnyCookieSession;

    fn unused_session_call<T: Send + 'static>() -> ApplicationFuture<'static, T> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable("测试会话替身不支持该操作"))
        })
    }

    impl PublicSessionPort for AnyCookieSession {
        fn inspect(
            &self,
            _raw_token: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionIdentity> {
            unused_session_call()
        }

        fn login(
            &self,
            _key: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicLoginResult> {
            unused_session_call()
        }

        fn current(
            &self,
            _raw_token: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionIdentity> {
            Box::pin(async {
                Ok(PublicSessionIdentity {
                    user_id: Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b),
                    username: "page-size-reader".to_owned(),
                    expires_at: None,
                    absolute_expires_at: None,
                })
            })
        }

        fn logout(&self, _raw_token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            unused_session_call()
        }

        fn get_preference(
            &self,
            raw_token: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
            Box::pin(async move {
                let identity = self.current(raw_token, now).await?;
                Ok(PublicSessionPreferenceResult {
                    preference: PublicSessionPreference {
                        user_id: identity.user_id,
                        duration_seconds: None,
                        version: 1,
                    },
                    identity,
                })
            })
        }

        fn set_preference(
            &self,
            _raw_token: String,
            _command: SetPublicSessionPreferenceCommand,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
            unused_session_call()
        }
    }

    fn public_state_with_session() -> PublicApiState {
        PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(AnyCookieSession)),
            PublicHttpSecurity::default(),
        )
    }

    /// 只装配会话、不装配任何读取端口：通过参数校验的列表请求返回 503，被参数校验拒绝的返回
    /// 400，因此 400 同时证明拒绝发生在读取端口之前。
    async fn page_size_response(uri: &str) -> (StatusCode, Option<String>) {
        let state = public_state_with_session();
        let gate = PublicHttpSecurity::default().anonymous_read_gate();
        let response = public_feature_routes(Router::<PublicApiState>::new(), gate)
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, "auction_public_session=page-size-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let code = serde_json::from_slice::<ApiErrorResponse>(&body).ok().map(|error| error.code);
        (status, code)
    }

    #[tokio::test]
    async fn every_public_list_route_caps_page_size_before_reading() {
        const AUCTION_ID: &str = "018f4c8e-7b2a-7c3d-8e4f-0123456789ab";
        // 页长取值错误：黑名单沿用其已发布的 `invalid_pagination`，其余路由为 `invalid_query`。
        // 重复键在解析查询白名单时就被拒绝，所有路由都是 `invalid_query`。
        let routes = [
            ("/api/v1/auctions".to_owned(), "invalid_query"),
            ("/api/v2/auctions".to_owned(), "invalid_query"),
            (format!("/api/v1/auctions/{AUCTION_ID}/lots"), "invalid_query"),
            (format!("/api/v1/auctions/{AUCTION_ID}/bid-log"), "invalid_query"),
            ("/api/v1/price-history".to_owned(), "invalid_query"),
            ("/api/v1/compensation-pool".to_owned(), "invalid_query"),
            (format!("/api/v1/compensation-pool/{AUCTION_ID}/movements"), "invalid_query"),
            (format!("/api/v1/compensation-pool/{AUCTION_ID}/lots"), "invalid_query"),
            ("/api/v1/blacklist".to_owned(), "invalid_pagination"),
            ("/api/v1/me/bids".to_owned(), "invalid_query"),
            (format!("/api/v1/events?auction_id={AUCTION_ID}"), "invalid_query"),
        ];
        for (route, rejection_code) in routes {
            let separator = if route.contains('?') { '&' } else { '?' };
            // 省略（各路由默认值）、下界与统一上界都放行到读取端口检查。
            for query in ["", "page_size=1", "page_size=200"] {
                let uri = if query.is_empty() {
                    route.clone()
                } else {
                    format!("{route}{separator}{query}")
                };
                let (status, code) = page_size_response(&uri).await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "uri={uri} code={code:?}");
            }
            for query in [
                "page_size=201",
                "page_size=0",
                "page_size=-1",
                "page_size=",
                "page_size=9223372036854775807",
                "page_size=9223372036854775808",
            ] {
                let uri = format!("{route}{separator}{query}");
                let (status, code) = page_size_response(&uri).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "uri={uri}");
                assert_eq!(code.as_deref(), Some(rejection_code), "uri={uri}");
            }
            let uri = format!("{route}{separator}page_size=1&page_size=1");
            let (status, code) = page_size_response(&uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "uri={uri}");
            assert_eq!(code.as_deref(), Some("invalid_query"), "uri={uri}");
        }
    }

    #[tokio::test]
    async fn cursor_pages_share_the_public_page_size_cap() {
        // 游标页 offset 恒为 0，不受 offset 上限保护；页长上限必须同样生效。
        let boundary = encode_public_cursor(&PublicAuctionCursor::Boundary {
            sequence: 3,
            id: Uuid::from_u128(0x018f_4c8e_7b2a_7c3d_8e4f_0123_4567_89ab),
        });
        let routes = [
            format!("/api/v1/auctions?cursor={boundary}"),
            format!("/api/v2/auctions?cursor={boundary}"),
            "/api/v1/auctions/018f4c8e-7b2a-7c3d-8e4f-0123456789ab/bid-log?cursor=start".to_owned(),
            "/api/v1/price-history?cursor=start".to_owned(),
            "/api/v1/compensation-pool?cursor=start".to_owned(),
        ];
        for route in routes {
            let (status, code) = page_size_response(&format!("{route}&page_size=200")).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "route={route} code={code:?}");
            let (status, code) = page_size_response(&format!("{route}&page_size=201")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "route={route}");
            assert_eq!(code.as_deref(), Some("invalid_query"), "route={route}");
        }
    }

    const CONTRACT_AUCTION: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const CONTRACT_LOT: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);

    fn contract_time() -> Timestamp {
        Timestamp::from_timestamp(1_763_200_000, 110_000_000).expect("样本时刻")
    }

    struct PublicContent {
        alternate: bool,
    }

    impl WorldReader for PublicContent {
        fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
            Box::pin(async {
                Ok([("main", true), ("isekai", false)]
                    .into_iter()
                    .map(|(id, enabled)| WorldSnapshot {
                        id: id.to_owned(),
                        enabled,
                        availability: if enabled { "available" } else { "unavailable" }.to_owned(),
                        policy_revision: 2,
                    })
                    .collect())
            })
        }
    }

    impl PublicBlacklistReader for PublicContent {
        fn list_public_blacklist(
            &self,
            request: PublicBlacklistRequest,
        ) -> ApplicationFuture<'_, PublicBlacklistPage> {
            Box::pin(async move {
                Ok(PublicBlacklistPage {
                    entries: if self.alternate {
                        Vec::new()
                    } else {
                        vec![PublicBlacklistEntry {
                            username: "ExamplePlayer".to_owned(),
                            public_reason: "示例封禁原因".to_owned(),
                            blacklisted_at: contract_time(),
                        }]
                    },
                    total: i64::from(!self.alternate),
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }
    }

    impl PublicRulesReader for PublicContent {
        fn get_public_rules(
            &self,
            request: PublicRulesRequest,
        ) -> ApplicationFuture<'_, Option<PublicRulesDocument>> {
            Box::pin(async move {
                if request.locale != "en"
                    || (self.alternate && request.kind == SiteDocumentKind::About)
                {
                    return Ok(None);
                }
                let title = match request.kind {
                    SiteDocumentKind::Rules => "Auction rules",
                    SiteDocumentKind::About => "About the auction",
                };
                Ok(Some(PublicRulesDocument {
                    locale: request.locale,
                    title: title.to_owned(),
                    markdown: format!("# {title}\n\nExample document."),
                    revision: 2,
                    published_at: contract_time(),
                }))
            })
        }
    }

    impl PublicForumReader for PublicContent {
        fn public_forum_sync_enabled(&self) -> ApplicationFuture<'_, Option<bool>> {
            Box::pin(async { Ok(Some(!self.alternate)) })
        }

        fn public_forum_thread(
            &self,
            auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicForumThread>> {
            Box::pin(async move {
                let published = !self.alternate;
                let at = contract_time();
                Ok(Some(PublicForumThread {
                    auction_id,
                    status: if published { "published" } else { "pending" }.to_owned(),
                    source_revision: 4,
                    thread_url: published
                        .then(|| "https://forums.e-hentai.org/index.php?showtopic=123".to_owned()),
                    title: published.then(|| "Example auction".to_owned()),
                    body: published.then(|| "Example first post".to_owned()),
                    content_revision: if published { 2 } else { 0 },
                    processed_through_post_number: if published { 8 } else { 2 },
                    content_synced_at: published.then_some(at),
                    content_last_success_at: published.then_some(at),
                    content_last_error_at: None,
                    content_last_error_code: None,
                    updated_at: at,
                }))
            })
        }
    }

    impl LotWatchPort for PublicContent {
        fn list(&self, _user_id: Uuid, _auction_id: Uuid) -> LotWatchFuture<'_, Vec<LotWatch>> {
            Box::pin(async {
                Ok(vec![LotWatch {
                    lot_id: CONTRACT_LOT,
                    watched: !self.alternate,
                    channel: "mm".to_owned(),
                    version: if self.alternate { 2 } else { 1 },
                }])
            })
        }

        fn set(&self, _command: SetLotWatchCommand) -> LotWatchFuture<'_, SetLotWatchResult> {
            Box::pin(async { Err(LotWatchError::Unavailable) })
        }
    }

    impl LotWatchPreferencePort for PublicContent {
        fn get(&self, user_id: Uuid) -> LotWatchFuture<'_, LotWatchPreference> {
            Box::pin(async move {
                Ok(LotWatchPreference {
                    user_id,
                    channel: if self.alternate { "both" } else { "mm" }.to_owned(),
                    version: if self.alternate { 2 } else { 1 },
                })
            })
        }

        fn set(
            &self,
            _command: SetLotWatchPreferenceCommand,
        ) -> LotWatchFuture<'_, LotWatchPreference> {
            Box::pin(async { Err(LotWatchError::Unavailable) })
        }

        fn pm_available(&self, _user_id: Uuid) -> LotWatchFuture<'_, bool> {
            Box::pin(async { Ok(self.alternate) })
        }
    }

    #[tokio::test]
    async fn admin_forum_contract_fixtures() {
        use crate::server::contract_fixtures::{AUCTION_ID, admin_get, admin_state};

        let thread_path = format!("/api/v1/admin/auctions/{AUCTION_ID}/forum");
        for (alternate, status_name, thread_name) in [
            (false, "admin/forum-sync", "admin/forum"),
            (true, "admin/forum-sync-disabled", "admin/forum-pending"),
        ] {
            let state = admin_state().with_forum_reader(Arc::new(PublicContent { alternate }));
            assert_http(
                status_name,
                StatusCode::OK,
                admin_get(state.clone(), "/api/v1/admin/forum/sync-status").await,
            )
            .await;
            assert_http(thread_name, StatusCode::OK, admin_get(state, &thread_path).await).await;
        }
    }

    async fn fixture_read(
        app: &Router,
        uri: &str,
        authenticated: bool,
    ) -> axum::response::Response {
        let mut request = Request::builder().uri(uri);
        if authenticated {
            request = request.header(header::COOKIE, "auction_public_session=fixture-placeholder");
        }
        app.clone().oneshot(request.body(Body::empty()).expect("请求")).await.expect("响应")
    }

    #[tokio::test]
    async fn public_content_contract_fixtures() {
        for alternate in [false, true] {
            let port = Arc::new(PublicContent { alternate });
            let state = PublicApiState::new(
                Some(port.clone()),
                Some(port.clone()),
                Some(port.clone()),
                None,
                Some(port.clone()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(Arc::new(AnyCookieSession)),
                PublicHttpSecurity::default(),
            )
            .with_registration_info(if alternate {
                Vec::new()
            } else {
                vec!["ExampleBot".to_owned()]
            })
            .with_lot_watches(port.clone())
            .with_lot_watch_preference(port);
            let app =
                public_feature_routes(Router::<PublicApiState>::new(), state.anonymous_read_gate())
                    .with_state(state);
            let forum_uri = format!("/api/v1/auctions/{CONTRACT_AUCTION}/forum");
            let watch_uri = format!("/api/v1/me/lot-watches?auction_id={CONTRACT_AUCTION}");

            let cases = if alternate {
                [
                    ("public/blacklist-empty", "/api/v1/blacklist", false, StatusCode::OK),
                    ("public/about-unpublished", "/api/v1/about", false, StatusCode::NOT_FOUND),
                    (
                        "public/forum-sync-disabled",
                        "/api/v1/forum/sync-status",
                        false,
                        StatusCode::OK,
                    ),
                    ("public/forum-pending", &forum_uri, false, StatusCode::OK),
                    (
                        "public/registration-info-empty",
                        "/api/v1/registration-info",
                        false,
                        StatusCode::OK,
                    ),
                    ("public/lot-watches-removed", &watch_uri, true, StatusCode::OK),
                    (
                        "public/lot-watch-preference-both",
                        "/api/v1/me/lot-watch-preference",
                        true,
                        StatusCode::OK,
                    ),
                    ("public/session-anonymous", "/api/v1/session", false, StatusCode::OK),
                ]
            } else {
                [
                    ("public/worlds", "/api/v1/worlds", false, StatusCode::OK),
                    ("public/blacklist", "/api/v1/blacklist", false, StatusCode::OK),
                    ("public/about", "/api/v1/about?lang=ja", false, StatusCode::OK),
                    ("public/forum", &forum_uri, false, StatusCode::OK),
                    (
                        "public/registration-info",
                        "/api/v1/registration-info",
                        false,
                        StatusCode::OK,
                    ),
                    ("public/lot-watches", &watch_uri, true, StatusCode::OK),
                    (
                        "public/lot-watch-preference",
                        "/api/v1/me/lot-watch-preference",
                        true,
                        StatusCode::OK,
                    ),
                    ("public/session-permanent", "/api/v1/session", true, StatusCode::OK),
                ]
            };
            for (name, uri, authenticated, status) in cases {
                assert_http(name, status, fixture_read(&app, uri, authenticated).await).await;
            }
            if !alternate {
                for (name, uri, authenticated) in [
                    ("public/rules", "/api/v1/rules", false),
                    ("public/forum-sync", "/api/v1/forum/sync-status", false),
                    ("public/session-preference", "/api/v1/me/session-preference", true),
                ] {
                    assert_http(name, StatusCode::OK, fixture_read(&app, uri, authenticated).await)
                        .await;
                }
            }
        }
    }

    impl PublicAccountReader for PublicContent {
        fn list_public_user_bids(
            &self,
            user_id: Uuid,
            _request: PublicMyBidsRequest,
        ) -> ApplicationFuture<'_, PublicMyBidsPage> {
            Box::pin(async move {
                let proxy = self.public_user_proxy(CONTRACT_LOT, user_id).await?;
                Ok(PublicMyBidsPage {
                    bids: vec![PublicMyBid {
                        id: Uuid::from_u128(0x4000_8000_0000_0000_0011),
                        auction_id: CONTRACT_AUCTION.parse().expect("场次 ID"),
                        lot_id: CONTRACT_LOT,
                        lot_number: 1,
                        amount: 2_000,
                        accepted_state_version: 12,
                        fact_index: 1,
                        mechanism: "ordinary".to_owned(),
                        created_at: contract_time(),
                        equipment_name: "Legendary Staff".to_owned(),
                        equipment_category: "Staff".to_owned(),
                        leading: false,
                    }],
                    bid_total: 1,
                    proxy_total: i64::from(proxy.is_some()),
                    proxies: proxy.into_iter().collect(),
                })
            })
        }

        fn public_user_proxy(
            &self,
            lot_id: Uuid,
            user_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>> {
            Box::pin(async move {
                if lot_id != CONTRACT_LOT {
                    return Ok(None);
                }
                let auction_id = CONTRACT_AUCTION.parse().expect("场次 ID");
                let at = contract_time();
                let mut transitions = vec![PublicProxyTransition {
                    transition_number: 1,
                    kind: "created".to_owned(),
                    old_maximum_amount: None,
                    new_maximum_amount: 5_000,
                    old_mode: None,
                    new_mode: "slow".to_owned(),
                    old_status: None,
                    new_status: "active".to_owned(),
                    reason: None,
                    resulting_state_version: 1,
                    created_at: at,
                }];
                if self.alternate {
                    transitions.push(PublicProxyTransition {
                        transition_number: 2,
                        kind: "final_fast".to_owned(),
                        old_maximum_amount: Some(5_000),
                        new_maximum_amount: 5_000,
                        old_mode: Some("slow".to_owned()),
                        new_mode: "fast".to_owned(),
                        old_status: Some("active".to_owned()),
                        new_status: "active".to_owned(),
                        reason: None,
                        resulting_state_version: 2,
                        created_at: at,
                    });
                }
                Ok(Some(PublicOwnedProxy {
                    authorization_id: Uuid::from_u128(0x4000_8000_0000_0000_0012),
                    auction_id,
                    lot_id,
                    user_id,
                    lot_number: 1,
                    authorization_sequence: 1,
                    usage_number: 1,
                    quota_used: 1,
                    quota_limit: 3,
                    mode: if self.alternate { "fast" } else { "slow" }.to_owned(),
                    maximum_amount: 5_000,
                    status: "active".to_owned(),
                    terminal_reason: None,
                    state_version: if self.alternate { 2 } else { 1 },
                    transitions,
                    created_at: at,
                    updated_at: at,
                    equipment_name: "Legendary Staff".to_owned(),
                    equipment_category: "Staff".to_owned(),
                    bid: Some(PublicProxyBid {
                        id: Uuid::from_u128(0x4000_8000_0000_0000_0013),
                        auction_id,
                        lot_id,
                        lot_number: 1,
                        amount: 3_000,
                        accepted_state_version: 13,
                        fact_index: 1,
                        mechanism: "proxy".to_owned(),
                        created_at: at,
                        leading: true,
                    }),
                    invalid_reason: None,
                    capabilities: PublicProxyCapabilities {
                        can_lower: true,
                        waiting: false,
                        at_maximum: false,
                    },
                }))
            })
        }
    }

    impl PublicLotReader for PublicContent {
        fn list_public_lots(
            &self,
            _auction_id: Uuid,
            _request: PublicLotListRequest,
        ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
            Box::pin(async {
                Err(ApplicationError::CapabilityUnavailable("样本不查询 lot 目录"))
            })
        }

        fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
            Box::pin(async move {
                let auction_id = CONTRACT_AUCTION.parse().expect("场次 ID");
                let equipment_id =
                    EquipmentId::from_uuid(Uuid::from_u128(0x4000_8000_0000_0000_0014));
                let at = contract_time();
                Ok(Some(PublicLotDetail {
                    equipment_id,
                    projection: PublicAuctionProjection {
                        auction_id,
                        world_id: "main".to_owned(),
                        sequence: 5,
                        auction_status: "live".to_owned(),
                        projection_kind: "live".to_owned(),
                        presentation_mode: "live".to_owned(),
                        bidding_open: true,
                        opens_at: at,
                        scheduled_ends_at: Some(at + auction_time::Duration::days(1)),
                        preview_revision: 0,
                        final_restriction: None,
                    },
                    preview: None,
                    live: Some(PublicLiveLotDetail {
                        summary: PublicLiveLotSummary {
                            id: lot_id,
                            equipment_id,
                            auction_id,
                            lot_number: 1,
                            status: "live".to_owned(),
                            seller_name: "ExampleSeller".to_owned(),
                            start_price: 1_000,
                            current_price: 3_000,
                            next_bid: 4_000,
                            has_leader: true,
                            leader_name: Some("page-size-reader".to_owned()),
                            effective_end_at: Some(at + auction_time::Duration::days(1)),
                            state_version: 13,
                            equipment: PublicEquipment {
                                name: "Legendary Staff".to_owned(),
                                category: "Staff".to_owned(),
                                equipment_group: EquipmentGroup::Rare,
                                level: None,
                                info_summary: "Staff".to_owned(),
                                stats: Default::default(),
                                avg_roll: None,
                                game_equipment_url: "https://hentaiverse.org/equip/123".to_owned(),
                                attributes: serde_json::json!({"quality": "Legendary"}),
                            },
                        },
                    }),
                }))
            })
        }
    }

    impl PublicRealtimeReader for PublicContent {
        fn list_public_events(
            &self,
            request: PublicRealtimeRequest,
        ) -> ApplicationFuture<'_, Option<PublicRealtimePage>> {
            Box::pin(async move {
                let events = if self.alternate {
                    Vec::new()
                } else {
                    vec![PublicRealtimeEvent {
                        event_id: Uuid::from_u128(0x4000_8000_0000_0000_0015),
                        stream_key: format!("auction:{}", request.auction_id),
                        stream_sequence: 2,
                        kind: "auction.final_phase_entered".to_owned(),
                        schema_version: 1,
                        payload: serde_json::json!({
                            "auction_id": request.auction_id.to_string(), "state_version": 12
                        }),
                        created_at: contract_time(),
                    }]
                };
                Ok(Some(PublicRealtimePage {
                    events,
                    next_after: 2,
                    has_more: false,
                    refetch_required: self.alternate,
                }))
            })
        }
    }

    #[tokio::test]
    async fn account_and_realtime_contract_fixtures() {
        for alternate in [false, true] {
            let port = Arc::new(PublicContent { alternate });
            let state = PublicApiState::new(
                None,
                None,
                None,
                None,
                None,
                None,
                Some(port.clone()),
                None,
                None,
                None,
                Some(port.clone()),
                Some(port),
                None,
                Some(Arc::new(AnyCookieSession)),
                PublicHttpSecurity::default(),
            );
            let app =
                public_feature_routes(Router::<PublicApiState>::new(), state.anonymous_read_gate())
                    .with_state(state);
            let proxy_uri = format!("/api/v1/lots/{CONTRACT_LOT}/proxy-authorizations/mine");
            let events_uri = format!("/api/v1/events?auction_id={CONTRACT_AUCTION}");
            let bids_name = if alternate { "public/my-bids-final" } else { "public/my-bids" };
            let proxy_name = if alternate { "public/my-proxy-final" } else { "public/my-proxy" };
            let events_name =
                if alternate { "public/realtime-events-refetch" } else { "public/realtime-events" };
            for (name, uri) in [
                (bids_name, "/api/v1/me/bids"),
                (proxy_name, &proxy_uri),
                (events_name, &events_uri),
            ] {
                assert_http(name, StatusCode::OK, fixture_read(&app, uri, true).await).await;
            }
            if !alternate {
                let other = Uuid::from_u128(0x4000_8000_0000_0000_0016);
                let uri = format!("/api/v1/lots/{other}/proxy-authorizations/mine");
                assert_http(
                    "public/my-proxy-empty",
                    StatusCode::OK,
                    fixture_read(&app, &uri, true).await,
                )
                .await;
            }
        }
    }

    #[tokio::test]
    async fn proxy_request_error_contract_fixtures() {
        use axum::extract::ConnectInfo;
        use axum::http::Method;

        let state = PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(AnyCookieSession)),
            PublicHttpSecurity::default(),
        );
        let app =
            public_feature_routes(Router::<PublicApiState>::new(), state.anonymous_read_gate())
                .with_state(state);
        let uri = format!("/api/v1/lots/{CONTRACT_LOT}/proxy-authorizations");
        for (name, maximum_amount, mode) in [
            ("public/error-invalid-maximum-amount", 0, "fast"),
            ("public/error-invalid-proxy-mode", 2_000, "other"),
        ] {
            let body = serde_json::json!({
                "command_id": "00000000-0000-4000-8000-000000000030", "maximum_amount": maximum_amount,
                "mode": mode,
            });
            let mut request = Request::builder()
                .method(Method::POST)
                .uri(&uri)
                .header(header::COOKIE, "auction_public_session=fixture-placeholder")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("代理命令");
            request
                .extensions_mut()
                .insert(ConnectInfo(std::net::SocketAddr::from(([192, 0, 2, 21], 3000))));
            let response = app.clone().oneshot(request).await.expect("代理响应");
            assert_http(name, StatusCode::BAD_REQUEST, response).await;
        }
    }
}

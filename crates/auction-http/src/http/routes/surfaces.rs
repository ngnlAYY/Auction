//! Public/Admin 路由表与静态资源 fallback 装配。

use super::health;
use super::*;

/// 公开端路由。
/// 健康检查路径沿用已冻结的旧契约；其余路径交给内嵌的公开端 SPA 产物处理。
/// `version` 由装配根传入自身的 `CARGO_PKG_VERSION`：契约 crate 内展开该宏只会得到契约
/// 自己的版本，而健康检查要报告的是运行进程的版本。
pub fn public_router<A>(
    version: &'static str,
    boundary: HttpBoundary,
    anonymous_read_gate: AnonymousReadGate,
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
    .fallback(web_assets::serve_public_asset::<A>)
    .layer(middleware::from_fn_with_state(boundary, boundary::enforce))
}

/// 管理端路由。
/// 管理端与公开端由各自装配根传入不同资源类型，避免任一二进制引用另一端产物。
pub fn admin_router<A>(version: &'static str, boundary: HttpBoundary) -> Router<AdminApiState>
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
    .fallback(web_assets::serve_admin_asset::<A>)
    .layer(middleware::from_fn_with_state(boundary, boundary::enforce))
}

/// 注册已冻结的 Public 路径。
/// 路由只负责绑定各自的 HTTP owner；未装配的窄端口仍由 handler 明确返回能力不可用，
/// 不用伪造的成功 payload 掩盖部署缺口。
fn public_feature_routes(
    router: Router<PublicApiState>,
    anonymous_read_gate: AnonymousReadGate,
) -> Router<PublicApiState> {
    use method_strict::{MethodRouterExt as _, get, post, put};

    let no_cache_routes = Router::<PublicApiState>::new()
        .route("/api/v1/worlds", get(list_worlds))
        .route("/api/v1/rules", get(rules::get))
        .route("/api/v1/blacklist", get(blacklist::list))
        .route("/api/v1/compensation-pool", get(compensation::pool))
        .route("/api/v1/auctions", get(list_auctions))
        .route("/api/v2/auctions", get(list_auctions_v2))
        .route("/api/v1/auctions/current", get(current_auction))
        .route("/api/v1/auctions/{auction_id}/bid-log", get(list_bid_log))
        .route("/api/v1/price-history", get(list_price_history))
        .route_layer(middleware::from_fn(response_cache::no_cache))
        // 匿名读闸门独立于登录/mutation 限流；写路由不挂载该层。
        .route_layer(middleware::from_fn_with_state(
            anonymous_read_gate.clone(),
            read_gate::enforce,
        ));
    // 匿名读但含完整装备 key 的投影：保持 no-store，并只由读闸门约束并发与来源额度。
    let anonymous_read_routes = Router::<PublicApiState>::new()
        .route("/api/v1/registration-info", get(registration::get))
        .route("/api/v1/forum/sync-status", get(forum::sync_status))
        .route("/api/v1/auctions/{auction_id}/forum", get(forum::get))
        .route("/api/v1/events", get(realtime::list))
        .route("/api/v1/compensation-pool/{auction_id}/movements", get(compensation::movements))
        .route("/api/v1/compensation-pool/{auction_id}/lots", get(compensation::lots))
        .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
        .route("/api/v1/lots/{lot_id}", get(lot_detail))
        .route_layer(middleware::from_fn(response_cache::no_store))
        .route_layer(middleware::from_fn_with_state(
            anonymous_read_gate.clone(),
            read_gate::enforce,
        ));
    // 动态图片显式提供 HEAD；倒计时只做秒级缓存，其余 JSON GET 不隐式扩展方法闭集。
    let countdown_routes = Router::<PublicApiState>::new()
        .route(
            "/api/v1/auctions/{auction_id}/countdown/{timer}",
            method_strict::get_head(countdown::get),
        )
        .route_layer(middleware::from_fn(response_cache::short_lived))
        .route_layer(middleware::from_fn_with_state(
            anonymous_read_gate.clone(),
            read_gate::enforce,
        ));
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
    let routes = no_cache_routes
        .merge(anonymous_read_routes)
        .merge(countdown_routes)
        .merge(no_store_routes)
        .route(
            "/api/v1/ws",
            // HTTP/1.1 使用 GET Upgrade，HTTP/2 使用扩展 CONNECT；两者共用会话与范围门禁。
            axum::routing::get(realtime::websocket)
                .connect(realtime::websocket)
                .strict_methods("CONNECT, GET"),
        );
    router.merge(routes.route_layer(middleware::from_fn(boundary::normalize_api_rejection)))
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
        .route("/api/v1/admin/dashboard", get(admin_dashboard::get))
        .route("/api/v1/admin/worlds", get(admin_worlds::list))
        .route("/api/v1/admin/worlds/{world_id}", patch(admin_worlds::set_enabled))
        .route(
            "/api/v1/admin/auctions",
            axum::routing::get(admin_auctions::list)
                .post(admin_auctions::create)
                .layer(DefaultBodyLimit::max(4 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post(4 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("GET, POST"),
        )
        .route(
            "/api/v1/admin/worlds/{world_id}/auction-intake/items",
            post(admin_intake::add_item).layer(DefaultBodyLimit::max(72 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(72 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/auction-forum/preview",
            post(admin_forum::preview).layer(DefaultBodyLimit::max(2 << 20)).layer(
                middleware::from_fn_with_state(strict_json::post(2 << 20), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/forum/sync-status", get(admin_forum::sync_status))
        .route("/api/v1/admin/auctions/{auction_id}/forum", get(admin_forum::get))
        .route("/api/v1/admin/auctions/{auction_id}/lots", get(admin_lots::list))
        .route(
            "/api/v1/admin/auctions/{auction_id}/settlement-report",
            get(admin_settlement_report),
        )
        .route("/api/v1/admin/settlement-catalog", get(admin_settlement_catalog::list))
        .route("/api/v1/admin/auctions/{auction_id}/recovery-report", get(admin_recovery_report))
        .route("/api/v1/admin/auctions/{auction_id}/preview", post(admin_preview_auction))
        .route(
            "/api/v1/admin/auctions/{auction_id}/schedule",
            patch(admin_auctions::update_schedule).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::patch(4 << 10), strict_json::enforce),
            ),
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
            patch(admin_lot_update::update).layer(DefaultBodyLimit::max(8 << 10)).layer(
                middleware::from_fn_with_state(strict_json::patch(8 << 10), strict_json::enforce),
            ),
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
            put(admin_mail::set_paused).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::put(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/mail/polls:trigger",
            post(admin_mail::trigger_poll).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/realtime/events", get(realtime_admin::list_events))
        .route("/api/v1/admin/realtime/lag", get(realtime_admin::list_lag))
        .route("/api/v1/admin/realtime/failures", get(realtime_admin::list_failures))
        .route("/api/v1/admin/realtime/failures/{failure_id}", get(realtime_admin::get_failure))
        .route(
            "/api/v1/admin/realtime/failures/{failure_id}/replay",
            post(realtime_admin::replay).layer(DefaultBodyLimit::max(64 << 10)).layer(
                middleware::from_fn_with_state(
                    strict_json::post_with_invalid_json(64 << 10, "invalid_replay", "重放参数无效"),
                    strict_json::enforce,
                ),
            ),
        )
        .route(
            "/api/v1/admin/rules",
            axum::routing::get(admin_rules::get)
                .post(admin_rules::validate)
                .put(admin_rules::publish)
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
            "/api/v1/admin/config",
            axum::routing::get(admin_runtime_config::get)
                .put(admin_runtime_config::update)
                .layer(DefaultBodyLimit::max(4 << 20))
                .layer(middleware::from_fn_with_state(
                    strict_json::put(4 << 20),
                    strict_json::enforce,
                ))
                .strict_methods("GET, PUT"),
        )
        .route(
            "/api/v1/admin/config:validate",
            post(admin_runtime_config::validate).layer(DefaultBodyLimit::max(4 << 20)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 20), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/users",
            axum::routing::get(admin_users::list)
                .post(admin_users::create)
                .patch(admin_users::mutate)
                .layer(DefaultBodyLimit::max(16 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_patch(16 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("GET, PATCH, POST"),
        )
        .route("/api/v1/admin/renames", get(admin_renames::list))
        .route(
            "/api/v1/admin/renames/{rename_id}/revert",
            post(admin_users::revert_rename).layer(DefaultBodyLimit::max(8 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(8 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/users/{user_id}/blacklist",
            axum::routing::put(admin_users::blacklist)
                .delete(admin_users::unblacklist)
                .layer(DefaultBodyLimit::max(8 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::put_delete(8 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("DELETE, PUT"),
        )
        .route(
            "/api/v1/admin/credentials",
            axum::routing::get(admin_credentials::list)
                .post(admin_credentials::create)
                .patch(admin_credentials::mutate)
                .delete(admin_credentials::delete)
                .layer(DefaultBodyLimit::max(128 << 10))
                .layer(middleware::from_fn_with_state(
                    strict_json::post_patch_delete(128 << 10),
                    strict_json::enforce,
                ))
                .strict_methods("DELETE, GET, PATCH, POST"),
        )
        .route(
            "/api/v1/admin/credentials/{credential_id}/check",
            post(admin_credentials::check).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route("/api/v1/admin/audit", get(admin_audit::list))
        .route("/api/v1/admin/deliveries", get(delivery_admin::list))
        .route("/api/v1/admin/player-support", get(delivery_admin::player_support))
        .route("/api/v1/admin/deliveries/{delivery_id}", get(delivery_admin::detail))
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/sent-proof/preview",
            post(delivery_admin::sent_proof_preview).layer(DefaultBodyLimit::max(16 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(16 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/sent-proof/confirm",
            post(delivery_admin::sent_proof_confirm).layer(DefaultBodyLimit::max(16 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(16 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/release",
            post(delivery_admin::release).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/retry",
            post(delivery_admin::retry).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/deliveries/{delivery_id}/reconcile",
            post(delivery_admin::reconcile).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        );
    router.merge(
        routes
            .route_layer(middleware::from_fn(response_cache::no_store))
            .route_layer(middleware::from_fn(boundary::normalize_api_rejection)),
    )
}

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
use crate::middleware::security::AnonymousReadGate;
use crate::middleware::{boundary, method_strict, read_gate, response_cache, strict_json};
use crate::public::browser_errors;
use crate::public::{
    create_proxy_authorization, current_session, get_my_proxy_authorization,
    get_session_preference, get_watch_preference, list_lot_watches, list_my_bids, login_session,
    logout_session, lower_proxy_authorization, place_bid, refresh_session_cookie, set_lot_watch,
    set_session_preference, set_watch_preference,
};
use crate::reads::{
    current_auction, list_auctions, list_auctions_v2, list_bid_log, list_lots, list_price_history,
    list_worlds, lot_detail,
};
use crate::state::{AdminApiState, PublicApiState};
use crate::{admin as admin_handlers, public as public_handlers};

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
        .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
        .route("/api/v1/lots/{lot_id}", get(lot_detail))
        .route("/api/v1/auctions/{auction_id}/bid-log", get(list_bid_log))
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
            post(admin_alerts::acknowledge).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
        )
        .route(
            "/api/v1/admin/alerts/{alert_id}/silence",
            post(admin_alerts::silence).layer(DefaultBodyLimit::max(4 << 10)).layer(
                middleware::from_fn_with_state(strict_json::post(4 << 10), strict_json::enforce),
            ),
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
    router.merge(
        routes
            .route_layer(middleware::from_fn(response_cache::no_store))
            .route_layer(middleware::from_fn(boundary::normalize_api_rejection)),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use super::public_feature_routes;
    use crate::middleware::security::PublicHttpSecurity;
    use crate::state::PublicApiState;

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

    #[tokio::test]
    async fn lots_routes_are_in_the_public_catalog_cache_group() {
        let id = "018f4c8e-7b2a-7c3d-8e4f-0123456789ab";
        for uri in [format!("/api/v1/auctions/{id}/lots"), format!("/api/v1/lots/{id}")] {
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
}

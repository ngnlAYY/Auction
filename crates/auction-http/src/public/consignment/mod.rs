//! Public 寄售路由的装配接口，身份、事务与读模型均从各自的 owner 注入。

use std::sync::Arc;

use auction_application::consignment::{
    ManageListingService, OwnedListingRead, PublicListingRead, RegisterListingService,
    RegistrationReplayCrypto,
};
use auction_domain::consignment::FeeBps;
use axum::{Router, middleware};

use crate::middleware::method_strict::{self, MethodRouterExt as _};
use crate::middleware::{boundary, response_cache};
use crate::state::PublicApiState;

pub(crate) mod command_receipts;
pub(crate) mod deposit;
mod input;
mod mapping;
mod mutation;
#[cfg(test)]
mod order_tests;
pub(crate) mod orders;
pub(crate) mod purchase;
mod read;
pub(crate) mod recovery;
mod response;
pub(crate) mod rights;
#[cfg(test)]
mod tests;

/// 已通过启动期制品门的寄售能力。各端口独立缺省，保留无数据库启动的语义。
#[derive(Clone)]
pub struct ConsignmentCapabilities {
    /// 配置和制品门共同决定的有效开关。
    pub enabled: bool,
    /// 当前配置的寄售费率，非寄售固定费率由登记用例处理。
    pub fee_bps: FeeBps,
    /// 公开目录、详情与持久登记就绪读取。
    pub public_read: Option<Arc<dyn PublicListingRead>>,
    /// 本人列表、详情与原命令账本读取。
    pub owned_read: Option<Arc<dyn OwnedListingRead>>,
    /// 已装配 URL 校验、可选凭证能力与登记准入策略的用例。
    pub register: Option<Arc<RegisterListingService>>,
    /// 已装配改价准入策略的本人管理用例。
    pub manage: Option<Arc<ManageListingService>>,
    /// 本地具备用于首次登记的装备凭证封装能力。
    pub registration_sealer_available: bool,
    /// 登记就绪读取检查当前比较钥所用的本地密码能力。
    pub replay_crypto: Option<Arc<dyn RegistrationReplayCrypto>>,
}

impl ConsignmentCapabilities {
    /// 保留已校验的启动配置，其余能力按无数据库骨架处理。
    pub fn without_database(enabled: bool, fee_bps: FeeBps) -> Self {
        Self { enabled, fee_bps, ..Self::default() }
    }
}

impl Default for ConsignmentCapabilities {
    fn default() -> Self {
        Self {
            enabled: false,
            fee_bps: FeeBps::DEFAULT_CONSIGNED,
            public_read: None,
            owned_read: None,
            register: None,
            manage: None,
            registration_sealer_available: false,
            replay_crypto: None,
        }
    }
}

/// 路由只借用寄售能力和既有 Public 守卫所用的状态。
pub(crate) trait ConsignmentState: Clone + Send + Sync + 'static {
    /// 既有会话、来源及限流守卫的状态。
    fn public_state(&self) -> &PublicApiState;

    /// 当前装配为此进程提供的寄售能力。
    fn consignment(&self) -> &ConsignmentCapabilities;
}

/// 将九条 Public 寄售 API 放入给定路由器，并沿用严格方法和响应边界。
pub(crate) fn mount<S: ConsignmentState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v1/consignment/status", method_strict::get(read::status::<S>))
        .route(
            "/api/v1/consignment/listings",
            axum::routing::get(read::list_public::<S>)
                .post(mutation::register::<S>)
                .strict_methods("GET, POST"),
        )
        .route(
            "/api/v1/consignment/listings/{listing_id}",
            method_strict::get(read::public_detail::<S>),
        )
        .route("/api/v1/me/consignment/listings", method_strict::get(read::list_owned::<S>))
        .route(
            "/api/v1/me/consignment/listings/{listing_id}",
            method_strict::get(read::owned_detail::<S>),
        )
        .route(
            "/api/v1/me/consignment/commands/{command_id}",
            method_strict::get(read::owned_command::<S>),
        )
        .route(
            "/api/v1/consignment/listings/{listing_id}/price",
            method_strict::patch(mutation::reprice::<S>),
        )
        .route(
            "/api/v1/consignment/listings/{listing_id}/withdraw",
            method_strict::post(mutation::withdraw::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

//! 正式 Admin 寄售接口的路由装配与依赖边界。

use std::sync::Arc;

use auction_application::consignment::{AdminListingRead, ManageListingService};
use axum::{Router, middleware};

use crate::middleware::method_strict::{get, post};
use crate::middleware::response_cache;
use crate::state::AdminApiState;

mod boundary;
mod input;
mod listing_v2;
pub(crate) mod loss;
mod mapping;
pub(crate) mod orders;
pub(crate) mod quarantine;
mod routes;
#[cfg(test)]
mod tests;

/// 正式 Admin 的寄售能力。读历史与命令仲裁在停用时仍可用。
#[derive(Clone, Default)]
pub struct AdminConsignmentCapabilities {
    /// Admin 列表、详情和碰撞记录的只读端口。
    pub read: Option<Arc<dyn AdminListingRead>>,
    /// 按当前版本执行下架及重放原回执的用例。
    pub manage: Option<Arc<ManageListingService>>,
}

/// 正式 Admin 寄售路由需要的会话状态、读端口和下架用例。
pub(crate) trait AdminConsignmentState: Clone + Send + Sync + 'static {
    /// 借用既有 Admin 状态，沿用同一个会话及限流、Origin、CSRF 策略。
    fn admin_state(&self) -> &AdminApiState;

    /// 借用有效态查询。停用时仍应提供；缺失时返回暂不可用。
    fn consignment_read(&self) -> Option<&dyn AdminListingRead>;

    /// 借用下架用例。停用时仍须访问原命令账本并处理有效关闭。
    fn consignment_manage(&self) -> Option<&ManageListingService>;
}

/// 向正式 Admin 路由器追加三条完整的 v1 路径。
pub(crate) fn mount<S: AdminConsignmentState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v1/admin/consignment/listings", get(routes::list::<S>))
        .route("/api/v1/admin/consignment/listings/{listing_id}", get(routes::detail::<S>))
        .route("/api/v1/admin/consignment/listings/{listing_id}/delist", post(routes::delist::<S>))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

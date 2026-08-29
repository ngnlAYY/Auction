//! Public 页面组件装配。

#[path = "pages/history.rs"]
mod history;
#[path = "pages/lot/mod.rs"]
mod lot;
#[path = "pages/routes.rs"]
mod routes;
#[path = "pages/shared.rs"]
mod shared;

// 这些出口由父 surface 统一 re-export 给 Dioxus 路由；即使本装配文件本身不直接调用，也必须
// 保留原有组件路径和可见性。
#[allow(unused_imports)]
pub(crate) use history::{AuctionHistoryPage, AuctionHistoryTable, AuctionList};
#[allow(unused_imports)]
pub(crate) use lot::{
    BidHistoryTable, EquipmentCard, LiveLotDetailView, LotDetail, LotDetailBack, LotDetailHeader,
    PreviewLotDetailView,
};
#[allow(unused_imports)]
pub(crate) use routes::{
    Account, BidLog, Blacklist, Compensation, CompensationPool, History, Login, Me, NotFound,
    Rules, Status,
};
#[allow(unused_imports)]
pub(crate) use shared::RetryPlaceholder;

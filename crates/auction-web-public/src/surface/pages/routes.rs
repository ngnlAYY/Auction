//! Public 路由到各领域页面的薄包装。

use super::super::*;

#[component]
pub(crate) fn BidLog() -> Element {
    reads::bid_log()
}

#[component]
pub(crate) fn History() -> Element {
    reads::history()
}

#[component]
pub(crate) fn Login(return_to: Option<String>) -> Element {
    session::login(return_to)
}

#[component]
pub(crate) fn Me() -> Element {
    account::page()
}

#[component]
pub(crate) fn Account() -> Element {
    redirect_public_alias("/me")
}

/// 拍品直达链接。独立的拍品页已由目录与各列表的详情抽屉取代：参数是拍品 UUID 时（被超提醒
/// 里的链接）解析出它所在的目录位置，并在看板上打开详情抽屉；其余取值（旧书签、早期提醒里
/// 的 lot 代码）无法确定是哪一场的拍品，回到拍卖页并保留原地址中的查询。
#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    let lot_id = uuid::Uuid::parse_str(&lot_code).ok().filter(|lot_id| !lot_id.is_nil());
    match lot_id {
        Some(lot_id) => rsx! {
            LotDeepLink { lot_id: lot_id.to_string() }
        },
        None => rsx! {
            AuctionPage {}
        },
    }
}

#[component]
pub(crate) fn Rules() -> Element {
    rules::page(crate::api::SiteDocument::Rules)
}

#[component]
pub(crate) fn About() -> Element {
    rules::page(crate::api::SiteDocument::About)
}

#[component]
pub(crate) fn Blacklist() -> Element {
    blacklist::page()
}

#[component]
pub(crate) fn CompensationPool() -> Element {
    compensation::pool_page()
}

#[component]
pub(crate) fn Compensation() -> Element {
    redirect_public_alias("/compensation-pool")
}

#[component]
pub(crate) fn Forum() -> Element {
    forum::page()
}

#[component]
pub(crate) fn Status() -> Element {
    realtime::page()
}

/// 未匹配任何已冻结路由时回到公开端首页。
/// 旧版 Public 对未知路径使用无历史记录跳转，而不是在外壳之外渲染独立 404；复用别名
/// 跳转边界也会保留旧 URL 中的筛选查询，避免分享链接意外丢失上下文。
#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    redirect_public_alias("/")
}

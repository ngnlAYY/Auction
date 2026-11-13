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

/// 独立的拍品页已由目录与各列表的详情抽屉取代。旧书签和被超提醒里的 `/lots/...` 链接回到
/// 拍卖页，并保留原地址中的世界与场次查询，落到对应场次的目录；参数本身不再读取。
#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    let _ = lot_code;
    redirect_public_alias("/")
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

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

#[component]
pub(crate) fn Rules() -> Element {
    rules::page()
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
///
/// 旧版 Public 对未知路径使用无历史记录跳转，而不是在外壳之外渲染独立 404；复用别名
/// 跳转边界也会保留旧 URL 中的筛选查询，避免分享链接意外丢失上下文。
#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    redirect_public_alias("/")
}

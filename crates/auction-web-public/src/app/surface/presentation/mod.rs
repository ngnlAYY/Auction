//! 设备专属纯展示入口；页面数据、表单草稿与命令始终由原有稳定宿主持有。
//! 两种控制视图由 CSS 选择，尺寸变化不建立资源、监听器或新的命令生命周期。

use super::*;

pub(crate) mod desktop;
pub(crate) mod mobile;

/// 导航目标与首页查询动作只维护一份；两端视图不复制会话或世界选择器。
#[component]
fn NavigationLinks(
    auction_home_href: String,
    reset_home_query: bool,
    on_home: EventHandler<()>,
) -> Element {
    rsx! {
        Link {
            to: auction_home_href,
            onclick_only: reset_home_query,
            onclick: move |_| on_home.call(()),
            {t!("nav-auctions")}
        }
        Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
        Link { to: Route::AuctionList {}, {t!("nav-auction-history")} }
        Link { to: Route::History {}, {t!("nav-price-history")} }
        Link { to: Route::Rules {}, {t!("page-rules-title")} }
        Link { to: Route::Blacklist {}, {t!("nav-blacklist")} }
        Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
        Link { to: Route::Forum {}, {t!("nav-forum")} }
    }
}

/// 同页锚点不会导航 Router，不卸载偏好表单、分页或待核对命令。
#[component]
fn AccountSectionLinks() -> Element {
    rsx! {
        a { href: "#outbid-notifications", {t!("outbid-title")} }
        a { href: "#login-duration", {t!("session-duration-title")} }
        a { href: "#account-watches-title", {t!("account-watches-title")} }
        a { href: "#account-bids-title", {t!("account-bids-title")} }
        a { href: "#account-proxies-title", {t!("account-proxies-title")} }
    }
}

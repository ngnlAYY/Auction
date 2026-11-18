//! 设备专属纯展示入口；页面数据、表单草稿与命令始终由原有稳定宿主持有。
//! 两种控制视图由 CSS 选择，尺寸变化不建立资源、监听器或新的命令生命周期。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::PublicSessionResource;
use super::router::Route;

pub(crate) mod console;
pub(crate) mod desktop;
pub(crate) mod mobile;

/// 导航目标与首页查询动作只维护一份；两端视图不复制会话或世界选择器。
#[component]
fn NavigationLinks(
    auction_home_href: String,
    reset_home_query: bool,
    on_home: EventHandler<()>,
) -> Element {
    let session = use_context::<PublicSessionResource>();
    let account_current = matches!(use_route::<Route>(), Route::Me {} | Route::Account {});
    let authenticated = session
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(Option::as_ref)
        .is_some_and(|identity| identity.authenticated);
    rsx! {
        Link {
            to: auction_home_href,
            onclick_only: reset_home_query,
            onclick: move |_| on_home.call(()),
            {t!("nav-auctions")}
        }
        Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
        Link { to: Route::Fulfillment {}, {t!("page-fulfillment-title")} }
        Link { to: Route::AuctionList {}, {t!("nav-auction-history")} }
        Link { to: Route::History {}, {t!("nav-price-history")} }
        Link { to: Route::Rules {}, {t!("page-rules-title")} }
        Link { to: Route::About {}, {t!("page-about-title")} }
        Link { to: Route::Blacklist {}, {t!("nav-blacklist")} }
        Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
        Link { to: Route::Forum {}, {t!("nav-forum")} }
        if authenticated {
            Link {
                class: "public-shell__nav-account",
                to: Route::Account {},
                aria_current: account_current.then_some("page"),
                {t!("nav-my-bids")}
            }
        } else {
            Link { to: Route::Login { return_to: None }, {t!("session-login")} }
        }
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

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::rc::Rc;

    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::history::MemoryHistory;
    use dioxus::prelude::*;

    use super::NavigationLinks;
    use crate::api::PublicSession;
    use crate::surface::{page_test_contexts, settle_page};

    #[derive(Clone, PartialEq, Routable)]
    enum NavigationRoute {
        #[route("/rules")]
        Rules {},
        #[route("/me")]
        Me {},
    }

    #[component]
    fn Rules() -> Element {
        links()
    }

    #[component]
    fn Me() -> Element {
        links()
    }

    fn links() -> Element {
        rsx! { NavigationLinks { auction_home_href: "/", reset_home_query: false, on_home: |_| {} } }
    }

    fn navigation() -> Element {
        page_test_contexts();
        let session = use_resource(|| {
            ready(Ok::<_, String>(Some(PublicSession {
                authenticated: true,
                user_id: "reader".to_owned(),
                username: "Test".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            })))
        });
        use_context_provider(|| session);
        rsx! { Router::<NavigationRoute> {} }
    }

    #[test]
    fn mobile_account_link_marks_only_the_account_page_current() {
        for (path, expected) in [("/me", true), ("/me?bid_page=2", true), ("/rules", false)] {
            let history: Rc<dyn History> = Rc::new(MemoryHistory::with_initial_path(path));
            let mut dom = VirtualDom::new(navigation).with_root_context(history);
            let mut edits = dom.rebuild_to_vec().edits;
            edits.extend(settle_page(&mut dom));
            let account = edits
                .iter()
                .find_map(|edit| match edit {
                    Mutation::SetAttribute {
                        id,
                        name: "href",
                        value: AttributeValue::Text(value),
                        ..
                    } if value == "/account" => Some(*id),
                    _ => None,
                })
                .expect("已登录时应有账户链接");
            let current = edits.iter().rev().find_map(|edit| match edit {
                Mutation::SetAttribute { id, name: "aria-current", value, .. }
                    if *id == account =>
                {
                    Some(matches!(value, AttributeValue::Text(text) if text == "page"))
                }
                _ => None,
            });
            assert_eq!(current.unwrap_or(false), expected, "{path}");
        }
    }
}

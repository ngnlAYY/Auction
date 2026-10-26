//! 桌面侧栏与手机折叠菜单的纯展示；路由、会话与菜单状态仍由认证外壳唯一持有。

pub(super) mod desktop;
pub(super) mod mobile;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::super::Route;

#[component]
fn NavigationLinks(current_route: Route) -> Element {
    rsx! {
        for (route, label) in [
            (Route::Dashboard {}, t!("nav-dashboard")),
            (Route::Mails {}, t!("nav-mails")),
            (Route::Auctions {}, t!("nav-auctions")),
            (Route::Cookies {}, t!("nav-cookies")),
            (Route::Users {}, t!("nav-users")),
            (Route::Renames {}, t!("nav-renames")),
            (Route::Deliveries {}, t!("nav-deliveries")),
            (Route::Outbox {}, t!("page-outbox-title")),
            (Route::Settlement {}, t!("nav-settlement")),
            (Route::Config {}, t!("nav-config")),
            (Route::RulesContent {}, t!("page-rules-content-title")),
            (Route::Audit {}, t!("nav-audit")),
        ] {
            Link {
                class: if current_route == route { "admin-shell__nav-link--active" } else { "" },
                to: route,
                "{label}"
            }
        }
    }
}

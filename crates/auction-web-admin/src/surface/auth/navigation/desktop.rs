//! 桌面常驻侧栏；不安装资源、草稿或命令宿主。

use super::*;

#[component]
pub(crate) fn Navigation(current_route: Route) -> Element {
    rsx! {
        nav {
            class: "admin-shell__nav admin-shell__nav--desktop",
            aria_label: t!("nav-label"),
            NavigationLinks { current_route }
        }
    }
}

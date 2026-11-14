//! 桌面常驻侧栏；不安装资源、草稿或命令宿主。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{NavigationIcon, NavigationLinks};
use crate::surface::Route;

#[component]
pub(crate) fn Navigation(
    current_route: Route,
    collapsed: bool,
    on_toggle: EventHandler<()>,
) -> Element {
    let collapse_label = if collapsed { t!("console-expand") } else { t!("console-collapse") };
    rsx! {
        // 折叠外观由外壳根节点的 is-sidebar-collapsed 驱动；侧栏自身只保留基础类。
        aside { class: "console-sidebar", aria_label: t!("nav-label"),
            NavigationLinks { current_route }
            div { class: "console-sidebar__foot",
                button {
                    class: "console-collapse",
                    r#type: "button",
                    aria_expanded: (!collapsed).to_string(),
                    title: collapse_label,
                    onclick: move |_| on_toggle.call(()),
                    NavigationIcon { kind: "chevron" }
                    span { "{collapse_label}" }
                }
            }
        }
    }
}

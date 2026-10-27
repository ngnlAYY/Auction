//! 手机菜单及触发按钮；只接收展示状态和事件，不按宽度挂载业务子树。

use super::*;
use dioxus::html::Key;

#[component]
pub(crate) fn NavigationToggle(
    open: bool,
    on_toggle: EventHandler<()>,
    on_mount: EventHandler<MountedEvent>,
) -> Element {
    rsx! {
        button {
            class: "admin-shell__nav-toggle",
            r#type: "button",
            aria_controls: "admin-primary-navigation",
            aria_expanded: open.to_string(),
            onmounted: move |event| on_mount.call(event),
            onclick: move |_| on_toggle.call(()),
            onkeydown: move |event| {
                if open && event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    on_toggle.call(());
                }
            },
            {t!("nav-label")}
        }
    }
}

#[component]
pub(crate) fn Navigation(
    current_route: Route,
    open: bool,
    on_navigate: EventHandler<()>,
    on_keydown: EventHandler<KeyboardEvent>,
) -> Element {
    rsx! {
        nav {
            id: "admin-primary-navigation",
            class: if open {
                "admin-shell__nav admin-shell__nav--mobile admin-shell__nav--open"
            } else {
                "admin-shell__nav admin-shell__nav--mobile"
            },
            aria_label: t!("nav-label"),
            onclick: move |_| on_navigate.call(()),
            onkeydown: move |event| on_keydown.call(event),
            NavigationLinks { current_route }
        }
    }
}

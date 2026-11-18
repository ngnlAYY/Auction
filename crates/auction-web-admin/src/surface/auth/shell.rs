//! 已认证的管理端外壳。

use std::rc::Rc;

use auction_web_shared::{LanguagePicker, ThemePicker, t, use_route_focus, use_second_ticker};
use auction_web_shared::{load_sidebar_collapsed, save_sidebar_collapsed};
use dioxus::html::{Key, MountedData};
use dioxus::prelude::*;

use super::activity::LOGOUT_BUTTON_CLASS;
use super::login::AdminReadinessStatus;
use super::navigation;
use crate::api::{AdminReadinessState, AdminSession};
use crate::surface::admin_api_error_message;
use crate::surface::pending::{PendingCommandNotices, use_pending_command_host};
use crate::surface::router::Route;

/// 已认证的管理端外壳与注销动作。
#[component]
pub(crate) fn AdminAuthenticatedShell(
    session: AdminSession,
    readiness: AdminReadinessState,
    readiness_refresh: Signal<u64>,
    session_confirmed: bool,
    redirecting: bool,
    logging_out: bool,
    logout_error: Option<String>,
    on_logout: EventHandler<()>,
    on_recheck: EventHandler<()>,
) -> Element {
    // 已确认会话供 `api::transport::fetch_csrf_token` 核对 actor 与 CSRF 未被其他标签页替换；
    // 缺少该 context 时全部写入都会失败关闭。
    use_context_provider(|| session.clone());
    let command_host = use_pending_command_host();
    super::super::config::use_config_draft();
    super::super::rules::use_rules_draft();
    // 外壳持有唯一的秒级共享时钟；需要每秒刷新的子树（如拍卖列表倒计时）通过
    // `use_second_tick` 订阅同一个 interval，各处不另起定时器。
    use_second_ticker();
    let mut navigation_open = use_signal(|| false);
    let mut sidebar_collapsed = use_signal(load_sidebar_collapsed);
    let mut navigation_toggle = use_signal(|| None::<Rc<MountedData>>);
    let current_route = use_route::<Route>();
    let settlement_route = matches!(&current_route, Route::Settlement {});
    // 侧栏待办数与顶栏就绪徽标随页面切换刷新；会话内的命令完成或服务恢复后，下一次
    // 路由变化即可看到最新值（服务端就绪检查有 1 秒缓存，不另起后台轮询）。
    let pending_refresh = use_signal(|| 0_u64);
    let pending_counts = use_resource(move || {
        let _ = pending_refresh();
        async {
            #[cfg(test)]
            if try_consume_context::<super::effects::TestSession>().is_some() {
                return Err("feature_not_supported".to_owned());
            }
            crate::api::fetch_admin_pending_counts().await
        }
    });
    use_context_provider(|| pending_counts);
    // 挂载时沿用外壳已有的就绪快照（首屏门禁正在等它），只在之后真正切换路由时重读。
    let refresh_route = use_signal(|| current_route.clone());
    // 浏览器前进/后退和页面内跳转也收起手机导航；页面焦点仍由路由 hook 接管。
    use_effect(use_reactive((&current_route,), move |(route,)| {
        navigation_open.set(false);
        refresh_after_navigation(route, refresh_route, pending_refresh, readiness_refresh);
    }));
    use_route_focus(format!("{current_route:?}"), "admin-content");
    let theme_picker = rsx! {
        ThemePicker { label: t!("theme-label"), dark_label: t!("theme-dark"), light_label: t!("theme-light") }
    };
    #[cfg(test)]
    let theme_picker = if try_consume_context::<super::effects::TestSession>().is_some() {
        VNode::empty()
    } else {
        theme_picker
    };
    rsx! {
        div {
            class: if sidebar_collapsed() {
                "admin-shell admin-console is-sidebar-collapsed"
            } else {
                "admin-shell admin-console"
            },
            a { class: "console-skip", href: "#admin-content", {t!("skip")} }
            header { class: "console-ticker",
                div { class: "console-ticker__left",
                    navigation::mobile::NavigationToggle {
                        open: navigation_open(),
                        on_mount: move |event: MountedEvent| navigation_toggle.set(Some(event.data())),
                        on_toggle: move |_| navigation_open.toggle(),
                    }
                    span { class: "console-ticker__dot", aria_hidden: "true" }
                    span { class: "console-ticker__auction", {t!("admin-brand")} }
                }
                div { class: "console-ticker__right",
                    AdminReadinessStatus { state: readiness }
                    LanguagePicker {
                        label: t!("language-label"),
                        catalog: &crate::surface::ADMIN_LOCALE_CATALOG,
                    }
                    {theme_picker}
                    button {
                        class: LOGOUT_BUTTON_CLASS,
                        r#type: "button",
                        disabled: logging_out,
                        onclick: move |_| on_logout.call(()),
                        {t!("admin-logout")}
                    }
                }
            }
            if let Some(code) = logout_error {
                p { class: "admin-shell__banner", role: "alert", {admin_api_error_message(&code)} }
            }
            if !logging_out && !session_confirmed {
                p { class: "admin-shell__banner", role: "alert",
                    {t!("admin-session-recheck-blocked")}
                    button {
                        r#type: "button",
                        onclick: move |_| on_recheck.call(()),
                        {t!("admin-retry")}
                    }
                }
            }
            div { class: "console-body",
                navigation::desktop::Navigation {
                    current_route: current_route.clone(),
                    collapsed: sidebar_collapsed(),
                    on_toggle: move |_| {
                        let next = !sidebar_collapsed();
                        sidebar_collapsed.set(next);
                        save_sidebar_collapsed(next);
                    },
                }
                navigation::mobile::Navigation {
                    current_route,
                    open: navigation_open(),
                    on_navigate: move |_| navigation_open.set(false),
                    on_keydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Escape && navigation_open() {
                            event.prevent_default();
                            event.stop_propagation();
                            navigation_open.set(false);
                            if let Some(toggle) = navigation_toggle() {
                                spawn(async move {
                                    let _ = toggle.set_focus(true).await;
                                });
                            }
                        }
                    },
                }
                main {
                    class: if settlement_route {
                        "console-main admin-shell__content--settlement"
                    } else {
                        "console-main"
                    },
                    id: "admin-content",
                    tabindex: "-1",
                    PendingCommandNotices { host: command_host }
                    fieldset {
                        disabled: !session_confirmed,
                        style: "display: contents;",
                        if !redirecting {
                            Outlet::<Route> {}
                        }
                    }
                }
            }
        }
    }
}

fn refresh_after_navigation(
    route: Route,
    mut refresh_route: Signal<Route>,
    mut pending_refresh: Signal<u64>,
    mut readiness_refresh: Signal<u64>,
) {
    if *refresh_route.peek() == route {
        return;
    }
    refresh_route.set(route);
    // 使用 peek，读取代数并回写时不会让当前 effect 因订阅了自身再跑一轮。
    let next_pending = pending_refresh.peek().saturating_add(1);
    pending_refresh.set(next_pending);
    let next_readiness = readiness_refresh.peek().saturating_add(1);
    readiness_refresh.set(next_readiness);
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::refresh_after_navigation;
    use crate::surface::router::Route;

    #[test]
    fn refreshes_pending_and_readiness_only_after_route_changes() {
        let counters = Rc::new(RefCell::new(Vec::new()));
        let mut dom = VirtualDom::new_with_props(record_route_refreshes, Rc::clone(&counters));
        dom.rebuild_in_place();
        assert_eq!(counters.take(), [(0, 0), (1, 1), (1, 1)]);
    }

    fn record_route_refreshes(counters: Rc<RefCell<Vec<(u64, u64)>>>) -> Element {
        let route = use_signal(|| Route::Dashboard {});
        let pending = use_signal(|| 0);
        let readiness = use_signal(|| 0);
        for next in [Route::Dashboard {}, Route::Users {}, Route::Users {}] {
            refresh_after_navigation(next, route, pending, readiness);
            counters.borrow_mut().push((*pending.peek(), *readiness.peek()));
        }
        rsx! {}
    }
}

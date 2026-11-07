use super::*;
use dioxus::html::{Key, MountedData};
use std::rc::Rc;

/// 已认证的管理端外壳与注销动作。
#[component]
pub(crate) fn AdminAuthenticatedShell(
    session: AdminSession,
    checked_generation: u64,
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
    session_confirmed: bool,
    redirecting: bool,
    on_logout: EventHandler<()>,
) -> Element {
    // 旧凭据表的 importedBy 列由当前登录操作者合成，不扩散到凭据 DTO 或数据库投影。
    use_context_provider(|| session.clone());
    super::super::config::use_config_draft();
    super::super::rules::use_rules_draft();
    // 外壳持有唯一的秒级共享时钟；需要每秒刷新的子树（如拍卖列表倒计时）通过
    // `use_second_tick` 订阅同一个 interval，各处不另起定时器。
    use_second_ticker();
    let mut navigation_open = use_signal(|| false);
    let mut navigation_toggle = use_signal(|| None::<Rc<MountedData>>);
    let mut logging_out = use_signal(|| false);
    let mut logout_error = use_signal(|| None::<String>);
    let current_route = use_route::<Route>();
    let settlement_route = matches!(&current_route, Route::Settlement {});
    // 侧栏待办数随页面切换刷新；会话内的命令完成后下一次路由变化即可看到最新值。
    let mut pending_refresh = use_signal(|| 0_u64);
    let pending_counts = use_resource(move || {
        let _ = pending_refresh();
        crate::api::fetch_admin_pending_counts()
    });
    use_context_provider(|| pending_counts);
    // 浏览器前进/后退和页面内跳转也收起手机导航；页面焦点仍由路由 hook 接管。
    use_effect(use_reactive((&current_route,), move |_| {
        navigation_open.set(false);
        pending_refresh.set(pending_refresh().saturating_add(1));
    }));
    use_route_focus(format!("{current_route:?}"), "admin-content");
    rsx! {
        div {
            class: "admin-shell",
            // 仅已确认会话安排过期检查；临时故障统一退避，不能按旧截止时间连续重查。
            if session_confirmed {
                AdminSessionRefresh {
                    checked_generation,
                    expires_at: session.expires_at.clone(),
                    absolute_expires_at: session.absolute_expires_at.clone(),
                    reload_generation,
                }
            } else {
                AdminSessionRetry { reload_generation }
            }
            a { class: "admin-shell__skip", href: "#admin-content", {t!("skip")} }
            header { class: "admin-shell__header",
                span { class: "admin-shell__logo", {t!("admin-brand")} }
                navigation::mobile::NavigationToggle {
                    open: navigation_open(),
                    on_mount: move |event: MountedEvent| navigation_toggle.set(Some(event.data())),
                    on_toggle: move |_| navigation_open.toggle(),
                }
                div { class: "admin-shell__controls",
                    div { class: "admin-shell__health",
                        AdminReadinessStatus { state: readiness }
                    }
                    LanguagePicker {
                        label: t!("language-label"),
                        default_locale: "zh-CN",
                    }
                    ThemePicker {
                        label: t!("theme-label"),
                        dark_label: t!("theme-dark"),
                        light_label: t!("theme-light"),
                    }
                    button {
                        class: "admin-shell__logout",
                        r#type: "button",
                        disabled: logging_out(),
                        onclick: move |_| {
                            if logging_out() {
                                return;
                            }
                            logging_out.set(true);
                            logout_error.set(None);
                            let csrf_token = session.csrf_token.clone();
                            spawn(async move {
                                let result = logout_admin(csrf_token).await;
                                logging_out.set(false);
                                match result {
                                    Ok(()) => on_logout.call(()),
                                    Err(code) => logout_error.set(Some(code)),
                                }
                            });
                        },
                        {t!("admin-logout")}
                    }
                }
            }
            if let Some(code) = logout_error() {
                p { class: "admin-shell__banner", role: "alert", {admin_api_error_message(&code)} }
            }
            if !session_confirmed {
                p { class: "admin-shell__banner", role: "alert",
                    {t!("admin-session-recheck-blocked")}
                    button {
                        r#type: "button",
                        onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                        {t!("admin-retry")}
                    }
                }
            }
            div { class: "admin-shell__body",
                navigation::desktop::Navigation { current_route: current_route.clone() }
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
                        "admin-shell__content admin-shell__content--settlement"
                    } else {
                        "admin-shell__content"
                    },
                    id: "admin-content",
                    tabindex: "-1",
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

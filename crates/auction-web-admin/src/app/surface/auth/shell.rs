use super::*;

/// 已认证的管理端外壳与注销动作。
#[component]
pub(crate) fn AdminAuthenticatedShell(
    session: AdminSession,
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
) -> Element {
    // 旧凭据表的 importedBy 列由当前登录操作者合成，不扩散到凭据 DTO 或数据库投影。
    use_context_provider(|| session.clone());
    super::super::config::use_config_draft();
    // 外壳持有唯一的秒级共享时钟；需要每秒刷新的子树（如拍卖列表倒计时）通过
    // `use_second_tick` 订阅同一个 interval，不再各自起定时器。
    use_second_ticker();
    let mut navigation_open = use_signal(|| false);
    let mut logging_out = use_signal(|| false);
    let mut logout_error = use_signal(|| None::<String>);
    let current_route = use_route::<Route>();
    let settlement_route = matches!(&current_route, Route::Settlement {});
    use_route_focus(format!("{current_route:?}"), "admin-content");
    rsx! {
        div {
            class: "admin-shell",
            AdminSessionRefresh {
                key: "{session.expires_at}",
                session: session.clone(),
                reload_generation,
            }
            a { class: "admin-shell__skip", href: "#admin-content", {t!("skip")} }
            header { class: "admin-shell__header",
                span { class: "admin-shell__logo", {t!("admin-brand")} }
                button {
                    class: "admin-shell__nav-toggle",
                    r#type: "button",
                    aria_controls: "admin-primary-navigation",
                    aria_expanded: navigation_open().to_string(),
                    onclick: move |_| navigation_open.set(!navigation_open()),
                    {t!("nav-label")}
                }
                div { class: "admin-shell__controls",
                    div { class: "admin-shell__health",
                        AdminReadinessStatus { state: readiness }
                    }
                    LanguagePicker {
                        label: t!("language-label"),
                        default_locale: "zh-CN",
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
                                    Ok(()) => {
                                        reload_generation.set(reload_generation().saturating_add(1));
                                    }
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
            div { class: "admin-shell__body",
                nav {
                    id: "admin-primary-navigation",
                    class: if navigation_open() {
                        "admin-shell__nav admin-shell__nav--open"
                    } else {
                        "admin-shell__nav"
                    },
                    aria_label: t!("nav-label"),
                    onclick: move |_| navigation_open.set(false),
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Dashboard {})),
                        to: Route::Dashboard {},
                        {t!("nav-dashboard")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Mails {})),
                        to: Route::Mails {},
                        {t!("nav-mails")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Auctions {})),
                        to: Route::Auctions {},
                        {t!("nav-auctions")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Cookies {})),
                        to: Route::Cookies {},
                        {t!("nav-cookies")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Users {})),
                        to: Route::Users {},
                        {t!("nav-users")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Renames {})),
                        to: Route::Renames {},
                        {t!("nav-renames")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Deliveries {})),
                        to: Route::Deliveries {},
                        {t!("nav-deliveries")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Outbox {})),
                        to: Route::Outbox {},
                        {t!("page-outbox-title")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Settlement {})),
                        to: Route::Settlement {},
                        {t!("nav-settlement")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Config {})),
                        to: Route::Config {},
                        {t!("nav-config")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::RulesContent {})),
                        to: Route::RulesContent {},
                        {t!("page-rules-content-title")}
                    }
                    Link {
                        class: admin_nav_class(matches!(&current_route, Route::Audit {})),
                        to: Route::Audit {},
                        {t!("nav-audit")}
                    }
                }
                main {
                    class: if settlement_route {
                        "admin-shell__content admin-shell__content--settlement"
                    } else {
                        "admin-shell__content"
                    },
                    id: "admin-content",
                    tabindex: "-1",
                    Outlet::<Route> {}
                }
            }
        }
    }
}

/// 输出当前导航项的 BEM 激活修饰类（与 admin.css 的 .admin-shell__nav-link--active 对应）。
fn admin_nav_class(active: bool) -> &'static str {
    if active { "admin-shell__nav-link--active" } else { "" }
}

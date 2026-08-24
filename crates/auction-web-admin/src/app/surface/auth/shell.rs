use super::*;

/// 已认证的管理端外壳与注销动作。
#[component]
pub(crate) fn AdminAuthenticatedShell(
    session: AdminSession,
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
) -> Element {
    let mut logging_out = use_signal(|| false);
    let mut logout_failed = use_signal(|| false);
    rsx! {
        div { class: "admin-shell",
            AdminSessionRefresh {
                key: "{session.expires_at}",
                session: session.clone(),
                reload_generation,
            }
            a { class: "admin-shell__skip", href: "#admin-content", {t!("skip")} }
            header { class: "admin-shell__header",
                Link { class: "admin-shell__logo", to: Route::Dashboard {},
                    {t!("admin-brand")}
                }
                div { class: "admin-shell__controls",
                    div { class: "admin-shell__health",
                        AdminReadinessStatus { state: readiness }
                    }
                    LanguagePicker { label: t!("language-label") }
                    button {
                        class: "admin-shell__logout",
                        r#type: "button",
                        disabled: logging_out(),
                        onclick: move |_| {
                            if logging_out() {
                                return;
                            }
                            logging_out.set(true);
                            logout_failed.set(false);
                            let csrf_token = session.csrf_token.clone();
                            spawn(async move {
                                let successful = logout_admin(csrf_token).await.is_ok();
                                logging_out.set(false);
                                if successful {
                                    reload_generation.set(reload_generation().saturating_add(1));
                                } else {
                                    logout_failed.set(true);
                                }
                            });
                        },
                        if logging_out() { {t!("admin-logout-working")} } else { {t!("admin-logout")} }
                    }
                }
            }
            if logout_failed() {
                p { class: "admin-shell__banner", role: "alert", {t!("admin-logout-error")} }
            }
            div { class: "admin-shell__body",
                nav { class: "admin-shell__nav", aria_label: t!("nav-label"),
                    Link { to: Route::Dashboard {}, {t!("nav-dashboard")} }
                    Link { to: Route::Mails {}, {t!("nav-mails")} }
                    Link { to: Route::Auctions {}, {t!("nav-auctions")} }
                    Link { to: Route::Cookies {}, {t!("page-cookies-title")} }
                    Link { to: Route::Users {}, {t!("nav-users")} }
                    Link { to: Route::Renames {}, {t!("page-renames-title")} }
                    Link { to: Route::Deliveries {}, {t!("nav-deliveries")} }
                    Link { to: Route::Outbox {}, {t!("page-outbox-title")} }
                    Link { to: Route::Settlement {}, {t!("nav-settlement")} }
                    Link { to: Route::Config {}, {t!("nav-config")} }
                    Link { to: Route::RulesContent {}, {t!("page-rules-content-title")} }
                    Link { to: Route::Audit {}, {t!("nav-audit")} }
                }
                main { class: "admin-shell__content", id: "admin-content",
                    Outlet::<Route> {}
                }
            }
        }
    }
}

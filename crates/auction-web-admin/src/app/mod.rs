//! 管理端浏览器界面（Dioxus CSR）。
//!
//! 路由集合按旧版 Auction-BF 的 `manifests/admin.routes.json` 冻结。管理端与公开端是
//! 不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，公开端也不
//! 得依赖本 crate。
//!
//! 界面文案取自 `i18n/*.ftl`，默认简体中文——沿用旧 `manifests/admin.locales.json` 的
//! `default_locale`。受支持语言与公开端一致，但译文资源彼此隔离。
//!
//! 本 crate 不得导入存储、配置或凭据访问路径。

#[path = "auction/auctions.rs"]
mod auctions;
#[path = "operations/audit.rs"]
mod audit;
#[path = "operations/config.rs"]
mod config;
#[path = "identity/credentials.rs"]
mod credentials;
#[path = "operations/dashboard.rs"]
mod dashboard;
#[path = "fulfillment/deliveries.rs"]
mod deliveries;
#[path = "auction/intake.rs"]
mod intake;
#[path = "auction/lots.rs"]
mod lots;
#[path = "fulfillment/mail.rs"]
mod mail;
#[path = "shared/pagination.rs"]
mod pagination;
#[path = "operations/realtime.rs"]
mod realtime;
#[path = "identity/renames.rs"]
mod renames;
#[path = "content/rules.rs"]
mod rules;
#[path = "auction/settlement.rs"]
mod settlement;
#[path = "operations/status.rs"]
mod status;
#[path = "identity/users.rs"]
mod users;

use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use unic_langid::langid;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use auction_web_shared::{
    BaseStylesheet, LanguagePicker, format_localized_time,
    i18n::{DEFAULT_ADMIN_LOCALE, initial_language},
    use_dialog_focus_trap, use_locale_effects,
};

use crate::api::{
    AdminReadinessState, AdminSession, fetch_admin_readiness, fetch_admin_session, login_admin,
    logout_admin,
};

/// 管理端 surface 样式。设计变量由 `auction-web-shared` 提供。
const ADMIN_CSS: Asset =
    asset!("/assets/admin.css", AssetOptions::css().with_preload(true).with_static_head(true));

/// 按旧版管理端约定把 UTC 时间显示为当前界面语言的 Asia/Shanghai 时间。
///
/// 旧版使用浏览器 `Intl.DateTimeFormat`，因此不能在 Rust 侧固定月份语言；WASM 路径继续
/// 使用浏览器的本地化格式，非 WASM 构建只保留静态检查所需的稳定兜底。`datetime` 属性
/// 仍保留原始 RFC3339 值，屏幕文案统一经过这一层。
pub(super) fn format_admin_time(value: &str) -> String {
    format_localized_time(value)
}

pub fn run() {
    dioxus::launch(App);
}

/// 管理端路由表。
///
/// 路径逐条对应旧系统已冻结的管理端 route。管理命令的实现必须走版本化 REST 契约，
/// 不在浏览器端重建状态机。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
enum Route {
    #[layout(AdminShell)]
        #[route("/")]
        Dashboard {},

        #[route("/dashboard")]
        DashboardAlias {},

        #[route("/login")]
        Login {},

        #[route("/auctions")]
        Auctions {},

        #[route("/auctions/:auction_id/lots")]
        AuctionLots { auction_id: String },

        #[route("/auction")]
        AuctionOverview {},

        #[route("/mails")]
        Mails {},

        #[route("/mail")]
        MailOverview {},

        #[route("/cookies")]
        Cookies {},

        #[route("/credentials")]
        Credentials {},

        #[route("/users")]
        Users {},

        #[route("/renames")]
        Renames {},

        #[route("/deliveries")]
        Deliveries {},

        #[route("/delivery")]
        DeliveryOverview {},

        #[route("/outbox")]
        Outbox {},

        #[route("/settlement")]
        Settlement {},

        #[route("/config")]
        Config {},

        #[route("/rules-content")]
        RulesContent {},

        #[route("/rules")]
        Rules {},

        #[route("/audit")]
        Audit {},

        #[route("/status")]
        Status {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
fn App() -> Element {
    // 语言资源在编译期内嵌；管理端默认简体中文，并沿用已保存的语言偏好。
    let i18n = use_init_i18n(|| {
        I18nConfig::new(initial_language(DEFAULT_ADMIN_LOCALE))
            .with_locale((langid!("en"), include_str!("../../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../../i18n/ko.ftl")))
            .with_fallback(DEFAULT_ADMIN_LOCALE)
    });
    use_locale_effects(i18n);

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: ADMIN_CSS }
        document::Title { {t!("app-title")} }
        Router::<Route> {}
    }
}

/// 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。
#[component]
fn AdminShell() -> Element {
    use_dialog_focus_trap();
    let mut reload_generation = use_signal(|| 0_u64);
    let readiness = use_resource(fetch_admin_readiness);
    let session = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_session()
    });
    let readiness_state = match &*readiness.read_unchecked() {
        None => AdminReadinessState::Checking,
        Some(Ok(true)) => AdminReadinessState::Ready,
        Some(Ok(false) | Err(_)) => AdminReadinessState::Degraded,
    };

    match &*session.read_unchecked() {
        None => rsx! {
            AdminSessionPanel { title: t!("page-login-title"), detail: t!("admin-session-loading") }
        },
        Some(Err(_)) => rsx! {
            section {
                class: "admin-login-page",
                role: "alert",
                aria_labelledby: "admin-session-error-title",
                h1 { id: "admin-session-error-title", {t!("page-login-title")} }
                p { class: "admin-panel__note", {t!("admin-session-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                    {t!("admin-retry")}
                }
            }
            AdminSessionRetry { reload_generation }
        },
        Some(Ok(None)) if !admin_is_login_path() => rsx! {
            AdminLoginRedirect {}
        },
        Some(Ok(None)) => rsx! {
            AdminLogin {
                reload_generation,
                readiness: readiness_state,
            }
        },
        Some(Ok(Some(_))) if admin_is_login_path() => rsx! {
            AdminReturnRedirect {}
        },
        Some(Ok(Some(current))) => rsx! {
            AdminAuthenticatedShell {
                session: current.clone(),
                reload_generation,
                readiness: readiness_state,
            }
        },
    }
}

/// 未认证访问受保护页面时，按旧版保存站内 return 地址并替换到登录页。
#[component]
fn AdminLoginRedirect() -> Element {
    let return_target = current_admin_location();
    let encoded_return =
        url::form_urlencoded::byte_serialize(return_target.as_bytes()).collect::<String>();
    let target = format!("/login?return={encoded_return}");
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

/// 已认证用户访问登录路径时，按旧版 return 参数回到原始站内地址。
#[component]
fn AdminReturnRedirect() -> Element {
    let target = admin_return_path();
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

fn admin_is_login_path() -> bool {
    current_admin_path().as_deref() == Some("/login")
}

#[cfg(target_arch = "wasm32")]
fn current_admin_path() -> Option<String> {
    web_sys::window().and_then(|window| window.location().pathname().ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_path() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn current_admin_location() -> String {
    let Some(window) = web_sys::window() else {
        return "/".to_owned();
    };
    let location = window.location();
    let pathname = location.pathname().unwrap_or_else(|_| "/".to_owned());
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    if pathname.starts_with('/') && pathname != "/login" {
        format!("{pathname}{search}{hash}")
    } else {
        "/".to_owned()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_location() -> String {
    "/".to_owned()
}

fn admin_return_path() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return "/".to_owned();
        };
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            if key == "return"
                && value.starts_with('/')
                && !value.starts_with("//")
                && !value.chars().any(char::is_control)
            {
                return value.into_owned();
            }
        }
    }
    "/".to_owned()
}

/// 会话查询失败时按旧版策略每 30 秒重试；成功后该组件随错误页一起卸载。
#[component]
fn AdminSessionRetry(mut reload_generation: Signal<u64>) -> Element {
    #[cfg(target_arch = "wasm32")]
    use_future(move || async move {
        loop {
            TimeoutFuture::new(30_000).await;
            reload_generation.set(reload_generation().saturating_add(1));
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = reload_generation;
    rsx! {}
}

/// 在 Admin 会话滑动过期前后触发一次 GET，复用服务端的 touch 语义刷新 Cookie 会话。
#[component]
fn AdminSessionRefresh(session: AdminSession, mut reload_generation: Signal<u64>) -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let delay = session_refresh_delay(&session.expires_at);
        use_future(move || async move {
            TimeoutFuture::new(delay).await;
            reload_generation.set(reload_generation().saturating_add(1));
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (session, reload_generation);
    }
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn session_refresh_delay(expires_at: &str) -> u32 {
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return 30_000;
    };
    let delay = expiry.timestamp_millis() as f64 - js_sys::Date::now();
    if !delay.is_finite() {
        return 30_000;
    }
    delay.clamp(0.0, 2_147_483_647.0) as u32
}

/// 会话查询尚未完成或暂时失败时的最小安全页面。
#[component]
fn AdminSessionPanel(title: String, detail: String) -> Element {
    rsx! {
        main { class: "admin-login-page", aria_busy: "true",
            section { class: "admin-login-card", aria_labelledby: "admin-session-title",
                h1 { id: "admin-session-title", "{title}" }
                p { class: "admin-panel__note", "{detail}" }
            }
        }
    }
}

/// Admin 登录表单；secret 只在提交时进入一次 Basic Auth 请求。
#[component]
fn AdminLogin(mut reload_generation: Signal<u64>, readiness: AdminReadinessState) -> Element {
    let mut actor = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut submitting = use_signal(|| false);
    let mut failed = use_signal(|| false);

    rsx! {
        main { class: "admin-login-page", aria_labelledby: "admin-login-title",
            section { class: "admin-login-card", aria_labelledby: "admin-login-title",
                h1 { id: "admin-login-title", {t!("page-login-title")} }
                p { class: "admin-panel__note", {t!("page-login-note")} }
                div { class: "admin-visually-hidden",
                    AdminReadinessStatus { state: readiness }
                }
                form {
                    class: "admin-auth-form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting() || actor().trim().is_empty() || secret().is_empty() {
                            failed.set(true);
                            return;
                        }
                        submitting.set(true);
                        failed.set(false);
                        let submitted_actor = actor();
                        let submitted_secret = secret();
                        secret.set(String::new());
                        spawn(async move {
                            let successful = login_admin(submitted_actor, submitted_secret).await.is_ok();
                            submitting.set(false);
                            if successful {
                                reload_generation.set(reload_generation().saturating_add(1));
                            } else {
                                failed.set(true);
                            }
                        });
                    },
                    label { for: "admin-actor", {t!("admin-login-actor")} }
                    input {
                        id: "admin-actor",
                        r#type: "text",
                        autocomplete: "username",
                        required: true,
                        disabled: submitting(),
                        value: "{actor}",
                        oninput: move |event| actor.set(event.value()),
                    }
                    label { for: "admin-secret", {t!("admin-login-secret")} }
                    input {
                        id: "admin-secret",
                        r#type: "password",
                        autocomplete: "current-password",
                        required: true,
                        disabled: submitting(),
                        value: "{secret}",
                        oninput: move |event| secret.set(event.value()),
                    }
                    button {
                        class: "admin-btn-primary",
                        r#type: "submit",
                        disabled: submitting(),
                        if submitting() { {t!("admin-login-working")} } else { {t!("admin-login-submit")} }
                    }
                }
                if failed() {
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        {t!("admin-login-error")}
                    }
                }
            }
        }
    }
}

/// 已认证的管理端外壳与注销动作。
#[component]
fn AdminAuthenticatedShell(
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

/// 全局 readiness 的无障碍状态提示；视觉布局沿用旧版的隐藏健康提示约定。
#[component]
fn AdminReadinessStatus(state: AdminReadinessState) -> Element {
    let (class, label) = match state {
        AdminReadinessState::Checking => ("checking", t!("admin-readiness-checking")),
        AdminReadinessState::Ready => ("ready", t!("admin-readiness-ready")),
        AdminReadinessState::Degraded => ("degraded", t!("admin-readiness-degraded")),
    };
    rsx! {
        p {
            class: "ui-health-status ui-health-status--{class}",
            "data-state": class,
            aria_live: "polite",
            role: "status",
            span { class: "ui-health-status__indicator", aria_hidden: "true" }
            "{label}"
        }
    }
}

/// 管理端异步状态面板。
///
/// 页面在加载或等待权威数据时复用该结构；它不是未实现功能的静态占位页。
#[component]
fn AdminPanel(title: String, detail: String) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-panel-title",
            h1 { id: "admin-panel-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
        }
    }
}

/// 管理端首屏数据加载失败时提供统一的重试入口。
///
/// 各页面的资源都由自己的刷新信号驱动；这里不保存错误正文，也不把底层响应暴露给浏览器。
#[component]
pub(super) fn AdminLoadError(title: String, detail: String, mut refresh: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-load-error-title",
            h1 { id: "admin-load-error-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| refresh.set(refresh().saturating_add(1)),
                {t!("admin-retry")}
            }
        }
    }
}

#[component]
fn Dashboard() -> Element {
    dashboard::page()
}

#[component]
fn DashboardAlias() -> Element {
    redirect_admin_alias("/")
}

#[component]
fn Login() -> Element {
    // 未认证时由 AdminShell 显示登录表单；已认证后访问旧登录路径回到总览。
    dashboard::page()
}

#[component]
fn Auctions() -> Element {
    auctions::page()
}

#[component]
fn AuctionLots(auction_id: String) -> Element {
    lots::page(auction_id)
}

#[component]
fn AuctionOverview() -> Element {
    redirect_admin_alias("/auctions")
}

#[component]
fn Mails() -> Element {
    mail::page()
}

#[component]
fn MailOverview() -> Element {
    redirect_admin_alias("/mails")
}

#[component]
fn Cookies() -> Element {
    // Auction-BF 的 /cookies 是凭据管理入口，/credentials 是迁移期间保留的明确别名。
    credentials::page()
}

#[component]
fn Credentials() -> Element {
    redirect_admin_alias("/cookies")
}

#[component]
fn Users() -> Element {
    users::page()
}

#[component]
fn Renames() -> Element {
    renames::page()
}

#[component]
fn Deliveries() -> Element {
    deliveries::page()
}

#[component]
fn DeliveryOverview() -> Element {
    redirect_admin_alias("/deliveries")
}

#[component]
fn Outbox() -> Element {
    realtime::outbox_page()
}

#[component]
fn Settlement() -> Element {
    settlement::page()
}

#[component]
fn Config() -> Element {
    config::page()
}

#[component]
fn RulesContent() -> Element {
    rules::page()
}

#[component]
fn Rules() -> Element {
    redirect_admin_alias("/rules-content")
}

#[component]
fn Audit() -> Element {
    audit::page()
}

#[component]
fn Status() -> Element {
    status::page()
}

/// 把旧版管理端别名无历史记录地收敛到 canonical URL，并保留查询字符串和片段。
///
/// Admin 旧链接仍保留在路由表中以兼容书签；`replace` 确保返回键不会在别名与正式地址间
/// 形成额外的导航环。片段由旧版路由行为保留，便于外部链接定位页面内锚点。
fn redirect_admin_alias(path: &'static str) -> Element {
    let target = format!("{path}{}", current_browser_suffix());
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn current_browser_suffix() -> String {
    let Some(window) = web_sys::window() else {
        return String::new();
    };
    let location = window.location();
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    format!("{search}{hash}")
}

#[cfg(not(target_arch = "wasm32"))]
fn current_browser_suffix() -> String {
    String::new()
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        main { class: "admin-shell",
            section { class: "admin-panel",
                h1 { {t!("not-found-title")} }
                p { class: "admin-panel__note", {t!("not-found-note")} }
            }
        }
    }
}

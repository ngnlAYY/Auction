//! Public 登录和会话页面。
//! 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影，注销仍通过服务端
//! 会话端口完成。

use auction_web_shared::{BaseErrorKind, base_api_error_message, legacy_t as t};
use dioxus::prelude::*;
use url::form_urlencoded;

use crate::api::{PublicSession, PublicSessionApiError, login_public, logout_public};

use super::{PublicSessionResource, Route};

#[derive(Debug, Clone, PartialEq)]
enum LoginState {
    Success(PublicSession),
    Failure(PublicSessionApiError),
}

#[derive(Clone)]
enum SessionMenuState {
    Checking,
    Unavailable,
    Anonymous,
    Authenticated { user_id: String, username: String },
}

#[derive(Clone)]
pub(crate) struct InitialLoginReturnLocation(String);

/// 在 Router 规范化地址前保存首次同源 pathname + search。
pub(crate) fn initial_login_return_location() -> InitialLoginReturnLocation {
    let value = browser_path_and_search().unwrap_or_else(|| "/".to_owned());
    InitialLoginReturnLocation(safe_return_path(Some(&value)))
}

pub(super) fn login(return_to: Option<String>) -> Element {
    let mut key = use_signal(String::new);
    let mut result = use_signal(|| None::<LoginState>);
    let mut submitting = use_signal(|| false);
    let mut session_resource = use_context::<PublicSessionResource>();
    let navigator = use_navigator();
    let return_path = safe_return_path(return_to.as_deref());
    let failed = matches!(result(), Some(LoginState::Failure(_)));

    rsx! {
        section {
            class: "public-account-login-page",
            aria_labelledby: "login-title",
            aria_busy: submitting().to_string(),
            h1 { id: "login-title", {t!("page-login-title")} }
            form {
                class: "public-account-login-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if submitting() {
                        return;
                    }
                    let submitted_key = key().trim().to_owned();
                    if submitted_key.is_empty() {
                        result.set(Some(LoginState::Failure(PublicSessionApiError::local(
                            "empty_key",
                        ))));
                        return;
                    }
                    // 原始 key 只用于这一次请求；成功或失败都不继续留在表单状态中。
                    key.set(String::new());
                    submitting.set(true);
                    let return_path = return_path.clone();
                    spawn(async move {
                        let login_result = login_public(submitted_key).await;
                        submitting.set(false);
                        match login_result {
                            Ok(session) => {
                                result.set(Some(LoginState::Success(session.clone())));
                                session_resource.set(Some(Ok(Some(session))));
                                let _ = navigator.replace(return_path);
                            }
                            Err(error) => result.set(Some(LoginState::Failure(error))),
                        }
                    });
                },
                label { for: "public-login-key", {t!("login-key-label")} }
                input {
                    id: "public-login-key",
                    r#type: "password",
                    autocomplete: "current-password",
                    autocapitalize: "none",
                    spellcheck: "false",
                    enterkeyhint: "go",
                    maxlength: "1024",
                    placeholder: if submitting() || failed {
                        "••••••••••••••••••••••••••••••••".to_owned()
                    } else {
                        t!("login-key-placeholder")
                    },
                    "data-masked": (submitting() || failed).to_string(),
                    disabled: submitting(),
                    value: "{key}",
                    oninput: move |event| key.set(event.value()),
                }
                if let Some(LoginState::Failure(error)) = result() {
                    p { class: "public-account-login-error", role: "alert",
                        span { {login_error_message(&error)} }
                        if let Some(reason) = login_blacklisted_reason(&error) {
                            span { class: "public-account-login-ban-reason",
                                {t!("login-blacklisted-reason", reason: reason)}
                            }
                        }
                    }
                }
                button {
                    r#type: "submit",
                    disabled: submitting() || key().trim().is_empty(),
                    "data-error": failed.to_string(),
                    if submitting() { {t!("login-working")} } else { {t!("login-submit")} }
                }
            }
            p { class: "public-account-login-hint", {t!("login-hint")} }
            if let Some(LoginState::Success(session)) = result() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {t!("login-success", username: session.username)}
                }
            }
        }
    }
}

/// 为需要认证的 CTA 保存当前 pathname 与 search；直接登录入口不携带返回地址。
pub(crate) fn login_href_for_current_location() -> String {
    let Some(return_to) = current_path_and_search() else {
        return "/login".to_owned();
    };
    // 旧版用 Router state 保存来源；根路径本就是登录页默认返回值，地址栏保持裸 `/login`。
    if return_to == "/" {
        return "/login".to_owned();
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("return_to", &return_to)
        .finish();
    format!("/login?{query}")
}

fn safe_return_path(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "/".to_owned();
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.starts_with("/login?")
        || value == "/login"
        || value.contains(['\\', '#'])
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return "/".to_owned();
    }
    value.to_owned()
}

fn current_path_and_search() -> Option<String> {
    let current = browser_path_and_search()?;
    if current.contains('?') {
        return (safe_return_path(Some(&current)) == current).then_some(current);
    }
    if let Some(initial) = try_consume_context::<InitialLoginReturnLocation>()
        && initial.0.split('?').next() == Some(current.as_str())
        && safe_return_path(Some(&initial.0)) == initial.0
    {
        return Some(initial.0);
    }
    (safe_return_path(Some(&current)) == current).then_some(current)
}

fn browser_path_and_search() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let location = web_sys::window()?.location();
        let pathname = location.pathname().ok()?;
        let search = location.search().unwrap_or_default();
        Some(format!("{pathname}{search}"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    None
}

/// 页头会话菜单；匿名状态保留旧版的“我的竞价 + 登录”入口，认证状态显示用户名和退出。
#[component]
pub(super) fn SessionMenu() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let mut logout_error = use_signal(|| None::<String>);
    let mut logout_busy = use_signal(|| false);

    let session_state = match &*session.read() {
        Some(Ok(Some(identity))) if identity.authenticated => SessionMenuState::Authenticated {
            user_id: identity.user_id.clone(),
            username: identity.username.clone(),
        },
        None => SessionMenuState::Checking,
        Some(Err(_)) => SessionMenuState::Unavailable,
        Some(Ok(None)) | Some(Ok(Some(_))) => SessionMenuState::Anonymous,
    };

    rsx! {
        div { class: "public-account-session-menu", aria_label: t!("session-menu-label"),
            {match session_state {
                SessionMenuState::Checking => rsx! {
                    span { role: "status", {t!("common-loading")} }
                },
                SessionMenuState::Unavailable => rsx! {
                    span { class: "action-feedback action-feedback--error", role: "alert",
                        {t!("session-error")}
                    }
                    button {
                        r#type: "button",
                        disabled: *session.state().read() != UseResourceState::Ready,
                        onclick: move |_| session.restart(),
                        {t!("data-retry")}
                    }
                },
                SessionMenuState::Anonymous => rsx! {
                    Link {
                        to: Route::Me {},
                        {t!("nav-my-bids")}
                    }
                    Link {
                        class: "public-account-session-login",
                        // typed query route 会把空参数序列化成 `/login?`；直接入口保持旧版精确 URL。
                        to: "/login",
                        {t!("session-login")}
                    }
                },
                SessionMenuState::Authenticated { user_id, username } => rsx! {
                    Link {
                        class: "public-account-session-name",
                        to: Route::Me {},
                        {username}
                    }
                    button {
                        class: "public-account-session-logout",
                        r#type: "button",
                        disabled: logout_busy(),
                        onclick: move |_| {
                            if logout_busy() {
                                return;
                            }
                            logout_error.set(None);
                            logout_busy.set(true);
                            let user_id = user_id.clone();
                            spawn(async move {
                                match logout_public(user_id).await {
                                    Ok(()) => session.set(Some(Ok(None))),
                                    Err(error) => logout_error.set(Some(error)),
                                }
                                logout_busy.set(false);
                            });
                        },
                        {t!("logout")}
                    }
                },
            }}
            if let Some(error) = logout_error() {
                // 登出失败必须给明眼用户可见的反馈（此前仅 visually-hidden 公告，
                // 只有读屏用户能感知），复用 crate 内既有的错误展示样式。
                span { class: "action-feedback action-feedback--error", role: "alert",
                    {match error.as_str() {
                        "session_lock_unavailable" => t!("session-lock-unavailable"),
                        "session_lock_timeout" => t!("session-lock-timeout"),
                        _ => t!("logout-error"),
                    }}
                }
            }
        }
    }
}

/// 登录失败的本地化文案：先匹配登录业务码，未命中走共享基础层（[`base_api_error_message`]
/// 的传输/通用码），最后落到通用错误兜底。
fn login_error_message(error: &PublicSessionApiError) -> String {
    match error.code.as_deref() {
        Some("empty_key" | "invalid_key") => return t!("common-error"),
        Some("invalid_credentials") => return t!("login-invalid-credentials"),
        Some("account_blacklisted") if error.status == Some(403) => {
            return t!("login-blacklisted");
        }
        Some("session_limit") => return t!("login-session-limit"),
        Some("session_lock_unavailable") => return t!("session-lock-unavailable"),
        Some("session_lock_timeout") => return t!("session-lock-timeout"),
        _ => {}
    }
    error
        .code
        .as_deref()
        .and_then(|code| {
            base_api_error_message(code, |kind| match kind {
                BaseErrorKind::Network => t!("login-network-error"),
                BaseErrorKind::Timeout => t!("login-request-timeout"),
                BaseErrorKind::InvalidResponse => t!("login-invalid-response"),
                _ => t!("common-error"),
            })
        })
        .unwrap_or_else(|| t!("common-error"))
}

fn login_blacklisted_reason(error: &PublicSessionApiError) -> Option<&str> {
    (error.status == Some(403) && error.code.as_deref() == Some("account_blacklisted"))
        .then_some(error.public_reason.as_deref())
        .flatten()
}

/// 同源认证广播与窗口聚焦只触发权威重读；账号真正改变后才销毁旧确认宿主。
pub(crate) fn use_public_session_sync(check_session: EventHandler<bool>) {
    #[cfg(target_arch = "wasm32")]
    let _guard = use_hook(move || {
        // Hook 保存 guard 到外壳卸载；最后一个引用释放时移除所有 DOM listener。
        std::rc::Rc::new(PublicSessionSync::install(check_session))
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = check_session;
}

/// 有限会话只在真实截止后复核一次；永久会话不建立计时器，也不把远期日期当永久。
pub(crate) fn use_public_session_expiry(
    session: PublicSessionResource,
    check_session: EventHandler<bool>,
) {
    #[cfg(target_arch = "wasm32")]
    {
        // 同一身份及截止的重复快照不重启已完成的任务，避免服务端时钟误差造成忙轮询。
        let deadline = use_memo(move || {
            session
                .read()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(|identity| identity.as_ref())
                .filter(|identity| identity.authenticated)
                .and_then(|identity| {
                    chrono::DateTime::parse_from_rfc3339(identity.expires_at.as_deref()?).ok().map(
                        |expiry| (identity.user_id.clone(), expiry.with_timezone(&chrono::Utc)),
                    )
                })
        });
        let _expiry_check = use_resource(move || {
            let deadline = deadline();
            async move {
                let Some((_, expiry)) = deadline else { return };
                loop {
                    // Date 头只有秒精度；给出一秒余量，长截止分段等待以免浏览器 i32 超时溢出。
                    let remaining = (expiry - crate::api::estimated_now())
                        .num_milliseconds()
                        .saturating_add(1_000);
                    if remaining <= 0 {
                        check_session.call(false);
                        return;
                    }
                    gloo_timers::future::TimeoutFuture::new(
                        remaining.min(i64::from(i32::MAX)) as u32
                    )
                    .await;
                }
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (session, check_session);
}

#[cfg(target_arch = "wasm32")]
struct PublicSessionSync {
    window: web_sys::Window,
    storage: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::StorageEvent)>,
    focus: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl PublicSessionSync {
    fn install(check_session: EventHandler<bool>) -> Option<Self> {
        use wasm_bindgen::{JsCast, closure::Closure};

        let window = web_sys::window()?;
        let storage = Closure::new(move |event: web_sys::StorageEvent| {
            if event.key().as_deref() == Some(crate::api::PUBLIC_SESSION_CHANGE_STORAGE_KEY) {
                // 随机修订只表示认证操作发生，同账号重登仍须保留原未知命令及 ID。
                check_session.call(false);
            }
        });
        let focus = Closure::new(move || check_session.call(false));
        let guard = Self { window, storage, focus };
        guard
            .window
            .add_event_listener_with_callback("storage", guard.storage.as_ref().unchecked_ref())
            .ok()?;
        guard
            .window
            .add_event_listener_with_callback("focus", guard.focus.as_ref().unchecked_ref())
            .ok()?;
        Some(guard)
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for PublicSessionSync {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;

        let _ = self
            .window
            .remove_event_listener_with_callback("storage", self.storage.as_ref().unchecked_ref());
        let _ = self
            .window
            .remove_event_listener_with_callback("focus", self.focus.as_ref().unchecked_ref());
    }
}

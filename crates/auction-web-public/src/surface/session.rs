//! Public 登录和会话页面。
//! 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影，注销仍通过服务端
//! 会话端口完成。

use std::future::Future;

use auction_web_shared::a11y::focus_element_by_id;
use auction_web_shared::{BaseErrorKind, base_api_error_message, t};
use dioxus::core::Runtime;
use dioxus::prelude::*;

pub(crate) use super::login_return::login_href_for_current_location;
use super::login_return::login_return_path;
use super::{PublicSessionResource, Route};
use crate::api::{
    PublicSession, PublicSessionApiError, SessionRefreshReason, fetch_registration_info,
    login_public, logout_public,
};

/// 账号面板入口按钮与面板的元素 ID；面板关闭时按 ID 把焦点还给入口。
const SESSION_TRIGGER_ID: &str = "public-account-session-trigger";
const SESSION_PANEL_ID: &str = "public-account-session-panel";
const SESSION_ENDED_DISMISS_ID: &str = "public-session-ended-dismiss";

#[cfg(test)]
#[derive(Clone)]
pub(in crate::surface) struct LoginFormFixture(
    pub std::rc::Rc<std::cell::Cell<Option<Signal<String>>>>,
);

/// Cookie 命令的生命期归外壳，路由页只消费仍有接收方的界面结果。
#[derive(Clone, Copy)]
pub(super) struct SessionCommandScope(pub ScopeId);

impl SessionCommandScope {
    pub(super) fn spawn(self, command: impl Future<Output = ()> + 'static) {
        Runtime::current().spawn(self.0, command);
    }
}

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

pub(super) fn login() -> Element {
    let mut key = use_signal(String::new);
    #[cfg(test)]
    if let Some(fixture) = try_use_context::<LoginFormFixture>() {
        fixture.0.set(Some(key));
    }
    let mut result = use_signal(|| None::<LoginState>);
    let mut submitting = use_signal(|| false);
    let mut key_input = use_signal(|| None::<MountedEvent>);
    use_effect(move || {
        if matches!(&*result.read(), Some(LoginState::Failure(_)))
            && let Some(input) = key_input()
        {
            spawn(async move {
                let _ = input.set_focus(true).await;
            });
        }
    });
    let registration_accounts = use_resource(|| async move {
        #[cfg(test)]
        if try_consume_context::<LoginFormFixture>().is_some() {
            return Ok(String::new());
        }
        fetch_registration_info().await.map(|info| info.bot_players.join(", "))
    });
    let navigator = use_navigator();
    let commands = use_context::<SessionCommandScope>();
    let return_path = login_return_path();
    let result_snapshot = result.read();
    let failed = matches!(result_snapshot.as_ref(), Some(LoginState::Failure(_)));

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
                    commands.spawn(async move {
                        let login_result = login_public(submitted_key).await;
                        // 先由外壳交付身份证据和复核通知，卸载的登录页不消费结果或改写地址。
                        if let Ok(mut busy) = submitting.try_write() {
                            *busy = false;
                        } else {
                            return;
                        }
                        match login_result {
                            Ok(session) => {
                                result.set(Some(LoginState::Success(session)));
                                let _ = navigator.replace(return_path);
                            }
                            Err(error) => result.set(Some(LoginState::Failure(error))),
                        }
                    });
                },
                label { for: "public-login-key", {t!("login-key-label")} }
                input {
                    id: "public-login-key",
                    name: "login_key",
                    r#type: "password",
                    autocomplete: "current-password",
                    autocapitalize: "none",
                    spellcheck: "false",
                    enterkeyhint: "go",
                    maxlength: "1024",
                    aria_invalid: failed.to_string(),
                    aria_describedby: if failed { "public-login-hint public-login-error" } else { "public-login-hint" },
                    placeholder: if submitting() || failed {
                        "••••••••••••••••••••••••••••••••".to_owned()
                    } else {
                        t!("login-key-placeholder")
                    },
                    "data-masked": (submitting() || failed).to_string(),
                    disabled: submitting(),
                    value: "{key}",
                    onmounted: move |event| key_input.set(Some(event)),
                    oninput: move |event| {
                        key.set(event.value());
                        result.set(None);
                    },
                }
                if let Some(LoginState::Failure(error)) = result_snapshot.as_ref() {
                    p { id: "public-login-error", class: "public-account-login-error", role: "alert",
                        span { {login_error_message(error)} }
                        if let Some(reason) = login_blacklisted_reason(error) {
                            span { class: "public-account-login-ban-reason",
                                {t!("login-blacklisted-reason", reason: reason)}
                            }
                        }
                    }
                }
                button {
                    r#type: "submit",
                    disabled: submitting(),
                    aria_busy: submitting().to_string(),
                    "data-error": failed.to_string(),
                    if submitting() { {t!("login-working")} } else { {t!("login-submit")} }
                }
            }
            {registration_hint(registration_accounts)}
            if let Some(LoginState::Success(session)) = result_snapshot.as_ref() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {t!("login-success", username: session.username.as_str())}
                }
            }
        }
    }
}

type RegistrationAccounts = Resource<Result<String, String>>;

fn registration_hint(accounts: RegistrationAccounts) -> Element {
    let snapshot = accounts.read();
    let (hint, role) = match snapshot.as_ref() {
        None => (t!("data-loading"), Some("status")),
        Some(Ok(names)) => {
            let bot_account = if names.is_empty() {
                String::new()
            } else {
                t!("login-bot-account", accounts: names.as_str())
            };
            (t!("login-hint", bot_account: bot_account), None)
        }
        Some(Err(_error)) => (t!("data-error"), Some("alert")),
    };
    rsx! {
        p { id: "public-login-hint", class: "public-account-login-hint", role, "{hint}" }
        if matches!(snapshot.as_ref(), Some(Err(_error))) {
            button {
                class: "public-state-retry",
                r#type: "button",
                onclick: move |_| retry_registration(accounts),
                {t!("data-retry")}
            }
        }
    }
}

fn retry_registration(mut accounts: RegistrationAccounts) {
    // 先清除上一次失败，让界面回到加载态。重试只启动名单读取。
    accounts.clear();
    accounts.restart();
}

/// 页头账号菜单：头像下拉，集中账户、退出与匿名登录入口。
#[component]
pub(super) fn SessionMenu() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let commands = use_context::<SessionCommandScope>();
    let mut logout_error = use_signal(|| None::<String>);
    let mut logout_busy = use_signal(|| false);
    let mut menu_open = use_signal(|| false);

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
                    // 顶栏保留唯一登录入口；目录的查看/出价/标记图标也可引导登录。
                    Link {
                        class: "public-account-session-login",
                        // typed query route 会把空参数序列化成 `/login?`；直接入口保持旧版精确 URL。
                        to: "/login",
                        {t!("session-login")}
                    }
                },
                SessionMenuState::Authenticated { user_id, username } => {
                    let initial = username
                        .chars()
                        .next()
                        .map(|value| value.to_uppercase().collect::<String>())
                        .unwrap_or_default();
                    rsx! {
                        // 展开式（disclosure）面板而非 ARIA 菜单：条目是普通链接与按钮，按 Tab 浏览，
                        // 不提供菜单要求的方向键导航。
                        button {
                            id: SESSION_TRIGGER_ID,
                            class: "public-account-session-trigger",
                            r#type: "button",
                            aria_controls: SESSION_PANEL_ID,
                            aria_expanded: menu_open().to_string(),
                            onclick: move |_| menu_open.set(!menu_open()),
                            onkeydown: move |event| {
                                if menu_open() && event.key() == Key::Escape {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    close_session_menu(menu_open);
                                }
                            },
                            span { class: "public-account-session-avatar", aria_hidden: "true", "{initial}" }
                            span { class: "public-account-session-name", "{username}" }
                            svg {
                                class: "public-account-session-chevron",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "1.5",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                "aria-hidden": "true",
                                path { d: "m6 9 6 6 6-6" }
                            }
                        }
                        if menu_open() {
                            div {
                                class: "public-account-session-backdrop",
                                onclick: move |_| close_session_menu(menu_open),
                            }
                            div {
                                id: SESSION_PANEL_ID,
                                class: "public-account-session-panel",
                                onkeydown: move |event| {
                                    if event.key() == Key::Escape {
                                        event.prevent_default();
                                        event.stop_propagation();
                                        close_session_menu(menu_open);
                                    }
                                },
                                // “我的”与退出不把焦点还给入口：路由切换接管焦点，退出后入口随会话卸载。
                                Link {
                                    class: "public-account-session-item",
                                    to: Route::Me {},
                                    onclick: move |_| menu_open.set(false),
                                    {t!("nav-my-bids")}
                                }
                                button {
                                    class: "public-account-session-item",
                                    r#type: "button",
                                    disabled: logout_busy(),
                                    onclick: move |_| {
                                        if logout_busy() {
                                            return;
                                        }
                                        menu_open.set(false);
                                        logout_error.set(None);
                                        logout_busy.set(true);
                                        let user_id = user_id.clone();
                                        commands.spawn(async move {
                                            match logout_public(user_id).await {
                                                Ok(()) => {},
                                                Err(error) => logout_error.set(Some(error)),
                                            }
                                            logout_busy.set(false);
                                        });
                                    },
                                    if logout_busy() { {t!("common-loading")} } else { {t!("logout")} }
                                }
                            }
                        }
                    }
                },
            }}
            if let Some(error) = logout_error() {
                // 登出失败必须有可见反馈，不能只留给读屏用户。
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

/// 读屏状态区域在外壳首次挂载时就存在；提示可手动关闭，路由切换保留状态。
#[component]
pub(super) fn SessionEndedNotice(ended: Signal<bool>) -> Element {
    rsx! {
        div { class: if ended() { "public-shell__session-ended-banner" } else { "" },
            span { class: "public-shell__session-ended-text", role: "status",
                if ended() { {t!("session-ended-elsewhere")} }
            }
            if ended() {
                Link {
                    class: "public-shell__session-ended-login",
                    to: "/login",
                    {t!("session-ended-relogin")}
                }
                button {
                    id: SESSION_ENDED_DISMISS_ID,
                    class: "public-shell__session-ended-dismiss",
                    r#type: "button",
                    aria_label: t!("catalog-close"),
                    onclick: move |_| dismiss_session_ended(ended, session_ended_dismiss_is_focused(), || {
                        focus_element_by_id("public-content");
                    }),
                    "✕"
                }
            }
        }
    }
}

fn session_ended_dismiss_is_focused() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .is_some_and(|element| element.id() == SESSION_ENDED_DISMISS_ID)
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

fn dismiss_session_ended(mut ended: Signal<bool>, owns_focus: bool, focus_content: impl FnOnce()) {
    if owns_focus {
        // 关闭按钮即将卸载，先把焦点交给常驻的正文入口。
        focus_content();
    }
    ended.set(false);
}

/// 收起账号面板并把焦点还给入口按钮；面板卸载时焦点不能落到 `body`。
fn close_session_menu(mut open: Signal<bool>) {
    open.set(false);
    focus_element_by_id(SESSION_TRIGGER_ID);
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
pub(crate) fn use_public_session_sync(check_session: EventHandler<SessionRefreshReason>) {
    #[cfg(target_arch = "wasm32")]
    let _guard = use_hook(move || {
        // Hook 保存 guard 到外壳卸载；最后一个引用释放时移除所有 DOM listener。
        std::rc::Rc::new(PublicSessionSync::install(check_session))
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = check_session;
}

#[cfg(target_arch = "wasm32")]
struct PublicSessionSync {
    window: web_sys::Window,
    storage: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::StorageEvent)>,
    focus: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl PublicSessionSync {
    fn install(check_session: EventHandler<SessionRefreshReason>) -> Option<Self> {
        use wasm_bindgen::{JsCast, closure::Closure};

        let window = web_sys::window()?;
        let storage = Closure::new(move |event: web_sys::StorageEvent| {
            if event.key().as_deref() == Some(crate::api::PUBLIC_SESSION_CHANGE_STORAGE_KEY)
                && event.new_value().is_some()
            {
                // 随机修订只表示认证操作发生，同账号重登仍须保留原未知命令及 ID。
                check_session.call(SessionRefreshReason::CookieChanged);
            }
        });
        let focus = Closure::new(move || check_session.call(SessionRefreshReason::Refresh));
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        RegistrationAccounts, dismiss_session_ended, registration_hint, retry_registration,
    };

    #[derive(Clone)]
    struct DismissalFixture {
        has_focus: bool,
        moved_focus: Rc<Cell<bool>>,
    }

    #[component]
    fn DismissalHarness() -> Element {
        let fixture = consume_context::<DismissalFixture>();
        let ended = use_signal(|| true);
        use_effect(move || {
            dismiss_session_ended(ended, fixture.has_focus, || {
                assert!(*ended.peek(), "焦点须在关闭按钮卸载前转移");
                fixture.moved_focus.set(true);
            });
            assert!(!*ended.peek());
        });
        rsx! {}
    }

    #[test]
    fn session_notice_moves_focus_only_when_its_dismiss_button_has_focus() {
        for has_focus in [true, false] {
            let moved_focus = Rc::new(Cell::new(false));
            let fixture = DismissalFixture { has_focus, moved_focus: moved_focus.clone() };
            let mut dom = VirtualDom::new(DismissalHarness).with_root_context(fixture);
            dom.rebuild_in_place();
            dom.process_events();
            assert_eq!(moved_focus.get(), has_focus);
        }
    }

    #[derive(Default)]
    struct RegistrationControl {
        reads: Cell<u8>,
        empty: bool,
        accounts: Cell<Option<RegistrationAccounts>>,
        retry_ready: Cell<bool>,
        retry_waker: RefCell<Option<Waker>>,
    }

    #[component]
    fn RegistrationHarness() -> Element {
        let control = consume_context::<Rc<RegistrationControl>>();
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                include_str!("../../i18n/en.ftl"),
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        })
        .expect("初始化英文译文");
        let script = control.clone();
        let accounts = use_resource(move || {
            let attempt = script.reads.get() + 1;
            script.reads.set(attempt);
            let script = script.clone();
            async move {
                if script.empty {
                    Ok(String::new())
                } else if attempt == 1 {
                    Err("configuration_unavailable".to_owned())
                } else {
                    poll_fn(|context| {
                        if script.retry_ready.get() {
                            Poll::Ready(())
                        } else {
                            *script.retry_waker.borrow_mut() = Some(context.waker().clone());
                            Poll::Pending
                        }
                    })
                    .await;
                    Ok("Auction Bot".to_owned())
                }
            }
        });
        control.accounts.set(Some(accounts));
        registration_hint(accounts)
    }

    fn has_text(edits: &[Mutation], text: &str) -> bool {
        edits.iter().any(|edit| match edit {
            Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } => {
                value.contains(text)
            }
            _ => false,
        })
    }

    #[test]
    fn empty_registration_response_is_a_success_after_loading() {
        let control =
            Rc::new(RegistrationControl { empty: true, ..RegistrationControl::default() });
        let mut dom = VirtualDom::new(RegistrationHarness).with_root_context(control.clone());
        let loading = dom.rebuild_to_vec();
        assert!(has_text(&loading.edits, "Loading…"));
        assert!(!has_text(&loading.edits, "MoogleMail"));

        dom.process_events();
        let loaded = dom.render_immediate_to_vec();
        assert!(has_text(&loaded.edits, "MoogleMail"));
        assert!(!has_text(&loaded.edits, "Retry"));
        assert_eq!(control.reads.get(), 1);
    }

    #[test]
    fn registration_failure_can_retry_the_list_and_show_the_recipient() {
        let control = Rc::new(RegistrationControl::default());
        let mut dom = VirtualDom::new(RegistrationHarness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let failed = dom.render_immediate_to_vec();
        assert!(has_text(&failed.edits, "temporarily unavailable"));
        assert!(!has_text(&failed.edits, "MoogleMail"));
        assert!(failed.edits.iter().any(|edit| matches!(
            edit,
            Mutation::NewEventListener { name, .. } if name == "click"
        )));
        let accounts = control.accounts.get().expect("挂载名单资源");
        dom.in_runtime(|| retry_registration(accounts));
        let loading = dom.render_immediate_to_vec();
        assert!(has_text(&loading.edits, "Loading…"));
        control.retry_ready.set(true);
        control.retry_waker.borrow_mut().take().expect("名单读取正在等待响应").wake();
        dom.process_events();
        let loaded = dom.render_immediate_to_vec();
        assert!(has_text(&loaded.edits, "Auction Bot"));
        assert_eq!(control.reads.get(), 2);
    }
}

//! “我的”登录时长设置；账号偏好与当前会话实际截止分开展示，不从永久默认值伪造已加载状态。

mod duration;
mod state;

use auction_web_shared::{Time, legacy_t as t};
use dioxus::prelude::*;

use self::duration::DurationUnit;
use self::state::{PreferenceOwner, PreferenceState};
use crate::app::surface::{PublicCommandOwner, PublicSessionResource};

/// 父页面按用户 UUID 隔离宿主；账号变化或卸载取消任务并销毁未决命令。
#[component]
pub(super) fn SessionPreference(user_id: String) -> Element {
    let session = use_context::<PublicSessionResource>();
    let mut state = use_signal(PreferenceState::default);
    let identity = use_context::<PublicCommandOwner>().0;
    let owner = PreferenceOwner { user_id, identity, session, state };
    use_effect({
        let mut owner = owner.clone();
        move || owner.load(false)
    });
    let current_expiry = use_memo(move || {
        session
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|identity| identity.as_ref())
            .filter(|identity| identity.authenticated)
            .map(|identity| identity.expires_at.clone())
    });
    use_effect(move || {
        // 只跟随实际截止变化，不因同身份重读重置用户草稿或未知命令；GET 可提供更新的截止。
        state.write().current_expires_at = current_expiry();
    });

    let snapshot = state.read();
    let busy = snapshot.busy;
    let pending = snapshot.pending.is_some();
    let ready = owner.session_ready() && snapshot.authoritative.is_some();
    let controls_disabled = !ready || busy || pending;
    let permanent = ready && snapshot.draft.permanent;
    let custom = ready && !snapshot.draft.permanent;
    let amount = snapshot.draft.amount.clone();
    let unit = snapshot.draft.unit.value();
    let maximum = snapshot.draft.unit.maximum();
    let current_expires_at = snapshot.current_expires_at.clone();
    let failed = snapshot.error.is_some();
    let validation_error = snapshot.validation_error
        || (!snapshot.pending_uncertain
            && snapshot
                .error
                .as_ref()
                .is_some_and(|error| error.code == "invalid_session_preference"));
    let message = feedback(&snapshot);
    let uncertain = snapshot.pending_uncertain;
    let confirmed = snapshot.confirmed;
    let mut submit = owner.clone();
    let mut retry = owner.clone();
    let mut refresh = owner.clone();
    let mut select_permanent = owner.clone();
    let mut select_custom = owner.clone();
    let mut change_amount = owner.clone();
    let mut change_unit = owner;

    rsx! {
        section { id: "login-duration", class: "public-session-preference",
            aria_busy: busy.to_string(),
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    submit.submit();
                },
                fieldset {
                    aria_describedby: "session-duration-hint session-duration-feedback",
                    legend { {t!("session-duration-title")} }
                    p { id: "session-duration-hint", class: "public-session-preference__hint",
                        {t!("session-duration-hint")}
                    }
                    div { class: "public-session-preference__choices",
                        div {
                            input {
                                id: "session-duration-permanent", r#type: "radio",
                                name: "session-duration-mode", value: "permanent",
                                checked: permanent, disabled: controls_disabled,
                                onchange: move |_| select_permanent.change(|draft| draft.permanent = true),
                            }
                            label { r#for: "session-duration-permanent", {t!("session-duration-permanent")} }
                        }
                        div {
                            input {
                                id: "session-duration-custom", r#type: "radio",
                                name: "session-duration-mode", value: "custom",
                                checked: custom, disabled: controls_disabled,
                                onchange: move |_| select_custom.change(|draft| draft.permanent = false),
                            }
                            label { r#for: "session-duration-custom", {t!("session-duration-custom")} }
                        }
                    }
                    if custom {
                        div { class: "public-session-preference__duration",
                            div { class: "public-session-preference__field",
                                label { r#for: "session-duration-amount", {t!("session-duration-amount")} }
                                input {
                                    id: "session-duration-amount", name: "session-duration-amount",
                                    r#type: "number", inputmode: "decimal", required: true,
                                    min: "0", max: maximum, step: "any", value: amount,
                                    disabled: controls_disabled,
                                    aria_invalid: validation_error.to_string(),
                                    aria_describedby: "session-duration-range session-duration-feedback",
                                    oninput: move |event| change_amount.change(|draft| draft.amount = event.value()),
                                }
                            }
                            div { class: "public-session-preference__field",
                                label { r#for: "session-duration-unit", {t!("session-duration-unit")} }
                                select {
                                    id: "session-duration-unit", name: "session-duration-unit",
                                    value: unit, disabled: controls_disabled,
                                    aria_describedby: "session-duration-range session-duration-feedback",
                                    onchange: move |event| {
                                        if let Some(unit) = DurationUnit::from_value(&event.value()) {
                                            change_unit.change(|draft| draft.unit = unit);
                                        }
                                    },
                                    option { value: "minutes", {t!("session-duration-minutes")} }
                                    option { value: "hours", {t!("session-duration-hours")} }
                                    option { value: "days", {t!("session-duration-days")} }
                                }
                            }
                        }
                        p { id: "session-duration-range", class: "public-session-preference__hint",
                            {t!("session-duration-range")}
                        }
                    }
                    if let Some(expires_at) = current_expires_at {
                        p { class: "public-session-preference__current",
                            span { {t!("session-duration-current")} }
                            if let Some(expires_at) = expires_at {
                                Time { value: expires_at }
                            } else {
                                span { {t!("session-duration-current-permanent")} }
                            }
                        }
                    }
                    p { class: "public-session-preference__hint", {t!("session-duration-security")} }
                    div { class: "public-session-preference__actions",
                        button {
                            r#type: "submit", disabled: controls_disabled,
                            aria_describedby: "session-duration-feedback",
                            {t!("session-duration-save")}
                        }
                        if failed && uncertain {
                            button {
                                r#type: "button", disabled: busy,
                                aria_describedby: "session-duration-feedback",
                                onclick: move |_| retry.submit(),
                                {t!("session-duration-retry")}
                            }
                        } else {
                            button {
                                r#type: "button", disabled: busy,
                                aria_describedby: "session-duration-feedback",
                                onclick: move |_| refresh.load(false),
                                {t!("session-duration-refresh")}
                            }
                        }
                    }
                    div { id: "session-duration-feedback", aria_live: "polite", aria_atomic: "true",
                        if let Some(message) = message {
                            p { class: "public-session-preference__error", "{message}" }
                        } else if busy {
                            p { if pending { {t!("session-duration-saving")} } else { {t!("session-duration-loading")} } }
                        } else if confirmed {
                            p { {t!("session-duration-confirmed")} }
                        }
                    }
                }
            }
        }
    }
}

fn feedback(state: &PreferenceState) -> Option<String> {
    if state.validation_error {
        return Some(t!("session-duration-validation"));
    }
    let error = state.error.as_ref()?;
    Some(if error.status == 409 {
        t!("session-duration-conflict")
    } else if state.pending_uncertain {
        t!("session-duration-unknown")
    } else if error.code == "session_lock_unavailable" {
        t!("session-lock-unavailable")
    } else if error.code == "session_lock_timeout" {
        t!("session-lock-timeout")
    } else if error.code == "invalid_session_preference" {
        t!("session-duration-validation")
    } else if state.pending.is_none() {
        t!("session-duration-load-error")
    } else {
        t!("session-duration-save-error")
    })
}

//! 账号警告渠道的独立命令宿主；只持有内存意图，不与竞价分页资源共用生命周期。

use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::PublicSessionResource;
use crate::api::{CommandError, fetch_lot_watch_preference, set_lot_watch_preference};

#[derive(Default)]
struct PreferenceState {
    authoritative: Option<PublicLotWatchPreferenceResponse>,
    selected: Option<String>,
    pending: Option<PublicSetLotWatchPreferenceRequest>,
    pending_uncertain: bool,
    busy: bool,
    error: Option<CommandError>,
    confirmed: bool,
}

#[derive(Clone)]
struct PreferenceOwner {
    user_id: String,
    session: PublicSessionResource,
    state: Signal<PreferenceState>,
}

impl PreferenceOwner {
    fn identity_matches(&self) -> bool {
        self.session.peek().as_ref().is_some_and(|snapshot| {
            snapshot
                .as_ref()
                .ok()
                .and_then(|identity| identity.as_ref())
                .is_some_and(|identity| identity.authenticated && identity.user_id == self.user_id)
        })
    }

    /// 只有首次读取、确定性失败后的显式刷新或已确认写入才重读；未知命令不得丢弃。
    fn load(&mut self, confirmed: bool) {
        if !self.identity_matches() {
            return;
        }
        if !confirmed {
            let state = self.state.peek();
            if state.busy || state.pending_uncertain {
                return;
            }
        }
        self.state.set(PreferenceState { busy: true, ..Default::default() });
        let mut owner = self.clone();
        spawn(async move {
            if !owner.identity_matches() {
                return;
            }
            let result = fetch_lot_watch_preference(&owner.user_id).await;
            if !owner.identity_matches() {
                return;
            }
            let mut state = owner.state.write();
            state.busy = false;
            match result {
                Ok(preference) => {
                    state.selected = Some(preference.channel.clone());
                    state.authoritative = Some(preference);
                    state.confirmed = confirmed;
                }
                Err(error) => state.error = Some(error),
            }
        });
    }

    fn submit(&mut self) {
        if !self.identity_matches() || self.state.peek().busy {
            return;
        }
        let payload = {
            let state = self.state.peek();
            if let Some(pending) = &state.pending {
                if !state.pending_uncertain {
                    return;
                }
                pending.clone()
            } else {
                let Some(preference) = &state.authoritative else { return };
                let Some(channel) = &state.selected else { return };
                PublicSetLotWatchPreferenceRequest {
                    command_id: Uuid::new_v4().to_string(),
                    channel: channel.clone(),
                    expected_version: preference.version,
                }
            }
        };
        {
            let mut state = self.state.write();
            state.pending = Some(payload.clone());
            state.busy = true;
            state.error = None;
            state.confirmed = false;
        }
        let mut owner = self.clone();
        spawn(async move {
            if !owner.identity_matches() {
                return;
            }
            let result = set_lot_watch_preference(&owner.user_id, &payload).await;
            if !owner.identity_matches() {
                return;
            }
            match result {
                // 幂等重放可以返回历史结果，必须 GET 后才展示当前渠道及下一次 CAS 版本。
                Ok(_) => owner.load(true),
                Err(error) => {
                    let mut state = owner.state.write();
                    state.busy = false;
                    // 重试被前置拒绝不证明首次未提交；仅明确冲突或成功可解除累计未知状态。
                    state.pending_uncertain = error.status != 409
                        && (state.pending_uncertain || error.outcome_uncertain());
                    state.error = Some(error);
                }
            }
        });
    }
}

/// 父账户组件按 user_id 设置 key；卸载取消所属任务，跨身份响应还需前后双重检查。
#[component]
pub(super) fn OutbidNotifications(user_id: String) -> Element {
    let session = use_context::<PublicSessionResource>();
    let mut state = use_signal(PreferenceState::default);
    let owner = PreferenceOwner { user_id, session, state };
    use_effect({
        let mut owner = owner.clone();
        move || owner.load(false)
    });
    let snapshot = state.read();
    let busy = snapshot.busy;
    let pending = snapshot.pending.is_some();
    let ready = snapshot.authoritative.is_some() && owner.identity_matches();
    let failed = snapshot.error.is_some();
    let uncertain = snapshot.pending_uncertain;
    let message = if !pending {
        t!("outbid-load-error")
    } else if snapshot.error.as_ref().is_some_and(|error| error.status == 409) {
        t!("outbid-conflict")
    } else if uncertain {
        t!("outbid-unknown")
    } else {
        t!("outbid-save-error")
    };
    let mm = snapshot.selected.as_deref() == Some("mm");
    let pm = snapshot.selected.as_deref() == Some("pm");
    let confirmed = snapshot.confirmed;
    let mut submit = owner.clone();
    let mut retry = owner.clone();
    let mut refresh = owner.clone();
    let select_mm = owner.clone();
    let select_pm = owner;

    rsx! {
        section { id: "outbid-notifications", class: "public-outbid-preference",
            aria_busy: busy.to_string(),
            fieldset {
                aria_describedby: "outbid-feedback",
                legend { {t!("outbid-title")} }
                div { class: "public-outbid-preference__choices",
                    div {
                        input {
                            id: "outbid-channel-mm", r#type: "radio", name: "outbid-channel",
                            value: "mm", checked: mm, disabled: !ready || busy || pending,
                            aria_describedby: "outbid-feedback",
                            onchange: move |_| {
                                if select_mm.identity_matches() && !state.peek().busy && state.peek().pending.is_none() {
                                    let mut state = state.write();
                                    state.selected = Some("mm".to_owned());
                                    state.confirmed = false;
                                }
                            },
                        }
                        label { r#for: "outbid-channel-mm", {t!("outbid-mm")} }
                    }
                    div {
                        input {
                            id: "outbid-channel-pm", r#type: "radio", name: "outbid-channel",
                            value: "pm", checked: pm, disabled: !ready || busy || pending,
                            aria_describedby: "outbid-feedback",
                            onchange: move |_| {
                                if select_pm.identity_matches() && !state.peek().busy && state.peek().pending.is_none() {
                                    let mut state = state.write();
                                    state.selected = Some("pm".to_owned());
                                    state.confirmed = false;
                                }
                            },
                        }
                        label { r#for: "outbid-channel-pm", {t!("outbid-pm")} }
                    }
                }
                div { class: "public-outbid-preference__actions",
                    button {
                        r#type: "button", disabled: !ready || busy || pending,
                        aria_describedby: "outbid-feedback",
                        onclick: move |_| submit.submit(),
                        {t!("outbid-save")}
                    }
                    if failed && uncertain {
                        button {
                            r#type: "button", disabled: busy,
                            aria_describedby: "outbid-feedback",
                            onclick: move |_| retry.submit(),
                            {t!("outbid-retry-command")}
                        }
                    } else if failed {
                        button {
                            r#type: "button", disabled: busy,
                            aria_describedby: "outbid-feedback",
                            onclick: move |_| {
                                if !refresh.state.peek().busy { refresh.load(false); }
                            },
                            {t!("outbid-refresh")}
                        }
                    }
                }
                div { id: "outbid-feedback", aria_live: "polite", aria_atomic: "true",
                    if failed {
                        p { class: "public-outbid-preference__error", "{message}" }
                    } else if busy {
                        p { if pending { {t!("outbid-saving")} } else { {t!("outbid-loading")} } }
                    } else if confirmed {
                        p { {t!("outbid-confirmed")} }
                    }
                }
            }
        }
    }
}

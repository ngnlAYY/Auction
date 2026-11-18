//! 账号警告渠道的独立命令宿主；只持有内存意图，不与竞价分页资源共用生命周期。
//! MM 与 PM 是两个独立开关：可以都开，也可以都关（标记只作收藏，不产生提醒）。

use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::{PublicSessionResource, use_content_loading};
use crate::api::{
    CommandError, channel_flags, channel_value, fetch_lot_watch_preference,
    set_lot_watch_preference,
};
use crate::surface::PublicCommandOwner;

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
    identity: Signal<Option<String>>,
    session: PublicSessionResource,
    state: Signal<PreferenceState>,
}

impl PreferenceOwner {
    fn identity_matches(&self) -> bool {
        self.identity.peek().as_deref() == Some(self.user_id.as_str())
    }

    /// 会话暂时失败不能清除已提交意图，但也不能授权新的渠道变更。
    fn session_ready(&self) -> bool {
        self.identity_matches()
            && *self.session.state().read() == UseResourceState::Ready
            && self.session.read().as_ref().is_some_and(|snapshot| {
                matches!(snapshot, Ok(Some(identity)) if identity.authenticated
                    && identity.user_id == self.user_id)
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
                if !self.session_ready() {
                    return;
                }
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
                    // 只有路由的明确冲突码能否定本命令；裸 409 或损坏响应不能抹掉首次未知。
                    state.pending_uncertain = !(error.status == 409
                        && error.code == "watch_preference_conflict")
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
    let identity = use_context::<PublicCommandOwner>().0;
    let owner = PreferenceOwner { user_id, identity, session, state };
    use_effect({
        let mut owner = owner.clone();
        move || owner.load(false)
    });
    let snapshot = state.read();
    let busy = snapshot.busy;
    let pending = snapshot.pending.is_some();
    let ready = owner.session_ready() && snapshot.authoritative.is_some();
    // 读取偏好由顶栏提示；保存是本表单的命令反馈，留在表单内。
    use_content_loading(busy && !pending);
    let failed = snapshot.error.is_some();
    let uncertain = snapshot.pending_uncertain;
    let message = if !pending {
        t!("outbid-load-error")
    } else if snapshot
        .error
        .as_ref()
        .is_some_and(|error| error.status == 409 && error.code == "watch_preference_conflict")
    {
        t!("outbid-conflict")
    } else if uncertain {
        t!("outbid-unknown")
    } else if snapshot
        .error
        .as_ref()
        .is_some_and(|error| error.status == 422 && error.code == "pm_channel_unavailable")
    {
        t!("outbid-pm-unavailable")
    } else {
        t!("outbid-save-error")
    };
    let (mm, pm) = snapshot.selected.as_deref().and_then(channel_flags).unwrap_or_default();
    // PM 需要论坛同步已启用且账号已绑定论坛账号；不可用时不允许新开启，已开启的仍可关闭。
    let pm_available =
        snapshot.authoritative.as_ref().is_some_and(|preference| preference.pm_available);
    let confirmed = snapshot.confirmed;
    let mut submit = owner.clone();
    let mut retry = owner.clone();
    let mut refresh = owner.clone();
    // 两个开关各自只改写本渠道，另一渠道保持当前选择；提交仍是一条完整的偏好命令。
    let toggle_owner = owner;
    let toggle = use_callback(move |(is_pm, checked): (bool, bool)| {
        if !toggle_owner.session_ready() || state.peek().busy || state.peek().pending.is_some() {
            return;
        }
        let mut state = state.write();
        let (mm, pm) = state.selected.as_deref().and_then(channel_flags).unwrap_or_default();
        let next = if is_pm { channel_value(mm, checked) } else { channel_value(checked, pm) };
        state.selected = Some(next.to_owned());
        state.confirmed = false;
    });

    rsx! {
        section { id: "outbid-notifications", class: "public-outbid-preference",
            aria_busy: busy.to_string(),
            fieldset {
                aria_describedby: "outbid-feedback",
                legend { {t!("outbid-title")} }
                div { class: "public-outbid-preference__choices",
                    div {
                        input {
                            id: "outbid-channel-mm", r#type: "checkbox", name: "outbid-channel-mm",
                            checked: mm, disabled: !ready || busy || pending,
                            aria_describedby: "outbid-feedback",
                            onchange: move |event| toggle.call((false, event.checked())),
                        }
                        label { r#for: "outbid-channel-mm", {t!("outbid-mm")} }
                    }
                    div {
                        input {
                            id: "outbid-channel-pm", r#type: "checkbox", name: "outbid-channel-pm",
                            checked: pm,
                            disabled: !ready || busy || pending || (!pm_available && !pm),
                            aria_describedby: "outbid-feedback outbid-channel-hint",
                            onchange: move |event| toggle.call((true, event.checked())),
                        }
                        label { r#for: "outbid-channel-pm", {t!("outbid-pm")} }
                    }
                }
                // 渠道说明常驻同一位置：都未选时说明不会收到提醒，PM 不可用时说明原因。
                div { id: "outbid-channel-hint", class: "public-outbid-preference__hint",
                    if ready && !mm && !pm {
                        p { {t!("outbid-none-hint")} }
                    }
                    if ready && !pm_available {
                        p { {t!("outbid-pm-unavailable")} }
                    }
                }
                div { class: "public-outbid-preference__actions",
                    button {
                        id: "public-outbid-save",
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
                            id: "public-outbid-refresh",
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
                    } else if busy && pending {
                        p { {t!("outbid-saving")} }
                    } else if confirmed {
                        p { {t!("outbid-confirmed")} }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;

    use super::OutbidNotifications;
    use crate::api::{CommandError, PublicSession};
    use crate::surface::{
        PageReadFixture, PublicCommandOwner, PublicSessionResource, dispatch_page_event,
        page_test_contexts, rendered_event_target, run_page_events, settle_page,
    };

    const USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const PATH: &str = "/api/v1/me/lot-watch-preference";
    const PM_REASON: &str =
        "PM needs forum sync to be enabled and a forum account linked to this account.";

    fn preferences() -> Element {
        page_test_contexts();
        use_context_provider(|| PublicCommandOwner(Signal::new(Some(USER.to_owned()))));
        let session: PublicSessionResource = use_resource(|| async {
            Ok(Some(PublicSession {
                authenticated: true,
                user_id: USER.to_owned(),
                username: "player".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            }))
        });
        use_context_provider(|| session);
        rsx! { OutbidNotifications { user_id: USER.to_owned() } }
    }

    #[test]
    fn pm_eligibility_rejection_explains_the_reason_and_reloads_the_capability() {
        run_page_events(
            module_path!(),
            "pm_eligibility_rejection_explains_the_reason_and_reloads_the_capability",
            exercise_rejection,
        );
    }

    fn exercise_rejection() {
        for code in ["pm_channel_unavailable", "invalid_request"] {
            let reads = Rc::new(Cell::new(0));
            let writes = Rc::new(Cell::new(0));
            let read = PageReadFixture::new({
                let reads = reads.clone();
                move |(path, user_id): (&'static str, String)| {
                    assert_eq!((path, user_id.as_str()), (PATH, USER));
                    let previous = reads.get();
                    reads.set(previous + 1);
                    Ok::<_, CommandError>(PublicLotWatchPreferenceResponse {
                        user_id,
                        channel: "pm".to_owned(),
                        version: 1,
                        pm_available: previous == 0,
                    })
                }
            });
            let write = PageReadFixture::new({
                let writes = writes.clone();
                move |(path, user_id, command): (
                    &'static str,
                    String,
                    PublicSetLotWatchPreferenceRequest,
                )| {
                    assert_eq!((path, user_id.as_str()), (PATH, USER));
                    assert_eq!((command.channel.as_str(), command.expected_version), ("pm", 1));
                    writes.set(writes.get() + 1);
                    Err::<PublicLotWatchPreferenceResponse, _>(CommandError {
                        status: 422,
                        code: code.to_owned(),
                    })
                }
            });
            let mut dom =
                VirtualDom::new(preferences).with_root_context(read).with_root_context(write);
            dom.rebuild_in_place();
            settle_page(&mut dom);
            let save = rendered_event_target(&dom, "button", "id", "public-outbid-save", "onclick");
            dispatch_page_event(&dom, "click", save, "");
            let feedback = settle_page(&mut dom);
            assert_eq!(writes.get(), 1);
            assert_eq!(reads.get(), 1);
            assert_eq!(feedback.iter().any(|edit| matches!(
                edit, Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } if value == PM_REASON
            )), code == "pm_channel_unavailable");
            let refresh =
                rendered_event_target(&dom, "button", "id", "public-outbid-refresh", "onclick");
            dispatch_page_event(&dom, "click", refresh, "");
            settle_page(&mut dom);
            assert_eq!(reads.get(), 2);
            assert_eq!(writes.get(), 1);
        }
    }
}

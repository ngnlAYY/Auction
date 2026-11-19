//! 登录时长设置的账号隔离命令宿主；未知结果冻结命令、时长和 CAS 版本直到明确确认。

use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use dioxus::core::Runtime;
use dioxus::prelude::*;
use uuid::Uuid;

use super::duration::DurationDraft;
use crate::api::{CommandError, fetch_session_preference, set_session_preference};
use crate::surface::PublicSessionResource;
use crate::surface::account::preference_reload::{PreferenceRead, PreferenceReload};
use crate::surface::commands::SessionAuthority;
use crate::surface::session::SessionCommandScope;

#[derive(Default)]
pub(super) struct PreferenceState {
    pub(super) authoritative: Option<PublicSessionPreferenceResponse>,
    pub(super) draft: DurationDraft,
    pub(super) current_expires_at: Option<Option<String>>,
    pub(super) pending: Option<PublicSetSessionPreferenceRequest>,
    pub(super) pending_uncertain: bool,
    pub(super) busy: bool,
    pub(super) error: Option<CommandError>,
    pub(super) validation_error: bool,
    pub(super) confirmed: bool,
}

impl PreferenceState {
    pub(super) fn reload_blocked(&self) -> bool {
        self.busy || self.pending_uncertain
    }

    fn accept_read(&mut self, preference: PublicSessionPreferenceResponse, preserve_draft: bool) {
        self.current_expires_at = Some(preference.expires_at.clone());
        if preserve_draft
            && self.authoritative.as_ref().is_some_and(|current| {
                self.draft != DurationDraft::from_seconds(current.duration_seconds)
            })
        {
            // 脏草稿仍须用编辑开始时的 CAS 版本，否则会覆盖另一页已确认的更新。
            return;
        }
        self.draft = DurationDraft::from_seconds(preference.duration_seconds);
        self.authoritative = Some(preference);
    }

    fn reject_write(&mut self, error: CommandError) {
        self.busy = false;
        // 只有路由的明确冲突码能否定本命令；未发送不能否定此前的未知提交。
        self.pending_uncertain = !(error.status == 409
            && error.code == "session_preference_conflict")
            && (self.pending_uncertain || error.outcome_uncertain());
        if !self.pending_uncertain
            && (error.not_sent()
                || matches!(
                    error.code.as_str(),
                    "session_lock_unavailable" | "session_lock_timeout"
                ))
        {
            // 首次尝试尚未发送，保留草稿并允许会话恢复后手动提交。
            self.pending = None;
        }
        self.error = Some(error);
    }
}

#[derive(Clone)]
pub(super) struct PreferenceOwner {
    pub(super) user_id: String,
    pub(super) identity: Signal<Option<String>>,
    pub(super) session: PublicSessionResource,
    pub(super) state: Signal<PreferenceState>,
    pub(super) form_scope: ScopeId,
    pub(super) commands: SessionCommandScope,
    pub(super) reload: PreferenceReload,
    pub(super) authority: SessionAuthority,
}

impl PreferenceOwner {
    pub(super) fn identity_matches(&self) -> bool {
        self.state.try_peek().is_ok()
            && self.identity.peek().as_deref() == Some(self.user_id.as_str())
    }

    /// 新输入和原样重试均须有当前会话证明；结果收口只依赖存活的同账号表单。
    pub(super) fn session_ready(&self) -> bool {
        self.identity_matches()
            && *self.session.state().read() == UseResourceState::Ready
            && self.session.read().as_ref().is_some_and(|snapshot| {
                matches!(snapshot, Ok(Some(identity)) if identity.authenticated
                    && identity.user_id == self.user_id)
            })
    }

    pub(super) fn change(&mut self, update: impl FnOnce(&mut DurationDraft)) {
        if !self.session_ready() {
            return;
        }
        let mut state = self.state.write();
        if state.busy || state.pending.is_some() || state.authoritative.is_none() {
            return;
        }
        update(&mut state.draft);
        state.validation_error = false;
        state.confirmed = false;
    }

    /// 未知命令不允许刷新后抛弃；明确成功后重读，避免把幂等历史结果当作最新账号偏好。
    pub(super) fn load(&mut self, read: PreferenceRead) {
        let confirmed = read == PreferenceRead::Confirmed;
        let preserve_draft = read == PreferenceRead::Background;
        if !self.identity_matches() {
            return;
        }
        if !confirmed && self.state.peek().reload_blocked() {
            return;
        }
        self.reload.mark_started();
        if preserve_draft {
            self.state.write().busy = true;
        } else {
            let current_expires_at = self.state.peek().current_expires_at.clone();
            self.state.set(PreferenceState {
                busy: true,
                current_expires_at,
                ..Default::default()
            });
        }
        let mut owner = self.clone();
        Runtime::current().spawn(self.form_scope, async move {
            if !owner.identity_matches() {
                return;
            }
            let result = fetch_session_preference(&owner.user_id).await;
            if !owner.identity_matches() {
                return;
            }
            let mut state = owner.state.write();
            state.busy = false;
            match result {
                Ok(preference) => {
                    state.accept_read(preference, preserve_draft);
                    if !preserve_draft || state.pending.is_none() {
                        state.error = None;
                        state.confirmed = confirmed;
                    }
                }
                Err(error) if !preserve_draft || state.pending.is_none() => {
                    state.error = Some(error);
                }
                Err(_error) => {}
            }
        });
    }

    pub(super) fn submit(&mut self) {
        if !self.session_ready() || self.state.peek().busy {
            return;
        }
        let payload = {
            let mut state = self.state.write();
            if let Some(pending) = &state.pending {
                if !state.pending_uncertain {
                    return;
                }
                pending.clone()
            } else {
                let Some(preference) = &state.authoritative else { return };
                let expected_version = preference.version;
                let Ok(duration_seconds) = state.draft.seconds() else {
                    state.validation_error = true;
                    return;
                };
                PublicSetSessionPreferenceRequest {
                    command_id: Uuid::new_v4().to_string(),
                    duration_seconds,
                    expected_version,
                }
            }
        };
        {
            let mut state = self.state.write();
            state.pending = Some(payload.clone());
            state.busy = true;
            state.error = None;
            state.validation_error = false;
            state.confirmed = false;
        }
        let mut owner = self.clone();
        let confirmation = self.authority.current();
        self.commands.spawn(async move {
            // 预期用户与 CAS 由服务端复核；一旦发送，表单卸载也要等收包和 Cookie 对账。
            let result = set_session_preference(&owner.user_id, &payload, || {
                confirmation.is_some()
                    && owner.session_ready()
                    && owner.authority.current() == confirmation
            })
            .await;
            if !owner.identity_matches() {
                return;
            }
            match result {
                Ok(_) => owner.load(PreferenceRead::Confirmed),
                Err(error) => owner.state.write().reject_write(error),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};

    use super::{DurationDraft, PreferenceState};
    use crate::api::CommandError;

    fn preference(duration_seconds: Option<i64>, version: i64) -> PublicSessionPreferenceResponse {
        PublicSessionPreferenceResponse {
            user_id: "018f4c8e-7b2a-4c3d-8e4f-000000000001".to_owned(),
            duration_seconds,
            version,
            expires_at: duration_seconds.map(|_| "2090-01-01T12:00:00Z".to_owned()),
        }
    }

    #[test]
    fn relogin_read_updates_a_clean_duration_but_preserves_a_dirty_draft_and_cas_version() {
        for (draft, expected_duration, expected_version) in
            [(Some(900), None, 12), (Some(3_600), Some(3_600), 11)]
        {
            let mut state = PreferenceState {
                authoritative: Some(preference(Some(900), 11)),
                draft: DurationDraft::from_seconds(draft),
                ..Default::default()
            };
            state.accept_read(preference(None, 12), true);
            assert_eq!(state.draft.seconds(), Ok(expected_duration));
            assert_eq!(state.authoritative.as_ref().unwrap().version, expected_version);
            assert_eq!(state.current_expires_at, Some(None));
        }
    }

    #[test]
    fn unsent_attempt_keeps_only_the_previously_unknown_command() {
        let frozen = PublicSetSessionPreferenceRequest {
            command_id: "018f4c8e-7b2a-4c3d-8e4f-000000000006".to_owned(),
            duration_seconds: Some(3_600),
            expected_version: 11,
        };
        for pending_uncertain in [false, true] {
            let mut state = PreferenceState {
                pending: Some(frozen.clone()),
                pending_uncertain,
                busy: true,
                draft: DurationDraft::from_seconds(frozen.duration_seconds),
                ..Default::default()
            };
            state.reject_write(CommandError::before_send());
            assert!(!state.busy);
            assert!(state.error.as_ref().unwrap().not_sent());
            assert_eq!(state.pending_uncertain, pending_uncertain);
            assert_eq!(state.pending, pending_uncertain.then(|| frozen.clone()));
            assert_eq!(state.draft.seconds(), Ok(frozen.duration_seconds));
        }
    }
}

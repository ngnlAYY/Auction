//! 登录时长设置的账号隔离命令宿主；未知结果冻结命令、时长和 CAS 版本直到明确确认。

use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use dioxus::prelude::*;
use uuid::Uuid;

use super::duration::DurationDraft;
use crate::api::{CommandError, fetch_session_preference, set_session_preference};
use crate::app::surface::PublicSessionResource;

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

#[derive(Clone)]
pub(super) struct PreferenceOwner {
    pub(super) user_id: String,
    pub(super) session: PublicSessionResource,
    pub(super) state: Signal<PreferenceState>,
}

impl PreferenceOwner {
    pub(super) fn identity_matches(&self) -> bool {
        self.session.peek().as_ref().is_some_and(|snapshot| {
            snapshot
                .as_ref()
                .ok()
                .and_then(|identity| identity.as_ref())
                .is_some_and(|identity| identity.authenticated && identity.user_id == self.user_id)
        })
    }

    pub(super) fn change(&mut self, update: impl FnOnce(&mut DurationDraft)) {
        if !self.identity_matches() {
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
    pub(super) fn load(&mut self, confirmed: bool) {
        if !self.identity_matches() {
            return;
        }
        if !confirmed {
            let state = self.state.peek();
            if state.busy || state.pending_uncertain {
                return;
            }
        }
        let current_expires_at = self.state.peek().current_expires_at.clone();
        self.state.set(PreferenceState { busy: true, current_expires_at, ..Default::default() });
        let mut owner = self.clone();
        spawn(async move {
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
                    state.draft = DurationDraft::from_seconds(preference.duration_seconds);
                    state.current_expires_at = Some(preference.expires_at.clone());
                    state.authoritative = Some(preference);
                    state.confirmed = confirmed;
                }
                Err(error) => state.error = Some(error),
            }
        });
    }

    pub(super) fn submit(&mut self) {
        if !self.identity_matches() || self.state.peek().busy {
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
        spawn(async move {
            if !owner.identity_matches() {
                return;
            }
            let result = set_session_preference(&owner.user_id, &payload).await;
            if !owner.identity_matches() {
                return;
            }
            match result {
                Ok(_) => owner.load(true),
                Err(error) => {
                    let mut state = owner.state.write();
                    state.busy = false;
                    // 后续重试被前置拒绝不能证明首次未提交；只有明确冲突或成功解除累计未知。
                    state.pending_uncertain = error.status != 409
                        && (state.pending_uncertain || error.outcome_uncertain());
                    if !state.pending_uncertain
                        && matches!(
                            error.code.as_str(),
                            "session_lock_unavailable" | "session_lock_timeout"
                        )
                    {
                        // 此次尚未发送且此前也没有未知提交，保留草稿并允许重新获取锁。
                        state.pending = None;
                    }
                    state.error = Some(error);
                }
            }
        });
    }
}

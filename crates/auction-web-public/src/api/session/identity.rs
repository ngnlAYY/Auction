//! 会话身份证据和会话复核的通知边界。
//! 有效的当前身份投影替换旧归属，`409 session_changed` 只否定请求携带的预期用户。

use dioxus::core::Runtime;
use dioxus::prelude::*;

use super::PublicSession;

/// 成功响应提供的完整会话或当前用户标识。
pub(crate) enum SessionIdentity {
    Snapshot(Option<PublicSession>),
    UserId(String),
}

/// Cookie 当前归属的证据；请求失败与普通 401 不属于此类型。
pub(crate) enum SessionIdentityEvidence {
    Current { identity: SessionIdentity, cookie_written: bool },
    ExpectedUserRejected(String),
}

/// 发给外壳的身份证据入口及本页身份修订。
#[derive(Clone, Copy)]
pub(crate) struct PublicSessionEvidenceTrigger {
    pub apply: EventHandler<SessionIdentityEvidence>,
    pub revision: Signal<u64>,
}

/// 不修改 Cookie 的请求绑定发出时的修订，避免迟到的响应覆盖更新的 Cookie 归属。
pub(in crate::api) struct SessionIdentityObservation {
    source: Option<(PublicSessionEvidenceTrigger, u64)>,
    session_revision: Option<String>,
    confirmed_logout: Option<String>,
}

impl SessionIdentityObservation {
    pub(in crate::api) fn start() -> Self {
        let source = current_context::<PublicSessionEvidenceTrigger>().map(|trigger| {
            let revision = *trigger.revision.peek();
            (trigger, revision)
        });
        Self {
            source,
            session_revision: get_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY),
            confirmed_logout: get_revision(LOGOUT_REVISION_KEY),
        }
    }

    pub(in crate::api) fn publish(self, identity: SessionIdentity) -> bool {
        if let Some((trigger, revision)) = self.source {
            if *trigger.revision.peek() != revision
                || self.session_revision != get_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY)
                || self.confirmed_logout != get_revision(LOGOUT_REVISION_KEY)
            {
                return false;
            }
            trigger
                .apply
                .call(SessionIdentityEvidence::Current { identity, cookie_written: false });
        }
        true
    }
}

/// 已完成的 Cookie 写入由同一个浏览器锁保证顺序，即使重登同一账号也使旧读证据过期。
pub(in crate::api) fn publish_cookie_identity(identity: SessionIdentity) {
    publish_identity(SessionIdentityEvidence::Current { identity, cookie_written: true });
}

pub(super) fn publish_read_identity(identity: SessionIdentity) {
    publish_identity(SessionIdentityEvidence::Current { identity, cookie_written: false });
}

fn publish_identity(evidence: SessionIdentityEvidence) {
    if let Some(trigger) = current_context::<PublicSessionEvidenceTrigger>() {
        trigger.apply.call(evidence);
    }
}

/// 一次复核的触发原因，不提供身份归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRefreshReason {
    Initial,
    Refresh,
    AuthenticationFailed,
    CookieChanged,
    LogoutUnknown,
    Expiry,
}

impl SessionRefreshReason {
    /// 认证失败应清除展示快照，最后确认的竞价宿主仍等候权威复核。
    pub(crate) fn invalidates(self) -> bool {
        matches!(self, Self::AuthenticationFailed | Self::Expiry)
    }

    pub(super) fn synchronizes_cookie(self) -> bool {
        matches!(self, Self::Initial | Self::Refresh)
    }
}

/// 外壳会话重读入口。
#[derive(Clone, Copy)]
pub(crate) struct PublicSessionCheckTrigger(pub(crate) EventHandler<SessionRefreshReason>);

impl PublicSessionCheckTrigger {
    /// 已确认退出与结果未知退出的修订值，只含随机标记。
    pub(crate) fn logout_revisions() -> (Option<String>, Option<String>) {
        (get_revision(LOGOUT_REVISION_KEY), get_revision(LOGOUT_UNKNOWN_REVISION_KEY))
    }
}

/// 标签页间只广播随机修订，不保存用户身份或凭据。
pub(crate) const PUBLIC_SESSION_CHANGE_STORAGE_KEY: &str = "hv.public.session-revision";
const LOGOUT_REVISION_KEY: &str = "hv.public.session-logout-revision";
const LOGOUT_UNKNOWN_REVISION_KEY: &str = "hv.public.session-logout-unknown-revision";

pub(super) fn mark_public_logout(confirmed: bool) {
    // 先在写锁内记录。排队中的权威 GET 可能早于跨页唤醒收到 Cookie 的变化。
    set_revision(if confirmed { LOGOUT_REVISION_KEY } else { LOGOUT_UNKNOWN_REVISION_KEY });
}

pub(in crate::api) fn observe_public_session_failure(
    status: u16,
    code: &str,
    expected_user: Option<&str>,
) {
    if observe_expected_user_rejection(Some(status), Some(code), expected_user) {
        broadcast_session_change();
    } else if status == 401 {
        notify_public_session_change(SessionRefreshReason::AuthenticationFailed);
    }
}

/// 会话复核自己的失败也用同一证据，外壳等当前任务结束后再启动复核。
pub(super) fn observe_expected_user_rejection(
    status: Option<u16>,
    code: Option<&str>,
    expected_user: Option<&str>,
) -> bool {
    if (status, code) == (Some(409), Some("session_changed"))
        && let Some(user_id) = expected_user
    {
        publish_identity(SessionIdentityEvidence::ExpectedUserRejected(user_id.to_owned()));
        return true;
    }
    false
}

pub(in crate::api) fn notify_public_session_change(reason: SessionRefreshReason) {
    broadcast_session_change();
    if let Some(trigger) = current_context::<PublicSessionCheckTrigger>() {
        trigger.0.call(reason);
    }
}

fn broadcast_session_change() {
    set_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY);
}

fn get_revision(key: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(fixture) = current_context::<super::transport::SessionHttpFixture>() {
        return (fixture.get_revision)(key);
    }
    auction_web_shared::storage::get_item(key)
}

fn set_revision(key: &str) {
    #[cfg(not(test))]
    write_storage_revision(key);
    #[cfg(test)]
    if let Some(fixture) = current_context::<super::transport::SessionHttpFixture>() {
        (fixture.set_revision)(key);
    } else {
        write_storage_revision(key);
    }
}

fn write_storage_revision(key: &str) {
    #[cfg(target_arch = "wasm32")]
    let _ = auction_web_shared::storage::set_item(key, &uuid::Uuid::new_v4().to_string());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

fn current_context<T: Clone + 'static>() -> Option<T> {
    Runtime::try_current()
        .filter(|runtime| runtime.try_current_scope_id().is_some())
        .and_then(|_| try_consume_context::<T>())
}

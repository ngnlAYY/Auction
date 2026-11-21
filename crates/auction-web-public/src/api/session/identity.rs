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
    ExpectedUserRejected { user_id: String, observed_revision: u64 },
}

/// 发给外壳的身份证据入口及本页身份修订。
#[derive(Clone, Copy)]
pub(crate) struct PublicSessionEvidenceTrigger {
    pub apply: EventHandler<SessionIdentityEvidence>,
    pub revision: Signal<u64>,
    pub owner_revision: Signal<u64>,
}

/// 不修改 Cookie 的请求绑定发出时的修订，避免迟到的响应覆盖更新的 Cookie 归属。
pub(crate) struct SessionIdentityObservation {
    source: Option<(EventHandler<SessionIdentityEvidence>, Signal<u64>, u64)>,
    session_revision: Option<String>,
    confirmed_logout: Option<String>,
    login_revision: Option<String>,
}

impl SessionIdentityObservation {
    pub(crate) fn start() -> Self {
        Self::observe(|trigger| trigger.revision)
    }
    /// 在组件事件作用域中固定外壳回调，供之后的异步写入收口使用。
    pub(crate) fn from_trigger(trigger: PublicSessionEvidenceTrigger) -> Self {
        let revision = trigger.revision;
        Self {
            source: Some((trigger.apply, revision, *revision.peek())),
            session_revision: get_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY),
            confirmed_logout: get_revision(LOGOUT_REVISION_KEY),
            login_revision: PublicSessionCheckTrigger::login_revision(),
        }
    }

    /// 同一身份的例行会话读取不淘汰同代在途标记集合。
    pub(in crate::api) fn watch_collection() -> Self {
        Self::observe(|trigger| trigger.owner_revision)
    }

    pub(in crate::api) fn user_owner() -> Self {
        Self::observe(|trigger| trigger.revision)
    }

    pub(in crate::api) fn reject_expected_user(&self, user_id: &str) -> bool {
        let Some((apply, _, observed_revision)) = self.source else {
            return false;
        };
        if !self.current() {
            return false;
        }
        apply.call(SessionIdentityEvidence::ExpectedUserRejected {
            user_id: user_id.to_owned(),
            observed_revision,
        });
        true
    }

    fn observe(select_revision: impl FnOnce(PublicSessionEvidenceTrigger) -> Signal<u64>) -> Self {
        let source = current_context::<PublicSessionEvidenceTrigger>().map(|trigger| {
            let revision = select_revision(trigger);
            (trigger.apply, revision, *revision.peek())
        });
        Self {
            source,
            session_revision: get_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY),
            confirmed_logout: get_revision(LOGOUT_REVISION_KEY),
            login_revision: PublicSessionCheckTrigger::login_revision(),
        }
    }

    /// 状态交付与缓存写入采用同一份请求发起时的身份观察。
    pub(crate) fn current(&self) -> bool {
        self.source.is_none_or(|(_, revision, observed)| *revision.peek() == observed)
            && self.cookie_current()
    }

    /// 例行会话复核可以推进本页证据，但偏好读回仍须绑定发起时的 Cookie 代次。
    pub(in crate::api) fn cookie_current(&self) -> bool {
        self.session_revision == PublicSessionCheckTrigger::session_revision()
            && self.confirmed_logout == get_revision(LOGOUT_REVISION_KEY)
            && self.login_revision == PublicSessionCheckTrigger::login_revision()
    }

    /// 与 Solid 共用主修订的十进制表示。
    pub(crate) fn cache_revision(&self) -> Option<String> {
        watch_cache_revision(self.session_revision.as_deref())
    }

    /// 主修订绑定发起的 HTTP 与其可用于 CAS 的集合。
    pub(crate) fn primary_revision(&self) -> Option<&str> {
        self.session_revision.as_deref()
    }

    pub(in crate::api) fn publish(&self, identity: SessionIdentity) -> bool {
        if !self.current() {
            return false;
        }
        if let Some((apply, _, _)) = self.source {
            apply.call(SessionIdentityEvidence::Current { identity, cookie_written: false });
        }
        true
    }
    /// 写入锁内的成功响应直接确认 Cookie 身份；复用请求开始时捕获的外壳回调，避免 await 后依赖当前作用域。
    pub(in crate::api) fn publish_cookie_write(&self, identity: SessionIdentity) {
        if let Some((apply, _, _)) = self.source {
            apply.call(SessionIdentityEvidence::Current { identity, cookie_written: true });
        }
    }
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

    /// 主修订只广播 Cookie 操作，不提供身份归属。
    pub(crate) fn session_revision() -> Option<String> {
        get_revision(PUBLIC_SESSION_CHANGE_STORAGE_KEY)
    }

    /// 成功解码的登录标记仍须由权威会话确认；存储失败时仅通知本标签页。
    pub(crate) fn login_revision() -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        if let Some(revision) = LOCAL_LOGIN_REVISION.with_borrow(Clone::clone) {
            return Some(revision);
        }
        get_revision(LOGIN_REVISION_KEY)
    }

    /// 持久标记快照只接受当前主修订，缺省修订编码为十进制 0。
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn watch_cache_revision() -> Option<String> {
        watch_cache_revision(Self::session_revision().as_deref())
    }
}

/// 标签页间只广播随机修订，不保存用户身份或凭据。
pub(crate) const PUBLIC_SESSION_CHANGE_STORAGE_KEY: &str = "hv.public.session-revision";
const LOGOUT_REVISION_KEY: &str = "hv.public.session-logout-revision";
const LOGOUT_UNKNOWN_REVISION_KEY: &str = "hv.public.session-logout-unknown-revision";
const LOGIN_REVISION_KEY: &str = "hv.public.session-login-revision";

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LOCAL_LOGIN_REVISION: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn mark_public_login() {
    #[cfg(target_arch = "wasm32")]
    {
        let revision = uuid::Uuid::new_v4().to_string();
        let shared = auction_web_shared::storage::set_item(LOGIN_REVISION_KEY, &revision);
        LOCAL_LOGIN_REVISION.with_borrow_mut(|local| *local = (!shared).then_some(revision));
    }
    #[cfg(not(target_arch = "wasm32"))]
    set_revision(LOGIN_REVISION_KEY);
}

fn watch_cache_revision(revision: Option<&str>) -> Option<String> {
    let Some(revision) = revision else { return Some("0".to_owned()) };
    let uuid = uuid::Uuid::parse_str(revision).ok()?;
    (uuid.hyphenated().to_string() == revision).then(|| uuid.as_u128().to_string())
}

pub(super) fn mark_public_logout(confirmed: bool) {
    // 先在写锁内记录。排队中的权威 GET 可能早于跨页唤醒收到 Cookie 的变化。
    set_revision(if confirmed { LOGOUT_REVISION_KEY } else { LOGOUT_UNKNOWN_REVISION_KEY });
}

pub(in crate::api) fn observe_public_session_failure(
    status: u16,
    code: &str,
    expected_user: Option<&str>,
) {
    observe_public_session_failure_inner(status, code, expected_user, None);
}

pub(in crate::api) fn observe_public_session_failure_observed(
    status: u16,
    code: &str,
    expected_user: Option<&str>,
    observation: &SessionIdentityObservation,
) {
    observe_public_session_failure_inner(status, code, expected_user, Some(observation));
}

fn observe_public_session_failure_inner(
    status: u16,
    code: &str,
    expected_user: Option<&str>,
    observation: Option<&SessionIdentityObservation>,
) {
    if observe_expected_user_rejection(Some(status), Some(code), expected_user, observation) {
        broadcast_session_change();
    } else if status == 409
        && code == "session_changed"
        && expected_user.is_some()
        && observation.is_some_and(|observation| !observation.current())
    {
        // 迟到的旧用户冲突不能覆盖新宿主，但当前 Cookie 仍须重新核验并通知其他标签页。
        broadcast_session_change();
        request_public_session_check(SessionRefreshReason::CookieChanged);
    } else if status == 401 {
        request_public_session_check(SessionRefreshReason::AuthenticationFailed);
    }
}

/// 会话复核自己的失败也用同一证据，外壳等当前任务结束后再启动复核。
pub(super) fn observe_expected_user_rejection(
    status: Option<u16>,
    code: Option<&str>,
    expected_user: Option<&str>,
    observation: Option<&SessionIdentityObservation>,
) -> bool {
    if (status, code) == (Some(409), Some("session_changed"))
        && let (Some(user_id), Some(observation)) = (expected_user, observation)
    {
        return observation.reject_expected_user(user_id);
    }
    false
}

/// 未发送的命令只复核本页；Cookie 未改变，无需广播新的主修订。
pub(in crate::api) fn request_public_session_check(reason: SessionRefreshReason) {
    if let Some(trigger) = current_context::<PublicSessionCheckTrigger>() {
        trigger.0.call(reason);
    }
}

pub(in crate::api) fn broadcast_session_change() {
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
    cfg_select! {
        test => if let Some(fixture) = current_context::<super::transport::SessionHttpFixture>() {
            (fixture.set_revision)(key);
        } else {
            write_storage_revision(key);
        },
        _ => { write_storage_revision(key); }
    }
}

fn write_storage_revision(key: &str) {
    cfg_select! {
        target_arch = "wasm32" => {
            let _ = auction_web_shared::storage::set_item(key, &uuid::Uuid::new_v4().to_string());
        }
        _ => { let _ = key; }
    }
}

fn current_context<T: Clone + 'static>() -> Option<T> {
    Runtime::try_current()
        .filter(|runtime| runtime.try_current_scope_id().is_some())
        .and_then(|_| try_consume_context::<T>())
}

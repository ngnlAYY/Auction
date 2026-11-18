//! 命令宿主的已确认身份和新写入门禁。
//! 旧身份的记录在“确认当前 Cookie 不再属于它”的那一刻销毁，不等待任何后续请求。
//! 成功解码的权威身份立即建立新归属。`409 session_changed` 只销毁请求预期用户的宿主，再复核当前 Cookie。

use auction_time::Timestamp;
use dioxus::core::current_scope_id;
use dioxus::prelude::*;

use super::host::CompetitionCommands;
use crate::api::{
    PublicSession, PublicSessionApiError, PublicSessionCheckTrigger, PublicSessionEvidenceTrigger,
    SessionIdentity, SessionIdentityEvidence, SessionRefreshReason, fetch_public_session,
};
use crate::surface::session::SessionCommandScope;
use crate::surface::watches::{WatchSessionRevision, invalidate_lot_watches};
use crate::surface::{PublicCommandOwner, PublicSessionResource};

/// 外壳的统一会话刷新入口及两个命令宿主的身份投影。
pub(in crate::surface) struct PublicSessionChecks {
    pub session: PublicSessionResource,
    pub page_owner: Signal<Option<String>>,
    pub competition_owner: Signal<Option<String>>,
    pub ended_elsewhere: Signal<bool>,
    pub check: EventHandler<SessionRefreshReason>,
}

#[derive(Clone, Copy)]
struct ConfirmedIdentity {
    commands: CompetitionCommands,
    owner: Signal<Option<String>>,
    last_session: Signal<Option<PublicSession>>,
    ended_elsewhere: Signal<bool>,
    revision: Signal<u64>,
    watches: WatchSessionRevision,
    confirmed_logout: Signal<Option<String>>,
    unknown_logout: Signal<Option<String>>,
    local_logout_unknown: Signal<bool>,
}

impl ConfirmedIdentity {
    fn set_owner(&mut self, next: Option<String>) -> bool {
        let previous = self.owner.peek().clone();
        if previous == next {
            return false;
        }
        if let Some(previous) = previous {
            invalidate_lot_watches(&previous);
        }
        // 旧账号的在途命令与标记快照在证据交付时失效，后续失败不能恢复旧归属。
        self.commands.discard_identity();
        advance_revision(self.revision);
        advance_revision(self.watches.boundary);
        self.last_session.set(None);
        self.owner.set(next);
        true
    }

    fn observe_confirmed_logout(&mut self, revision: Option<String>) -> bool {
        if revision.is_none() || *self.confirmed_logout.peek() == revision {
            return false;
        }
        self.confirmed_logout.set(revision);
        self.set_owner(None);
        self.ended_elsewhere.set(false);
        true
    }

    fn finish_reconciliation(&mut self, unknown_revision: Option<String>) {
        // 结果未知的退出只抑制这一轮提示，包括读回失败。
        if unknown_revision.is_some() {
            self.unknown_logout.set(unknown_revision);
        }
        self.local_logout_unknown.set(false);
    }

    fn apply(
        &mut self,
        evidence: SessionIdentityEvidence,
        mut session: PublicSessionResource,
        read_reason: SessionRefreshReason,
        mut recheck_after_evidence: Signal<bool>,
    ) {
        match evidence {
            SessionIdentityEvidence::Current { identity, cookie_written } => {
                let previous = self.last_session.peek().clone();
                let (confirmed, unknown) = PublicSessionCheckTrigger::logout_revisions();
                let confirmed_logout = self.observe_confirmed_logout(confirmed);
                let next = match &identity {
                    SessionIdentity::Snapshot(session) => {
                        session.as_ref().map(|identity| identity.user_id.clone())
                    }
                    SessionIdentity::UserId(user_id) => Some(user_id.clone()),
                };
                let changed = self.set_owner(next.clone());
                if !changed && (cookie_written || matches!(identity, SessionIdentity::Snapshot(_)))
                {
                    advance_revision(self.revision);
                }
                if next.is_some() || cookie_written {
                    self.ended_elsewhere.set(false);
                }
                match identity {
                    SessionIdentity::Snapshot(snapshot) => {
                        if let Some(current) = &snapshot {
                            if !session_is_unexpired(current) {
                                // 到期 GET 仍返回过期快照时保留已消费的截止，避免重新排定计时。
                                if read_reason != SessionRefreshReason::Expiry {
                                    self.last_session.set(Some(current.clone()));
                                }
                                session.clear();
                                return;
                            }
                            self.last_session.set(Some(current.clone()));
                        } else {
                            let browser_logout = cookie_written
                                || confirmed_logout
                                || *self.local_logout_unknown.peek()
                                || (unknown.is_some() && *self.unknown_logout.peek() != unknown);
                            if !browser_logout
                                && read_reason != SessionRefreshReason::Expiry
                                && previous.as_ref().is_some_and(session_is_unexpired)
                            {
                                self.ended_elsewhere.set(true);
                            }
                        }
                        let confirmed = Ok(snapshot);
                        if session.peek().as_ref() != Some(&confirmed) {
                            session.set(Some(confirmed));
                        }
                    }
                    SessionIdentity::UserId(_) if changed || confirmed_logout => {
                        // 用户 ID 已能确定归属；新写入仍需完整会话快照。
                        session.clear();
                        if !cookie_written {
                            recheck_after_evidence.set(true);
                        }
                    }
                    SessionIdentity::UserId(_) => {}
                }
            }
            SessionIdentityEvidence::ExpectedUserRejected(expected_user) => {
                // 冲突只否定这次请求的预期用户；保留其他已确认归属并暂停重试。
                if self.owner.peek().as_deref() == Some(expected_user.as_str()) {
                    self.set_owner(None);
                    self.ended_elsewhere.set(false);
                }
                session.clear();
                recheck_after_evidence.set(true);
            }
        }
    }
}

pub(in crate::surface) fn use_public_session_checks(
    commands: CompetitionCommands,
) -> PublicSessionChecks {
    let mut identity = ConfirmedIdentity {
        commands,
        owner: use_signal(|| None),
        last_session: use_signal(|| None),
        ended_elsewhere: use_signal(|| false),
        revision: use_signal(|| 0),
        watches: WatchSessionRevision { boundary: use_signal(|| 0), refresh: use_signal(|| 0) },
        confirmed_logout: use_signal(|| PublicSessionCheckTrigger::logout_revisions().0),
        unknown_logout: use_signal(|| PublicSessionCheckTrigger::logout_revisions().1),
        local_logout_unknown: use_signal(|| false),
    };
    let mut read_reason = use_signal(|| SessionRefreshReason::Initial);
    let mut recheck_after_evidence = use_signal(|| false);
    let mut expiry_waiting = use_signal(|| false);
    let mut session = use_resource(move || {
        let reason = *read_reason.peek();
        async move {
            let previous_user = identity.owner.peek().clone();
            let (result, unknown_revision) = fetch_public_session(reason, |confirmed| {
                let mut identity = identity;
                if identity.observe_confirmed_logout(confirmed) {
                    let mut session = consume_context::<PublicSessionResource>();
                    session.set(Some(Ok(None)));
                }
            })
            .await;
            identity.finish_reconciliation(unknown_revision);
            read_reason.set(SessionRefreshReason::Refresh);
            let current = result.map_err(PublicSessionApiError::into_read_code)?;
            if let Some(current) = &current {
                if !session_is_unexpired(current) {
                    return Err("invalid_response".to_owned());
                }
                if matches!(
                    reason,
                    SessionRefreshReason::CookieChanged | SessionRefreshReason::LogoutUnknown
                ) && previous_user.as_deref() == Some(current.user_id.as_str())
                {
                    // 同账号保留可展示快照，重读标记用独立修订，不影响公开缓存。
                    advance_revision(identity.watches.refresh);
                }
            }
            Ok(current)
        }
    });
    let check = use_callback(move |reason: SessionRefreshReason| {
        if reason.invalidates() {
            // 普通 401 与到期计时只暂停重试；是否改变归属由 GET 确认。
            session.clear();
        }
        if reason == SessionRefreshReason::Expiry {
            expiry_waiting.set(session.pending());
            if session.pending() {
                return;
            }
        }
        if reason == SessionRefreshReason::Refresh
            && session.pending()
            && *read_reason.peek() != SessionRefreshReason::Refresh
        {
            return;
        }
        if reason == SessionRefreshReason::LogoutUnknown {
            identity.local_logout_unknown.set(true);
        }
        if identity.observe_confirmed_logout(PublicSessionCheckTrigger::logout_revisions().0) {
            session.set(Some(Ok(None)));
        }
        read_reason.set(reason);
        session.restart();
    });
    let apply = use_callback(move |evidence| {
        identity.apply(evidence, session, *read_reason.peek(), recheck_after_evidence);
    });
    use_effect(move || {
        if recheck_after_evidence() {
            recheck_after_evidence.set(false);
            // 同步请求也能报告冲突，复核不能在资源自己的任务里重启资源。
            check.call(SessionRefreshReason::CookieChanged);
        } else if expiry_waiting() && *session.state().read() != UseResourceState::Pending {
            // 截止可能在读取已经取到响应头后才到，当前读回不能吞掉到期复核。
            check.call(SessionRefreshReason::Expiry);
        }
    });
    use_context_provider(|| PublicSessionCheckTrigger(check));
    use_context_provider(|| PublicSessionEvidenceTrigger { apply, revision: identity.revision });
    use_context_provider(|| session);
    use_context_provider(|| SessionCommandScope(current_scope_id()));
    use_context_provider(|| identity.watches);
    use_public_session_expiry(identity.last_session, check);
    PublicSessionChecks {
        session,
        page_owner: identity.owner,
        competition_owner: identity.owner,
        ended_elsewhere: identity.ended_elsewhere,
        check,
    }
}

fn advance_revision(mut revision: Signal<u64>) {
    let next = revision.peek().wrapping_add(1);
    revision.set(next);
}

/// 自然到期与提前撤销按最近一次确认的服务端截止区分。
fn session_is_unexpired(identity: &PublicSession) -> bool {
    let Some(expires_at) = identity.expires_at.as_deref() else {
        return true;
    };
    Timestamp::parse_from_rfc3339(expires_at).is_ok_and(|expiry| expiry > session_now())
}

/// 有限会话只在真实截止后复核一次；永久会话不建立计时器，也不把远期日期当永久。
fn use_public_session_expiry(
    confirmed: Signal<Option<PublicSession>>,
    check_session: EventHandler<SessionRefreshReason>,
) {
    #[cfg(any(target_arch = "wasm32", test))]
    {
        // 同一身份及截止的重复快照不重启已完成的任务，避免服务端时钟误差造成忙轮询。
        let deadline = use_memo(move || {
            confirmed.read().as_ref().and_then(|identity| {
                Timestamp::parse_from_rfc3339(identity.expires_at.as_deref()?)
                    .ok()
                    .map(|expiry| (identity.user_id.clone(), expiry))
            })
        });
        let _expiry_check = use_resource(move || {
            let deadline = deadline();
            async move {
                let Some((_, expiry)) = deadline else { return };
                #[cfg(all(test, not(target_arch = "wasm32")))]
                if try_consume_context::<crate::surface::commands::session_fixture::ExpiryClock>()
                    .is_none()
                {
                    return;
                }
                loop {
                    // Date 头只有秒精度；给出一秒余量，长截止分段等待以免浏览器 i32 超时溢出。
                    let remaining =
                        (expiry - session_now()).num_milliseconds().saturating_add(1_000);
                    if remaining <= 0 {
                        check_session.call(SessionRefreshReason::Expiry);
                        return;
                    }
                    wait_for_expiry(remaining.min(i64::from(i32::MAX)) as u32).await;
                }
            }
        });
    }
    #[cfg(not(any(target_arch = "wasm32", test)))]
    let _ = (confirmed, check_session);
}

fn session_now() -> Timestamp {
    #[cfg(test)]
    if let Some(clock) =
        try_consume_context::<crate::surface::commands::session_fixture::ExpiryClock>()
    {
        return clock.now();
    }
    crate::api::estimated_now()
}

#[cfg(any(target_arch = "wasm32", test))]
async fn wait_for_expiry(millis: u32) {
    #[cfg(test)]
    if let Some(clock) =
        try_consume_context::<crate::surface::commands::session_fixture::ExpiryClock>()
    {
        clock.wait(millis).await;
        return;
    }
    auction_web_shared::timer::TimeoutFuture::new(millis).await;
}

/// 单个命令宿主使用的会话判定。
#[derive(Clone)]
pub(crate) struct CommandSession {
    user_id: Option<String>,
    owner: Signal<Option<String>>,
    session: PublicSessionResource,
    confirmed: Memo<bool>,
}

/// 由按身份加键的宿主提供，表单和标记操作共用同一写入门禁。
pub(crate) fn use_command_session_provider(user_id: Option<String>) {
    let owner = use_context::<PublicCommandOwner>().0;
    let session = use_command_session(user_id, owner);
    use_context_provider(|| session);
}

pub(super) fn use_command_session(
    user_id: Option<String>,
    owner: Signal<Option<String>>,
) -> CommandSession {
    let session = use_context::<PublicSessionResource>();
    let confirmed = use_memo({
        let user_id = user_id.clone();
        move || {
            identity_confirmed(user_id.as_deref(), owner.read().as_deref(), session.read().as_ref())
        }
    });
    CommandSession { user_id, owner, session, confirmed }
}

impl CommandSession {
    /// 复核不会改变已确认身份，使用 memo 避免同账号的更新重启私有读取。
    pub(crate) fn identity_confirmed(&self) -> bool {
        (self.confirmed)()
    }

    /// 渲染和事件处理都检查当前会话；已确认身份可保留草稿，复核中、读取失败或换人时禁止新写入。
    pub(crate) fn can_submit_new(&self) -> bool {
        can_submit_new(
            self.user_id.as_deref(),
            self.owner.read().as_deref(),
            *self.session.state().read(),
            self.session.read().as_ref(),
        )
    }
}

fn identity_confirmed(
    user_id: Option<&str>,
    owner: Option<&str>,
    snapshot: Option<&Result<Option<PublicSession>, String>>,
) -> bool {
    let Some(user_id) = user_id.filter(|user_id| Some(*user_id) == owner) else {
        return false;
    };
    match snapshot {
        Some(Ok(identity)) => identity
            .as_ref()
            .is_some_and(|identity| identity.authenticated && identity.user_id == user_id),
        // 复核尚无新快照或读取失败时，外壳仍保留最后确认的宿主身份。
        _ => true,
    }
}

fn can_submit_new(
    user_id: Option<&str>,
    owner: Option<&str>,
    state: UseResourceState,
    snapshot: Option<&Result<Option<PublicSession>, String>>,
) -> bool {
    state == UseResourceState::Ready
        && snapshot.is_some_and(Result::is_ok)
        && identity_confirmed(user_id, owner, snapshot)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::LazyLock;

    use auction_contracts::{
        PublicLotWatchResponse, PublicLotWatchesResponse, PublicSetSessionPreferenceRequest,
    };
    use auction_time::Timestamp;
    use dioxus::prelude::*;

    use super::{
        PublicSessionChecks, can_submit_new, identity_confirmed, use_public_session_checks,
    };
    use crate::api::{
        PublicSession, PublicSessionApiError, SessionRefreshReason, SessionRequest, TransportError,
        login_public, logout_public, set_session_preference,
    };
    use crate::surface::PublicCommandOwner;
    use crate::surface::commands::session_fixture::{self, ExpiryClock, ResponseGate};
    use crate::surface::commands::{
        use_command_session_provider, use_competition_commands_provider,
    };
    use crate::surface::watches::{
        LotWatchScope, SkipWatchReads, WatchSessionRevision, load_watch_cache, save_watch_cache,
        use_lot_watched, use_watch_command_owner,
    };

    const USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const OTHER_USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000002";
    const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000005";
    const LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000006";
    const REVISION: &str = "hv.public.session-revision";
    const CONFIRMED_LOGOUT: &str = "hv.public.session-logout-revision";
    const UNKNOWN_LOGOUT: &str = "hv.public.session-logout-unknown-revision";
    const EXPIRES: &str = "2090-01-01T12:00:00Z";
    static PREFERENCE: LazyLock<PublicSetSessionPreferenceRequest> =
        LazyLock::new(|| PublicSetSessionPreferenceRequest {
            command_id: LOT.to_owned(),
            duration_seconds: Some(60),
            expected_version: 1,
        });

    thread_local! {
        static CONTROLS: Cell<Option<Controls>> = const { Cell::new(None) };
    }

    #[derive(Clone, Copy)]
    struct Controls {
        session: crate::surface::PublicSessionResource,
        state: ReadSignal<UseResourceState>,
        owner: Signal<Option<String>>,
        ended: Signal<bool>,
        check: EventHandler<SessionRefreshReason>,
        mutate: EventHandler<SessionRequest<'static>>,
        result: Signal<Option<Result<(), String>>>,
        watch_refresh: Signal<u64>,
    }

    #[component]
    fn SessionHarness() -> Element {
        session_fixture::use_session_fixture(USER);
        let commands = use_competition_commands_provider();
        let PublicSessionChecks {
            session,
            competition_owner: owner,
            ended_elsewhere: ended,
            check,
            ..
        } = use_public_session_checks(commands);
        let mut result = use_signal(|| None);
        let mutate = use_callback(move |request| {
            spawn(async move {
                let response = match request {
                    SessionRequest::Login(key) => login_public(key.to_owned())
                        .await
                        .map(|_| ())
                        .map_err(PublicSessionApiError::into_read_code),
                    SessionRequest::Logout(user_id) => logout_public(user_id.to_owned()).await,
                    SessionRequest::SetPreference(user_id, command) => {
                        set_session_preference(user_id, command)
                            .await
                            .map(|_| ())
                            .map_err(|error| error.code)
                    }
                    _ => panic!("测试只提交会话命令"),
                };
                result.set(Some(response));
            });
        });
        let watch_refresh = use_context::<WatchSessionRevision>().refresh;
        let state = use_hook(|| session.state());
        let observe_watches = try_use_context::<Rc<WatchProjection>>().is_some();
        let refresh = use_signal(|| 0_u64);
        use_context_provider(|| refresh);
        use_context_provider(|| PublicCommandOwner(owner));
        use_context_provider(|| SkipWatchReads);
        CONTROLS.set(Some(Controls {
            session,
            state,
            owner,
            ended,
            check,
            mutate,
            result,
            watch_refresh,
        }));
        rsx! {
            if observe_watches {
                for user_id in owner() {
                    MarkedAuction { key: "{user_id}", user_id }
                }
            }
        }
    }

    #[derive(Default)]
    struct WatchProjection(Cell<bool>);

    #[component]
    fn MarkedAuction(user_id: String) -> Element {
        use_command_session_provider(Some(user_id.clone()));
        let commands = use_watch_command_owner(Some(user_id));
        rsx! {
            {commands}
            LotWatchScope { auction_id: AUCTION.to_owned(), MarkedLot {} }
        }
    }

    #[component]
    fn MarkedLot() -> Element {
        let view = use_context::<Rc<WatchProjection>>();
        use_drop({
            let view = view.clone();
            move || view.0.set(false)
        });
        view.0.set(use_lot_watched(LOT));
        rsx! {}
    }

    fn settle(dom: &mut VirtualDom) {
        for _ in 0..8 {
            dom.process_events();
            dom.render_immediate_to_vec();
        }
    }

    fn mount() -> (VirtualDom, Controls) {
        session_fixture::reset();
        mount_harness(VirtualDom::new(SessionHarness))
    }

    fn mount_harness(mut dom: VirtualDom) -> (VirtualDom, Controls) {
        dom.rebuild_in_place();
        settle(&mut dom);
        let controls = CONTROLS.get().unwrap();
        assert_eq!(controls.owner.peek().as_deref(), Some(USER));
        session_fixture::take_requests();
        (dom, controls)
    }

    impl Controls {
        fn current_user(self) -> Option<String> {
            self.session.peek().as_ref().and_then(|result| {
                result.as_ref().ok()?.as_ref().map(|identity| identity.user_id.clone())
            })
        }

        fn can_retry(self) -> bool {
            can_submit_new(
                Some(USER),
                self.owner.peek().as_deref(),
                *self.state.peek(),
                self.session.peek().as_ref(),
            )
        }
    }

    #[test]
    fn signed_in_boot_reuses_the_first_locked_get_for_cookie_sync() {
        session_fixture::reset();
        session_fixture::script([
            session_fixture::read(Some(USER)).under_lock(),
            session_fixture::reply(
                SessionRequest::Sync(USER),
                session_fixture::session_response_with_deadline(Some(USER), None),
            )
            .under_lock(),
        ]);
        let mut dom = VirtualDom::new(SessionHarness);
        dom.rebuild_in_place();
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["GET", "PUT"]);
        assert!(CONTROLS.get().unwrap().can_retry());
    }

    #[test]
    fn login_body_failure_reads_the_current_identity_once() {
        let (mut dom, controls) = mount();
        let request = SessionRequest::Login("fixture");
        session_fixture::script([
            session_fixture::reply(request, Err(TransportError::Network(Some(200)))).under_lock(),
            session_fixture::read(Some(OTHER_USER)).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["POST", "GET"]);
        assert_eq!(session_fixture::take_revision_writes(), [(REVISION, false)]);
        assert_eq!(controls.owner.peek().as_deref(), Some(OTHER_USER));
        assert_eq!(controls.current_user().as_deref(), Some(OTHER_USER));
        assert_eq!(*controls.result.peek(), Some(Err("network_error".to_owned())));
    }

    #[test]
    fn malformed_logout_conflict_pauses_writes_until_rechecked() {
        let (mut dom, controls) = mount();
        let read = Rc::new(ResponseGate::default());
        let request = SessionRequest::Logout(USER);
        session_fixture::script([
            session_fixture::reply(
                request,
                Ok(session_fixture::response(409, "损坏正文".to_owned())),
            ),
            read.hold_read(Some(USER)).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert!(read.waiting(), "损坏的 409 仍须复核身份");
        assert!(controls.session.pending());
        assert!(!controls.can_retry());
        assert_eq!(controls.owner.peek().as_deref(), Some(USER));
        read.release();
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["DELETE", "GET"]);
        assert!(controls.can_retry());
        assert_eq!(*controls.result.peek(), Some(Err("invalid_response".to_owned())));
        assert_eq!(controls.owner.peek().as_deref(), Some(USER));
    }

    #[test]
    fn logout_body_failure_reads_anonymous_without_announcing_an_external_end() {
        let (mut dom, controls) = mount();
        let request = SessionRequest::Logout(USER);
        session_fixture::script([
            session_fixture::reply(request, Err(TransportError::Network(Some(200)))).under_lock(),
            session_fixture::read(None).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["DELETE", "GET"]);
        assert_eq!(
            session_fixture::take_revision_writes(),
            [(UNKNOWN_LOGOUT, true), (REVISION, false),]
        );
        assert_eq!(*controls.result.peek(), Some(Err("network_error".to_owned())));
        assert_eq!(*controls.session.peek(), Some(Ok(None)));
        assert!(controls.owner.peek().is_none());
        assert!(!*controls.ended.peek());
    }

    #[test]
    fn another_tabs_focus_waits_for_the_logout_revision_before_reading_anonymous() {
        for complete_receipt in [false, true] {
            let (mut quitting_tab, quitting) = mount();
            let (mut focused_tab, focused) = mount_harness(VirtualDom::new(SessionHarness));
            session_fixture::serialize_cookie_lock();
            let body = Rc::new(ResponseGate::default());
            let request = SessionRequest::Logout(USER);
            let reply = if complete_receipt {
                Ok(session_fixture::response(
                    200,
                    r#"{"code":"ok","message":"已退出","data":null}"#.to_owned(),
                ))
            } else {
                Err(TransportError::Network(Some(200)))
            };
            session_fixture::script([
                body.hold(request, reply).under_lock(),
                session_fixture::read(None).under_lock(),
                session_fixture::read(None).under_lock(),
            ]);
            quitting_tab.in_runtime(|| quitting.mutate.call(request));
            settle(&mut quitting_tab);
            assert!(body.waiting());

            focused_tab.in_runtime(|| focused.check.call(SessionRefreshReason::Refresh));
            settle(&mut focused_tab);
            assert!(!*focused.ended.peek(), "退出正文未收完，聚焦复核不能误报");
            assert_eq!(session_fixture::take_requests(), ["DELETE"]);
            assert!(focused.session.pending());
            assert_eq!(focused.owner.peek().as_deref(), Some(USER));

            body.release();
            settle(&mut quitting_tab);
            settle(&mut focused_tab);
            session_fixture::assert_consumed();
            assert_eq!(session_fixture::take_requests(), ["GET", "GET"]);
            assert_eq!(*focused.session.peek(), Some(Ok(None)));
            assert!(!*focused.ended.peek());
        }
    }

    #[test]
    fn other_routes_conflict_codes_do_not_settle_logout_or_session_preference() {
        let cases = [
            (SessionRequest::Logout(USER), "session_preference_conflict", "DELETE"),
            (
                SessionRequest::SetPreference(USER, &PREFERENCE),
                "watch_preference_conflict",
                "PUT preference",
            ),
        ];
        for (request, code, method) in cases {
            let (mut dom, controls) = mount();
            session_fixture::script([
                session_fixture::reply(request, session_fixture::rejection(409, code)),
                session_fixture::read(Some(USER)).under_lock(),
            ]);
            dom.in_runtime(|| controls.mutate.call(request));
            settle(&mut dom);
            assert_eq!(*controls.result.peek(), Some(Err("invalid_response".to_owned())));
            session_fixture::assert_consumed();
            assert_eq!(session_fixture::take_requests(), [method, "GET"]);
            assert_eq!(controls.owner.peek().as_deref(), Some(USER));
        }
    }

    #[test]
    fn recognized_session_preference_conflict_does_not_reconcile_the_cookie() {
        let (mut dom, controls) = mount();
        let request = SessionRequest::SetPreference(USER, &PREFERENCE);
        session_fixture::script([session_fixture::reply(
            request,
            session_fixture::rejection(409, "session_preference_conflict"),
        )]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert_eq!(*controls.result.peek(), Some(Err("session_preference_conflict".to_owned())));
        assert_eq!(session_fixture::take_requests(), ["PUT preference"]);
        assert!(session_fixture::take_revision_writes().is_empty());
    }

    #[test]
    fn preference_body_failure_pauses_retries_and_reads_current_identity_once() {
        for failure in [TransportError::Network(Some(200)), TransportError::Timeout] {
            let (mut dom, controls) = mount();
            let read = Rc::new(ResponseGate::default());
            let request = SessionRequest::SetPreference(USER, &PREFERENCE);
            session_fixture::script([
                session_fixture::reply(request, Err(failure)).under_lock(),
                read.hold(
                    SessionRequest::Read,
                    session_fixture::session_response_with_deadline(Some(USER), Some(EXPIRES)),
                )
                .under_lock(),
            ]);
            assert!(controls.can_retry());
            dom.in_runtime(|| controls.mutate.call(request));
            settle(&mut dom);
            assert!(read.waiting());
            assert!(controls.session.pending());
            assert!(!controls.can_retry());
            assert_eq!(controls.owner.peek().as_deref(), Some(USER));
            assert_eq!(*controls.result.peek(), Some(Err(failure.code().to_owned())));

            // 聚焦不会把在途对账换成普通的持久期同步。
            dom.in_runtime(|| controls.check.call(SessionRefreshReason::Refresh));
            settle(&mut dom);
            assert!(controls.session.pending());
            read.release();
            settle(&mut dom);
            assert_eq!(session_fixture::take_requests(), ["PUT preference", "GET"]);
            assert_eq!(session_fixture::take_revision_writes(), [(REVISION, false)]);
            assert_eq!(controls.current_user().as_deref(), Some(USER));
            assert!(controls.can_retry());
            let snapshot = controls.session.peek();
            let identity = snapshot.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap();
            assert_eq!(identity.expires_at.as_deref(), Some(EXPIRES));
        }
    }

    #[test]
    fn preference_full_success_status_with_invalid_body_broadcasts_while_locked() {
        let (mut dom, controls) = mount();
        let request = SessionRequest::SetPreference(USER, &PREFERENCE);
        session_fixture::script([
            session_fixture::reply(
                request,
                Ok(session_fixture::response(200, "损坏正文".to_owned())),
            )
            .under_lock(),
            session_fixture::read(Some(USER)).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["PUT preference", "GET"]);
        assert_eq!(session_fixture::take_revision_writes(), [(REVISION, true)]);
        assert_eq!(*controls.result.peek(), Some(Err("invalid_response".to_owned())));
        assert!(controls.can_retry());
    }

    #[test]
    fn login_rejection_leaves_the_cookie_and_does_not_read_back() {
        let (mut dom, controls) = mount();
        let request = SessionRequest::Login("fixture");
        session_fixture::script([session_fixture::reply(
            request,
            session_fixture::rejection(401, "invalid_credentials"),
        )]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["POST"]);
        assert!(session_fixture::take_revision_writes().is_empty());
        assert_eq!(*controls.result.peek(), Some(Err("invalid_credentials".to_owned())));
        assert_eq!(controls.current_user().as_deref(), Some(USER));
    }

    #[test]
    fn routine_anonymous_read_announces_the_end_of_a_confirmed_session_once() {
        let (mut dom, mut controls) = mount();
        session_fixture::script([session_fixture::read(None)]);
        dom.in_runtime(|| controls.check.call(SessionRefreshReason::Refresh));
        settle(&mut dom);
        assert!(*controls.ended.peek());
        dom.in_runtime(|| controls.ended.set(false));
        session_fixture::script([session_fixture::read(None)]);
        dom.in_runtime(|| controls.check.call(SessionRefreshReason::Refresh));
        settle(&mut dom);
        assert!(!*controls.ended.peek());
    }

    #[test]
    fn cross_tab_logout_revision_is_consumed_even_if_the_read_fails() {
        for key in [CONFIRMED_LOGOUT, UNKNOWN_LOGOUT] {
            let (mut dom, controls) = mount();
            session_fixture::mark_external_logout(key);
            session_fixture::script([session_fixture::reply(
                SessionRequest::Read,
                Err(TransportError::Network(None)),
            )
            .under_lock()]);
            dom.in_runtime(|| controls.check.call(SessionRefreshReason::CookieChanged));
            settle(&mut dom);
            assert_eq!(session_fixture::take_requests(), ["GET"]);
            assert_eq!(controls.owner.peek().as_deref(), (key == UNKNOWN_LOGOUT).then_some(USER));
            assert!(!*controls.ended.peek());
            assert_eq!(*controls.session.peek(), Some(Err("network_error".to_owned())));

            session_fixture::script([session_fixture::read(None)]);
            dom.in_runtime(|| controls.check.call(SessionRefreshReason::Refresh));
            settle(&mut dom);
            assert_eq!(*controls.ended.peek(), key == UNKNOWN_LOGOUT);
        }
    }

    #[test]
    fn cross_tab_unknown_logout_suppresses_the_notice_for_its_own_read_only() {
        let (mut dom, controls) = mount();
        session_fixture::mark_external_logout(UNKNOWN_LOGOUT);
        session_fixture::script([session_fixture::read(None).under_lock()]);
        dom.in_runtime(|| controls.check.call(SessionRefreshReason::CookieChanged));
        settle(&mut dom);
        assert_eq!(session_fixture::take_requests(), ["GET"]);
        assert!(controls.owner.peek().is_none());
        assert!(!*controls.ended.peek());
    }

    #[test]
    fn expiry_is_rechecked_once_even_if_an_earlier_read_failed() {
        for at_expiry_succeeds in [false, true] {
            session_fixture::reset();
            let now = Timestamp::parse_from_rfc3339("2090-01-01T11:59:50Z").unwrap();
            let clock = ExpiryClock::new(now);
            session_fixture::script([
                session_fixture::read_finite(USER, EXPIRES).under_lock(),
                session_fixture::reply(
                    SessionRequest::Sync(USER),
                    session_fixture::session_response_with_deadline(Some(USER), Some(EXPIRES)),
                )
                .under_lock(),
            ]);
            let (mut dom, controls) =
                mount_harness(VirtualDom::new(SessionHarness).with_root_context(clock.clone()));
            assert!(clock.waiting());
            session_fixture::script([session_fixture::reply(
                SessionRequest::Read,
                Err(TransportError::Timeout),
            )]);
            dom.in_runtime(|| controls.check.call(SessionRefreshReason::Refresh));
            settle(&mut dom);
            assert_eq!(*controls.session.peek(), Some(Err("request_timeout".to_owned())));
            assert!(controls.owner.peek().is_some());
            assert!(clock.waiting());

            session_fixture::script([if at_expiry_succeeds {
                session_fixture::read(None).under_lock()
            } else {
                session_fixture::reply(
                    SessionRequest::Read,
                    session_fixture::rejection(503, "storage_unavailable"),
                )
                .under_lock()
            }]);
            clock.advance_to(Timestamp::parse_from_rfc3339("2090-01-01T12:00:02Z").unwrap());
            settle(&mut dom);
            assert_eq!(session_fixture::take_requests(), ["GET", "GET"]);
            if at_expiry_succeeds {
                assert_eq!(*controls.session.peek(), Some(Ok(None)));
                assert!(controls.owner.peek().is_none());
            } else {
                assert_eq!(*controls.session.peek(), Some(Err("storage_unavailable".to_owned())));
                assert_eq!(controls.owner.peek().as_deref(), Some(USER));
            }
            assert!(!*controls.ended.peek());
            clock.advance_to(Timestamp::parse_from_rfc3339("2090-01-01T12:10:00Z").unwrap());
            settle(&mut dom);
            assert!(session_fixture::take_requests().is_empty());
        }
    }

    fn watch_snapshot(user: &str) -> PublicLotWatchesResponse {
        PublicLotWatchesResponse {
            user_id: user.to_owned(),
            auction_id: AUCTION.to_owned(),
            watches: vec![PublicLotWatchResponse {
                lot_id: LOT.to_owned(),
                watched: true,
                channel: "mm".to_owned(),
                version: 1,
            }],
        }
    }

    fn mount_with_watch_snapshot() -> (VirtualDom, Controls, Rc<WatchProjection>) {
        session_fixture::reset();
        save_watch_cache(&watch_snapshot(USER));
        let displayed = Rc::new(WatchProjection::default());
        let (dom, controls) =
            mount_harness(VirtualDom::new(SessionHarness).with_root_context(displayed.clone()));
        assert!(displayed.0.get());
        (dom, controls, displayed)
    }

    #[test]
    fn same_account_login_keeps_the_watch_snapshot_and_requests_private_revalidation() {
        let (mut dom, controls, displayed) = mount_with_watch_snapshot();
        let mark = watch_snapshot(USER);
        let request = SessionRequest::Login("fixture");
        let read = Rc::new(ResponseGate::default());
        session_fixture::script([
            session_fixture::reply(request, session_fixture::login_success(USER)),
            read.hold_read(Some(USER)).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert!(read.waiting());
        assert!(displayed.0.get());
        assert_eq!(load_watch_cache(USER, AUCTION), Some(mark.clone()));
        assert_eq!(*controls.watch_refresh.peek(), 0);
        read.release();
        settle(&mut dom);
        assert_eq!(load_watch_cache(USER, AUCTION), Some(mark));
        assert!(displayed.0.get());
        assert_eq!(*controls.watch_refresh.peek(), 1);
        assert_eq!(controls.current_user().as_deref(), Some(USER));
        assert_eq!(session_fixture::take_requests(), ["POST", "GET"]);
    }

    #[test]
    fn confirmed_logout_clears_only_the_departing_account_watches_before_read_back() {
        let (mut dom, controls, displayed) = mount_with_watch_snapshot();
        let other = watch_snapshot(OTHER_USER);
        save_watch_cache(&other);
        let request = SessionRequest::Logout(USER);
        let read = Rc::new(ResponseGate::default());
        session_fixture::script([
            session_fixture::reply(
                request,
                Ok(session_fixture::response(
                    200,
                    r#"{"code":"ok","message":"已退出","data":null}"#.to_owned(),
                )),
            ),
            read.hold(SessionRequest::Read, Err(TransportError::Timeout)).under_lock(),
        ]);
        dom.in_runtime(|| controls.mutate.call(request));
        settle(&mut dom);
        assert!(read.waiting());
        assert!(controls.owner.peek().is_none());
        assert!(!displayed.0.get());
        assert_eq!(load_watch_cache(USER, AUCTION), None);
        assert_eq!(load_watch_cache(OTHER_USER, AUCTION), Some(other));
        read.release();
        settle(&mut dom);
        assert_eq!(
            session_fixture::take_revision_writes(),
            [(CONFIRMED_LOGOUT, true), (REVISION, false),]
        );
        assert!(!*controls.ended.peek());
        assert_eq!(load_watch_cache(USER, AUCTION), None);
    }

    mod form {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use auction_contracts::{ApiResponse, PublicSessionPreferenceResponse};
        use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
        use dioxus::dioxus_core::{Mutation, Mutations};
        use dioxus::html::PlatformEventData;
        use dioxus::prelude::*;
        use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
        use uuid::Uuid;

        use super::{AUCTION, LOT, OTHER_USER, PREFERENCE, REVISION, USER};
        use crate::api::{CommandError, SessionRequest, TransportError};
        use crate::surface::commands::form_fixture;
        use crate::surface::commands::host::{CompetitionCommandHost, CompetitionCommands};
        use crate::surface::commands::pending::PendingBidCommand;
        use crate::surface::commands::request::{CommandPayload, CommandStatus, CommandTarget};
        use crate::surface::commands::session_fixture::{self, ResponseGate};
        use crate::surface::commands::test_support::{enable_requests, requests};
        use crate::surface::commands::{
            use_competition_commands_provider, use_public_session_checks,
        };
        use crate::surface::session::{LoginFormFixture, SessionEndedNotice, login};
        use crate::surface::session_preference::SessionPreference;
        use crate::surface::{
            LotLinkRequest, PublicCommandOwner, PublicContentLoading, PublicSessionResource,
        };

        const RECEIPT_LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000007";

        struct UnauthorizedReply {
            body: Rc<ResponseGate>,
            failure: Option<TransportError>,
        }

        #[derive(Clone, Copy)]
        struct ShellControls {
            session: PublicSessionResource,
            owner: Signal<Option<String>>,
            commands: CompetitionCommands,
            bid: EventHandler<&'static str>,
            retry: EventHandler<&'static str>,
            away: EventHandler<()>,
            preferences: EventHandler<()>,
            can_submit: Callback<(), bool>,
        }

        thread_local! {
            static SHELL: Cell<Option<ShellControls>> = const { Cell::new(None) };
            static UNAUTHORIZED_REPLY: RefCell<Option<UnauthorizedReply>> =
                const { RefCell::new(None) };
        }

        #[derive(Clone, Debug, PartialEq, Routable)]
        enum TestRoute {
            #[layout(SessionLayout)]
            #[route("/")]
            FormPage {},
            #[route("/away")]
            Away {},
            #[route("/preference")]
            Preferences {},
        }

        #[component]
        fn SessionRouter() -> Element {
            try_use_init_i18n_from_bundle(|| {
                build_bundle(
                    &DEFAULT_PUBLIC_LOCALE,
                    include_str!("../../../i18n/en.ftl"),
                    &DEFAULT_PUBLIC_LOCALE,
                    None,
                )
            })
            .expect("初始化英文译文");
            rsx! { Router::<TestRoute> {} }
        }

        #[component]
        fn SessionLayout() -> Element {
            session_fixture::use_session_fixture(USER);
            let commands = use_competition_commands_provider();
            let checks = use_public_session_checks(commands);
            let refresh = use_signal(|| 0_u64);
            use_context_provider(|| refresh);
            use_context_provider(|| LotLinkRequest(Signal::new(None)));
            use_context_provider(|| PublicCommandOwner(checks.page_owner));
            let loading = use_signal(std::collections::BTreeSet::new);
            use_context_provider(|| PublicContentLoading(loading));
            let origin = use_signal(|| ());
            let bid = use_callback(move |lot_id: &'static str| {
                let store = commands.current().expect("原账号竞价宿主已挂载");
                let target = Rc::new(CommandTarget {
                    world: "main".to_owned(),
                    auction_id: AUCTION.to_owned(),
                    lot_id: lot_id.to_owned(),
                    lot_code: "Clo16".to_owned(),
                    lot_name: "Legendary Shield".to_owned(),
                });
                let payload = CommandPayload::Bid(PendingBidCommand {
                    user_id: Uuid::parse_str(USER).unwrap(),
                    command_id: Uuid::new_v4(),
                    amount: 10_000,
                });
                assert!(store.submit(target, payload, origin));
                commands.detach(origin);
            });
            let navigator = use_navigator();
            let retry = use_callback(move |lot_id: &'static str| {
                if let Some(store) = commands.current()
                    && let Some(command) = store.command(lot_id)
                {
                    store.retry(lot_id, command.payload.command_id(), None);
                }
            });
            let away = use_callback(move |()| {
                navigator.push(TestRoute::Away {});
            });
            let preferences = use_callback(move |()| {
                navigator.push(TestRoute::Preferences {});
            });
            let can_submit =
                use_callback(move |()| commands.current().is_some_and(|store| store.can_submit()));
            SHELL.set(Some(ShellControls {
                session: checks.session,
                owner: checks.competition_owner,
                commands,
                bid,
                retry,
                away,
                preferences,
                can_submit,
            }));
            rsx! {
                CompetitionCommandHost { owner: checks.competition_owner }
                SessionEndedNotice { ended: checks.ended_elsewhere }
                Outlet::<TestRoute> {}
            }
        }

        #[component]
        fn FormPage() -> Element {
            login(None)
        }

        #[component]
        fn Away() -> Element {
            rsx! { p { "另一条路由" } }
        }

        #[component]
        fn Preferences() -> Element {
            rsx! { SessionPreference { user_id: USER.to_owned() } }
        }

        fn settle(dom: &mut VirtualDom) -> Mutations {
            let mut mutations = Mutations::default();
            for _ in 0..8 {
                dom.process_events();
                dom.render_immediate(&mut mutations);
            }
            mutations
        }

        #[test]
        fn session_preference_form_retries_the_identical_command_after_unknown_conflict() {
            session_fixture::reset();
            let key = Rc::new(Cell::new(None));
            let mut dom = VirtualDom::new(SessionRouter).with_root_context(LoginFormFixture(key));
            dom.rebuild_in_place();
            settle(&mut dom);
            session_fixture::take_requests();

            let preference = PublicSessionPreferenceResponse {
                user_id: USER.to_owned(),
                duration_seconds: Some(900),
                version: 17,
                expires_at: None,
            };
            let payload =
                serde_json::to_string(&ApiResponse::ok("已读取偏好", preference)).unwrap();
            session_fixture::script([session_fixture::reply(
                SessionRequest::ReadPreference,
                Ok(session_fixture::response(200, payload)),
            )]);
            SHELL.get().unwrap().preferences.call(());
            let mounted = settle(&mut dom);
            session_fixture::assert_consumed();
            assert_eq!(session_fixture::take_requests(), ["GET preference"]);
            let form = mounted
                .edits
                .iter()
                .find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "submit" => Some(*id),
                    _ => None,
                })
                .expect("登录时长表单已挂载");

            let command = SessionRequest::SetPreference(USER, &PREFERENCE);
            let unknown = || {
                [
                    session_fixture::reply(
                        command,
                        session_fixture::rejection(409, "watch_preference_conflict"),
                    )
                    .under_lock(),
                    session_fixture::read(Some(USER)).under_lock(),
                ]
            };
            session_fixture::script(unknown());
            form_fixture::submit(&dom, form);
            let failed = settle(&mut dom);
            session_fixture::assert_consumed();
            assert_eq!(session_fixture::take_requests(), ["PUT preference", "GET"]);
            assert!(failed.edits.iter().any(|edit| matches!(
                edit,
                Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                    if value == "Retry original request"
            )));
            let retry = failed
                .edits
                .iter()
                .chain(&mounted.edits)
                .find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                    _ => None,
                })
                .expect("结果未知已显示原样重试按钮");

            session_fixture::script(unknown());
            let click = Event::new(Rc::new(PlatformEventData::new(Box::new(()))), true);
            dom.runtime().handle_event("click", click.into_any(), retry);
            settle(&mut dom);
            session_fixture::assert_consumed();
            assert_eq!(session_fixture::take_requests(), ["PUT preference", "GET"]);

            let commands = session_fixture::take_preference_commands();
            let [first, replay] = commands.as_slice() else {
                panic!("必须恰好发送两次偏好命令")
            };
            assert!(Uuid::parse_str(&first.command_id).is_ok());
            assert_eq!(first.command_id, replay.command_id);
            assert_eq!(first.duration_seconds, Some(900));
            assert_eq!(first.duration_seconds, replay.duration_seconds);
            assert_eq!(first.expected_version, 17);
            assert_eq!(first.expected_version, replay.expected_version);
        }

        #[test]
        fn received_401_header_pauses_other_bid_retries_before_the_body_finishes() {
            for failure in
                [Some(TransportError::Timeout), Some(TransportError::Network(Some(401))), None]
            {
                session_fixture::reset();
                enable_requests();
                let key = Rc::new(Cell::new(None));
                let mut dom =
                    VirtualDom::new(SessionRouter).with_root_context(LoginFormFixture(key));
                dom.rebuild_in_place();
                settle(&mut dom);
                let controls = SHELL.get().unwrap();
                assert!(controls.can_submit.call(()));
                dom.in_runtime(|| controls.bid.call(RECEIPT_LOT));
                settle(&mut dom);
                requests()
                    .last()
                    .unwrap()
                    .complete(Err(CommandError { status: 0, code: "network_error".to_owned() }));
                settle(&mut dom);

                let body = Rc::new(ResponseGate::default());
                UNAUTHORIZED_REPLY.replace(Some(UnauthorizedReply { body: body.clone(), failure }));
                CommandError::intercept_test_requests_with_head(|_, _, _| {
                    let UnauthorizedReply { body, failure } =
                        UNAUTHORIZED_REPLY.take().expect("只应发送首次竞价");
                    Ok((
                        401,
                        Box::pin(async move {
                            body.wait().await;
                            match failure {
                            Some(error) => Err(error),
                            None => Ok((401, Some(r#"{"code":"unauthenticated","message":"未认证","data":null}"#.to_owned()))),
                        }
                        }),
                    ))
                });
                session_fixture::take_requests();
                let read = Rc::new(ResponseGate::default());
                session_fixture::script([read.hold_read(Some(USER)).under_lock()]);

                dom.in_runtime(|| controls.bid.call(LOT));
                settle(&mut dom);
                assert!(body.waiting());
                assert!(read.waiting(), "收到 401 响应头后、正文结束前，已发出会话 GET");
                assert_eq!(controls.owner.peek().as_deref(), Some(USER));
                assert!(!controls.can_submit.call(()));
                controls.retry.call(RECEIPT_LOT);
                settle(&mut dom);
                assert_eq!(session_fixture::take_requests(), ["GET"]);

                body.release();
                settle(&mut dom);
                assert!(!controls.can_submit.call(()));
                dom.in_runtime(|| {
                    let store = controls.commands.current().unwrap();
                    let status = store.command(LOT).map(|command| command.status);
                    let expected = failure.map(|error| CommandStatus::Unknown {
                        retrying: false,
                        error: CommandError { code: error.code().to_owned(), status: 401 },
                    });
                    assert_eq!(status, expected);
                    assert!(matches!(
                        store.command(RECEIPT_LOT).unwrap().status,
                        CommandStatus::Unknown { retrying: false, .. }
                    ));
                });

                read.release();
                settle(&mut dom);
                assert!(session_fixture::take_requests().is_empty(), "正文收口不能重复读回");
                assert_eq!(session_fixture::take_revision_writes(), [(REVISION, false)]);
                assert!(controls.can_submit.call(()));
            }
        }

        #[test]
        fn submitted_login_survives_route_change_and_retires_old_commands_before_failed_read() {
            session_fixture::reset();
            enable_requests();
            let key = Rc::new(Cell::new(None));
            let mut dom =
                VirtualDom::new(SessionRouter).with_root_context(LoginFormFixture(key.clone()));
            let mounted = dom.rebuild_to_vec();
            let form = mounted
                .edits
                .iter()
                .find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "submit" => Some(*id),
                    _ => None,
                })
                .expect("真实登录表单已挂载");
            settle(&mut dom);
            let controls = SHELL.get().unwrap();
            let mut input = key.get().unwrap();

            dom.in_runtime(|| controls.bid.call(LOT));
            settle(&mut dom);
            requests()
                .last()
                .unwrap()
                .complete(Err(CommandError { status: 0, code: "network_error".to_owned() }));
            settle(&mut dom);
            dom.in_runtime(|| controls.bid.call(RECEIPT_LOT));
            settle(&mut dom);
            requests().last().unwrap().complete(Ok(()));
            settle(&mut dom);
            dom.in_runtime(|| {
                let store = controls.commands.current().unwrap();
                assert!(store.command(LOT).unwrap().status.unknown());
                assert_eq!(store.receipts().0.len(), 1);
                input.set("fixture".to_owned());
            });

            let response = Rc::new(ResponseGate::default());
            let read = Rc::new(ResponseGate::default());
            session_fixture::take_requests();
            session_fixture::script([
                response
                    .hold(
                        SessionRequest::Login("fixture"),
                        session_fixture::login_success(OTHER_USER),
                    )
                    .under_lock(),
                read.hold(SessionRequest::Read, Err(TransportError::Network(None))).under_lock(),
            ]);
            form_fixture::submit(&dom, form);
            settle(&mut dom);
            assert!(response.waiting());
            assert!(input.peek().is_empty());
            dom.in_runtime(|| controls.away.call(()));
            settle(&mut dom);
            assert!(input.try_peek().is_err(), "切路由确实卸载了登录表单");
            assert_eq!(controls.owner.peek().as_deref(), Some(USER));

            response.release();
            settle(&mut dom);
            assert!(read.waiting(), "卸载后仍由外壳收包并发起读回");
            assert_eq!(controls.owner.peek().as_deref(), Some(OTHER_USER));
            read.release();
            settle(&mut dom);
            assert_eq!(*controls.session.peek(), Some(Err("network_error".to_owned())));
            assert_eq!(session_fixture::take_requests(), ["POST", "GET"]);
            assert!(input.try_peek().is_err(), "卸载的登录表单没有触发返回导航");
            dom.in_runtime(|| {
                let store = controls.commands.current().unwrap();
                assert!(store.command(LOT).is_none());
                assert!(store.receipts().0.is_empty());
            });
        }
    }

    fn session(user_id: &str) -> Result<Option<PublicSession>, String> {
        Ok(Some(PublicSession {
            authenticated: true,
            user_id: user_id.to_owned(),
            username: "竞价者".to_owned(),
            expires_at: None,
            absolute_expires_at: None,
        }))
    }

    #[test]
    fn rechecking_the_same_identity_keeps_the_form_and_pauses_new_commands() {
        let user_id = Some("alice");
        let session = session("alice");
        let snapshot = Some(&session);
        assert!(identity_confirmed(user_id, user_id, snapshot));
        assert!(identity_confirmed(user_id, user_id, None));
        assert!(!can_submit_new(user_id, user_id, UseResourceState::Pending, snapshot));
        assert!(!can_submit_new(user_id, user_id, UseResourceState::Ready, None));
        assert!(can_submit_new(user_id, user_id, UseResourceState::Ready, snapshot));
    }

    #[test]
    fn failed_recheck_keeps_the_form_without_allowing_new_commands() {
        let user_id = Some("alice");
        let error = Err("network_error".to_owned());
        let snapshot = Some(&error);
        assert!(identity_confirmed(user_id, user_id, snapshot));
        assert!(!can_submit_new(user_id, user_id, UseResourceState::Ready, snapshot));
        assert!(!identity_confirmed(user_id, None, snapshot));
    }

    #[test]
    fn anonymous_or_different_identity_cannot_keep_the_form_or_submit() {
        let user_id = Some("alice");
        for session in [Ok(None), session("bob")] {
            let snapshot = Some(&session);
            assert!(!identity_confirmed(user_id, user_id, snapshot));
            assert!(!can_submit_new(user_id, user_id, UseResourceState::Ready, snapshot));
        }
        assert!(!identity_confirmed(user_id, None, None));
    }
}

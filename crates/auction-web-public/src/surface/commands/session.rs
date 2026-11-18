//! 命令宿主的已确认身份和新写入门禁。
//! 旧身份的记录在“确认当前 Cookie 不再属于它”的那一刻销毁，不等待任何后续请求。
//! 成功解码的权威身份立即建立新归属。`409 session_changed` 只销毁请求预期用户的宿主，再复核当前 Cookie。

use dioxus::prelude::*;

use super::host::CompetitionCommands;
use crate::api::{
    PublicSession, PublicSessionApiError, PublicSessionCheckTrigger, PublicSessionEvidenceTrigger,
    SessionIdentity, SessionIdentityEvidence, SessionRefreshReason, estimated_now,
    fetch_public_session,
};
use crate::surface::{PublicCommandOwner, PublicSessionResource};

/// 外壳的统一会话刷新入口及两个命令宿主的身份投影。
pub(in crate::surface) struct PublicSessionChecks {
    pub session: PublicSessionResource,
    pub page_owner: Signal<Option<String>>,
    pub competition_owner: Signal<Option<String>>,
    pub ended_elsewhere: Signal<bool>,
    pub check: EventHandler<SessionRefreshReason>,
}

pub(in crate::surface) fn use_public_session_checks(
    commands: CompetitionCommands,
) -> PublicSessionChecks {
    let mut page_owner = use_signal(|| None::<String>);
    let mut competition_owner = use_signal(|| None::<String>);
    let mut ended_elsewhere = use_signal(|| false);
    let revision = use_signal(|| 0_u64);
    let mut recheck_after_evidence = use_signal(|| false);
    let mut session = use_resource(|| async {
        fetch_public_session().await.map_err(PublicSessionApiError::into_read_code)
    });
    let check = use_callback(move |reason: SessionRefreshReason| {
        if reason.invalidates() {
            if session.peek().as_ref().is_some_and(|result| {
                result.as_ref().is_ok_and(|identity| {
                    identity.as_ref().is_some_and(session_ended_by_revocation)
                })
            }) {
                ended_elsewhere.set(true);
            }
            // 普通 401 仅清除会话快照并暂停重试，先前命令是否受理尚未确定。
            page_owner.set(None);
            session.set(None);
        }
        session.restart();
    });
    let apply = use_callback(move |evidence: SessionIdentityEvidence| match evidence {
        SessionIdentityEvidence::Current { identity, cookie_written } => {
            let next = match &identity {
                SessionIdentity::Snapshot(session) => {
                    session.as_ref().map(|identity| identity.user_id.clone())
                }
                SessionIdentity::UserId(user_id) => Some(user_id.clone()),
            };
            let changed = *competition_owner.peek() != next;
            if changed || cookie_written {
                advance_revision(revision);
            }
            if changed {
                commands.discard_identity();
                competition_owner.set(next.clone());
            }
            if next.is_some() || cookie_written {
                ended_elsewhere.set(false);
            }
            if *page_owner.peek() != next {
                page_owner.set(next);
            }
            match identity {
                SessionIdentity::Snapshot(snapshot) => {
                    let confirmed = Ok(snapshot);
                    if session.peek().as_ref() != Some(&confirmed) {
                        session.set(Some(confirmed));
                    }
                }
                SessionIdentity::UserId(_) if changed => {
                    // 私有响应能确定归属，完整会话快照仍须重读才能用于新写入。
                    session.set(None);
                    if !cookie_written {
                        recheck_after_evidence.set(true);
                    }
                }
                SessionIdentity::UserId(_) => {}
            }
        }
        SessionIdentityEvidence::ExpectedUserRejected(expected_user) => {
            if competition_owner.peek().as_deref() == Some(expected_user.as_str()) {
                commands.discard_identity();
                advance_revision(revision);
                competition_owner.set(None);
                page_owner.set(None);
                session.set(None);
                ended_elsewhere.set(false);
            }
            recheck_after_evidence.set(true);
        }
    });
    use_effect(move || {
        if recheck_after_evidence() {
            recheck_after_evidence.set(false);
            // 同步请求也能报告冲突，复核不能在资源自己的任务里重启资源。
            check.call(SessionRefreshReason::Refresh);
        }
    });
    use_context_provider(|| PublicSessionCheckTrigger(check));
    use_context_provider(|| PublicSessionEvidenceTrigger { apply, revision });
    PublicSessionChecks { session, page_owner, competition_owner, ended_elsewhere, check }
}

fn advance_revision(mut revision: Signal<u64>) {
    let next = revision.peek().wrapping_add(1);
    revision.set(next);
}

/// 未到期却失效的认证会话只来自服务端撤销；自然到期不显示“已在其他设备登录”。
fn session_ended_by_revocation(identity: &PublicSession) -> bool {
    if !identity.authenticated {
        return false;
    }
    let Some(expires_at) = identity.expires_at.as_deref() else {
        return true;
    };
    auction_time::Timestamp::parse_from_rfc3339(expires_at)
        .is_ok_and(|expiry| expiry > estimated_now())
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
    use dioxus::prelude::UseResourceState;

    use super::{can_submit_new, identity_confirmed};
    use crate::api::PublicSession;

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

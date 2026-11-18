//! 命令宿主的已确认身份和新写入门禁。

use dioxus::prelude::*;

use crate::api::PublicSession;
use crate::surface::{PublicCommandOwner, PublicSessionResource};

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
    let session = use_context::<PublicSessionResource>();
    let confirmed = use_memo({
        let user_id = user_id.clone();
        move || {
            identity_confirmed(user_id.as_deref(), owner.read().as_deref(), session.read().as_ref())
        }
    });
    use_context_provider(|| CommandSession { user_id, owner, session, confirmed });
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

//! 会话结束通知账本。只驻留在外壳内存，随界面卸载的草稿与命令不参与归属判断。

use std::rc::Rc;

use super::transition::Actions;
use super::{Cookie, Effect, Phase, State};
use crate::api::AdminSession;

#[derive(Clone)]
struct EndEvidence {
    session: Rc<AdminSession>,
    saw_other_generation: bool,
}

impl EndEvidence {
    fn matches(&self, session: &AdminSession, cookie: &Cookie) -> bool {
        self.session.same_identity(session)
            && cookie_can_end_session(session, cookie, self.saw_other_generation)
    }
}

/// 已归属的过期证据、待本地结果核对的对端通知，以及已结清的会话标识。
#[derive(Clone, Default)]
pub(super) struct EndNotices {
    expiration: Option<EndEvidence>,
    peer: Option<EndEvidence>,
    settled: Option<u64>,
}

fn end_notice_id(session: &AdminSession) -> u64 {
    // 64 位 FNV-1a 指纹仅用于通知去重。0xff 分隔 UTF-8 字段。
    session
        .actor
        .bytes()
        .chain([0xff])
        .chain(session.csrf_token.bytes())
        .chain([0xff])
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// Cookie 明确属于原会话，或稳定缺失且未观察到异代时，才能归属自然过期。
pub(super) fn cookie_can_end_session(
    session: &AdminSession,
    cookie: &Cookie,
    saw_other_generation: bool,
) -> bool {
    match cookie {
        Cookie::Present(token) => token.as_ref() == session.csrf_token.as_str(),
        Cookie::Missing => !saw_other_generation,
        Cookie::Unavailable => false,
    }
}

impl State {
    /// 为复核固定原有身份。界面已卸载时仍可核对账本和实发的退出意图。
    pub(super) fn notice_origin(&self) -> Option<&Rc<AdminSession>> {
        self.phase
            .verified()
            .map(|verified| &verified.session)
            .or_else(|| self.logout_intent.as_ref().map(|intent| &intent.target.session))
            .or(match &self.phase {
                Phase::LoggingOut(_, logout) => Some(&logout.target.session),
                _ => None,
            })
            .or_else(|| self.notices.expiration.as_ref().map(|evidence| &evidence.session))
            .or_else(|| self.notices.peer.as_ref().map(|evidence| &evidence.session))
    }

    /// 在撤下宿主前保存已经归属的续期 401；等待退出回执或后续权威读取来结清。
    pub(super) fn defer_expiration(&mut self, session: Rc<AdminSession>) {
        self.notices.expiration =
            Some(EndEvidence { session, saw_other_generation: self.saw_other_generation });
    }

    /// 固定对端通知到原有身份；同一身份的可信本地结论负责结清。
    pub(super) fn remember_peer_end(&mut self) {
        if self.notices.peer.is_none()
            && let Some(session) = self.notice_origin().cloned()
        {
            self.notices.peer =
                Some(EndEvidence { session, saw_other_generation: self.saw_other_generation });
        }
    }

    /// 另一代 Cookie 或正文使缺失 Cookie 的 401 无法归属原有身份。
    pub(super) fn observe_end_notice_identity(&mut self, differs: impl Fn(&AdminSession) -> bool) {
        for evidence in [&mut self.notices.expiration, &mut self.notices.peer].into_iter().flatten()
        {
            evidence.saw_other_generation |= differs(&evidence.session);
        }
    }

    /// 界面卸载后，稳定的读取也可核对账本里的待结清证据。
    pub(super) fn has_end_evidence(&self, session: &AdminSession, cookie: &Cookie) -> bool {
        [&self.notices.expiration, &self.notices.peer]
            .into_iter()
            .flatten()
            .any(|evidence| evidence.matches(session, cookie))
    }

    /// 完整 DELETE、可归属的 401 和对端通知用同一会话身份结清，迟到的回执不会再广播。
    pub(super) fn settle_end_notice(&mut self, session: &AdminSession, actions: &mut Actions) {
        let heard_from_peer =
            self.notices.peer.take_if(|evidence| evidence.session.same_identity(session)).is_some();
        self.notices.expiration.take_if(|evidence| evidence.session.same_identity(session));
        let settled = Some(end_notice_id(session));
        if self.notices.settled != settled {
            self.notices.settled = settled;
            if !heard_from_peer {
                actions.effects.push(Effect::BroadcastEnded);
            }
        }
    }

    /// 确认新代际时，旧读取与退出票已失效，旧续期也无法归属新宿主。
    pub(super) fn retain_settled_end_notice(&mut self, confirmed: &AdminSession) {
        if self.notices.settled.is_some_and(|settled| settled != end_notice_id(confirmed)) {
            self.notices.settled = None;
        }
    }

    /// 权威读取确认会话仍在或已换代时，结束本轮推断；已结清的身份仍用于去重。
    pub(super) fn clear_pending_end_notices(&mut self) {
        self.notices.expiration = None;
        self.notices.peer = None;
    }
}

//! Admin 会话的纯状态机。只有已认证且身份和退出结果均无疑问时才允许写入。
//! 身份未确认或退出未确认时遮蔽一次性 Key，恢复后须由操作员重新打开。
//! 在途登录由外壳登记，广播卸载表单后合并到登录结清时的一次 GET。
//! 退出请求独立持有回执；收到广播立即卸载敏感页面，退出期间只登记事后复核。
//! 过期与退出的证据独立于敏感宿主，结束通知按会话身份最多结清一次。
//! 同一代际的截止只向后更新，截止与单调计时基准来自同一份服务端投影。
//! 广播期间的同代续期只暂存，确认身份的 GET 才能采用。
//! 续期结果未知只做一次有界的只读复核。限流后仅保留 POST 发出之后的输入，
//! 在退避下界与会话截止之间重新安排，等待期间不因输入时效丢弃。
//! 确认会话失效也立即撤下敏感页面。

use std::rc::Rc;

use self::notices::EndNotices;
use super::KeyVisibility;
use super::activity::RenewalTiming;
use crate::api::AdminSession;

mod deadlines;
mod logout;
mod notices;
mod read_results;
mod transition;

#[cfg(test)]
mod tests;

pub(in crate::surface::auth) use deadlines::session_lifetime_millis;
pub(in crate::surface::auth) use transition::step;

/// 可读的 CSRF Cookie 仅用于本地判断会话代际。
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Cookie {
    Unavailable,
    Missing,
    Present(Rc<str>),
}

impl Cookie {
    fn from_session(session: &AdminSession) -> Self {
        Self::Present(Rc::from(session.csrf_token.as_str()))
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        !matches!(self, Self::Unavailable) && !matches!(other, Self::Unavailable) && self != other
    }

    fn agrees_with(&self, other: &Self) -> bool {
        matches!(self, Self::Present(_)) && self == other
    }
}

/// 请求与定时器的标识只在本外壳内使用。
pub(super) type Ticket = u64;

#[derive(Clone)]
struct Request {
    ticket: Ticket,
    cookie: Cookie,
}

impl Request {
    fn is_stale(&self, sent: &Cookie, observed: &Cookie, cleared_by_logout: bool) -> bool {
        // 成功 DELETE 的 Set-Cookie 会清空代际。此时仍须排除派发期间换了实际请求 Cookie。
        self.cookie.conflicts_with(sent)
            || (!cleared_by_logout
                && (sent.conflicts_with(observed) || self.cookie.conflicts_with(observed)))
    }

    fn matches_generation(&self, sent: &Cookie, observed: &Cookie) -> bool {
        !matches!(sent, Cookie::Unavailable) && self.cookie == *sent && sent == observed
    }
}

#[derive(Clone)]
struct Verified {
    session: Rc<AdminSession>,
    cookie: Cookie,
}

impl Verified {
    fn new(session: Rc<AdminSession>) -> Self {
        Self { cookie: Cookie::from_session(&session), session }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckReason {
    Startup,
    Routine,
    Explicit,
    Discover,
    Login,
    Broadcast,
    CookieChanged,
    AfterLogout,
    StayAfterLogout,
    AfterRenewal,
}

#[derive(Clone)]
struct Check {
    reason: CheckReason,
    request: Request,
    origin: Option<Rc<AdminSession>>,
    logout_ticket: Option<Ticket>,
}

#[derive(Clone)]
struct Logout {
    request: Request,
    target: Verified,
    saw_other_generation: bool,
    review_after: bool,
    was_writable: bool,
    continuing: bool,
}

#[derive(Clone)]
struct LogoutIntent {
    target: Verified,
    last_sent: Ticket,
    can_infer_from_read: bool,
}

#[derive(Clone)]
struct Renewal {
    request: Request,
    origin: Rc<AdminSession>,
    source: Option<Verified>,
    epoch: u64,
}

/// 用户可观察的会话阶段与当前认证宿主。
#[derive(Clone, Default)]
enum Phase {
    #[default]
    Starting,
    Authenticated(Verified),
    Checking(Option<Verified>),
    CheckingLogin,
    Unavailable(Option<Verified>, String),
    LoggingOut(Option<Verified>, Logout),
    LogoutUnconfirmed(Option<Verified>),
    LoggedOut,
}

impl Phase {
    fn verified(&self) -> Option<&Verified> {
        match self {
            Self::Authenticated(verified) => Some(verified),
            Self::Checking(verified)
            | Self::Unavailable(verified, _)
            | Self::LoggingOut(verified, _)
            | Self::LogoutUnconfirmed(verified) => verified.as_ref(),
            Self::Starting | Self::CheckingLogin | Self::LoggedOut => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Activity {
    timing: Option<RenewalTiming>,
    latest_input: Option<f64>,
    retry_not_before: f64,
}

/// 单个标签页的会话状态及属于它的在途请求。
#[derive(Clone, Default)]
pub(super) struct State {
    phase: Phase,
    check: Option<Check>,
    login: Option<Ticket>,
    // 记录存在即仍欠通知。能否由 GET 推断结束由意图单独保存。
    logout_intent: Option<LogoutIntent>,
    notices: EndNotices,
    identity_unconfirmed: bool,
    saw_other_generation: bool,
    key_visibility_revision: u64,
    renewal: Option<Renewal>,
    buffered_renewal: Option<Verified>,
    activity: Activity,
    check_timer: Option<Ticket>,
    renewal_timer: Option<Ticket>,
    next_ticket: Ticket,
    view_epoch: u64,
    login_error: Option<String>,
    logout_error: Option<String>,
}

/// 组件渲染与写门禁共用的快照。
#[derive(Clone, Default, PartialEq)]
pub(super) struct View {
    pub(super) session: Option<Rc<AdminSession>>,
    pub(super) epoch: u64,
    pub(super) can_write: bool,
    pub(super) logging_out: bool,
    pub(super) logout_unconfirmed: bool,
    pub(super) logout_decision_busy: bool,
    pub(super) key_visibility: KeyVisibility,
    pub(super) logged_out: bool,
    pub(super) login_pending: bool,
    pub(super) login_error: Option<String>,
    pub(super) error: Option<String>,
    pub(super) logout_error: Option<String>,
}

impl State {
    /// 只有当前读取票可以实际派发。
    pub(super) fn has_read(&self, ticket: Ticket) -> bool {
        self.check.as_ref().is_some_and(|check| check.request.ticket == ticket)
    }

    /// 发送前复核后仍持有该票才可派发退出。
    pub(super) fn has_logout(&self, ticket: Ticket) -> bool {
        matches!(&self.phase, Phase::LoggingOut(_, logout) if logout.request.ticket == ticket)
    }

    /// 发送前复核后仍持有该票才可派发活动续期。
    pub(super) fn has_renewal(&self, ticket: Ticket) -> bool {
        self.renewal.as_ref().is_some_and(|renewal| renewal.request.ticket == ticket)
    }

    /// 派发前重新隐藏或已经提交本地登录时，撤销尚未发出的匿名探测票。
    pub(super) fn cancel_discovery_before_send(&mut self, ticket: Ticket, visible: bool) -> bool {
        (!visible || self.login.is_some())
            && self
                .check
                .take_if(|check| {
                    check.request.ticket == ticket && check.reason == CheckReason::Discover
                })
                .is_some()
    }

    fn prepare_read(&mut self, ticket: Ticket, actions: &mut transition::Actions) {
        let Some(mut check) = self.check.take_if(|check| check.request.ticket == ticket) else {
            return;
        };
        if !matches!(actions.cookie, Cookie::Unavailable) && check.request.cookie != actions.cookie
        {
            // 旧票尚未发出，按实际代际合并后续读取。登记时的证据不能用于补播或解开退出锁。
            self.revoke_logout_read_notice();
            check.logout_ticket = None;
            check.request.cookie = actions.cookie.clone();
            if check.reason == CheckReason::StayAfterLogout {
                check.reason = CheckReason::AfterLogout;
            }
        }
        self.check = Some(check);
    }

    fn logout_unconfirmed(&self) -> bool {
        match &self.phase {
            Phase::LogoutUnconfirmed(_) => true,
            Phase::LoggingOut(_, logout) => logout.continuing,
            _ => false,
        }
    }

    fn keys_hidden(&self) -> bool {
        self.identity_unconfirmed || self.logout_unconfirmed()
    }

    /// 组件渲染与发送门禁读取同一份已提交的状态。
    pub(super) fn view(&self) -> View {
        let logging_out = matches!(self.phase, Phase::LoggingOut(..));
        let logout_unconfirmed = self.logout_unconfirmed();
        View {
            session: self.phase.verified().map(|verified| Rc::clone(&verified.session)),
            epoch: self.view_epoch,
            can_write: matches!(self.phase, Phase::Authenticated(_)) && !self.keys_hidden(),
            logging_out,
            logout_unconfirmed,
            logout_decision_busy: logout_unconfirmed && (logging_out || self.check.is_some()),
            key_visibility: KeyVisibility {
                hidden: self.keys_hidden(),
                revision: self.key_visibility_revision,
            },
            logged_out: matches!(self.phase, Phase::LoggedOut | Phase::CheckingLogin),
            login_pending: self.login.is_some() || matches!(self.phase, Phase::CheckingLogin),
            login_error: self.login_error.clone(),
            error: match &self.phase {
                Phase::Unavailable(_, error) => Some(error.clone()),
                _ => None,
            },
            logout_error: self.logout_error.clone(),
        }
    }
}

/// 只读会话复核的服务端结果。
#[cfg_attr(test, derive(Clone))]
pub(super) enum ReadOutcome {
    Authenticated(Rc<AdminSession>),
    Unauthenticated,
    Failed(String),
}

/// 退出结果，明确拒绝与需要复核的结果使用不同迁移。
#[cfg_attr(test, derive(Clone))]
pub(super) enum LogoutOutcome {
    Succeeded,
    Rejected(String),
    Unknown(String),
    RateLimited(String),
}

/// 续期结果，退避只约束下一批人工输入。
#[cfg_attr(test, derive(Clone))]
pub(super) enum RenewalOutcome {
    Succeeded(Rc<AdminSession>),
    Unauthenticated,
    RateLimited(Option<u64>),
    CsrfRejected,
    Unknown,
    Rejected,
}

/// 例行复核与活动续期各持有一个可取消的定时器。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Timer {
    Check,
    Renewal,
}

/// 浏览器输入和请求结果。回执携带实际派发时读取的 Cookie 代际。
#[cfg_attr(test, derive(Clone))]
pub(super) enum Event {
    Start,
    LoginFinished(Ticket, Result<Rc<AdminSession>, String>),
    LoginInputChanged,
    CheckRequested,
    WriteReviewIdentity { epoch: u64, session: Rc<AdminSession> },
    ExternalUnauthenticated { epoch: u64, session: Rc<AdminSession> },
    Broadcast,
    Visible,
    Input { on_logout: bool },
    LogoutClicked,
    ContinueLogout,
    StayAfterLogout,
    TimerElapsed(Timer, Ticket),
    ReadSending(Ticket),
    ReadFinished(Ticket, Cookie, ReadOutcome),
    LogoutSending(Ticket),
    LogoutFinished(Ticket, Cookie, LogoutOutcome),
    LogoutNotSent(Ticket, Cookie, LogoutOutcome),
    RenewalSending(Ticket),
    RenewalFinished(Ticket, Cookie, RenewalOutcome),
}

/// 外壳按顺序执行的副作用；广播内容由浏览器适配固定。
pub(super) enum Effect {
    Read(Ticket),
    CancelRead,
    Logout(Ticket, Rc<AdminSession>),
    Renew(Ticket, Rc<AdminSession>),
    ArmTimer(Timer, Ticket, u32),
    CancelTimer(Timer),
    BroadcastEnded,
}

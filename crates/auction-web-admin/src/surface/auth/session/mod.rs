//! Admin 会话的纯状态机。已认证才允许写入，例行 GET 在途仍属于已认证状态。
//! 在途登录由外壳登记，广播卸载表单后合并到登录结清时的一次 GET。
//! 退出请求独立持有回执；收到广播立即卸载敏感页面，退出期间只登记事后复核。
//! 退出成功只通知一次。旧 Cookie 代际的回执只能要求重读当前 Cookie。
//! 同一代际的截止只向后更新，截止与单调计时基准来自同一份服务端投影。
//! 广播期间的同代续期只暂存，确认身份的 GET 才能采用。
//! 续期结果未知只做一次有界的只读复核。限流后仅保留 POST 发出之后的输入，
//! 在退避下界与会话截止之间重新安排，等待期间不因输入时效丢弃。
//! 确认会话失效也立即撤下敏感页面。

use std::rc::Rc;

use super::activity::RenewalTiming;
use crate::api::AdminSession;

mod deadlines;
mod transition;

#[cfg(test)]
mod tests;

pub(super) use deadlines::session_lifetime_millis;
pub(super) use transition::step;

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
    Login,
    Broadcast,
    CookieChanged,
    AfterLogout,
    AfterRenewal,
}

#[derive(Clone)]
struct Check {
    reason: CheckReason,
    request: Request,
}

#[derive(Clone)]
struct Logout {
    request: Request,
    review_after: bool,
    was_writable: bool,
}

#[derive(Clone)]
struct Renewal {
    request: Request,
    source: Option<Verified>,
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
    LoggedOut,
}

impl Phase {
    fn verified(&self) -> Option<&Verified> {
        match self {
            Self::Authenticated(verified) => Some(verified),
            Self::Checking(verified)
            | Self::Unavailable(verified, _)
            | Self::LoggingOut(verified, _) => verified.as_ref(),
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
    pub(super) logged_out: bool,
    pub(super) login_pending: bool,
    pub(super) login_error: Option<String>,
    pub(super) error: Option<String>,
    pub(super) logout_error: Option<String>,
}

impl State {
    pub(super) fn view(&self) -> View {
        View {
            session: self.phase.verified().map(|verified| Rc::clone(&verified.session)),
            epoch: self.view_epoch,
            can_write: matches!(self.phase, Phase::Authenticated(_)),
            logging_out: matches!(self.phase, Phase::LoggingOut(..)),
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
    Broadcast,
    Visible,
    Input { on_logout: bool },
    LogoutClicked,
    TimerElapsed(Timer, Ticket),
    ReadFinished(Ticket, Cookie, ReadOutcome),
    LogoutFinished(Ticket, Cookie, LogoutOutcome),
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

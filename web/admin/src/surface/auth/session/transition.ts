// 会话事件归约与副作用的派发条件。
// 状态迁移只在这里发生；浏览器适配负责执行返回的副作用。

import { unreachable } from "@auction/web-shared/result";
import { type AdminSession, adminSessionSameIdentity } from "../../../api/session";
import { activityDelay, estimatedExpirationMillis, renewalTimingFromSession } from "../activity";
import { isNewerProjection, sessionCheckDelay, sessionLifetimeMillis } from "./deadlines";
import { type Actions, commitDraft, type Draft, stateDraft } from "./draft";
import {
  type CheckReason,
  type Cookie,
  cookieAgreesWith,
  cookieConflictsWith,
  cookieFromSession,
  cookiesEqual,
  type Effect,
  type Event,
  initialActivity,
  type LogoutOutcome,
  phaseVerified,
  type ReadOutcome,
  type RenewalOutcome,
  type Request,
  requestIsStale,
  type State,
  type Ticket,
  type Timer,
  type Verified,
  verifiedNew,
} from "./index";
import { takeVerified, verifiedMut, withVerified } from "./phase";

const CHECK_RETRY_MILLIS = 30_000;

/**
 * 用单次事件与当前观测推进会话，浏览器适配负责执行返回的动作。
 * Cookie 代际与单调时钟由调用方在事件发生时采集，回执不得重新读取当前值。
 */
export function step(
  state: State,
  event: Event,
  cookie: Cookie,
  now: number,
): readonly [State, readonly Effect[]] {
  const draft = stateDraft(state);
  const actions: Actions = { cookie, now, effects: [] };
  // 浏览器已经切到新代际时，旧代际的续期投影不能跨会话暂存或借用已派发的 POST。
  if (
    draft.bufferedRenewal !== null &&
    cookieConflictsWith(draft.bufferedRenewal.cookie, actions.cookie)
  ) {
    draft.bufferedRenewal = null;
  }
  if (
    draft.renewal !== null &&
    draft.renewal.source !== null &&
    cookieConflictsWith(draft.renewal.source.cookie, actions.cookie)
  ) {
    draft.renewal.source = null;
  }
  switch (event.kind) {
    case "start":
      if (draft.phase.kind === "starting") {
        beginRead(draft, "startup", actions);
      }
      break;
    case "login_finished":
      // 只有当前在途的登录 POST 能推进阶段；广播后重挂的表单不能接管旧回执。
      if (draft.login === event.ticket) {
        draft.login = null;
        if (event.outcome.ok) {
          if (cookieAgreesWith(cookieFromSession(event.outcome.value), actions.cookie)) {
            draft.activity.timing = renewalTimingFromSession(event.outcome.value);
          }
        } else {
          draft.loginError = event.outcome.error;
        }
        // 未知结果不重发 POST；一次 GET 用当前 Cookie 决定是否登出或保留表单。
        beginRead(draft, "login", actions);
      }
      break;
    case "login_input_changed":
      if (draft.phase.kind === "logged_out" && draft.login === null) {
        draft.loginError = null;
      }
      break;
    case "check_requested":
      if (draft.phase.kind !== "logged_out") {
        beginRead(draft, "explicit", actions);
      }
      break;
    case "broadcast":
      hideSession(draft, actions);
      beginRead(draft, "broadcast", actions);
      break;
    case "visible":
      // 已有复核在途时不得叠加；退出期间也不允许 GET 快照恢复授权。
      if (
        draft.check === null &&
        draft.phase.kind !== "logged_out" &&
        draft.phase.kind !== "logging_out"
      ) {
        beginRead(draft, "routine", actions);
      }
      break;
    case "input":
      if (!event.onLogout && draft.phase.kind === "authenticated") {
        draft.activity.latestInput = actions.now;
        scheduleRenewal(draft, actions);
      }
      break;
    case "logout_clicked":
      startLogout(draft, actions);
      break;
    case "timer_elapsed":
      timerElapsed(draft, event.timer, event.ticket, actions);
      break;
    case "read_finished":
      finishRead(draft, event.ticket, event.cookie, event.outcome, actions);
      break;
    case "logout_finished":
      finishLogout(draft, event.ticket, event.cookie, event.outcome, actions);
      break;
    case "renewal_finished":
      finishRenewal(draft, event.ticket, event.cookie, event.outcome, actions);
      break;
    default:
      unreachable(event);
  }
  return [commitDraft(state, draft), actions.effects];
}
/** 标识只在当前外壳内递增；回执必须与派发时的票号一致才被接受。 */
function ticket(draft: Draft): Ticket {
  draft.nextTicket += 1;
  return draft.nextTicket;
}

function request(draft: Draft, actions: Actions): Request {
  return { ticket: ticket(draft), cookie: cookieSnapshot(actions.cookie) };
}

/** 登记一次登录 POST；已有在途登录时不再派发。 */
export function startLogin(
  state: State,
): { readonly state: State; readonly ticket: Ticket } | null {
  if (state.phase.kind !== "logged_out" || state.login !== null) {
    return null;
  }
  const draft = stateDraft(state);
  const loginTicket = ticket(draft);
  draft.login = loginTicket;
  draft.loginError = null;
  return { state: commitDraft(state, draft), ticket: loginTicket };
}

/** 派发时冻结的 Cookie 副本；调用方随后改写原对象不能改变回执使用的代际。 */
type FrozenCookie =
  | { readonly kind: "unavailable" }
  | { readonly kind: "missing" }
  | { readonly kind: "present"; readonly value: string };

function cookieSnapshot(cookie: Cookie): FrozenCookie {
  switch (cookie.kind) {
    case "unavailable":
      return Object.freeze({ kind: "unavailable" });
    case "missing":
      return Object.freeze({ kind: "missing" });
    case "present":
      return Object.freeze({ kind: "present", value: cookie.value });
    default:
      return unreachable(cookie);
  }
}

function timerValue(draft: Draft, timer: Timer): Ticket | null {
  return timer === "check" ? draft.checkTimer : draft.renewalTimer;
}

function setTimer(draft: Draft, timer: Timer, value: Ticket | null): void {
  if (timer === "check") {
    draft.checkTimer = value;
  } else {
    draft.renewalTimer = value;
  }
}

function cancelTimer(draft: Draft, timer: Timer, actions: Actions): void {
  if (timerValue(draft, timer) !== null) {
    setTimer(draft, timer, null);
    actions.effects.push({ kind: "cancel_timer", timer });
  }
}

function armTimer(draft: Draft, timer: Timer, delay: number, actions: Actions): void {
  cancelTimer(draft, timer, actions);
  const armed = ticket(draft);
  setTimer(draft, timer, armed);
  actions.effects.push({ kind: "arm_timer", timer, ticket: armed, delay });
}

function cancelRead(draft: Draft, actions: Actions): void {
  if (draft.check !== null) {
    draft.check = null;
    actions.effects.push({ kind: "cancel_read" });
  }
}

/** 撤下敏感页面；旧宿主的草稿、未决命令和弹窗随宿主失效，旧回执不能重挂同一棵树。 */
function hideSession(draft: Draft, actions: Actions): void {
  if (takeVerified(draft) !== null) {
    draft.viewEpoch += 1;
  }
  draft.activity = initialActivity();
  draft.logoutError = null;
  cancelTimer(draft, "renewal", actions);
}

function beginRead(draft: Draft, reason: CheckReason, actions: Actions): void {
  if (draft.login !== null) {
    // 登录 POST 尚未结清时不发复核；广播只能收起表单，回执后仍归这次 POST。
    if (reason === "broadcast") {
      draft.phase = { kind: "checking", verified: null };
    }
    return;
  }
  if (draft.phase.kind === "logging_out") {
    // DELETE 等待期间不可用 GET 的快照恢复授权，也不能撤销退出回执的所有权。
    draft.phase = {
      kind: "logging_out",
      verified: draft.phase.verified,
      logout: { ...draft.phase.logout, reviewAfter: true },
    };
    return;
  }
  if (
    reason !== "after_renewal" &&
    draft.check !== null &&
    draft.check.reason === reason &&
    cookiesEqual(draft.check.request.cookie, actions.cookie)
  ) {
    return;
  }
  const keepsPermission =
    reason === "routine" &&
    draft.phase.kind === "authenticated" &&
    !cookieConflictsWith(draft.phase.verified.cookie, actions.cookie);
  cancelRead(draft, actions);
  cancelTimer(draft, "check", actions);
  if (!keepsPermission) {
    cancelTimer(draft, "renewal", actions);
    draft.phase =
      reason === "login" && draft.phase.kind === "logged_out"
        ? { kind: "checking_login" }
        : { kind: "checking", verified: takeVerified(draft) };
  }
  const sent = request(draft, actions);
  actions.effects.push({ kind: "read", ticket: sent.ticket });
  draft.check = { reason, request: sent };
}

function rereadCurrentCookie(draft: Draft, actions: Actions): void {
  const verified = phaseVerified(draft.phase);
  if (verified === null || !cookieAgreesWith(verified.cookie, actions.cookie)) {
    hideSession(draft, actions);
  }
  beginRead(draft, "cookie_changed", actions);
}

function finishRead(
  draft: Draft,
  finished: Ticket,
  sent: Cookie,
  outcome: ReadOutcome,
  actions: Actions,
): void {
  const pending = draft.check;
  if (pending === null || pending.request.ticket !== finished) {
    return;
  }
  draft.check = null;
  if (requestIsStale(pending.request, sent, actions.cookie, false)) {
    rereadCurrentCookie(draft, actions);
    return;
  }
  switch (outcome.kind) {
    case "authenticated": {
      const verified = verifiedNew(outcome.session);
      if (
        cookieConflictsWith(verified.cookie, actions.cookie) ||
        cookieConflictsWith(verified.cookie, sent)
      ) {
        rereadCurrentCookie(draft, actions);
      } else {
        const previous = phaseVerified(draft.phase);
        if (previous !== null && !adminSessionSameIdentity(previous.session, verified.session)) {
          endSession(draft, false, actions);
        } else {
          mergeBufferedRenewal(draft, verified, sent, actions);
          confirm(draft, verified, actions);
        }
      }
      break;
    }
    case "unauthenticated":
      endSession(draft, shouldNotifyExpiration(draft, sent, actions), actions);
      break;
    case "failed":
      // 临时故障关闭写门禁，但仍保留已确认宿主的草稿和待对账命令。
      draft.phase = { kind: "unavailable", verified: takeVerified(draft), error: outcome.error };
      cancelTimer(draft, "renewal", actions);
      armTimer(draft, "check", CHECK_RETRY_MILLIS, actions);
      break;
    default:
      unreachable(outcome);
  }
}

function confirm(draft: Draft, verified: Verified, actions: Actions): void {
  const current = phaseVerified(draft.phase);
  let next = verified;
  if (
    current !== null &&
    adminSessionSameIdentity(current.session, next.session) &&
    !isNewerProjection(next.session, current.session)
  ) {
    next = { session: current.session, cookie: next.cookie };
  }
  if (
    draft.renewal !== null &&
    draft.renewal.source !== null &&
    !adminSessionSameIdentity(draft.renewal.source.session, next.session)
  ) {
    draft.renewal.source = null;
  }
  // 当前代际已由服务端确认；重新挂载的表单不再显示上一次登录失败。
  draft.loginError = null;
  draft.phase = { kind: "authenticated", verified: next };
  scheduleCheck(draft, actions);
  scheduleRenewal(draft, actions);
}

/** 广播复核确认同代同身份后并入暂存的续期投影；否则丢弃。 */
function mergeBufferedRenewal(
  draft: Draft,
  verified: Verified,
  sent: Cookie,
  actions: Actions,
): void {
  const pending = draft.bufferedRenewal;
  draft.bufferedRenewal = null;
  if (
    pending === null ||
    !cookieAgreesWith(pending.cookie, sent) ||
    !cookieAgreesWith(pending.cookie, actions.cookie) ||
    !adminSessionSameIdentity(pending.session, verified.session)
  ) {
    return;
  }
  draft.activity.timing = renewalTimingFromSession(pending.session);
  draft.activity.retryNotBefore = 0;
  if (isNewerProjection(pending.session, verified.session)) {
    verified.session = pending.session;
  }
}

function endSession(draft: Draft, notify: boolean, actions: Actions): void {
  hideSession(draft, actions);
  retireRenewal(draft);
  cancelRead(draft, actions);
  cancelTimer(draft, "check", actions);
  draft.phase = { kind: "logged_out" };
  if (notify) {
    actions.effects.push({ kind: "broadcast_ended" });
  }
}

/** 会话已经结束：丢弃暂存投影，并让仍持票的续期回执失去宿主归属。 */
function retireRenewal(draft: Draft): void {
  draft.bufferedRenewal = null;
  if (draft.renewal !== null) {
    draft.renewal.source = null;
  }
}

/** 到达绝对截止时浏览器也会清空 Cookie；持续缺失与读取失败有不同的代际证据。 */
function shouldNotifyExpiration(draft: Draft, sent: Cookie, actions: Actions): boolean {
  return (
    phaseVerified(draft.phase) !== null &&
    sent.kind !== "unavailable" &&
    cookiesEqual(sent, actions.cookie)
  );
}

function startLogout(draft: Draft, actions: Actions): void {
  if (draft.phase.kind === "logging_out") {
    return;
  }
  const wasWritable = draft.phase.kind === "authenticated";
  const verified = takeVerified(draft);
  if (verified === null) {
    return;
  }
  if (cookieConflictsWith(verified.cookie, actions.cookie)) {
    draft.phase = { kind: "checking", verified };
    beginRead(draft, "explicit", actions);
    return;
  }
  cancelRead(draft, actions);
  cancelTimer(draft, "check", actions);
  cancelTimer(draft, "renewal", actions);
  draft.bufferedRenewal = null;
  draft.activity.latestInput = null;
  draft.logoutError = null;
  const sent = request(draft, actions);
  actions.effects.push({ kind: "logout", ticket: sent.ticket, session: verified.session });
  draft.phase = {
    kind: "logging_out",
    verified,
    logout: { request: sent, reviewAfter: false, wasWritable },
  };
}

function finishLogout(
  draft: Draft,
  finished: Ticket,
  sent: Cookie,
  outcome: LogoutOutcome,
  actions: Actions,
): void {
  const phase = draft.phase;
  if (phase.kind !== "logging_out" || phase.logout.request.ticket !== finished) {
    return;
  }
  const success = outcome.kind === "succeeded";
  // 成功 DELETE 的 Set-Cookie 会清空代际；仍须排除派发期间换了实际请求 Cookie。
  const clearedByLogout = success && actions.cookie.kind === "missing";
  const stale = requestIsStale(phase.logout.request, sent, actions.cookie, clearedByLogout);
  const reviewAfter = phase.logout.reviewAfter;
  const wasWritable = phase.logout.wasWritable;
  retireRenewal(draft);
  draft.phase = { kind: "checking", verified: takeVerified(draft) };
  if (stale) {
    rereadCurrentCookie(draft, actions);
    return;
  }
  switch (outcome.kind) {
    case "succeeded":
      endSession(draft, true, actions);
      if (reviewAfter) {
        beginRead(draft, "after_logout", actions);
      }
      break;
    case "rejected":
      if (wasWritable && !reviewAfter && cookieAgreesWith(sent, actions.cookie)) {
        draft.logoutError = outcome.error;
        const verified = takeVerified(draft);
        if (verified !== null) {
          confirm(draft, verified, actions);
        }
      } else {
        draft.logoutError = outcome.error;
        beginRead(draft, "after_logout", actions);
      }
      break;
    case "unknown":
    case "rate_limited":
      draft.logoutError = outcome.error;
      beginRead(draft, "after_logout", actions);
      break;
    default:
      unreachable(outcome);
  }
}

function scheduleCheck(draft: Draft, actions: Actions): void {
  if (draft.phase.kind !== "authenticated" || draft.check !== null) {
    return;
  }
  const session = draft.phase.verified.session;
  const delay = sessionCheckDelay(
    sessionLifetimeMillis(session),
    session.receivedAtMillis,
    actions.now,
  );
  armTimer(draft, "check", delay, actions);
}

function renewalDelay(draft: Draft, now: number): number | null {
  if (draft.phase.kind !== "authenticated") {
    return null;
  }
  return activityDelay(
    draft.activity.timing,
    draft.activity.latestInput,
    now,
    estimatedExpirationMillis(draft.phase.verified.session),
    draft.activity.retryNotBefore,
  );
}

function scheduleRenewal(draft: Draft, actions: Actions): void {
  if (
    draft.phase.kind !== "authenticated" ||
    draft.renewal !== null ||
    draft.renewalTimer !== null
  ) {
    return;
  }
  const delay = renewalDelay(draft, actions.now);
  if (delay !== null) {
    armTimer(draft, "renewal", delay, actions);
  } else {
    draft.activity.latestInput = null;
  }
}

function timerElapsed(draft: Draft, timer: Timer, finished: Ticket, actions: Actions): void {
  if (timerValue(draft, timer) !== finished) {
    return;
  }
  setTimer(draft, timer, null);
  if (timer === "check") {
    beginRead(draft, "routine", actions);
  } else {
    startRenewal(draft, actions);
  }
}

function startRenewal(draft: Draft, actions: Actions): void {
  if (draft.phase.kind !== "authenticated") {
    return;
  }
  const verified = draft.phase.verified;
  if (cookieConflictsWith(verified.cookie, actions.cookie)) {
    beginRead(draft, "explicit", actions);
    return;
  }
  if (renewalDelay(draft, actions.now) !== 0) {
    scheduleRenewal(draft, actions);
    return;
  }
  const session = verified.session;
  // 消耗已派发的活动批次。等待回执期间记录的输入属于下一次派发。
  draft.activity.latestInput = null;
  const sent = request(draft, actions);
  actions.effects.push({ kind: "renew", ticket: sent.ticket, session });
  draft.renewal = { request: sent, source: { session, cookie: verified.cookie } };
}

function finishRenewal(
  draft: Draft,
  finished: Ticket,
  sent: Cookie,
  outcome: RenewalOutcome,
  actions: Actions,
): void {
  const renewal = draft.renewal;
  if (renewal === null || renewal.request.ticket !== finished) {
    return;
  }
  draft.renewal = null;
  const source =
    renewal.source !== null &&
    !cookieConflictsWith(renewal.source.cookie, sent) &&
    !requestIsStale(renewal.request, sent, actions.cookie, false)
      ? renewal.source
      : null;
  if (source === null) {
    if (
      draft.phase.kind === "authenticated" &&
      cookieConflictsWith(draft.phase.verified.cookie, actions.cookie)
    ) {
      rereadCurrentCookie(draft, actions);
    } else {
      scheduleRenewal(draft, actions);
    }
    return;
  }
  switch (outcome.kind) {
    case "succeeded":
      if (adminSessionSameIdentity(source.session, outcome.session)) {
        applyRenewal(draft, outcome.session, sent, actions);
      } else {
        scheduleRenewal(draft, actions);
      }
      break;
    case "unauthenticated":
      if (draft.phase.kind === "logging_out") {
        hideSession(draft, actions);
        beginRead(draft, "after_renewal", actions);
      } else {
        endSession(draft, shouldNotifyExpiration(draft, sent, actions), actions);
      }
      break;
    case "rate_limited": {
      const delay = outcome.retryAfterMillis ?? draft.activity.timing?.intervalMillis ?? 5_000;
      draft.activity.retryNotBefore = f64Max(draft.activity.retryNotBefore, actions.now + delay);
      // 只可按退避下界安排已记录的新输入，已被拒绝的请求没有可重发批次。
      scheduleRenewal(draft, actions);
      break;
    }
    case "csrf_rejected":
    case "unknown":
      draft.activity.latestInput = null;
      // 即使写门禁已关闭，在途 POST 也可能已经提交。只启动一次有预算的 GET。
      beginRead(draft, "after_renewal", actions);
      break;
    case "rejected":
      draft.activity.latestInput = null;
      break;
    default:
      unreachable(outcome);
  }
}

/** Rust `f64::max`：任一参数为 NaN 时取另一个；`Math.max` 会把另一方也变成 NaN。 */
function f64Max(left: number, right: number): number {
  if (Number.isNaN(left)) {
    return right;
  }
  if (Number.isNaN(right)) {
    return left;
  }
  return Math.max(left, right);
}

function applyRenewal(draft: Draft, session: AdminSession, sent: Cookie, actions: Actions): void {
  const projectionCookie = cookieFromSession(session);
  if (
    cookieConflictsWith(projectionCookie, actions.cookie) ||
    cookieConflictsWith(projectionCookie, sent)
  ) {
    rereadCurrentCookie(draft, actions);
    return;
  }
  if (
    (draft.phase.kind === "checking" && draft.phase.verified === null) ||
    (draft.phase.kind === "unavailable" && draft.phase.verified === null)
  ) {
    // 广播复核尚未确认当前代际；只暂存同代同身份的投影，写门禁保持关闭。
    if (
      cookieAgreesWith(projectionCookie, sent) &&
      cookieAgreesWith(projectionCookie, actions.cookie)
    ) {
      draft.bufferedRenewal = { session, cookie: projectionCookie };
    }
    return;
  }
  if (draft.phase.kind === "logging_out") {
    return;
  }
  const verified = verifiedMut(draft.phase);
  if (verified === null || !adminSessionSameIdentity(verified.session, session)) {
    return;
  }
  draft.activity.timing = renewalTimingFromSession(session);
  draft.activity.retryNotBefore = 0;
  const nextSession = isNewerProjection(session, verified.session) ? session : verified.session;
  draft.phase = withVerified(draft.phase, { session: nextSession, cookie: verified.cookie });
  scheduleCheck(draft, actions);
  scheduleRenewal(draft, actions);
}

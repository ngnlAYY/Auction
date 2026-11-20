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
  logoutUnconfirmed,
  phaseVerified,
  type RenewalOutcome,
  requestIsStale,
  type State,
  type Ticket,
  type Timer,
  type Verified,
} from "./index";
import {
  canDecideLogout,
  finishLogout,
  prepareLogout,
  revokeLogoutReadNotice,
  startLogout,
} from "./logout";
import {
  clearPendingEndNotices,
  deferExpiration,
  noticeOrigin,
  rememberPeerEnd,
  retainSettledEndNotice,
  settleEndNotice,
} from "./notices";
import {
  endSession,
  hideSession,
  markIdentityUnconfirmed,
  observeIdentity,
  observeSessionBody,
  takeVerified,
  verifiedMut,
  withVerified,
} from "./phase";
import { finishRead } from "./read-results";
import {
  armTimer,
  cancelRead,
  cancelTimer,
  cookieSnapshot,
  request,
  setTimer,
  ticket,
  timerValue,
} from "./requests";

/**
 * 用单次事件与当前观测推进会话，浏览器适配负责执行返回的动作。
 * 当前 Cookie 先经过身份观测；回执携带的实发代际只用于核对自己的归属。
 */
export function step(
  state: State,
  event: Event,
  cookie: Cookie,
  now: number,
): readonly [State, readonly Effect[]] {
  if (event.kind === "write_review_identity" && event.epoch !== state.viewEpoch) {
    // 旧宿主的回调连当前 Cookie 观测也不能送入新宿主。
    return [state, []];
  }
  if (event.kind === "external_unauthenticated") {
    const current = phaseVerified(state.phase);
    if (
      event.epoch !== state.viewEpoch ||
      current === null ||
      !adminSessionSameIdentity(current.session, event.session)
    ) {
      return [state, []];
    }
  }
  const draft = stateDraft(state);
  const actions: Actions = { cookie, now, effects: [] };
  const keysWereHidden = keysHidden(draft);
  const revision = draft.keyVisibilityRevision;
  const completingRead =
    event.kind === "read_finished" && draft.check?.request.ticket === event.ticket;
  const cookieChanged = observeIdentity(draft, actions);
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
          observeSessionBody(draft, noticeOrigin(draft), event.outcome.value, actions);
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
    case "write_review_identity":
      if (
        observeSessionBody(
          draft,
          phaseVerified(draft.phase)?.session ?? null,
          event.session,
          actions,
        )
      ) {
        beginRead(draft, "explicit", actions);
      }
      break;
    case "external_unauthenticated": {
      const pending = draft.check;
      const stay =
        pending?.reason === "stay_after_logout" &&
        cookiesEqual(pending.request.cookie, actions.cookie);
      // 401 之前发出的 GET 与续期不能恢复授权。新票保留同代「留在后台」的人工决定。
      markIdentityUnconfirmed(draft, false, actions);
      deferExpiration(draft, event.session);
      cancelRead(draft, actions);
      beginRead(draft, stay ? "stay_after_logout" : "explicit", actions);
      break;
    }
    case "broadcast":
      rememberPeerEnd(draft);
      if (draft.check?.reason === "login") {
        beginRead(draft, "login", actions);
      } else if (draft.phase.kind === "logged_out" && draft.login === null) {
        beginRead(draft, "discover", actions);
      } else {
        hideSession(draft, actions);
        beginRead(draft, "broadcast", actions);
      }
      break;
    case "visible":
      beginRead(draft, draft.phase.kind === "logged_out" ? "discover" : "routine", actions);
      break;
    case "input":
      if (!event.onLogout && draft.phase.kind === "authenticated") {
        draft.activity.latestInput = actions.now;
        scheduleRenewal(draft, actions);
      }
      break;
    case "logout_clicked":
      if (!logoutUnconfirmed(draft.phase)) {
        startLogout(draft, false, actions);
      }
      break;
    case "continue_logout":
      if (canDecideLogout(draft)) {
        startLogout(draft, true, actions);
      }
      break;
    case "stay_after_logout":
      if (canDecideLogout(draft)) {
        beginRead(draft, "stay_after_logout", actions);
      }
      break;
    case "timer_elapsed":
      timerElapsed(draft, event.timer, event.ticket, actions);
      break;
    case "read_sending":
      prepareRead(draft, event.ticket, actions);
      break;
    case "read_finished":
      finishRead(draft, event.ticket, event.cookie, event.outcome, actions);
      break;
    case "logout_sending":
      prepareLogout(draft, event.ticket, actions);
      break;
    case "logout_finished":
    case "logout_not_sent":
      finishLogout(
        draft,
        event.ticket,
        event.cookie,
        event.outcome,
        event.kind === "logout_finished",
        actions,
      );
      break;
    case "renewal_sending":
      prepareRenewal(draft, event.ticket, actions);
      break;
    case "renewal_finished":
      finishRenewal(draft, event.ticket, event.cookie, event.outcome, actions);
      break;
    default:
      unreachable(event);
  }
  // 当前读取自行处理权威结论、重读或退避。其他事件即使已过时，观测到换代也要复核。
  if (cookieChanged && draft.identityUnconfirmed && !completingRead) {
    beginRead(draft, "routine", actions);
  }
  if (!keysWereHidden && keysHidden(draft) && draft.keyVisibilityRevision === revision) {
    // 即使界面合并了两次渲染，旧的打开动作也不能在复核成功后重新显示明文。
    draft.keyVisibilityRevision += 1;
  }
  return [commitDraft(state, draft), actions.effects];
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

function keysHidden(draft: Draft): boolean {
  return draft.identityUnconfirmed || logoutUnconfirmed(draft.phase);
}

/** 按本轮已处理的 Cookie 代际合并在途读取，保留本地登录的优先级。 */
export function beginRead(draft: Draft, reason: CheckReason, actions: Actions): void {
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
      logout: {
        ...draft.phase.logout,
        reviewAfter:
          draft.phase.logout.reviewAfter || reason !== "routine" || draft.identityUnconfirmed,
      },
    };
    return;
  }
  if (draft.check !== null) {
    const pending = draft.check;
    if (pending.reason === "login") {
      if (
        actions.cookie.kind === "unavailable" ||
        cookiesEqual(pending.request.cookie, actions.cookie)
      ) {
        return;
      }
      reason = "login";
    }
    if (reason === "routine" && actions.cookie.kind === "unavailable") {
      return;
    }
    if (
      cookiesEqual(pending.request.cookie, actions.cookie) &&
      (reason === "routine" ||
        reason === "discover" ||
        (reason === "explicit" && pending.reason === "stay_after_logout") ||
        (pending.reason === reason && reason !== "after_renewal"))
    ) {
      return;
    }
  }
  const keepsPermission =
    reason === "routine" &&
    draft.phase.kind === "authenticated" &&
    !cookieConflictsWith(draft.phase.verified.cookie, actions.cookie);
  cancelRead(draft, actions);
  cancelTimer(draft, "check", actions);
  if (!keepsPermission && !logoutUnconfirmed(draft.phase) && reason !== "discover") {
    cancelTimer(draft, "renewal", actions);
    draft.phase =
      reason === "login" &&
      (draft.phase.kind === "logged_out" || draft.phase.kind === "checking_login")
        ? { kind: "checking_login" }
        : { kind: "checking", verified: takeVerified(draft) };
  }
  const sent = request(draft, actions);
  actions.effects.push({ kind: "read", ticket: sent.ticket });
  draft.check = {
    reason,
    request: sent,
    origin: noticeOrigin(draft),
    logoutTicket: draft.logoutIntent?.lastSent ?? null,
  };
}

function prepareRead(draft: Draft, ticket: Ticket, actions: Actions): void {
  const pending = draft.check;
  if (pending === null || pending.request.ticket !== ticket) {
    return;
  }
  if (
    actions.cookie.kind !== "unavailable" &&
    !cookiesEqual(pending.request.cookie, actions.cookie)
  ) {
    // 旧票尚未发出，按实际代际合并后续读取。登记时的证据不能用于补播或解开退出锁。
    revokeLogoutReadNotice(draft);
    draft.check = {
      ...pending,
      reason: pending.reason === "stay_after_logout" ? "after_logout" : pending.reason,
      request: { ticket, cookie: cookieSnapshot(actions.cookie) },
      logoutTicket: null,
    };
  }
}

/** 采用同身份的已确认投影并恢复日常复核与续期。 */
export function confirm(draft: Draft, verified: Verified, actions: Actions): void {
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
  draft.logoutIntent = null;
  clearPendingEndNotices(draft);
  retainSettledEndNotice(draft, next.session);
  draft.identityUnconfirmed = false;
  draft.sawOtherGeneration = false;
  draft.phase = { kind: "authenticated", verified: next };
  scheduleCheck(draft, actions);
  scheduleRenewal(draft, actions);
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
  if (renewalDelay(draft, actions.now) !== 0) {
    scheduleRenewal(draft, actions);
    return;
  }
  const session = verified.session;
  // 消耗已派发的活动批次。等待回执期间记录的输入属于下一次派发。
  draft.activity.latestInput = null;
  const sent = request(draft, actions);
  actions.effects.push({ kind: "renew", ticket: sent.ticket, session });
  draft.renewal = {
    request: sent,
    origin: session,
    source: { session, cookie: verified.cookie },
    epoch: draft.viewEpoch,
  };
}

function prepareRenewal(draft: Draft, ticket: Ticket, actions: Actions): void {
  const renewal = draft.renewal;
  if (renewal === null || renewal.request.ticket !== ticket) {
    return;
  }
  const current = draft.phase;
  if (
    current.kind !== "authenticated" ||
    renewal.source === null ||
    !adminSessionSameIdentity(renewal.source.session, current.verified.session) ||
    cookieConflictsWith(renewal.request.cookie, actions.cookie)
  ) {
    draft.renewal = null;
    scheduleRenewal(draft, actions);
  }
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
  const current = phaseVerified(draft.phase);
  if (current !== null && renewal.epoch !== draft.viewEpoch) {
    scheduleRenewal(draft, actions);
    return;
  }
  // Cookie 冲突可以撤销投影资格。当前票和原宿主仍匹配时，正文依然是身份证据。
  const owner = noticeOrigin(draft);
  const origin = owner !== null && adminSessionSameIdentity(renewal.origin, owner) ? owner : null;
  if (outcome.kind === "succeeded" && observeSessionBody(draft, origin, outcome.session, actions)) {
    draft.activity.latestInput = null;
    beginRead(draft, "after_renewal", actions);
    return;
  }
  const requestMatches = !requestIsStale(renewal.request, sent, actions.cookie, false);
  const projectionAllowed =
    requestMatches && renewal.source !== null && !cookieConflictsWith(renewal.source.cookie, sent);
  // 退出限流等迁移会撤销续期投影。原票的可信 401 仍能结束同一 epoch 的会话。
  const ownedExpiration =
    outcome.kind === "unauthenticated" &&
    requestMatches &&
    cookiesEqual(sent, actions.cookie) &&
    sent.kind === "present" &&
    sent.value === renewal.origin.csrfToken &&
    (projectionAllowed ||
      (renewal.epoch === draft.viewEpoch &&
        current !== null &&
        adminSessionSameIdentity(current.session, renewal.origin)));
  if (!projectionAllowed && !ownedExpiration) {
    scheduleRenewal(draft, actions);
    return;
  }
  switch (outcome.kind) {
    case "succeeded":
      if (adminSessionSameIdentity(renewal.origin, outcome.session)) {
        applyRenewal(draft, outcome.session, sent, actions);
      }
      break;
    case "unauthenticated": {
      if (draft.phase.kind === "logging_out") {
        if (ownedExpiration) {
          deferExpiration(draft, renewal.origin);
        }
        hideSession(draft, actions);
        beginRead(draft, "after_renewal", actions);
      } else {
        if (ownedExpiration) {
          settleEndNotice(draft, renewal.origin, actions);
        }
        // 使 401 之前的 GET 票失效，只有新读取可以重新确认写权限。
        endSession(draft, actions);
        if (!projectionAllowed) {
          beginRead(draft, "discover", actions);
        }
      }
      break;
    }
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
    beginRead(draft, "cookie_changed", actions);
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

// 本地退出意图、发送前围栏与结束通知归属。

import { unreachable } from "@auction/web-shared/result";
import { adminSessionSameIdentity } from "../../../api/session";
import type { Actions, Draft } from "./draft";
import {
  type Check,
  type Cookie,
  cookieAgreesWith,
  cookieConflictsWith,
  type Logout,
  type LogoutOutcome,
  phaseVerified,
  requestIsStale,
  requestMatchesGeneration,
  type Ticket,
  type Verified,
} from "./index";
import { settleEndNotice } from "./notices";
import { endSession, retireRenewal, takeVerified } from "./phase";
import { cancelRead, cancelTimer, request } from "./requests";
import { beginRead, confirm } from "./transition";

/** 退出结果存疑时，同一轮只接受一个人工决定。 */
export function canDecideLogout(draft: Draft): boolean {
  return draft.phase.kind === "logout_unconfirmed" && draft.check === null;
}

/** 冻结本次 DELETE 的目标；人工继续时只有原意图的可读代际才能发送。 */
export function startLogout(draft: Draft, continuing: boolean, actions: Actions): void {
  if (draft.phase.kind === "logging_out") {
    return;
  }
  const verified = phaseVerified(draft.phase);
  if (verified === null) {
    if (continuing) {
      beginRead(draft, "after_logout", actions);
    }
    return;
  }
  const intent = draft.logoutIntent;
  const maySend = continuing
    ? intent !== null &&
      adminSessionSameIdentity(intent.target.session, verified.session) &&
      cookieAgreesWith(intent.target.cookie, actions.cookie)
    : !draft.identityUnconfirmed &&
      intent === null &&
      !cookieConflictsWith(verified.cookie, actions.cookie);
  if (!maySend) {
    beginRead(draft, "explicit", actions);
    return;
  }
  const wasWritable = draft.phase.kind === "authenticated";
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
    logout: {
      request: sent,
      target: verified,
      sawOtherGeneration: false,
      reviewAfter: false,
      wasWritable,
      continuing,
    },
  };
}

/** 处理实发前的观测后，只有当前票和原身份仍匹配才能派发 DELETE。 */
export function prepareLogout(draft: Draft, ticket: Ticket, actions: Actions): void {
  const phase = draft.phase;
  if (phase.kind !== "logging_out" || phase.logout.request.ticket !== ticket) {
    return;
  }
  const { logout, verified } = phase;
  const sent = actions.cookie;
  const intent = draft.logoutIntent;
  const maySend = logout.continuing
    ? intent !== null &&
      adminSessionSameIdentity(intent.target.session, logout.target.session) &&
      cookieAgreesWith(intent.target.cookie, sent) &&
      cookieAgreesWith(intent.target.cookie, logout.request.cookie) &&
      intent.lastSent !== ticket
    : !draft.identityUnconfirmed && intent === null;
  if (
    !maySend ||
    verified === null ||
    !adminSessionSameIdentity(verified.session, logout.target.session) ||
    cookieConflictsWith(logout.target.cookie, sent) ||
    cookieConflictsWith(logout.request.cookie, sent)
  ) {
    leaveLogoutRequest(draft, logout.continuing);
    beginRead(draft, "after_logout", actions);
    return;
  }
  // 独立回执归属只看这次实际发送后的代际；先前撤销的 GET 补播资格仍保留。
  draft.phase = { ...phase, logout: { ...logout, sawOtherGeneration: false } };
}

function recordLogoutSent(draft: Draft, logout: Logout, sent: Cookie): void {
  const intent = draft.logoutIntent;
  // 发送前失败保留上次的票，曾撤销的 GET 补播资格不能被新的发送恢复。
  draft.logoutIntent = {
    target: intent?.target ?? logout.target,
    lastSent: logout.request.ticket,
    canInferFromRead:
      !logout.sawOtherGeneration &&
      cookieAgreesWith(logout.request.cookie, sent) &&
      cookieAgreesWith(logout.target.cookie, sent) &&
      intent?.canInferFromRead !== false,
  };
}

function leaveLogoutRequest(draft: Draft, unconfirmed: boolean): void {
  const verified = takeVerified(draft);
  draft.phase = unconfirmed
    ? { kind: "logout_unconfirmed", verified }
    : { kind: "checking", verified };
}

/** 原意图已见异代时撤销 GET 补播资格，同时保留独立 DELETE 回执的归属。 */
export function observeLogoutIdentity(
  draft: Draft,
  conflicts: (target: Verified) => boolean,
): void {
  const phase = draft.phase;
  if (phase.kind === "logging_out" && conflicts(phase.logout.target)) {
    draft.phase = { ...phase, logout: { ...phase.logout, sawOtherGeneration: true } };
  }
  const intent = draft.logoutIntent;
  if (intent !== null && conflicts(intent.target)) {
    revokeLogoutReadNotice(draft);
  }
}

/** 缺少代际证据时只关闭 GET 补播，可信的独立 DELETE 成功仍可消费通知。 */
export function revokeLogoutReadNotice(draft: Draft): void {
  if (draft.logoutIntent?.canInferFromRead === true) {
    draft.logoutIntent = { ...draft.logoutIntent, canInferFromRead: false };
  }
}

/** 用读取登记时的退出票及三个观测点证明通知归属，避免误播到另一会话。 */
export function logoutNoticeFromRead(
  draft: Draft,
  check: Check,
  sent: Cookie,
  observed: Cookie,
): boolean | null {
  const intent = draft.logoutIntent;
  if (intent === null) {
    return null;
  }
  if (
    check.origin === null ||
    !adminSessionSameIdentity(check.origin, intent.target.session) ||
    check.logoutTicket !== intent.lastSent ||
    !requestMatchesGeneration(check.request, sent, observed) ||
    !(cookieAgreesWith(intent.target.cookie, sent) || sent.kind === "missing")
  ) {
    revokeLogoutReadNotice(draft);
    return false;
  }
  return intent.canInferFromRead;
}

/** 完整成功或权威 401 才能结束本地退出，后续拒绝不能解除先前未知的写锁。 */
export function finishLogout(
  draft: Draft,
  ticket: Ticket,
  sent: Cookie,
  outcome: LogoutOutcome,
  dispatched: boolean,
  actions: Actions,
): void {
  const phase = draft.phase;
  if (phase.kind !== "logging_out" || phase.logout.request.ticket !== ticket) {
    return;
  }
  if (dispatched) {
    recordLogoutSent(draft, phase.logout, sent);
  }
  const success = outcome.kind === "succeeded";
  const unknown = outcome.kind === "unknown";
  // DELETE 正文可能读失败而响应头已清 Cookie；只有稳定读取能补齐通知证据。
  const mayHaveClearedCookie = (unknown || success) && actions.cookie.kind === "missing";
  const { continuing, reviewAfter, wasWritable, request, target, sawOtherGeneration } =
    phase.logout;
  const stale =
    requestIsStale(request, sent, actions.cookie, mayHaveClearedCookie) ||
    cookieConflictsWith(target.cookie, sent) ||
    (!mayHaveClearedCookie && cookieConflictsWith(target.cookie, actions.cookie));
  // 已核对的原代发送票收到完整成功且清除了 Cookie，可独立于 GET 补播资格结清通知。
  const clearedByOwnRequest =
    success &&
    actions.cookie.kind === "missing" &&
    requestMatchesGeneration(request, sent, target.cookie);
  const notify =
    (!sawOtherGeneration || clearedByOwnRequest) && draft.logoutIntent?.lastSent === ticket;
  retireRenewal(draft);
  leaveLogoutRequest(draft, continuing || unknown);
  if (stale) {
    revokeLogoutReadNotice(draft);
    beginRead(draft, "cookie_changed", actions);
    return;
  }
  switch (outcome.kind) {
    case "succeeded":
      if (notify) {
        settleEndNotice(draft, target.session, actions);
      }
      endSession(draft, actions);
      if (reviewAfter) {
        beginRead(draft, "after_logout", actions);
      }
      break;
    case "unknown":
      draft.logoutError = outcome.error;
      beginRead(draft, "after_logout", actions);
      break;
    case "rejected":
    case "rate_limited":
      draft.logoutError = outcome.error;
      if (continuing) {
        if (reviewAfter) {
          beginRead(draft, "after_logout", actions);
        }
        break;
      }
      if (
        outcome.kind === "rejected" &&
        wasWritable &&
        !reviewAfter &&
        !draft.identityUnconfirmed &&
        cookieAgreesWith(sent, actions.cookie)
      ) {
        draft.logoutIntent = null;
        const verified = takeVerified(draft);
        if (verified !== null) {
          confirm(draft, verified, actions);
        }
      } else {
        // 已撤销的资格留到复核终结，避免随后把无法归属的 401 当作自然过期。
        if (draft.logoutIntent?.canInferFromRead === true) {
          draft.logoutIntent = null;
        }
        beginRead(draft, "after_logout", actions);
      }
      break;
    default:
      unreachable(outcome);
  }
}

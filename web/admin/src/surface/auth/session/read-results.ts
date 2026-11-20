// 会话读取结果的归属、投影校验与过期通知。

import { unreachable } from "@auction/web-shared/result";
import { adminSessionSameIdentity } from "../../../api/session";
import { renewalTimingFromSession } from "../activity";
import { isNewerProjection } from "./deadlines";
import type { Actions, Draft } from "./draft";
import {
  type Cookie,
  cookieAgreesWith,
  cookieConflictsWith,
  logoutUnconfirmed,
  phaseVerified,
  type ReadOutcome,
  requestIsStale,
  requestMatchesGeneration,
  type Ticket,
  type Verified,
  verifiedNew,
} from "./index";
import { logoutNoticeFromRead, revokeLogoutReadNotice } from "./logout";
import {
  clearPendingEndNotices,
  cookieCanEndSession,
  hasEndEvidence,
  settleEndNotice,
} from "./notices";
import { endSession, observeSessionBody, takeVerified } from "./phase";
import { armTimer, cancelTimer } from "./requests";
import { beginRead, confirm } from "./transition";

const CHECK_RETRY_MILLIS = 30_000;

/** 用当前票的权威读取确认身份。旧票与姓名、代际不符的投影不能恢复授权。 */
export function finishRead(
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
  const needsGeneration = draft.identityUnconfirmed || logoutUnconfirmed(draft.phase);
  if (outcome.kind === "authenticated") {
    // 当前票的正文也是证据。Cookie 变动只决定回执能否采用，不能抹掉已见过的异代。
    observeSessionBody(draft, pending.origin, outcome.session, actions);
  }
  // 本页登录优先于被动探测。探测只可给旧身份留下证据，登录结论仍由专属 GET 决定。
  const discovering = pending.reason === "discover";
  if (discovering && draft.login !== null) {
    return;
  }
  const logoutNotice = logoutNoticeFromRead(draft, pending, sent, actions.cookie);
  if (requestIsStale(pending.request, sent, actions.cookie, false)) {
    if (!discovering) {
      beginRead(draft, pending.reason === "login" ? "login" : "cookie_changed", actions);
    }
    return;
  }
  switch (outcome.kind) {
    case "authenticated": {
      const verified = verifiedNew(outcome.session);
      if (
        cookieConflictsWith(verified.cookie, pending.request.cookie) ||
        cookieConflictsWith(verified.cookie, sent) ||
        cookieConflictsWith(verified.cookie, actions.cookie)
      ) {
        revokeLogoutReadNotice(draft);
        if (!discovering) {
          failRead(draft, "invalid_response", actions);
        }
        return;
      }
      const previous = phaseVerified(draft.phase) ?? draft.logoutIntent?.target ?? null;
      if (needsGeneration && !requestMatchesGeneration(pending.request, sent, actions.cookie)) {
        failRead(draft, "invalid_response", actions);
        return;
      }
      if (previous !== null && !adminSessionSameIdentity(previous.session, verified.session)) {
        clearPendingEndNotices(draft);
        endSession(draft, actions);
        return;
      }
      if (draft.phase.kind === "logout_unconfirmed") {
        draft.identityUnconfirmed = false;
        draft.sawOtherGeneration = false;
        // 只有操作员明确选择留在后台的同代读取能结束退出锁定。
        if (pending.reason !== "stay_after_logout") {
          clearPendingEndNotices(draft);
          if (draft.phase.verified === null) {
            draft.phase = { kind: "logout_unconfirmed", verified };
          }
          return;
        }
        draft.logoutError = null;
      }
      if (discovering) {
        draft.viewEpoch += 1;
      }
      mergeBufferedRenewal(draft, verified, sent, actions);
      confirm(draft, verified, actions);
      break;
    }
    case "unauthenticated": {
      const origin = pending.origin;
      const current = phaseVerified(draft.phase);
      if (
        origin !== null &&
        requestMatchesGeneration(pending.request, sent, actions.cookie) &&
        (logoutNotice === true ||
          hasEndEvidence(draft, origin, sent) ||
          (logoutNotice === null &&
            current !== null &&
            adminSessionSameIdentity(current.session, origin) &&
            cookieCanEndSession(origin, sent, draft.sawOtherGeneration)))
      ) {
        settleEndNotice(draft, origin, actions);
      }
      if (!discovering) {
        endSession(draft, actions);
      }
      break;
    }
    case "failed":
      if (!discovering) {
        failRead(draft, outcome.error, actions);
      }
      break;
    default:
      unreachable(outcome);
  }
}

function failRead(draft: Draft, error: string, actions: Actions): void {
  if (!logoutUnconfirmed(draft.phase)) {
    // 临时故障关闭写门禁，但仍保留已确认宿主的草稿和待对账命令。
    draft.phase = { kind: "unavailable", verified: takeVerified(draft), error };
    cancelTimer(draft, "renewal", actions);
    armTimer(draft, "check", CHECK_RETRY_MILLIS, actions);
  }
}

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

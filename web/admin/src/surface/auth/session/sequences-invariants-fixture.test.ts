// 会话状态机有界穷举的安全不变量，按外壳能观察的视图和副作用断言。

import { expect, test } from "bun:test";
import type { AdminSession } from "../../../api/session";

import {
  type Cookie,
  type Effect,
  type Event,
  type State,
  stateView,
  type Ticket,
  type View,
  viewsEqual,
} from "./index";
import { authenticated, NOW, sessionFixture } from "./session-fixture";

interface StayRead {
  readonly ticket: Ticket;
  readonly cookie: Cookie;
}

/** 退出结果未知后保留原身份，直到有结束证据或同代人工复核。 */
export interface LogoutLock {
  readonly session: Pick<AdminSession, "actor" | "csrfToken">;
  readonly stay: StayRead | null;
}

/** 按回执和已派发的人工票推进影子锁。 */
export function advanceLogoutLock(
  lock: LogoutLock | null,
  previous: State,
  event: Event,
  observed: Cookie,
  effects: readonly Effect[],
  view: View,
): LogoutLock | null {
  if (lock === null) {
    if (
      event.kind !== "logout_finished" ||
      event.outcome.kind !== "unknown" ||
      previous.phase.kind !== "logging_out" ||
      event.ticket !== previous.phase.logout.request.ticket
    ) {
      return null;
    }
    const { actor, csrfToken } = previous.phase.logout.target.session;
    lock = { session: { actor, csrfToken }, stay: null };
  }

  // 可信 DELETE 成功、当前 GET / 续期的可信 401、权威 GET 确认换代都可结束旧会话。
  // 收到广播、观测到异代 Cookie、业务 401 触发复核，都需要等待权威结果。
  if (confirmsEnd(lock, previous, event, observed, view.loggedOut)) return null;
  if (
    event.kind === "read_finished" &&
    event.ticket === lock.stay?.ticket &&
    event.ticket === previous.check?.request.ticket &&
    event.outcome.kind === "authenticated" &&
    sameObservation(lock.stay.cookie, event.cookie) &&
    sameObservation(lock.stay.cookie, observed) &&
    sameIdentity(lock, event.outcome.session)
  ) {
    return null;
  }

  const stay = advanceStayRead(lock.stay, lock.session.csrfToken, event, observed, effects);
  return stay === lock.stay ? lock : { ...lock, stay };
}

function confirmsEnd(
  lock: LogoutLock,
  previous: State,
  event: Event,
  observed: Cookie,
  loggedOut: boolean,
): boolean {
  switch (event.kind) {
    case "logout_finished":
      return (
        loggedOut &&
        previous.phase.kind === "logging_out" &&
        event.ticket === previous.phase.logout.request.ticket &&
        event.outcome.kind === "succeeded"
      );
    case "read_finished": {
      if (event.ticket !== previous.check?.request.ticket) return false;
      if (event.outcome.kind === "unauthenticated") return loggedOut;
      if (event.outcome.kind !== "authenticated") return false;
      const session = event.outcome.session;
      const bodyCookie: Cookie = { kind: "present", value: session.csrfToken };
      // 异代正文只能在实发与收包时的 Cookie 均印证该身份时解除退出锁。
      return (
        !sameIdentity(lock, session) &&
        sameObservation(bodyCookie, event.cookie) &&
        sameObservation(bodyCookie, observed)
      );
    }
    case "renewal_finished":
      return (
        loggedOut &&
        event.ticket === previous.renewal?.request.ticket &&
        event.outcome.kind === "unauthenticated"
      );
    default:
      return false;
  }
}

function sameIdentity(lock: LogoutLock, session: AdminSession): boolean {
  return lock.session.actor === session.actor && lock.session.csrfToken === session.csrfToken;
}

/** 记录读票的替换和作废。可信 401 换票时可保留同代人工选择。 */
function advanceStayRead(
  stay: StayRead | null,
  csrfToken: string,
  event: Event,
  observed: Cookie,
  effects: readonly Effect[],
): StayRead | null {
  const read = effects.find((effect) => effect.kind === "read");
  if (read !== undefined) {
    const explicitlyStayed =
      event.kind === "stay_after_logout" &&
      observed.kind === "present" &&
      observed.value === csrfToken;
    const replacesAfter401 =
      event.kind === "external_unauthenticated" &&
      stay !== null &&
      sameObservation(stay.cookie, observed);
    return explicitlyStayed || replacesAfter401 ? { ticket: read.ticket, cookie: observed } : null;
  }
  if (stay === null) return null;
  if (event.kind === "read_finished" && event.ticket === stay.ticket) return null;
  if (
    event.kind === "read_sending" &&
    event.ticket === stay.ticket &&
    observed.kind !== "unavailable" &&
    !sameObservation(stay.cookie, observed)
  ) {
    return null;
  }
  return effects.some((effect) => effect.kind === "cancel_read") ? null : stay;
}

function sameObservation(left: Cookie, right: Cookie): boolean {
  return left.kind === "present"
    ? right.kind === "present" && left.value === right.value
    : left.kind === right.kind;
}

/** 核对每一步的授权、可见性、在途请求和通知约束，并返回是否发送通知。 */
export function verifyTransition(
  previous: State,
  event: Event,
  observed: Cookie,
  next: State,
  effects: readonly Effect[],
  before: View = stateView(previous),
  view: View = stateView(next),
): boolean {
  let reads = 0;
  let deletes = 0;
  let renewals = 0;
  let notices = 0;
  for (const effect of effects) {
    if (effect.kind === "read") reads += 1;
    if (effect.kind === "logout") deletes += 1;
    if (effect.kind === "renew") renewals += 1;
    if (effect.kind === "broadcast_ended") notices += 1;
  }

  invariant(reads <= 1 && deletes + renewals <= 1 && notices <= 1, "同一步产生重复请求或通知");
  invariant(
    deletes === 0 || event.kind === "logout_clicked" || event.kind === "continue_logout",
    "退出未经人工发起",
  );
  if (view.canWrite) {
    invariant(
      view.session !== null && !view.keyVisibility.hidden && !view.loggingOut,
      "未确认时开写",
    );
    invariant(next.phase.kind === "authenticated", "未认证时开写");
  }
  if (view.keyVisibility.hidden || view.loggingOut) {
    invariant(!view.canWrite && renewals === 0, "遮蔽或退出中开启写入或续期");
  }
  if (next.phase.kind === "logout_unconfirmed" || next.identityUnconfirmed) {
    invariant(view.keyVisibility.hidden, "身份或退出未确认时显示 Key");
  }
  if (event.kind === "renewal_finished" && !before.canWrite) {
    invariant(!view.canWrite, "活动回执恢复了未确认的写许可");
  }
  if (event.kind === "read_finished" && previous.check?.request.ticket !== event.ticket) {
    invariant(before.canWrite || !view.canWrite, "已作废的读取票恢复写入");
    invariant(notices === 0, "已作废的读取票发出通知");
  }
  if (event.kind === "read_finished" && event.outcome.kind === "failed") {
    invariant(notices === 0, "读取失败被当成会话结束");
  }
  if (before.session !== null && view.session !== null && view.epoch === before.epoch) {
    invariant(
      view.session.actor === before.session.actor &&
        view.session.csrfToken === before.session.csrfToken,
      "旧宿主采用了另一身份",
    );
    if (
      event.kind !== "input" &&
      ((observed.kind === "present" && observed.value !== before.session.csrfToken) ||
        observed.kind === "missing")
    ) {
      invariant(view.keyVisibility.hidden && !view.canWrite, "观察到代际变化仍显示 Key 或允许写入");
    }
  }
  if (event.kind === "external_unauthenticated") {
    if (event.epoch !== before.epoch) {
      invariant(viewsEqual(before, view) && effects.length === 0, "旧宿主的失效事件触发新操作");
    } else if (before.session !== null) {
      invariant(!view.canWrite && view.keyVisibility.hidden, "当前宿主收到失效证据后仍开放");
    }
  }
  if (event.kind === "input" && event.onLogout) {
    invariant(
      next.activity.latestInput === previous.activity.latestInput && effects.length === 0,
      "退出控件输入被用作续期活动",
    );
  }
  if (event.kind === "broadcast") {
    invariant(!view.canWrite && notices === 0, "收到对端通知时打开写入或回播");
  }
  return notices === 1;
}

function invariant(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

test("穷举断言夹具会拒绝注销时改变活动时间的伪造后态", () => {
  const session = sessionFixture("fixture-cookie", 60);
  const state = authenticated(session);
  const event = { kind: "input", onLogout: true } as const;
  expect(() => verifyTransition(state, event, session.cookie, state, [])).not.toThrow();
  const invalid = { ...state, activity: { ...state.activity, latestInput: NOW + 1 } };
  expect(() => verifyTransition(state, event, session.cookie, invalid, [])).toThrow();
});

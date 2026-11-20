// 会话状态机有界穷举的状态不变量核对；不构造事件，只判断迁移前后关系与副作用。

import { expect, test } from "bun:test";

import type { AdminSession } from "../../../api/session";
import { type Cookie, type Effect, type Event, type Phase, type State, stateView } from "./index";
import { authenticated, NOW, sessionFixture } from "./session-fixture";

interface EffectCounts {
  reads: number;
  writes: number;
  notices: number;
  renewalDelay: number | null;
}

function activeLogout(state: State) {
  return state.phase.kind === "logging_out" ? state.phase.logout.request : null;
}

function verifiedFor(phase: Phase) {
  return "verified" in phase ? phase.verified : null;
}

function cookiesMatch(left: Cookie, right: Cookie): boolean {
  return left.kind === "present"
    ? right.kind === "present" && left.value === right.value
    : left.kind === right.kind;
}

function sameIdentity(left: AdminSession, right: AdminSession): boolean {
  return left.actor === right.actor && left.csrfToken === right.csrfToken;
}

/** 核对一次状态迁移的全部不变量与副作用。 */
export function verifyTransition(
  previous: State,
  event: Event,
  observed: Cookie,
  next: State,
  effects: readonly Effect[],
  now: number,
): void {
  const count: EffectCounts = { reads: 0, writes: 0, notices: 0, renewalDelay: null };
  for (const effect of effects) {
    switch (effect.kind) {
      case "read":
        count.reads += 1;
        break;
      case "logout":
      case "renew":
        count.writes += 1;
        break;
      case "broadcast_ended":
        count.notices += 1;
        break;
      case "arm_timer":
        if (effect.timer === "renewal") {
          expect(count.renewalDelay).toBeNull();
          count.renewalDelay = effect.delay;
        }
        break;
      default:
        break;
    }
  }
  const view = stateView(next);
  expect(view.canWrite).toBe(next.phase.kind === "authenticated");
  expect(view.session !== null).toBe(verifiedFor(next.phase) !== null);
  expect(count.reads <= 1 && count.writes <= 1 && count.notices <= 1).toBe(true);
  expect(next.renewal === null || next.renewalTimer === null).toBe(true);
  if (next.check !== null) {
    expect(
      next.phase.kind === "checking" ||
        next.phase.kind === "checking_login" ||
        (next.check.reason === "routine" && view.canWrite),
    ).toBe(true);
  }
  if (view.loggingOut) {
    expect(next.check).toBeNull();
    expect(next.checkTimer).toBeNull();
    expect(next.renewalTimer).toBeNull();
  }
  if (event.kind === "input" && event.onLogout) {
    expect(next.activity.latestInput).toBe(previous.activity.latestInput);
    expect(effects).toHaveLength(0);
  }
  if (event.kind === "broadcast") {
    expect(view.canWrite).toBe(false);
    expect(view.session).toBeNull();
    expect(count.notices).toBe(0);
  }
  if (next.bufferedRenewal !== null) {
    expect(!view.canWrite && view.session === null).toBe(true);
    expect(cookiesMatch(next.bufferedRenewal.cookie, observed)).toBe(true);
    expect(
      (next.phase.kind === "checking" && next.phase.verified === null) ||
        (next.phase.kind === "unavailable" && next.phase.verified === null),
    ).toBe(true);
  }
  if (previous.login !== null) {
    expect(count.writes).toBe(0);
    expect(count.notices).toBe(0);
    expect(!view.canWrite && view.session === null).toBe(true);
    if (event.kind === "login_finished" && event.ticket === previous.login) {
      expect(next.login).toBeNull();
      expect(count.reads).toBe(1);
      expect(next.check?.reason).toBe("login");
      expect(next.check !== null && cookiesMatch(next.check.request.cookie, observed)).toBe(true);
    } else {
      expect(next.login).toBe(previous.login);
      expect(count.reads).toBe(0);
    }
  }

  const previousLogout = previous.phase.kind === "logging_out" ? previous.phase.logout : null;
  if (
    previousLogout !== null &&
    !(event.kind === "logout_finished" && event.ticket === previousLogout.request.ticket)
  ) {
    expect(view.loggingOut && !view.canWrite).toBe(true);
    expect(activeLogout(next)?.ticket).toBe(previousLogout.request.ticket);
    expect([count.reads, count.writes, count.notices]).toEqual([0, 0, 0]);
    if (event.kind === "broadcast") {
      expect(next.phase.kind).toBe("logging_out");
      if (next.phase.kind === "logging_out") {
        expect(next.phase.logout.reviewAfter).toBe(true);
      }
    }
  }

  if (
    event.kind === "read_finished" &&
    previous.check !== null &&
    previous.check.request.ticket === event.ticket
  ) {
    if (!cookiesMatch(event.cookie, observed)) {
      verifyReplacedGeneration(next, observed, count);
    } else if (event.outcome.kind === "authenticated") {
      if (
        event.cookie.kind !== "present" ||
        event.cookie.value !== event.outcome.session.csrfToken
      ) {
        verifyReplacedGeneration(next, observed, count);
      } else {
        verifyProjection(previous, next, event.outcome.session, true);
        expect(next.bufferedRenewal).toBeNull();
      }
    } else if (event.outcome.kind === "unauthenticated") {
      expect(view.loggedOut && view.session === null).toBe(true);
      expect(next.bufferedRenewal).toBeNull();
      expect(count.notices).toBe(verifiedFor(previous.phase) !== null ? 1 : 0);
    }
  } else if (event.kind === "logout_finished" && activeLogout(previous)?.ticket === event.ticket) {
    const cleared = event.outcome.kind === "succeeded" && observed.kind === "missing";
    if (!cookiesMatch(event.cookie, observed) && !cleared) {
      verifyReplacedGeneration(next, observed, count);
    } else if (event.outcome.kind === "succeeded") {
      const deferred = previousLogout?.reviewAfter === true;
      expect(!view.loggingOut && !view.canWrite && view.session === null).toBe(true);
      expect(count.notices).toBe(1);
      expect(count.reads).toBe(deferred ? 1 : 0);
    } else if (event.outcome.kind === "unknown" || event.outcome.kind === "rate_limited") {
      expect(!view.loggingOut && !view.canWrite).toBe(true);
      expect(count.notices).toBe(0);
      expect(count.reads).toBe(1);
    }
  } else if (
    event.kind === "renewal_finished" &&
    previous.renewal !== null &&
    previous.renewal.request.ticket === event.ticket
  ) {
    expect(next.renewal).toBeNull();
    const source = previous.renewal.source;
    const belongs =
      source !== null &&
      cookiesMatch(source.cookie, event.cookie) &&
      cookiesMatch(event.cookie, observed);
    if (!belongs) {
      if (
        previous.phase.kind === "authenticated" &&
        !cookiesMatch(previous.phase.verified.cookie, observed)
      ) {
        verifyReplacedGeneration(next, observed, count);
      } else {
        verifyRetiredRenewal(previous, next, count, now);
      }
    } else {
      switch (event.outcome.kind) {
        case "unknown":
        case "csrf_rejected":
          expect(count.writes).toBe(0);
          expect(count.reads).toBe(view.loggingOut ? 0 : 1);
          expect(count.renewalDelay).toBeNull();
          expect(view.canWrite).toBe(false);
          if (view.loggingOut) {
            expect(next.phase.kind).toBe("logging_out");
            if (next.phase.kind === "logging_out") {
              expect(next.phase.logout.reviewAfter).toBe(true);
            }
          }
          break;
        case "unauthenticated":
          expect(view.session === null && !view.canWrite).toBe(true);
          expect(next.bufferedRenewal).toBeNull();
          break;
        case "rate_limited":
          verifyRateLimit(previous, next, count, event.outcome.retryAfterMillis, now);
          break;
        case "succeeded":
          if (!view.loggingOut && verifiedFor(previous.phase) !== null) {
            verifyProjection(previous, next, event.outcome.session, false);
          } else if (
            (previous.phase.kind === "checking" && previous.phase.verified === null) ||
            (previous.phase.kind === "unavailable" && previous.phase.verified === null)
          ) {
            expect(next.bufferedRenewal?.session).toBe(event.outcome.session);
            expect([count.reads, count.writes, count.notices]).toEqual([0, 0, 0]);
          }
          break;
        default:
          break;
      }
    }
  }
}

function verifyRetiredRenewal(
  previous: State,
  next: State,
  count: EffectCounts,
  now: number,
): void {
  expect([count.writes, count.reads, count.notices]).toEqual([0, 0, 0]);
  expect(next.phase.kind).toBe(previous.phase.kind);
  expect(stateView(next)).toEqual(stateView(previous));
  expect(next.check?.request.ticket).toBe(previous.check?.request.ticket);
  expect(next.checkTimer).toBe(previous.checkTimer);
  expect(next.activity.timing).toEqual(previous.activity.timing);
  expect(next.activity.retryNotBefore).toBe(previous.activity.retryNotBefore);
  expect(count.renewalDelay).toBe(expectedRetiredRenewalDelay(previous, now));
}

function expectedRetiredRenewalDelay(previous: State, now: number): number | null {
  const { timing, latestInput, retryNotBefore } = previous.activity;
  if (previous.phase.kind !== "authenticated" || latestInput === null) {
    return null;
  }
  // 用退役前的输入、活动节奏和服务端投影独立核对新定时器。
  const deferred = latestInput < retryNotBefore;
  const scheduled = deferred
    ? retryNotBefore
    : timing === null
      ? now
      : timing.sinceMillis + timing.intervalMillis;
  const delay = Math.ceil(Math.max(0, scheduled - now));
  const due = now + delay;
  return due < expirationMillis(previous.phase.verified.session) &&
    (deferred || due - latestInput <= 60_000)
    ? delay
    : null;
}

function verifyProjection(
  previous: State,
  next: State,
  received: AdminSession,
  fromRead: boolean,
): void {
  let expected: AdminSession = received;
  const current = verifiedFor(previous.phase);
  if (current !== null) {
    if (!sameIdentity(current.session, received)) {
      return;
    }
    expected = laterProjection(expected, current.session);
  }
  if (
    fromRead &&
    previous.bufferedRenewal !== null &&
    sameIdentity(previous.bufferedRenewal.session, received)
  ) {
    expected = laterProjection(previous.bufferedRenewal.session, expected);
  }
  const verified = verifiedFor(next.phase);
  expect(verified).not.toBeNull();
  expect(verified?.session).toBe(expected);
}

function earlierExpiration(session: AdminSession): bigint {
  let earliest = session.absoluteExpiresAt;
  if (session.expiresAt < earliest) {
    earliest = session.expiresAt;
  }
  return earliest;
}

function expirationMillis(session: AdminSession): number {
  return (
    session.receivedAtMillis +
    Number((earlierExpiration(session) - session.serverTime) / 1_000_000n)
  );
}

function laterProjection(first: AdminSession, second: AdminSession): AdminSession {
  const key = (session: AdminSession): readonly [bigint, bigint] => [
    earlierExpiration(session),
    session.serverTime,
  ];
  const left = key(first);
  const right = key(second);
  return left[0] > right[0] || (left[0] === right[0] && left[1] > right[1]) ? first : second;
}

function verifyReplacedGeneration(next: State, observed: Cookie, count: EffectCounts): void {
  const view = stateView(next);
  expect([count.writes, count.notices]).toEqual([0, 0]);
  expect(view.canWrite).toBe(false);
  const verified = verifiedFor(next.phase);
  if (verified !== null) {
    expect(cookiesMatch(verified.cookie, observed)).toBe(true);
  }
  if (view.loggingOut) {
    expect(next.phase.kind).toBe("logging_out");
    if (next.phase.kind === "logging_out") {
      expect(next.phase.logout.reviewAfter).toBe(true);
    }
  } else {
    expect(next.check?.reason).toBe("cookie_changed");
    expect(next.check !== null && cookiesMatch(next.check.request.cookie, observed)).toBe(true);
    expect(count.reads).toBe(1);
  }
}

function verifyRateLimit(
  previous: State,
  next: State,
  count: EffectCounts,
  retryAfter: number | null,
  now: number,
): void {
  expect([count.writes, count.notices, count.reads]).toEqual([0, 0, 0]);
  const delay = retryAfter ?? previous.activity.timing?.intervalMillis ?? 5_000;
  const earliest = Math.max(previous.activity.retryNotBefore, now + delay);
  expect(next.activity.retryNotBefore >= earliest).toBe(true);
  if (next.phase.kind !== "authenticated") {
    expect(next.activity.latestInput).toBe(previous.activity.latestInput);
    expect(count.renewalDelay).toBeNull();
    return;
  }
  const expiration = expirationMillis(next.phase.verified.session);
  const delayMillis = Math.ceil(earliest - now);
  const retained =
    previous.activity.latestInput !== null && now + delayMillis < expiration
      ? previous.activity.latestInput
      : null;
  expect(count.renewalDelay).toBe(retained === null ? null : delayMillis);
  expect(next.activity.latestInput).toBe(retained);
}

test("穷举断言夹具会拒绝注销时改变活动时间的伪造后态", () => {
  const session = sessionFixture("fixture-cookie", 60);
  const state = authenticated(session);
  const event = { kind: "input", onLogout: true } as const;
  expect(() => verifyTransition(state, event, session.cookie, state, [], NOW)).not.toThrow();
  const invalid = { ...state, activity: { ...state.activity, latestInput: NOW + 1 } };
  expect(() => verifyTransition(state, event, session.cookie, invalid, [], NOW)).toThrow();
});

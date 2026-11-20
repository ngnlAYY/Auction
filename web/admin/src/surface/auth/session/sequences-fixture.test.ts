// 会话状态机的有界穷举：每条事件序列都保持写许可、代际与在途请求的不变量。

import { expect, test } from "bun:test";

import {
  type Cookie,
  cookiesEqual,
  type Event,
  type LogoutOutcome,
  phaseVerified,
  type ReadOutcome,
  type RenewalOutcome,
  type Request,
  type State,
  type Ticket,
} from "./index";
import { verifyTransition } from "./sequences-invariants-fixture.test";
import {
  apply,
  authenticated,
  loginAfterOldSessionExpires,
  NOW,
  projectionAfter,
  readTicket,
  type SessionFixture,
  sessionFixture,
  startRenewal,
} from "./session-fixture";
import { startLogin, step } from "./transition";

interface Projections {
  readonly initial: SessionFixture;
  readonly extended: SessionFixture;
}

interface Cases {
  readonly current: Projections;
  readonly previous: Projections;
}

function projectionsFor(token: string): Projections {
  const initial = sessionFixture(token, 60);
  return { initial, extended: projectionAfter(initial, 1, 600, NOW) };
}

/** 执行到指定长度的全部事件序列，并核对序列总数与每一项状态不变量。 */
export function runExhaustive(maxLength: number): void {
  const cases: Cases = {
    current: projectionsFor("current-cookie"),
    previous: projectionsFor("previous-cookie"),
  };
  const seeds = initialStates(cases);
  const eventCount = EVENTS.reduce((count, kind) => count + (hasResult(kind) ? 2 : 1), 0);
  expect(seeds).toHaveLength(14);
  expect(eventCount).toBe(47);
  let count = 0;
  for (const seed of seeds) {
    count += visit(cases, seed, 0, maxLength);
  }
  let expected = 0;
  for (let length = 0; length <= maxLength; length += 1) {
    expected += eventCount ** length;
  }
  expected *= seeds.length;
  expect(count).toBe(expected);
}

function initialStates(cases: Cases): readonly State[] {
  const current = cases.current.initial;
  const initial: State = {
    phase: { kind: "starting" },
    check: null,
    bufferedRenewal: null,
    renewal: null,
    login: null,
    loginError: null,
    activity: { timing: null, latestInput: null, retryNotBefore: 0 },
    checkTimer: null,
    renewalTimer: null,
    nextTicket: 0,
    viewEpoch: 0,
    logoutError: null,
  };
  const [startingCheck, startEffects] = apply(current, initial, { kind: "start" });
  const [anonymous] = apply(current, startingCheck, {
    kind: "read_finished",
    ticket: readTicket(startEffects),
    cookie: current.cookie,
    outcome: { kind: "unauthenticated" },
  });
  const signedIn = authenticated(current);
  const [routine, routineEffects] = apply(current, signedIn, { kind: "visible" });
  const [unavailable] = apply(current, routine, {
    kind: "read_finished",
    ticket: readTicket(routineEffects),
    cookie: current.cookie,
    outcome: { kind: "failed", error: "storage_unavailable" },
  });
  const [explicit] = apply(current, signedIn, { kind: "check_requested" });
  const [loggingOut] = apply(current, signedIn, { kind: "logout_clicked" });
  const [logoutWithBroadcast] = apply(current, loggingOut, { kind: "broadcast" });
  const [renewing] = startRenewal(current, signedIn);
  const [logoutWithRenewal] = apply(current, renewing, { kind: "logout_clicked" });
  const [renewingWithInput] = apply(current, renewing, { kind: "input", onLogout: false });
  const loggingIn = startLogin(anonymous);
  if (loggingIn === null) throw new Error("匿名状态应能开始登录");
  const [loginWithBroadcast] = step(
    loggingIn.state,
    { kind: "broadcast" },
    { kind: "missing" },
    NOW,
  );
  const [loginWithOldRenewal] = loginAfterOldSessionExpires(cases.previous.initial);
  return [
    initial,
    startingCheck,
    anonymous,
    signedIn,
    routine,
    unavailable,
    explicit,
    loggingOut,
    logoutWithBroadcast,
    logoutWithRenewal,
    renewingWithInput,
    loggingIn.state,
    loginWithBroadcast,
    loginWithOldRenewal,
  ];
}

const EVENTS = [
  "start",
  "login_finished",
  "explicit",
  "broadcast",
  "broadcast_after_cookie_clear",
  "visible",
  "input_continue",
  "input_logout",
  "exit",
  "timer_check",
  "timer_renewal",
  "read_ok_initial",
  "read_ok_extended",
  "read_unauthorized",
  "read_failed",
  "logout_ok",
  "logout_cleared",
  "logout_rejected",
  "logout_unknown",
  "logout_limited",
  "renewal_ok_initial",
  "renewal_ok_extended",
  "renewal_unauthorized",
  "renewal_limited_10k",
  "renewal_limited_120k",
  "renewal_limited_none",
  "renewal_csrf",
  "renewal_unknown",
  "renewal_rejected",
] as const;

type Kind = (typeof EVENTS)[number];

function hasResult(kind: Kind): boolean {
  return (
    kind === "login_finished" ||
    kind.startsWith("read_") ||
    (kind.startsWith("logout_") && kind !== "logout_cleared") ||
    kind.startsWith("renewal_")
  );
}

function visit(cases: Cases, state: State, depth: number, maxLength: number): number {
  if (depth === maxLength) {
    return 1;
  }
  const now = NOW + depth * 100;
  let count = 1;
  for (const kind of EVENTS) {
    for (const old of hasResult(kind) ? [false, true] : [false]) {
      const [event, observed] = eventFor(cases, state, kind, old);
      const [next, effects] = step(state, event, observed, now);
      try {
        verifyTransition(state, event, observed, next, effects, now);
      } catch (error) {
        throw new Error(
          `深度 ${depth} 事件 ${kind} old=${old} observed=${JSON.stringify(observed)} 前态=${JSON.stringify(state, (_, value) => (typeof value === "bigint" ? `${value}n` : value))} 后态=${JSON.stringify(next, (_, value) => (typeof value === "bigint" ? `${value}n` : value))}\n原因=${error instanceof Error ? (error.stack ?? error.message) : String(error)}`,
          { cause: error },
        );
      }
      count += visit(cases, next, depth + 1, maxLength);
    }
  }
  return count;
}

function eventFor(cases: Cases, state: State, kind: Kind, old: boolean): readonly [Event, Cookie] {
  const request = requestFor(state, kind);
  const cookie =
    request?.cookie ?? phaseVerified(state.phase)?.cookie ?? cases.current.initial.cookie;
  const fromPrevious = cookiesEqual(cookie, cases.previous.initial.cookie);
  const fixture = fromPrevious ? cases.previous : cases.current;
  const replacement = fromPrevious ? cases.current : cases.previous;
  let observed: Cookie;
  if (kind === "broadcast_after_cookie_clear" || kind === "logout_cleared") {
    observed = { kind: "missing" };
  } else if (kind === "login_finished" && old) {
    observed = { kind: "missing" };
  } else if (old) {
    observed = replacement.initial.cookie;
  } else {
    observed = cookie;
  }
  const ticket = request?.ticket ?? 0;
  let event: Event;
  switch (kind) {
    case "start":
      event = { kind: "start" };
      break;
    case "login_finished":
      event = {
        kind: "login_finished",
        ticket: state.login ?? 0,
        outcome: old
          ? { ok: false, error: "request_timeout" }
          : { ok: true, value: fixture.initial.session },
      };
      break;
    case "explicit":
      event = { kind: "check_requested" };
      break;
    case "broadcast":
    case "broadcast_after_cookie_clear":
      event = { kind: "broadcast" };
      break;
    case "visible":
      event = { kind: "visible" };
      break;
    case "input_continue":
      event = { kind: "input", onLogout: false };
      break;
    case "input_logout":
      event = { kind: "input", onLogout: true };
      break;
    case "exit":
      event = { kind: "logout_clicked" };
      break;
    case "timer_check":
      event = { kind: "timer_elapsed", timer: "check", ticket: state.checkTimer ?? 0 };
      break;
    case "timer_renewal":
      event = { kind: "timer_elapsed", timer: "renewal", ticket: state.renewalTimer ?? 0 };
      break;
    case "read_ok_initial":
      event = readEvent(ticket, cookie, {
        kind: "authenticated",
        session: fixture.initial.session,
      });
      break;
    case "read_ok_extended":
      event = readEvent(ticket, cookie, {
        kind: "authenticated",
        session: fixture.extended.session,
      });
      break;
    case "read_unauthorized":
      event = readEvent(ticket, cookie, { kind: "unauthenticated" });
      break;
    case "read_failed":
      event = readEvent(ticket, cookie, { kind: "failed", error: "storage_unavailable" });
      break;
    case "logout_ok":
      event = logoutEvent(ticket, cookie, { kind: "succeeded" });
      break;
    case "logout_cleared":
      event = logoutEvent(ticket, cookie, { kind: "succeeded" });
      break;
    case "logout_rejected":
      event = logoutEvent(ticket, cookie, { kind: "rejected", error: "origin_forbidden" });
      break;
    case "logout_unknown":
      event = logoutEvent(ticket, cookie, { kind: "unknown", error: "request_timeout" });
      break;
    case "logout_limited":
      event = logoutEvent(ticket, cookie, { kind: "rate_limited", error: "rate_limited" });
      break;
    case "renewal_ok_initial":
      event = renewalEvent(ticket, cookie, { kind: "succeeded", session: fixture.initial.session });
      break;
    case "renewal_ok_extended":
      event = renewalEvent(ticket, cookie, {
        kind: "succeeded",
        session: fixture.extended.session,
      });
      break;
    case "renewal_unauthorized":
      event = renewalEvent(ticket, cookie, { kind: "unauthenticated" });
      break;
    case "renewal_limited_10k":
      event = renewalEvent(ticket, cookie, { kind: "rate_limited", retryAfterMillis: 10_000 });
      break;
    case "renewal_limited_120k":
      event = renewalEvent(ticket, cookie, { kind: "rate_limited", retryAfterMillis: 120_000 });
      break;
    case "renewal_limited_none":
      event = renewalEvent(ticket, cookie, { kind: "rate_limited", retryAfterMillis: null });
      break;
    case "renewal_csrf":
      event = renewalEvent(ticket, cookie, { kind: "csrf_rejected" });
      break;
    case "renewal_unknown":
      event = renewalEvent(ticket, cookie, { kind: "unknown" });
      break;
    case "renewal_rejected":
      event = renewalEvent(ticket, cookie, { kind: "rejected" });
      break;
  }
  return [event, observed];
}

function requestFor(state: State, kind: Kind): Request | null {
  if (kind.startsWith("read_")) {
    return state.check?.request ?? null;
  }
  if (kind.startsWith("logout_")) {
    return activeLogout(state);
  }
  if (kind.startsWith("renewal_")) {
    return state.renewal?.request ?? null;
  }
  return null;
}

function readEvent(ticket: Ticket, cookie: Cookie, outcome: ReadOutcome): Event {
  return { kind: "read_finished", ticket, cookie, outcome };
}

function logoutEvent(ticket: Ticket, cookie: Cookie, outcome: LogoutOutcome): Event {
  return { kind: "logout_finished", ticket, cookie, outcome };
}

function renewalEvent(ticket: Ticket, cookie: Cookie, outcome: RenewalOutcome): Event {
  return { kind: "renewal_finished", ticket, cookie, outcome };
}

function activeLogout(state: State): Request | null {
  return state.phase.kind === "logging_out" ? state.phase.logout.request : null;
}

test("穷举夹具从单个种子生成空序列及全部一阶事件变体", () => {
  const cases = {
    current: projectionsFor("current-cookie"),
    previous: projectionsFor("previous-cookie"),
  };
  const state = authenticated(cases.current.initial);
  expect(visit(cases, state, 0, 0)).toBe(1);
  expect(visit(cases, state, 0, 1)).toBe(48);
});

// 会话状态机的有界穷举：每条事件序列都保持写许可、代际与在途请求的不变量。

import { expect, test } from "bun:test";

import {
  type Cookie,
  cookiesEqual,
  type Effect,
  type Event,
  initialState,
  type LogoutOutcome,
  phaseVerified,
  type ReadOutcome,
  type RenewalOutcome,
  type Request,
  type State,
  stateView,
  type Ticket,
} from "./index";
import {
  advanceLogoutLock,
  type LogoutLock,
  verifyTransition,
} from "./sequences-invariants-fixture.test";
import {
  apply,
  authenticated,
  loginAfterOldSessionExpires,
  logoutTicket,
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
export function runExhaustive(maxLength: number, boundaryOnly = false): number {
  const cases: Cases = {
    current: projectionsFor("current-cookie"),
    previous: projectionsFor("previous-cookie"),
  };
  const starting = initialStates(cases);
  const seeds = boundaryOnly ? starting.competing : starting.all;
  const kinds = boundaryOnly ? BOUNDARY_EVENTS : EVENTS;
  const eventCount = eventsPerState(kinds);
  let frontier: Frontier = new Map();
  for (const { state, lock } of seeds) {
    const identity = stateView(state).session;
    addPrefix(frontier, state, { identity: identity?.csrfToken ?? null, ended: false }, lock, 1);
  }
  let count = seeds.length;
  for (let depth = 0; depth < maxLength; depth += 1) {
    const next: Frontier = new Map();
    for (const { state, notice, lock, paths } of frontier.values()) {
      const before = stateView(state);
      for (const [event, observed] of eventsFor(cases, state, kinds)) {
        const [following, effects] = step(state, event, observed, NOW + depth * 100);
        const view = following === state ? before : stateView(following);
        const nextLock = advanceLogoutLock(lock, state, event, observed, effects, view);
        if (nextLock !== null && view.canWrite) {
          const ticket = event.kind === "read_finished" ? event.ticket : "无";
          throw new Error(
            `退出结果仍未知却恢复写入：第 ${depth + 1} 步 ${state.phase.kind} → ${event.kind}，票 ${ticket}，人工票 ${lock?.stay?.ticket ?? "无"}`,
          );
        }
        const notified = verifyTransition(state, event, observed, following, effects, before, view);
        const identity = view.session?.csrfToken ?? notice.identity;
        const ended = identity === notice.identity && notice.ended;
        if (ended && notified) {
          throw new Error(`同一身份重复发出结束通知：第 ${depth + 1} 步 ${event.kind}`);
        }
        count += paths;
        if (depth + 1 < maxLength) {
          addPrefix(next, following, { identity, ended: ended || notified }, nextLock, paths);
        }
      }
    }
    frontier = next;
  }
  let expected = 0;
  for (let length = 0; length <= maxLength; length += 1) {
    expected += seeds.length * eventCount ** length;
  }
  expect(count).toBe(expected);
  return count;
}

interface NoticeHistory {
  readonly identity: string | null;
  readonly ended: boolean;
}

interface Prefix {
  readonly state: State;
  readonly lock: LogoutLock | null;
}

type Frontier = Map<string, Prefix & { notice: NoticeHistory; paths: number }>;

function addPrefix(
  frontier: Frontier,
  state: State,
  notice: NoticeHistory,
  lock: LogoutLock | null,
  paths: number,
): void {
  // 同深度、同生产状态、同通知历史及影子锁的前缀才可合并。
  const key = JSON.stringify([state, notice, lock], (_, value) =>
    typeof value === "bigint" ? `${value}n` : value,
  );
  const existing = frontier.get(key);
  if (existing === undefined) {
    frontier.set(key, { state, notice, lock, paths });
  } else {
    existing.paths += paths;
  }
}

function initialStates(cases: Cases): { all: readonly Prefix[]; competing: readonly Prefix[] } {
  const current = cases.current.initial;
  const initial = initialState();
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
  const [startingLogout, logoutEffects] = apply(current, signedIn, { kind: "logout_clicked" });
  const logout = logoutTicket(logoutEffects);
  const [loggingOut] = apply(current, startingLogout, { kind: "logout_sending", ticket: logout });
  function logoutStep(prefix: Prefix, event: Event): readonly [Prefix, readonly Effect[]] {
    const [state, effects] = apply(current, prefix.state, event);
    const lock = advanceLogoutLock(
      prefix.lock,
      prefix.state,
      event,
      current.cookie,
      effects,
      stateView(state),
    );
    return [{ state, lock }, effects];
  }
  const [unconfirmed, review] = logoutStep(
    { state: loggingOut, lock: null },
    logoutEvent(logout, current.cookie, { kind: "unknown", error: "request_timeout" }),
  );
  const reviewTicket = readTicket(review);
  const [failedReview] = logoutStep(
    unconfirmed,
    readEvent(reviewTicket, current.cookie, { kind: "failed", error: "storage_unavailable" }),
  );
  const [unconfirmedIdle] = logoutStep(
    unconfirmed,
    readEvent(reviewTicket, current.cookie, {
      kind: "authenticated",
      session: current.session,
    }),
  );
  const [staying] = logoutStep(unconfirmedIdle, { kind: "stay_after_logout" });
  function seed(state: State): Prefix {
    return { state, lock: null };
  }
  const [discovering] = apply(current, anonymous, { kind: "visible" });
  const [logoutWithBroadcast] = apply(current, loggingOut, { kind: "broadcast" });
  const [renewing] = startRenewal(current, signedIn);
  const [logoutStarted, logoutStartedEffects] = apply(current, renewing, {
    kind: "logout_clicked",
  });
  const withRenewal = logoutTicket(logoutStartedEffects);
  const [logoutWithRenewal] = apply(current, logoutStarted, {
    kind: "logout_sending",
    ticket: withRenewal,
  });
  const [rateLimited] = apply(
    current,
    logoutWithRenewal,
    logoutEvent(withRenewal, current.cookie, { kind: "rate_limited", error: "rate_limited" }),
  );
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
  return {
    all: [
      ...[
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
        discovering,
      ].map(seed),
      unconfirmed,
      unconfirmedIdle,
      staying,
      failedReview,
    ],
    competing: [seed(logoutWithRenewal), seed(rateLimited), unconfirmed, staying, failedReview],
  };
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
  "continue_logout",
  "stay_after_logout",
  "read_sending",
  "logout_sending",
  "renewal_sending",
  "external_unauthenticated",
  "old_external_unauthenticated",
  "timer_check",
  "timer_renewal",
  "read_ok_initial",
  "read_ok_extended",
  "read_other_body",
  "read_retired_ok",
  "read_unauthorized",
  "read_failed",
  "logout_ok",
  "logout_cleared",
  "logout_rejected",
  "logout_not_sent",
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

const BOUNDARY_EVENTS: readonly Kind[] = [
  "visible",
  "explicit",
  "broadcast_after_cookie_clear",
  "exit",
  "continue_logout",
  "stay_after_logout",
  "read_sending",
  "logout_sending",
  "renewal_sending",
  "external_unauthenticated",
  "old_external_unauthenticated",
  "timer_check",
  "read_other_body",
  "read_ok_initial",
  "read_retired_ok",
  "read_unauthorized",
  "read_failed",
  "logout_cleared",
  "logout_unknown",
  "logout_limited",
  "renewal_ok_initial",
  "renewal_unauthorized",
];

function hasResult(kind: Kind): boolean {
  return (
    kind === "login_finished" ||
    (kind.startsWith("read_") && kind !== "read_retired_ok") ||
    (kind.startsWith("logout_") && kind !== "logout_cleared") ||
    kind.startsWith("renewal_")
  );
}

function eventsPerState(kinds: readonly Kind[]): number {
  return kinds.reduce((count, kind) => count + (hasResult(kind) ? 2 : 1), 0);
}

function* eventsFor(
  cases: Cases,
  state: State,
  kinds: readonly Kind[],
): Iterable<readonly [Event, Cookie]> {
  for (const kind of kinds) {
    yield eventFor(cases, state, kind, false);
    if (hasResult(kind)) {
      yield eventFor(cases, state, kind, true);
    }
  }
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
    case "continue_logout":
    case "stay_after_logout":
    case "read_sending":
    case "logout_sending":
    case "renewal_sending":
      event = { kind, ticket };
      break;
    case "external_unauthenticated":
    case "old_external_unauthenticated":
      event = {
        kind: "external_unauthenticated",
        epoch: state.viewEpoch - (kind === "old_external_unauthenticated" ? 1 : 0),
        session: phaseVerified(state.phase)?.session ?? fixture.initial.session,
      };
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
    case "read_other_body":
      event = readEvent(ticket, cookie, {
        kind: "authenticated",
        session: replacement.initial.session,
      });
      break;
    case "read_retired_ok":
      event = readEvent(ticket > 0 ? ticket - 1 : 0, cookie, {
        kind: "authenticated",
        session: fixture.initial.session,
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
    case "logout_not_sent":
      event = {
        kind: "logout_not_sent",
        ticket,
        cookie,
        outcome: { kind: "rejected", error: "invalid_request" },
      };
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

test("穷举夹具保留全部起点及一阶事件变体", () => {
  const cases = {
    current: projectionsFor("current-cookie"),
    previous: projectionsFor("previous-cookie"),
  };
  const state = authenticated(cases.current.initial);
  expect([...eventsFor(cases, state, EVENTS)]).toHaveLength(62);
  expect(initialStates(cases).all).toHaveLength(19);
  expect(initialStates(cases).competing).toHaveLength(5);
});

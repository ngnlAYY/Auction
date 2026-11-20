// 会话投影合并：迟到的读取与续期按截止和观察时刻保留较新一份。

import { expect, test } from "bun:test";

import { type State, stateView, type Ticket } from "./index";
import {
  apply,
  armedTimer,
  authenticated,
  cloneState,
  logoutTicket,
  NOW,
  projectionAfter,
  readOk,
  readTicket,
  type SessionFixture,
  sessionFixture,
  startRenewal,
  startRenewalAt,
} from "./session-fixture";
import { step } from "./transition";

test("late_get_keeps_renewed_deadline_and_its_clock：迟到的 GET 保留续期截止与其时钟", () => {
  const current = sessionFixture("current-cookie", 60);
  // GET 先读取旧截止，但其响应取时可能晚于续期响应的服务端时刻。
  const readSnapshot = projectionAfter(current, 57, 3, 59_000);
  const renewed = projectionAfter(current, 56, 60, 57_000);
  const [checking, effects] = apply(current, authenticated(current), { kind: "visible" }, 55_000);
  const check = readTicket(effects);
  const [renewing, renewal] = startRenewalAt(current, checking, 56_000);

  const [afterRenewal] = apply(
    current,
    renewing,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "succeeded", session: renewed.session },
    },
    57_000,
  );
  const [merged, mergeEffects] = apply(current, afterRenewal, readOk(readSnapshot, check), 59_000);
  expect(stateView(merged).session).toBe(renewed.session);
  expect(armedTimer(mergeEffects, "check")[1]).toBe(57_000);

  const [input, inputEffects] = apply(current, merged, { kind: "input", onLogout: false }, 59_200);
  const [timer, delay] = armedTimer(inputEffects, "renewal");
  expect(delay).toBe(17_800);
  const [, dueEffects] = apply(
    current,
    input,
    { kind: "timer_elapsed", timer: "renewal", ticket: timer },
    77_000,
  );
  expect(dueEffects).toHaveLength(1);
  const renew = dueEffects[0];
  if (renew?.kind !== "renew") {
    throw new Error("应只派发一次续期");
  }
  expect(renew.session).toBe(renewed.session);
});

test("broadcast_read_keeps_same_generation_renewal_and_its_clock：广播读取保留同代续期与其时钟", () => {
  const current = sessionFixture("current-cookie", 60);
  const readSnapshot = projectionAfter(current, 57, 3, 59_000);
  const renewed = projectionAfter(current, 56, 60, 57_000);
  const [renewing, renewal] = startRenewalAt(current, authenticated(current), 55_000);
  const [hidden, effects] = apply(current, renewing, { kind: "broadcast" }, 56_000);
  const check = readTicket(effects);
  const [buffered, renewalEffects] = apply(
    current,
    hidden,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "succeeded", session: renewed.session },
    },
    57_000,
  );
  expect(stateView(buffered).session).toBeNull();
  expect(stateView(buffered).canWrite).toBe(false);
  expect(renewalEffects).toHaveLength(0);

  const [merged, readEffects] = apply(current, buffered, readOk(readSnapshot, check), 59_000);
  expect(stateView(merged).canWrite).toBe(true);
  expect(stateView(merged).session).toBe(renewed.session);
  expect(armedTimer(readEffects, "check")[1]).toBe(57_000);
  const [, inputEffects] = apply(current, merged, { kind: "input", onLogout: false }, 59_200);
  expect(armedTimer(inputEffects, "renewal")[1]).toBe(17_800);
});

function broadcastWithBufferedRenewal(): readonly [SessionFixture, State, Ticket] {
  const current = sessionFixture("current-cookie", 60);
  const renewed = projectionAfter(current, 1, 600, NOW);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [hidden, effects] = apply(current, renewing, { kind: "broadcast" });
  const check = readTicket(effects);
  const [buffered] = apply(current, hidden, {
    kind: "renewal_finished",
    ticket: renewal,
    cookie: current.cookie,
    outcome: { kind: "succeeded", session: renewed.session },
  });
  expect(buffered.bufferedRenewal).not.toBeNull();
  return [current, buffered, check];
}

test("broadcast_read_discards_renewal_for_replaced_cookie_or_identity：广播读取对替换的 Cookie 或身份丢弃续期", () => {
  const [current, state, check] = broadcastWithBufferedRenewal();
  const replacement = sessionFixture("new-cookie", 60);
  const [next, effects] = apply(replacement, state, readOk(current, check));
  expect(stateView(next).canWrite).toBe(false);
  expect(next.bufferedRenewal).toBeNull();
  const [confirmed] = apply(replacement, next, readOk(replacement, readTicket(effects)));
  expect(stateView(confirmed).session).toBe(replacement.session);

  const differentActor = {
    ...current.session,
    actor: "different-operator",
  };
  const sameCookie: SessionFixture = { session: differentActor, cookie: current.cookie };
  const [sameGeneration] = apply(sameCookie, cloneState(state), readOk(sameCookie, check));
  expect(sameGeneration.bufferedRenewal).toBeNull();
  expect(stateView(sameGeneration).session).toBe(sameCookie.session);
});

test("broadcast_read_discards_renewal_when_cookie_is_unavailable_or_anonymous：Cookie 不可读或匿名时广播读取丢弃续期", () => {
  const [current, state, check] = broadcastWithBufferedRenewal();
  const [unreadable] = step(
    cloneState(state),
    readOk(current, check),
    { kind: "unavailable" },
    NOW,
  );
  expect(unreadable.bufferedRenewal).toBeNull();
  expect(stateView(unreadable).session).toBe(current.session);

  const [anonymous, effects] = apply(current, cloneState(state), {
    kind: "read_finished",
    ticket: check,
    cookie: current.cookie,
    outcome: { kind: "unauthenticated" },
  });
  expect(effects).toHaveLength(0);
  expect(anonymous.bufferedRenewal).toBeNull();
  expect(stateView(anonymous).loggedOut).toBe(true);
  expect(stateView(anonymous).canWrite).toBe(false);
});

test("broadcast_read_failure_keeps_renewal_hidden_until_a_newer_read_confirms：广播读取失败让续期保持隐藏直到更新读取确认", () => {
  const [current, state, check] = broadcastWithBufferedRenewal();
  const [failed] = apply(current, state, {
    kind: "read_finished",
    ticket: check,
    cookie: current.cookie,
    outcome: { kind: "failed", error: "storage_unavailable" },
  });
  expect(failed.bufferedRenewal).not.toBeNull();
  expect(stateView(failed).session).toBeNull();
  expect(stateView(failed).canWrite).toBe(false);

  const [refreshing, effects] = apply(current, failed, { kind: "check_requested" });
  const newer = projectionAfter(current, 2, 700, NOW + 10);
  const [confirmed] = apply(current, refreshing, readOk(newer, readTicket(effects)), NOW + 10);
  expect(stateView(confirmed).canWrite).toBe(true);
  expect(stateView(confirmed).session).toBe(newer.session);
  expect(confirmed.bufferedRenewal).toBeNull();
});

test("renewal_after_logout_cannot_extend_the_next_session_review：退出后的续期不能延长下一次会话复核", () => {
  const current = sessionFixture("current-cookie", 60);
  const renewed = projectionAfter(current, 1, 600, NOW);
  const [renewing, renewal] = startRenewal(current, authenticated(current));
  const [loggingOut, logoutEffects] = apply(current, renewing, { kind: "logout_clicked" });
  const logout = logoutTicket(logoutEffects);
  const [hidden] = apply(current, loggingOut, { kind: "broadcast" });
  const [limited, reviewEffects] = apply(current, hidden, {
    kind: "logout_finished",
    ticket: logout,
    cookie: current.cookie,
    outcome: { kind: "rate_limited", error: "rate_limited" },
  });
  const check = readTicket(reviewEffects);
  const [late, lateEffects] = apply(current, limited, {
    kind: "renewal_finished",
    ticket: renewal,
    cookie: current.cookie,
    outcome: { kind: "succeeded", session: renewed.session },
  });
  expect(lateEffects).toHaveLength(0);
  expect(late.bufferedRenewal).toBeNull();
  expect(stateView(late).canWrite).toBe(false);
  const [reviewed] = apply(current, late, readOk(current, check));
  expect(stateView(reviewed).session).toBe(current.session);
});

test("late_renewal_keeps_a_later_get_deadline_and_its_clock：迟到的续期保留更晚的 GET 截止与其时钟", () => {
  const current = sessionFixture("current-cookie", 60);
  const renewed = projectionAfter(current, 10, 60, 13_000);
  const readSnapshot = projectionAfter(current, 11, 65, 12_000);
  const [renewing, renewal] = startRenewalAt(current, authenticated(current), 10_000);
  const [checking, effects] = apply(current, renewing, { kind: "visible" }, 11_000);
  const [read] = apply(current, checking, readOk(readSnapshot, readTicket(effects)), 12_000);
  const [merged, mergeEffects] = apply(
    current,
    read,
    {
      kind: "renewal_finished",
      ticket: renewal,
      cookie: current.cookie,
      outcome: { kind: "succeeded", session: renewed.session },
    },
    13_000,
  );
  expect(stateView(merged).session).toBe(readSnapshot.session);
  expect(armedTimer(mergeEffects, "check")[1]).toBe(63_000);
});

test("equal_deadlines_keep_the_later_server_observation_and_its_clock：相同截止保留较晚的服务端观察与其时钟", () => {
  const current = sessionFixture("current-cookie", 60);
  const newer = projectionAfter(current, 10, 50, 11_000);
  const older = projectionAfter(current, 9, 51, 14_000);
  const [checking, effects] = apply(current, authenticated(current), { kind: "visible" }, 10_000);
  const [first] = apply(current, checking, readOk(newer, readTicket(effects)), 11_000);
  expect(stateView(first).session).toBe(newer.session);

  const [second, secondEffects] = apply(current, first, { kind: "visible" }, 12_000);
  const [merged, mergeEffects] = apply(
    current,
    second,
    readOk(older, readTicket(secondEffects)),
    14_000,
  );
  expect(stateView(merged).session).toBe(newer.session);
  expect(armedTimer(mergeEffects, "check")[1]).toBe(46_000);
});

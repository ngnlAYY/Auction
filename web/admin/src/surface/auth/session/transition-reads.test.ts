// 会话状态机的读取代际：例行与显式读取只为已确认 Cookie 保留写入。

import { expect, test } from "bun:test";

import { type Cookie, stateView } from "./index";
import {
  apply,
  authenticated,
  countNotices,
  NOW,
  readOk,
  readTicket,
  sessionFixture,
} from "./session-fixture";
import { step } from "./transition";

test("routine_and_explicit_reads_only_keep_writes_for_the_confirmed_cookie：例行与显式读取只为已确认 Cookie 保留写入", () => {
  const current = sessionFixture("current-cookie", 900);
  const other = sessionFixture("other-cookie", 900);
  const [visible] = apply(current, authenticated(current), { kind: "visible" });
  expect(stateView(visible).canWrite).toBe(true);
  const [explicit, effects] = apply(current, visible, { kind: "check_requested" });
  expect(stateView(explicit).canWrite).toBe(false);
  expect(effects.some((effect) => effect.kind === "cancel_read")).toBe(true);
  const [confirmed] = apply(current, explicit, readOk(current, readTicket(effects)));
  expect(stateView(confirmed).canWrite).toBe(true);

  const [changed, changedEffects] = apply(other, confirmed, { kind: "visible" });
  expect(stateView(changed).canWrite).toBe(false);
  const [anonymous, anonymousEffects] = apply(
    other,
    changed,
    readOk(other, readTicket(changedEffects)),
  );
  expect(stateView(anonymous).loggedOut).toBe(true);
  expect(stateView(anonymous).session).toBeNull();
  expect(countNotices(anonymousEffects)).toBe(0);
});

test("confirmed_expiration_unmounts_and_broadcasts_only_a_verified_generation：确认过期撤下页面且只广播已核实的代际", () => {
  const current = sessionFixture("current-cookie", 900);
  const cases: Array<[Cookie, number]> = [
    [current.cookie, 1],
    [{ kind: "missing" }, 1],
    [{ kind: "unavailable" }, 0],
  ];
  for (const [cookie, expectedNotices] of cases) {
    const [checking, effects] = step(authenticated(current), { kind: "visible" }, cookie, NOW);
    const [refreshed, settledEffects] = step(
      checking,
      {
        kind: "read_finished",
        ticket: readTicket(effects),
        cookie,
        outcome: { kind: "unauthenticated" },
      },
      cookie,
      NOW,
    );
    expect(refreshed.phase.kind).toBe("logged_out");
    expect(stateView(refreshed).canWrite).toBe(false);
    expect(stateView(refreshed).session).toBeNull();
    expect(countNotices(settledEffects)).toBe(expectedNotices);
  }
});

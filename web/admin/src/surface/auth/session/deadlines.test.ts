// Admin 会话复核：服务端剩余时长与单调时钟增量。

import { expect, test } from "bun:test";

import type { AdminSession } from "../../../api/session";
import { sessionCheckDelay, sessionLifetimeMillis } from "./deadlines";

function session(actor: string, csrfToken: string): AdminSession {
  const serverTime = BigInt(Date.UTC(2026, 10, 14, 0, 0, 0)) * 1_000_000n;
  const second = 1_000_000_000n;
  return {
    actor,
    csrfToken,
    csrf_token: csrfToken,
    expiresAt: serverTime + 15n * 60n * second,
    absoluteExpiresAt: serverTime + 8n * 3_600n * second,
    serverTime,
    receivedAtMillis: 0,
  };
}

test("session_checks_use_server_duration_and_elapsed_monotonic_time：复核使用服务端时长与单调时钟增量", () => {
  const baseline = session("ops", "csrf-a");
  const lifetime = sessionLifetimeMillis(baseline);
  expect(lifetime).toBe(900_000);
  for (const received of [1, Number(baseline.serverTime / 1_000_000n) + 86_400_000]) {
    expect(sessionCheckDelay(lifetime, received, received + 250)).toBe(898_750);
  }
});

test("short_or_elapsed_session_deadlines_wait_past_cutoff：短或已过截止的会话等到截止之后再查", () => {
  const current = session("ops", "csrf-a");
  const short: AdminSession = {
    ...current,
    absoluteExpiresAt: current.serverTime + 3_000_000_000n,
  };
  const lifetime = sessionLifetimeMillis(short);
  expect(lifetime).toBe(3_000);
  expect(sessionCheckDelay(lifetime, 100, 100)).toBe(2_000);
  expect(sessionCheckDelay(lifetime, 100, 2_100)).toBe(2_000);
  for (const remaining of [0, -50]) {
    expect(sessionCheckDelay(remaining, 100, 100)).toBe(2_000);
  }
});

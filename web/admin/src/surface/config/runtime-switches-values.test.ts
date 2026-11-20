// 用数据库时间核对外部访问暂停的意图与到期重读。

import { describe, expect, test } from "bun:test";
import type { RuntimeSwitchListResponse } from "@auction/contracts/dto/admin/runtime-switches";

import { pauseRefreshMillis, pauseState } from "./runtime-switches-values";

function snapshot(
  operation: string,
  expiresAt: string | null,
  databaseTime: string,
): RuntimeSwitchListResponse {
  return {
    database_time: databaseTime,
    database_intent: [
      {
        name: "external_access_paused",
        revision: 7,
        operation,
        value: operation === "set" ? true : null,
        expires_at: expiresAt,
        actor_id: "operator",
        entrypoint: "cli",
        reason: "drill",
      },
    ],
    admin_last_success_age_seconds: 1,
    admin_applied: [],
  };
}

describe("runtime switch values", () => {
  test("indefinite_pause_stays_paused_and_offers_resume", () => {
    const now = "2026-11-19T00:00:00Z";
    expect(pauseState(snapshot("set", null, now))).toEqual({
      revision: 7,
      paused: true,
      pausedUntil: null,
    });
  });

  test("timed_pause_is_active_until_database_time_reaches_expiry", () => {
    const now = "2026-11-19T00:00:00Z";
    const until = "2026-11-19T02:00:00Z";
    const active = pauseState(snapshot("set", until, now));
    expect(active).toEqual({
      revision: 7,
      paused: true,
      pausedUntil: 1_795_053_600_000_000_000n,
    });
    expect(pauseState(snapshot("set", "2026-11-18T23:59:59Z", now))).toEqual({
      revision: 7,
      paused: false,
      pausedUntil: null,
    });
  });

  test("cleared_or_missing_switch_is_not_paused", () => {
    const now = "2026-11-19T00:00:00Z";
    expect(pauseState(snapshot("clear", null, now))).toEqual({
      revision: 7,
      paused: false,
      pausedUntil: null,
    });
    const empty: RuntimeSwitchListResponse = {
      database_time: now,
      database_intent: [],
      admin_last_success_age_seconds: null,
      admin_applied: [],
    };
    expect(pauseState(empty)).toEqual({ revision: 0, paused: false, pausedUntil: null });
  });

  test("timed_pause_refresh_uses_database_time_and_rounds_up", () => {
    const databaseTime = "2023-11-14T22:13:20Z";
    const until = "2023-11-14T22:13:20.010300Z";
    expect(pauseRefreshMillis(snapshot("set", until, databaseTime))).toBe(11);
    expect(pauseRefreshMillis(snapshot("set", until, until))).toBeNull();
  });

  test("indefinite_or_cleared_pause_never_schedules_expiry_refresh", () => {
    const now = "2023-11-14T22:13:20Z";
    expect(pauseRefreshMillis(snapshot("set", null, now))).toBeNull();
    expect(pauseRefreshMillis(snapshot("clear", null, now))).toBeNull();
  });
});

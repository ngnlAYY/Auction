// Admin 人工活动续期：合并间隔、截止边界与限流退避。

import { expect, test } from "bun:test";

import { activityDelay, estimatedExpirationMillis, renewalTimingFromSession } from "./activity";

const SERVER_TIME = BigInt(Date.UTC(2026, 10, 18, 0, 0, 0)) * 1_000_000n;
const SECOND_NANOS = 1_000_000_000n;

/** 与 Rust activity 测试助手一致：服务端 2026-11-18、单调接收基准 100。 */
function activitySession(idleSeconds: number, absoluteSeconds: number) {
  return {
    actor: "operator",
    csrfToken: "test-csrf",
    csrf_token: "test-csrf",
    expiresAt: SERVER_TIME + BigInt(idleSeconds) * SECOND_NANOS,
    absoluteExpiresAt: SERVER_TIME + BigInt(absoluteSeconds) * SECOND_NANOS,
    serverTime: SERVER_TIME,
    receivedAtMillis: 100,
  };
}

test("interval_uses_one_third_of_the_earliest_deadline_with_bounds：间隔取最早截止的三分之一并受上下界约束", () => {
  // Rust 同时直接断言 `renewal_interval_millis` 的三个输入；这里用同样的会话时长经公开入口取得。
  expect(renewalTimingFromSession(activitySession(60, 3_600)).intervalMillis).toBe(20_000);
  expect(renewalTimingFromSession(activitySession(900, 3_600)).intervalMillis).toBe(60_000);
  expect(renewalTimingFromSession(activitySession(6, 3_600)).intervalMillis).toBe(5_000);
  expect(renewalTimingFromSession(activitySession(900, 30)).intervalMillis).toBe(10_000);
});

test("one_early_input_schedules_a_renewal_before_each_deadline：一次输入在截止前安排一次续期", () => {
  for (const [idle, absolute, interval] of [
    [60, 3_600, 20_000],
    [900, 3_600, 60_000],
    [900, 30, 10_000],
  ] as const) {
    const session = activitySession(idle, absolute);
    const timing = renewalTimingFromSession(session);
    const expiresAt = estimatedExpirationMillis(session);
    const input = session.receivedAtMillis + 1_000;
    const delay = activityDelay(timing, input, input, expiresAt, 0);
    expect(delay).toBe(interval - 1_000);
    if (delay === null) throw new Error("应安排续期");
    const dueAt = input + delay;
    expect(dueAt < expiresAt).toBe(true);
    expect(activityDelay(timing, input, dueAt, expiresAt, 0)).toBe(0);
    expect(activityDelay(timing, null, dueAt, expiresAt, 0)).toBeNull();
    expect(activityDelay(timing, input, expiresAt, expiresAt, 0)).toBeNull();
  }
});

test("interval_floor_cannot_queue_a_renewal_at_the_deadline：最小间隔不能在截止时刻排队续期", () => {
  const session = activitySession(900, 5);
  const timing = renewalTimingFromSession(session);
  const expiresAt = estimatedExpirationMillis(session);
  const input = session.receivedAtMillis + 1_000;
  expect(activityDelay(timing, input, input, expiresAt, 0)).toBeNull();
});

test("continuous_input_keeps_one_renewal_at_each_minute_boundary：连续输入在每个分钟边界只续期一次", () => {
  const timing = { sinceMillis: 1_000, intervalMillis: 60_000 };
  const expiresAt = 900_000;
  expect(activityDelay(timing, 2_000, 2_000, expiresAt, 0)).toBe(59_000);
  expect(activityDelay(timing, 35_000, 35_000, expiresAt, 0)).toBe(26_000);
  expect(activityDelay(timing, 60_000, 61_000, expiresAt, 0)).toBe(0);
  const renewed = { sinceMillis: 61_000, intervalMillis: 60_000 };
  expect(activityDelay(renewed, 62_000, 62_000, expiresAt, 0)).toBe(59_000);
});

test("activity_can_renew_once_then_idle_and_stale_input_cannot_renew：可续期一次后闲置，陈旧输入不能续期", () => {
  const expiresAt = 900_000;
  const timing = { sinceMillis: 0, intervalMillis: 60_000 };
  expect(activityDelay(null, 100, 100, expiresAt, 0)).toBe(0);
  expect(activityDelay(null, null, 100, expiresAt, 0)).toBeNull();
  expect(activityDelay(timing, 75_000, 75_000, expiresAt, 0)).toBe(0);
  expect(activityDelay(timing, null, 135_000, expiresAt, 0)).toBeNull();
  expect(activityDelay(timing, 500, 65_000, expiresAt, 0)).toBeNull();
});

test("rate_limited_activity_waits_for_retry_after_and_new_input：限流活动等待重试时间与新输入", () => {
  const timing = { sinceMillis: 1_000, intervalMillis: 5_000 };
  expect(activityDelay(timing, null, 9_000, 90_000, 12_000)).toBeNull();
  expect(activityDelay(timing, 9_000, 9_000, 90_000, 12_000)).toBe(3_000);
  expect(activityDelay(timing, 9_000, 12_000, 90_000, 12_000)).toBe(0);
  expect(activityDelay(timing, 9_000, 9_000, 12_000, 12_000)).toBeNull();
});

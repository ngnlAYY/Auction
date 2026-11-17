// 竞价合并刷新判定：未消费修订号只排定一次，窗口内合并，抖动范围有界。

import { expect, test } from "bun:test";
import {
  BID_REFRESH_COALESCE_MILLIS,
  BID_REFRESH_JITTER_MILLIS,
  type BidRefreshDecision,
  coalesceWindowMillis,
  planBidRefresh,
} from "./refresh";

/** 用“窗口从首条通知开始、不因后续通知延长”的同一规则模拟一个资源的重读次数。
 * 只建模合并策略本身，并按基础窗口计时；抖动只推迟截止时刻，边界另测。 */
function simulateRereads(notificationMillis: readonly number[]): number {
  let consumed = 0;
  let scheduled = false;
  let deadline = 0;
  let rereads = 0;
  let revision = 0;

  for (const now of notificationMillis) {
    // 计时器在截止时刻完成：先消费当时已到达的最新修订号并计数一次重读。
    if (scheduled && now >= deadline) {
      scheduled = false;
      rereads += 1;
      consumed = revision;
    }
    revision += 1;
    const decision: BidRefreshDecision = planBidRefresh(revision, consumed, scheduled, true);
    if (decision !== "idle" && decision !== "wait_for_ready" && decision !== "coalescing") {
      scheduled = true;
      deadline = now + BID_REFRESH_COALESCE_MILLIS;
    }
  }
  if (scheduled) {
    rereads += 1;
  }
  return rereads;
}

test("first_notification_schedules_exactly_once：首次通知只为当前修订号排定一次重读", () => {
  expect(planBidRefresh(7, 6, false, true)).toEqual({ kind: "schedule", revision: 7 });
});

test("consumed_revision_does_not_read_again：已消费的修订号不会重复重读", () => {
  expect(planBidRefresh(7, 7, false, true)).toBe("idle");
  // 即使上一窗口仍在等待，也不重复排定同一修订号。
  expect(planBidRefresh(7, 7, true, true)).toBe("idle");
});

test("notification_during_window_coalesces_without_extending：窗口内通知合并且不延长截止", () => {
  expect(planBidRefresh(8, 7, true, true)).toBe("coalescing");
});

test("not_ready_resource_waits_and_keeps_revision：资源未就绪时等待且保留修订号", () => {
  expect(planBidRefresh(8, 7, false, false)).toBe("wait_for_ready");
  // 就绪后同一修订号仍会排定，不因等待期间的状态变化丢失读取。
  expect(planBidRefresh(8, 7, false, true)).toEqual({ kind: "schedule", revision: 8 });
});

test("burst_inside_single_window_reads_once：单个窗口内的突发通知只重读一次", () => {
  expect(simulateRereads([0, 10, 20, 30, 40])).toBe(1);
});

test("window_is_not_extended_by_trailing_notifications：尾随通知不会延长窗口", () => {
  // 首条 0ms 的窗口在 100ms 截止；99ms 的通知合入，150ms 的通知另起窗口。
  expect(simulateRereads([0, 99, 150])).toBe(2);
});

test("sustained_bidding_reads_once_per_window：持续竞价按窗口重读而不是逐条重读", () => {
  // 每 40ms 一条、共 7 条：窗口在 0、120、240ms 分别开启，读 3 次而不是 7 次。
  expect(simulateRereads([0, 40, 80, 120, 160, 200, 240])).toBe(3);
});

test("sparse_bidding_reads_once_per_bid：稀疏竞价在每条通知后各重读一次", () => {
  // 出价间隔大于窗口时每条通知各读一次，不做延迟合并。
  expect(simulateRereads([0, 200, 400])).toBe(3);
});

test("jittered_window_spans_base_to_exclusive_upper_bound：抖动窗口覆盖基础下界到开区间上界", () => {
  const upper = BID_REFRESH_COALESCE_MILLIS + BID_REFRESH_JITTER_MILLIS;
  expect([BID_REFRESH_COALESCE_MILLIS, upper]).toEqual([100, 250]);
  // 样本 0 取基础窗口，样本趋近 1 仍小于上界。
  expect(coalesceWindowMillis(0)).toBe(100);
  expect(coalesceWindowMillis(0.5)).toBe(175);
  expect(coalesceWindowMillis(1 - Number.EPSILON)).toBe(249);
  // 随机源越界或给出非有限值时也不越出窗口。
  expect(coalesceWindowMillis(1)).toBe(249);
  expect(coalesceWindowMillis(-0.25)).toBe(100);
  expect(coalesceWindowMillis(Number.NaN)).toBe(100);
  expect(coalesceWindowMillis(Number.POSITIVE_INFINITY)).toBe(100);
  for (let step = 0; step < 1_000; step += 1) {
    const window = coalesceWindowMillis(step / 1_000);
    expect(window).toBeGreaterThanOrEqual(100);
    expect(window).toBeLessThan(upper);
  }
});

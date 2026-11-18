// 实时缓存的防重、版本栅栏与事实基线回归测试。
// 这些用例只覆盖纯数据合并语义，不建立 HTTP/WebSocket 连接。

import { expect, test } from "bun:test";

import type { RealtimeEvent, RealtimePage } from "./protocol";

const { createRealtimeCache, createRealtimeCacheAfter } = await import("./cache");

function event(sequence: number, eventId: string, kind: string, payload: unknown): RealtimeEvent {
  return {
    event_id: eventId,
    stream_sequence: sequence,
    kind,
    schema_version: 1,
    payload,
    created_at: "2026-11-04T00:00:00Z",
  };
}

function page(events: readonly RealtimeEvent[], nextAfter: number): RealtimePage {
  return { events, next_after: nextAfter, has_more: false, refetch_required: false };
}

function previewUpdate(sequence: number, revision: number): RealtimeEvent {
  return event(sequence, `preview-${sequence}`, "auction.preview_updated", {
    auction_id: "auction-1",
    preview_revision: revision,
  });
}

function bid(
  sequence: number,
  eventId: string,
  bidId: string,
  stateVersion: number,
  factIndex: number,
  amount: number,
): RealtimeEvent {
  return event(sequence, eventId, "lot.bid_accepted", {
    lot_id: "lot-1",
    bid_id: bidId,
    state_version: stateVersion,
    fact_index: factIndex,
    amount,
    mechanism: "ordinary",
    effective_end: "2026-11-04T12:00:00Z",
  });
}

test("newer_revision_refreshes_and_duplicate_event_is_ignored：较新 revision 刷新，重复事件忽略", () => {
  const cache = createRealtimeCache("auction-1", 7);
  const preview = previewUpdate(1, 8);

  const merge = cache.mergePage(page([preview], 1));
  expect(merge.refresh_required).toBe(true);
  expect(merge.bids_changed).toBe(false);
  expect(merge.continue_catch_up).toBe(true);
  expect(merge.degraded).toBe(false);
  expect(cache.after()).toBe(1);

  // 同一事件重放不能再次触发资源失效，也不能推进已确认游标。
  const replay = cache.mergePage(page([preview], 1));
  expect(replay.refresh_required).toBe(false);
  expect(replay.continue_catch_up).toBe(true);
  expect(cache.after()).toBe(1);
});

test("http_replay_applies_events_after_hidden_sequences_and_advances_the_cursor：HTTP 回放跳过隐藏事件并推进游标", () => {
  const cache = createRealtimeCache("auction-1", 0);
  const merge = cache.mergePage({
    ...page([previewUpdate(2, 9), bid(4, "bid-event-4", "bid-1", 1, 1, 1_500)], 5),
    has_more: true,
  });

  expect(merge.refresh_required).toBe(true);
  expect(merge.bids_changed).toBe(true);
  expect(merge.degraded).toBe(false);
  expect(merge.continue_catch_up).toBe(true);
  expect(cache.after()).toBe(5);
});

test("stale_events_are_ignored_without_reporting_a_gap：过期事件忽略且不报告缺口", () => {
  const cache = createRealtimeCacheAfter("auction-1", 0, 5);
  const merge = cache.mergePage(page([event(3, "stale", "auction.created", {})], 5));

  expect(merge.refresh_required).toBe(false);
  expect(merge.bids_changed).toBe(false);
  expect(merge.degraded).toBe(false);
  expect(cache.after()).toBe(5);
});

test("accepted_bid_marks_only_the_bid_resource_changed：接受竞价只标记竞价资源变化", () => {
  const cache = createRealtimeCache("auction-1", 0);
  const merge = cache.mergePage(page([bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1));

  expect(merge.bids_changed).toBe(true);
  expect(merge.refresh_required).toBe(false);
  expect(merge.continue_catch_up).toBe(true);
  expect(cache.after()).toBe(1);
});

test("conflicting_bid_identity_or_state_version_is_a_gap：竞价身份或状态版本冲突形成缺口", () => {
  const cache = createRealtimeCache("auction-1", 0);
  expect(cache.mergePage(page([bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1)).bids_changed).toBe(
    true,
  );

  // 同一 bid_id 携带不同金额与版本，说明本地事实与权威流冲突。
  const conflict = bid(2, "bid-event-2", "bid-1", 2, 2, 2_000);
  const merge = cache.mergePage(page([conflict], 2));
  expect(merge.degraded).toBe(true);
  expect(merge.refresh_required).toBe(true);
  expect(merge.continue_catch_up).toBe(false);

  // 另一条事实跳过中间版本同样不能接受。
  const skippedCache = createRealtimeCache("auction-1", 0);
  expect(
    skippedCache.mergePage(page([bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1)).bids_changed,
  ).toBe(true);
  const skipped = bid(2, "bid-event-2", "bid-2", 3, 1, 2_000);
  expect(skippedCache.mergePage(page([skipped], 2)).degraded).toBe(true);
});

test("refetch_page_advances_the_baseline_for_new_lots：重读页推进新 lot 的事实基线", () => {
  const cache = createRealtimeCache("auction-1", 0);
  const refetch: RealtimePage = {
    events: [bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)],
    next_after: 9,
    has_more: false,
    refetch_required: true,
  };
  const merge = cache.mergePage(refetch);
  expect(merge.refresh_required).toBe(true);
  expect(merge.degraded).toBe(true);
  expect(merge.bids_changed).toBe(false);
  expect(cache.after()).toBe(9);

  // refetch 后的首条事实不要求从 fact_index 1 重建历史。
  const baseline = bid(10, "bid-event-10", "bid-10", 5, 2, 2_000);
  const baselineMerge = cache.mergePage(page([baseline], 10));
  expect(baselineMerge.bids_changed).toBe(true);
  expect(baselineMerge.degraded).toBe(false);

  // 同一 lot 的后续事实仍须严格连续。
  const regression = bid(11, "bid-event-11", "bid-11", 6, 4, 2_500);
  expect(cache.mergePage(page([regression], 11)).degraded).toBe(true);
});

test("malformed_bid_payload_or_unknown_event_fails_closed：畸形竞价载荷或未知事件失败关闭", () => {
  const malformedCache = createRealtimeCache("auction-1", 0);
  const malformed = event(1, "broken", "lot.bid_accepted", { lot_id: "lot-1" });
  expect(malformedCache.mergePage(page([malformed], 1)).degraded).toBe(true);

  const unknownCache = createRealtimeCache("auction-1", 0);
  const unknown = { auction_id: "auction-1" };
  const unknownEvent = event(1, "unknown", "auction.unknown", unknown);
  expect(unknownCache.mergePage(page([unknownEvent], 1)).degraded).toBe(true);
});

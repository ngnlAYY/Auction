// 竞价日志时间排序与落败代理字段的回归断言。

import { expect, test } from "bun:test";

import type { JsonObject } from "@auction/web-shared/net/wire";

const { validBidLogEntry, validBidLogPage } = await import("./bid-log");

function proxyBid(defeatedProxy: unknown | undefined): JsonObject {
  const bid: JsonObject = {
    id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000010",
    auction_id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000002",
    lot_id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000003",
    lot_number: 1,
    amount: 15_750_000,
    equipment_name: "Legendary Staff",
    equipment_category: "Staff",
    bidder_name: "winner",
    source: "proxy",
    valid: true,
    accepted_state_version: 11,
    fact_index: 1,
    mechanism: "proxy",
    created_at: "2026-11-14T12:57:20Z",
  };
  if (defeatedProxy !== undefined) {
    bid.defeated_proxy = defeatedProxy;
  }
  return bid;
}

function bidPage(...bids: JsonObject[]): JsonObject {
  return { bids, page: 1, page_size: 20, total: bids.length };
}

test.each([
  ["00.11Z", "00.1Z"],
  ["05.5Z", "05Z"],
  ["00.100000002Z", "00.100000001Z"],
])("bid_log_orders_by_instant：%s 排在 %s 前", (later, earlier) => {
  const first = { ...proxyBid(undefined), created_at: `2026-11-14T12:57:${later}` };
  const second = {
    ...proxyBid(undefined),
    id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000011",
    created_at: `2026-11-14T12:57:${earlier}`,
  };
  expect(validBidLogPage(bidPage(first, second))).toBe(true);
  expect(validBidLogPage(bidPage(second, first))).toBe(false);
});

test("equal_bid_instants_use_the_remaining_sort_keys：同一时刻按余下的键排序", () => {
  const first = {
    ...proxyBid(undefined),
    created_at: "2026-11-14T12:57:00.10Z",
    accepted_state_version: 12,
  };
  const second = {
    ...proxyBid(undefined),
    id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000011",
    created_at: "2026-11-14T12:57:00.1Z",
  };
  expect(validBidLogPage(bidPage(first, second))).toBe(true);
  expect(validBidLogPage(bidPage(second, first))).toBe(false);
});

test("bid_log_rejects_an_unparseable_instant_even_for_one_entry：单条记录的非法时刻也失败关闭", () => {
  const bid = { ...proxyBid(undefined), created_at: "2026-11-14" };
  expect(validBidLogPage(bidPage(bid))).toBe(false);
});

test("bid_log_entry_accepts_a_defeated_proxy_only_on_the_winning_proxy_fact：落败代理只随胜者代理第 1 条事实返回", () => {
  const defeated = { bidder_name: "challenger", maximum_amount: 15_000_000 };
  expect(validBidLogEntry(proxyBid(undefined))).toBe(true);
  expect(validBidLogEntry(proxyBid(defeated))).toBe(true);
  for (const invalid of [
    null,
    { bidder_name: "challenger" },
    { bidder_name: "", maximum_amount: 15_000_000 },
    { bidder_name: "challenger", maximum_amount: 0 },
    { bidder_name: "challenger", maximum_amount: 15_000_000, mode: "slow" },
    { bidder_name: "winner", maximum_amount: 15_000_000 },
  ]) {
    expect(validBidLogEntry(proxyBid(invalid)), JSON.stringify(invalid)).toBe(false);
  }
  const secondFact = proxyBid(defeated);
  secondFact.fact_index = 2;
  expect(validBidLogEntry(secondFact), "只随第 1 条事实返回").toBe(false);
  const ordinary = proxyBid(defeated);
  ordinary.mechanism = "ordinary";
  delete ordinary.source;
  expect(validBidLogEntry(ordinary), "普通出价不带落败代理").toBe(false);
});

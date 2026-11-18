// 履约进度目录的排序、范围选择身份与当前目录门禁。

import { expect, test } from "bun:test";
import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import { ok } from "@auction/web-shared/result";

import {
  nextScopeSelection,
  type ScopeSelection,
  sameScopeSelection,
} from "../bid-log/scope-selection";
import {
  currentFulfillmentAuctions,
  type FulfillmentAuctionsSnapshot,
  orderFulfillmentAuctionDirectory,
} from "./model";

function auctionSummary(worldId: string, sequence: number, suffix: number): AuctionSummaryResponse {
  return {
    id: `00000000-0000-0000-0000-0000000000${suffix.toString().padStart(2, "0")}`,
    world_id: worldId,
    sequence,
    status: "settled",
    scheduled_open_at: "2026-01-01T00:00:00Z",
    opened_at: "2026-01-01T00:00:00Z",
    final_phase_at: null,
    effective_end_at: null,
    settled_at: "2026-01-02T00:00:00Z",
    lot_count: 0,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: 1,
  };
}

test("all_world_directory_keeps_main_first_and_each_world_order：全部世界目录先主世界后异世界并保留世界内顺序", () => {
  const mainFirst = auctionSummary("main", 9, 1);
  const mainSecond = auctionSummary("main", 2, 2);
  const isekaiFirst = auctionSummary("isekai", 8, 3);
  const isekaiSecond = auctionSummary("isekai", 3, 4);
  const main = [mainFirst, mainSecond];
  const isekai = [isekaiFirst, isekaiSecond];
  const mixed = [isekaiFirst, mainFirst, isekaiSecond, mainSecond, mainFirst];

  const ordered = orderFulfillmentAuctionDirectory(mixed);

  expect(ordered.map((auction) => auction.world_id)).toEqual(["main", "main", "isekai", "isekai"]);
  expect(ordered.map((auction) => auction.sequence)).toEqual([9, 2, 8, 3]);
  // 稳定分组只改变世界之间的先后，不重排世界内的接口顺序；重复项保留首次出现。
  expect(ordered.slice(0, main.length)).toEqual(main);
  expect(ordered.slice(main.length)).toEqual(isekai);
});

const INITIAL_SELECTION: ScopeSelection = { scope: "all", generation: 0 };

test("scope_generation_changes_with_the_scope：范围不变序号不变，变化时加一，切走再切回得到新序号", () => {
  expect(nextScopeSelection(INITIAL_SELECTION, "all")).toBe(INITIAL_SELECTION);
  const isekai = nextScopeSelection(INITIAL_SELECTION, "isekai");
  expect(isekai).toEqual({ scope: "isekai", generation: 1 });
  const returned = nextScopeSelection(isekai, "all");
  expect(returned).toEqual({ scope: "all", generation: 2 });
  expect(sameScopeSelection(returned, INITIAL_SELECTION)).toBe(false);
});

test("fulfillment_directory_requires_the_current_scope_generation：代际不匹配时场次选项为空，等新目录返回", () => {
  const result = ok([auctionSummary("main", 1, 1)]);
  const snapshot: FulfillmentAuctionsSnapshot = {
    selection: { scope: "all", generation: 1 },
    generation: 7,
    result,
  };
  expect(currentFulfillmentAuctions(snapshot, { scope: "all", generation: 1 })).toBe(result);
  // 切回同一范围但代际不同：不得沿用上一轮目录启用场次下拉框。
  expect(currentFulfillmentAuctions(snapshot, { scope: "all", generation: 2 })).toBeNull();
  expect(currentFulfillmentAuctions(snapshot, { scope: "main", generation: 1 })).toBeNull();
  expect(currentFulfillmentAuctions(undefined, { scope: "all", generation: 1 })).toBeNull();
});

test("fulfillment_directory_survives_same_scope_background_refresh：同范围后台刷新仍保留旧目录选项", () => {
  const result = ok([auctionSummary("main", 1, 1)]);
  const snapshot: FulfillmentAuctionsSnapshot = {
    selection: { scope: "main", generation: 2 },
    generation: 3,
    result,
  };
  // 刷新代号只约束纠偏与进度读取，不改变“当前可展示目录”。
  expect(currentFulfillmentAuctions(snapshot, { scope: "main", generation: 2 })).toBe(result);
});

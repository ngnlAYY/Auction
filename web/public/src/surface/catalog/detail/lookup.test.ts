// 只读详情查找的状态迁移：重复请求、覆盖、首次读取与迟到结果。

import { expect, test } from "bun:test";
import { LOT_DETAIL_FETCH_ERROR, type LotDetailFetchError } from "../../../api/queries/lots";
import type { LookupState, LotDetailTarget } from "./lookup";

const { requested, resolved } = await import("./lookup");
const MISSING: LotDetailFetchError = { kind: "not_found" };

function target(lotId: string): LotDetailTarget {
  return { auctionId: "auction-1", lotId, world: null };
}

test("repeated_request_for_the_same_lot_does_not_restart_the_read：同一拍品的重复请求不会重启读取", () => {
  expect(requested({ kind: "loading", target: target("lot-a") }, target("lot-a"))).toBeNull();
  expect(requested({ kind: "open", target: target("lot-a") }, target("lot-a"))).toBeNull();
});

test("a_new_request_replaces_any_other_state：新请求替换其他任何状态", () => {
  const states: LookupState[] = [
    { kind: "closed" },
    { kind: "failed", target: target("lot-a"), error: LOT_DETAIL_FETCH_ERROR },
    { kind: "loading", target: target("lot-a") },
    { kind: "open", target: target("lot-a") },
  ];
  for (const state of states) {
    expect(requested(state, target("lot-b"))).toEqual({
      kind: "loading",
      target: target("lot-b"),
    });
  }
});

test("first_read_decides_between_open_and_failed：首次读取成功打开、失败关闭", () => {
  const loading: LookupState = { kind: "loading", target: target("lot-a") };
  expect(resolved(loading, "lot-a", null)).toEqual({ kind: "open", target: target("lot-a") });
  for (const error of [MISSING, LOT_DETAIL_FETCH_ERROR]) {
    expect(resolved(loading, "lot-a", error)).toEqual({
      kind: "failed",
      target: target("lot-a"),
      error,
    });
  }
});

test("late_or_background_results_never_change_the_state：迟到或后台结果不改变状态", () => {
  // 已展示时的后台重读失败保留抽屉；已关闭或已改看其他拍品的结果被丢弃。
  expect(resolved({ kind: "open", target: target("lot-a") }, "lot-a", MISSING)).toBeNull();
  expect(resolved({ kind: "closed" }, "lot-a", null)).toBeNull();
  const failed: LookupState = { kind: "failed", target: target("lot-a"), error: MISSING };
  expect(resolved(failed, "lot-a", null)).toBeNull();
  expect(resolved({ kind: "loading", target: target("lot-b") }, "lot-a", null)).toBeNull();
});

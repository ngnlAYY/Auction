// 只读详情查找的状态迁移：重复请求、覆盖、首次读取与迟到结果。

import { expect, test } from "bun:test";
import { LOT_DETAIL_FETCH_ERROR, type LotDetailFetchError } from "../../../api/queries/lots";
import type { LookupState, LotDetailTarget } from "./lookup";

const { requested, resolved } = await import("./lookup");
const MISSING: LotDetailFetchError = { kind: "not_found" };

function target(lotId: string): LotDetailTarget {
  return { auctionId: "auction-1", lotId, world: null, lotCode: "Clo01", entryKey: null };
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
  expect(resolved(loading, target("lot-a"), null, false)).toEqual({
    kind: "open",
    target: target("lot-a"),
  });
  for (const error of [MISSING, LOT_DETAIL_FETCH_ERROR]) {
    expect(resolved(loading, target("lot-a"), error, false)).toEqual({
      kind: "failed",
      target: target("lot-a"),
      error,
    });
  }
});

test("暂时失败只允许保留同代快照，明确不存在总是撤下抽屉", () => {
  const opened: LookupState = { kind: "open", target: target("lot-a") };
  expect(resolved(opened, opened.target, LOT_DETAIL_FETCH_ERROR, true)).toBeNull();
  expect(resolved(opened, opened.target, LOT_DETAIL_FETCH_ERROR, false)).toMatchObject({
    kind: "failed",
    error: LOT_DETAIL_FETCH_ERROR,
  });
  expect(resolved(opened, opened.target, MISSING, true)).toMatchObject({
    kind: "failed",
    error: MISSING,
  });
});

test("已关闭、失败或换目标之后丢弃迟到结果", () => {
  expect(resolved({ kind: "closed" }, target("lot-a"), null, false)).toBeNull();
  const failed: LookupState = { kind: "failed", target: target("lot-a"), error: MISSING };
  expect(resolved(failed, target("lot-a"), null, false)).toBeNull();
  expect(
    resolved({ kind: "loading", target: target("lot-b") }, target("lot-a"), null, false),
  ).toBeNull();
});

test("同一拍品的不同历史条目各自建立读取，旧条目的结果不能交付", () => {
  const first = { ...target("lot-a"), entryKey: "first" };
  const second = { ...target("lot-a"), entryKey: "second" };
  expect(requested({ kind: "open", target: first }, second)).toEqual({
    kind: "loading",
    target: second,
  });
  expect(resolved({ kind: "loading", target: second }, first, MISSING, false)).toBeNull();
});

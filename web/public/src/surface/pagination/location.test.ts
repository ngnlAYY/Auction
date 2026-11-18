// 局部写址只重新编码目标键，其他已知或未知的查询参数逐字保留。

import { expect, test } from "bun:test";
import { flush } from "solid-js";

import { usePublicQueryState } from "../query";
import { mountReadPage } from "./browser-fixture";
import { updatePublicQueryPairs } from "./location";

test("排序可替换等价编码的目标键，保留重复买家与非法日期的原始片段", () => {
  const raw = "%62uyer=Alice&buyer=Bad%0aName&from=2026%2d02-30&other=%7e&bare";
  const initial = `?${raw}&history_%73ort=closed_desc&page=3`;
  const probe: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const mounted = mountReadPage(`/history${initial}`, () => {
    probe.state = usePublicQueryState().state;
  });
  try {
    const state = probe.state;
    if (state === undefined) throw new Error("未装配查询状态");
    mounted.router.replace(`/history${initial}#results`);
    flush();
    mounted.rejectWrites("push");
    expect(
      updatePublicQueryPairs(state, ["history_sort"], [["history_sort", "price_desc"]], "push"),
    ).toBe(false);
    expect(state.peek().historySort).toBe("closed_desc");
    expect(mounted.location.search).toBe(initial);

    mounted.rejectWrites(false);
    expect(
      updatePublicQueryPairs(state, ["history_sort"], [["history_sort", "price_desc"]], "push"),
    ).toBe(true);
    expect(state.peek().historySort).toBe("price_desc");
    expect(mounted.location.search).toBe(`?${raw}&page=3&history_sort=price_desc`);
    expect(mounted.location.hash).toBe("#results");
  } finally {
    mounted.dispose();
  }
});

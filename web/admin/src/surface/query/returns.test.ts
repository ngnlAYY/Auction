// 结算退件 query：非法值回退未结案，合法闭集往返并省略缺省值。

import { expect, test } from "bun:test";
import { mountQueryState } from "./query-fixture";
import { type ReturnsQuery, returnsQueryFilters, useReturnsQueryState } from "./returns";

const DEFAULT_QUERY: ReturnsQuery = {
  all: false,
  status: "",
  matchState: "",
  obligationKind: "",
  world: "",
  page: 1n,
};

test("非法值回退到未结案且不限的第一页", () => {
  const empty = mountQueryState("/admin/returns", useReturnsQueryState);
  try {
    const query = empty.state.state.peek();
    expect(query).toEqual(DEFAULT_QUERY);
    expect(returnsQueryFilters(query).openOnly).toBe(true);
  } finally {
    empty.dispose();
  }
  const mounted = mountQueryState(
    `/admin/returns?${new URLSearchParams([
      ["scope", "everything"],
      ["status", "open"],
      ["match_state", "bound"],
      ["obligation_kind", "payout"],
      ["world", "third"],
      ["page", "-2"],
    ]).toString()}`,
    useReturnsQueryState,
  );
  try {
    const query = mounted.state.state.peek();
    expect(query).toEqual(DEFAULT_QUERY);
    expect(returnsQueryFilters(query).openOnly).toBe(true);
  } finally {
    mounted.dispose();
  }
});

test("合法值往返且缺省键不写地址栏", () => {
  const mounted = mountQueryState("/admin/returns", useReturnsQueryState);
  let writtenAddress: string;
  try {
    const query: ReturnsQuery = {
      all: true,
      status: "take_unknown",
      matchState: "unbound",
      obligationKind: "buyer_refund",
      world: "isekai",
      page: 3n,
    };
    mounted.state.set(query, "push");
    expect([...mounted.fake.location.searchParams]).toEqual([
      ["scope", "all"],
      ["status", "take_unknown"],
      ["match_state", "unbound"],
      ["obligation_kind", "buyer_refund"],
      ["world", "isekai"],
      ["page", "3"],
    ]);
    expect(returnsQueryFilters(query).openOnly).toBe(false);
    writtenAddress = mounted.fake.location.href;
  } finally {
    mounted.dispose();
  }

  const restored = mountQueryState(writtenAddress, useReturnsQueryState);
  try {
    expect(restored.state.state.peek()).toEqual({
      all: true,
      status: "take_unknown",
      matchState: "unbound",
      obligationKind: "buyer_refund",
      world: "isekai",
      page: 3n,
    });
  } finally {
    restored.dispose();
  }

  const defaults = mountQueryState(writtenAddress, useReturnsQueryState);
  try {
    defaults.state.set(DEFAULT_QUERY, "push");
    expect(defaults.fake.location.search).toBe("");
  } finally {
    defaults.dispose();
  }
});

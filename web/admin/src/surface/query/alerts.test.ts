// 告警台 query：非法值回退、闭集往返与缺省值省略。

import { expect, test } from "bun:test";

import { type AlertsQuery, alertStatusChoice, useAlertsQueryState } from "./alerts";
import { mountQueryState } from "./query-fixture";

const DEFAULT_QUERY: AlertsQuery = { status: "active", page: 1n };

function parseAt(search: string): AlertsQuery {
  const mounted = mountQueryState(`/admin/alerts${search}`, useAlertsQueryState);
  try {
    return mounted.state.state.peek();
  } finally {
    mounted.dispose();
  }
}

test("缺失或非法值回退到活动告警第一页", () => {
  expect(parseAt("")).toEqual(DEFAULT_QUERY);
  expect(parseAt("?status=silenced&page=0")).toEqual({ status: "active", page: 1n });
  expect(parseAt("?status=ALL&page=-3")).toEqual({ status: "active", page: 1n });
  expect(parseAt("?page=abc").page).toBe(1n);
});

test("闭集状态和正页码往返", () => {
  for (const status of ["active", "resolved", "all"] as const) {
    const query: AlertsQuery = { status, page: 3n };
    const mounted = mountQueryState("/admin/alerts", useAlertsQueryState);
    let writtenSearch: string;
    try {
      mounted.state.set(query, "push");
      expect(mounted.state.state.peek()).toEqual(query);
      expect(mounted.fake.location.searchParams.getAll("status")).toEqual(
        status === "active" ? [] : [status],
      );
      expect(mounted.fake.location.searchParams.getAll("page")).toEqual(["3"]);
      writtenSearch = mounted.fake.location.search;
    } finally {
      mounted.dispose();
    }
    expect(parseAt(writtenSearch)).toEqual(query);
  }
  expect(alertStatusChoice("resolved")).toBe("resolved");
  expect(alertStatusChoice("")).toBeNull();
});

test("缺省值不写入地址栏", () => {
  const mounted = mountQueryState("/admin/alerts?status=all&page=3", useAlertsQueryState);
  try {
    mounted.state.set(DEFAULT_QUERY, "push");
    expect(mounted.fake.location.search).toBe("");

    mounted.state.set({ status: "resolved", page: 2n }, "push");
    expect(mounted.fake.location.search).toBe("?status=resolved&page=2");

    mounted.state.set({ status: "active", page: 4n }, "push");
    expect(mounted.fake.location.search).toBe("?page=4");
  } finally {
    mounted.dispose();
  }
});

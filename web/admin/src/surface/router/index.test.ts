// 管理端冷启动书签保留查询串与片段身份的回归。

import { describe, expect, test } from "bun:test";

import { captureInitialAdminLocation, initialAdminUrl } from "./index";

function bookmark(pathname: string, search: string, hash: string) {
  const captured = captureInitialAdminLocation(pathname, search, hash);
  if (captured === null) throw new Error(`书签未被捕获：${pathname}${search}${hash}`);
  return captured;
}

describe("管理端路由（对应 Rust surface/router/mod.rs）", () => {
  test("admin_bookmark_keeps_query_and_fragment_independently", () => {
    const withHash = bookmark("/dashboard", "", "#dashboard-title");
    expect(initialAdminUrl(withHash, "/dashboard", "", "")).toBe("/dashboard#dashboard-title");

    const withQuery = bookmark("/audit", "?page=2", "");
    expect(initialAdminUrl(withQuery, "/audit", "", "")).toBe("/audit?page=2");

    const trailing = bookmark("/audit/", "?page=2", "#audit");
    expect(initialAdminUrl(trailing, "/audit", "?page=2", "")).toBe("/audit?page=2#audit");
    expect(initialAdminUrl(trailing, "/users", "", "")).toBeNull();
    expect(initialAdminUrl(trailing, "/audit", "?page=3", "")).toBeNull();
    expect(initialAdminUrl(trailing, "/audit", "", "#other")).toBeNull();
    expect(initialAdminUrl(trailing, "/audit", "?page=2", "#audit")).toBeNull();

    expect(captureInitialAdminLocation("/audit", "", "")).toBeNull();
    expect(captureInitialAdminLocation("//example.test", "?page=2", "")).toBeNull();
  });
});

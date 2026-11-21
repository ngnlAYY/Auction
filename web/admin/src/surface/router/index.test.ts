// 管理端冷启动书签保留查询串与片段身份的回归。

import { describe, expect, test } from "bun:test";
import { captureInitialAdminLocation, initialAdminUrl } from "./index";
import { withLayout } from "./route-load-environment-fixture";

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

test.each([
  ["/dashboard", "/"],
  ["/auction", "/auctions"],
  ["/mail", "/mails"],
] as const)("挂载别名 %s 后替换到 %s 并保留查询与片段", async (alias, canonical) => {
  for (const hash of ["", "#target-section"]) {
    const search = "?page=2";
    const expectedSearch = canonical === "/" ? search : `${search}&page_size=10`;
    const replacements: string[] = [];
    await withLayout(
      async (fixture) => {
        expect(replacements).toEqual([`${canonical}${search}`]);
        expect(fixture.router.currentLocationString()).toBe(`${canonical}${expectedSearch}${hash}`);
        expect(fixture.browser.fake.location.pathname).toBe(canonical);
        expect(fixture.browser.fake.location.search).toBe(expectedSearch);
        expect(fixture.browser.fake.location.hash).toBe(hash);
      },
      {
        start: `${alias}${search}${hash}`,
        onRouterCreated(router) {
          const replace = router.replace.bind(router);
          router.replace = (href) => {
            replacements.push(href);
            replace(href);
          };
        },
      },
    );
  }
});

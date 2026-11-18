// 目录呈现的入口链接与分类键归一化。

import { expect, test } from "bun:test";

const { catalogCategoryKey, historicalBidLogHref, historicalFulfillmentHref } = await import(
  "./presentation"
);

test("historical_entries_preserve_the_current_world_scope：行内入口继承当前世界范围", () => {
  const auctionId = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
  const cases: readonly (readonly [string | null, string | null])[] = [
    [null, null],
    ["main", "main"],
    ["isekai", "isekai"],
  ];
  const entries = [
    ["/bidlog", historicalBidLogHref],
    ["/fulfillment", historicalFulfillmentHref],
  ] as const;
  for (const [expectedPath, hrefFor] of entries) {
    for (const [world, expectedWorld] of cases) {
      const href = hrefFor(auctionId, world);
      const question = href.indexOf("?");
      expect(question).toBeGreaterThanOrEqual(0);
      expect(href.slice(0, question)).toBe(expectedPath);
      const values = new URLSearchParams(href.slice(question + 1));
      expect(values.get("auction")).toBe(auctionId);
      // 缺省表示全部世界，主世界必须显式携带，不能被场次自己的世界改回缺省。
      expect(values.get("world")).toBe(expectedWorld);
    }
  }
});

test("category_keys_accept_public_keys_canonical_names_and_lowercase_names：分类键接受公开键、规范名与小写名", () => {
  for (const value of ["one", "One-Handed", "one-handed"]) {
    expect(catalogCategoryKey(value)).toBe("one");
  }
  expect(catalogCategoryKey("Heavy")).toBe("hea");
  expect(catalogCategoryKey("heavy")).toBe("hea");
  // 其他大小写变体、`all` 与未知分类不归入任何大类。
  for (const value of ["ONE-HANDED", "One-handed", "all", "Unknown", ""]) {
    expect(catalogCategoryKey(value)).toBe("other");
  }
});

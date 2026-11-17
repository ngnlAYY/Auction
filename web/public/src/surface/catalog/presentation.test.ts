// 目录呈现的入口链接与分类键归一化。

import { expect, test } from "bun:test";

const { catalogCategoryKey, historicalFulfillmentHref } = await import("./presentation");

test("fulfillment_entry_opens_the_round_in_its_world：履约入口按世界打开对应场次", () => {
  const auctionId = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
  const cases: readonly (readonly [string, string | null])[] = [
    ["isekai", "isekai"],
    ["main", null],
  ];
  for (const [world, expectedWorld] of cases) {
    const href = historicalFulfillmentHref(auctionId, world);
    const question = href.indexOf("?");
    expect(question).toBeGreaterThanOrEqual(0);
    const path = href.slice(0, question);
    const search = href.slice(question + 1);
    expect(path).toBe("/fulfillment");
    const values = new URLSearchParams(search);
    expect(values.get("auction")).toBe(auctionId);
    // 主世界是页面缺省世界，与出价记录入口一样不写 `world`。
    expect(values.get("world")).toBe(expectedWorld);
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

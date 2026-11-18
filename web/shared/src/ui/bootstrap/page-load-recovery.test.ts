// 页面代码加载失败后的重载判定：同一地址在保护时间内只重载一次。

import { describe, expect, test } from "bun:test";

import { pageLoadReloadDecision } from "./page-load-recovery";

const HREF = "https://example.test/bidlog?world=main";

describe("pageLoadReloadDecision", () => {
  test("没有标记时重载，并写下带时刻与地址的标记", () => {
    expect(pageLoadReloadDecision(null, HREF, 1_000)).toEqual({
      reload: true,
      mark: `1000|${HREF}`,
    });
  });

  test("同一地址在保护时间内不再重载", () => {
    const first = pageLoadReloadDecision(null, HREF, 1_000);
    expect(pageLoadReloadDecision(first.mark, HREF, 1_000 + 29_999).reload).toBe(false);
  });

  test("保护时间过后可以再次重载", () => {
    const first = pageLoadReloadDecision(null, HREF, 1_000);
    expect(pageLoadReloadDecision(first.mark, HREF, 1_000 + 30_000).reload).toBe(true);
  });

  test("换了地址不受上一次标记限制", () => {
    const first = pageLoadReloadDecision(null, HREF, 1_000);
    expect(pageLoadReloadDecision(first.mark, "https://example.test/history", 2_000).reload).toBe(
      true,
    );
  });

  test("地址里的竖线不影响解析", () => {
    const href = "https://example.test/history?name=a|b";
    const first = pageLoadReloadDecision(null, href, 5_000);
    expect(pageLoadReloadDecision(first.mark, href, 6_000).reload).toBe(false);
  });

  test("时钟回拨按刚重载过处理", () => {
    expect(pageLoadReloadDecision(`9000|${HREF}`, HREF, 1_000).reload).toBe(false);
  });

  test("损坏的标记不阻止重载", () => {
    expect(pageLoadReloadDecision("garbage", HREF, 1_000).reload).toBe(true);
    expect(pageLoadReloadDecision(`abc|${HREF}`, HREF, 1_000).reload).toBe(true);
    expect(pageLoadReloadDecision(`|${HREF}`, HREF, 1_000).reload).toBe(true);
  });
});

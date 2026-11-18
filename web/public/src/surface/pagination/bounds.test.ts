// 页码纠偏保持列表独立、保护其他参数，写入被拒时停止读取。

import { expect, test } from "bun:test";
import { flush } from "solid-js";

import { usePublicQueryState } from "../query";
import { publicOffsetLastPage, usePublicPageBounds } from "./bounds";
import { mountReadPage } from "./browser-fixture";

test("账户第 1001 页可达，第 1002 页只替换那一张表的页码", () => {
  expect(publicOffsetLastPage(10)).toBe(1001);
  const mounted = mountReadPage(
    "/me?bid_page=7&proxy_page=1002&outside=keep#login-duration",
    () => {
      const query = usePublicQueryState().state;
      const bounds = usePublicPageBounds(query, "Me", () => [
        ["bidPage", 10],
        ["proxyPage", 10],
      ]);
      expect(bounds.blocked("bidPage")).toBe(false);
      expect(bounds.blocked("proxyPage")).toBe(true);
    },
  );
  try {
    expect(mounted.location.searchParams.get("bid_page")).toBe("7");
    expect(mounted.location.searchParams.get("proxy_page")).toBe("1001");
    expect(mounted.location.searchParams.get("outside")).toBe("keep");
    expect(
      mounted.writes.find((write) => write.url.searchParams.get("proxy_page") === "1001")?.mode,
    ).toBe("replace");
  } finally {
    mounted.dispose();
  }
});

test("越界页只由自己的路由纠偏，并保留重复筛选和 i64 页码的边界", () => {
  const probes: { bounds?: ReturnType<typeof usePublicPageBounds> } = {};
  const mounted = mountReadPage(
    "/history?buyer=A&buyer=B&page_size=37&page=9007199254740993",
    () => {
      const query = usePublicQueryState().state;
      probes.bounds = usePublicPageBounds(query, "AuctionList", (query) => [
        ["page", query.pageSize],
      ]);
    },
  );
  try {
    expect(probes.bounds?.blocked("page")).toBe(true);
    expect(mounted.location.searchParams.get("page")).toBe("9007199254740993");
    expect(mounted.location.searchParams.getAll("buyer")).toEqual(["A", "B"]);
    expect(mounted.writes.some((write) => write.url.searchParams.get("page") === "271")).toBe(
      false,
    );
  } finally {
    mounted.dispose();
  }
  const own = mountReadPage("/auctions?buyer=A&buyer=B&page_size=37&page=9007199254740993", () => {
    const query = usePublicQueryState().state;
    usePublicPageBounds(query, "AuctionList", (query) => [["page", query.pageSize]]);
  });
  try {
    expect(own.location.searchParams.get("page")).toBe("271");
    expect(own.location.searchParams.getAll("buyer")).toEqual(["A", "B"]);
  } finally {
    own.dispose();
  }
});

test("拒绝替换时阻止越界页读取，重试成功才解除门禁", () => {
  const probes: { bounds?: ReturnType<typeof usePublicPageBounds> } = {};
  const mounted = mountReadPage("/fulfillment", () => {
    const query = usePublicQueryState().state;
    probes.bounds = usePublicPageBounds(query, "Fulfillment", (query) => [
      ["page", Math.min(100, query.pageSize)],
    ]);
  });
  try {
    mounted.router.replace("/fulfillment?page=102&page_size=200");
    mounted.rejectWrites(true);
    flush();
    expect(probes.bounds?.failed("page")).toBe(true);
    expect(mounted.location.searchParams.get("page")).toBe("102");

    mounted.rejectWrites(false);
    probes.bounds?.retry();
    flush();
    expect(probes.bounds?.blocked("page")).toBe(false);
    expect(mounted.location.searchParams.get("page")).toBe("101");
  } finally {
    mounted.dispose();
  }
});

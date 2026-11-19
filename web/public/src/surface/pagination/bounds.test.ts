// 页码纠偏保持列表独立、保护其他参数，写入被拒时停止读取。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";

import {
  AUCTION_ID,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import { updatePublicQueryState, usePublicQueryState } from "../query";
import { updatePublicPage } from ".";
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

test.each([
  ["page=9007199254740993&page=bad", ["501"]],
  ["page=9007199254740993&page=2", ["9007199254740993", "2"]],
  ["page=2&page=9007199254740993", ["501"]],
  ["page=9223372036854775807&page=9223372036854775808", ["501"]],
  ["page=2&page=9223372036854775808", ["2", "9223372036854775808"]],
] as const)("看板按最后一个有效的正 i64 纠偏 %s", (pages, expected) => {
  const mounted = mountReadPage(`/?${pages}&page_size=20`, () => {
    const query = usePublicQueryState().state;
    usePublicPageBounds(query, "AuctionIndex", (query) => [["page", query.pageSize]]);
  });
  try {
    expect(mounted.location.searchParams.getAll("page")).toEqual([...expected]);
  } finally {
    mounted.dispose();
  }
});

test.each([
  ["page=2&bidlog_page=9223372036854775807", "2", false],
  ["page=9223372036854775807&bidlog_page=2", "501", true],
  ["page=bad&bidlog_page=9223372036854775807&bidlog_page=bad", "501", true],
  ["page=9223372036854775808&bidlog_page=3", "3", false],
] as const)("出价记录先取有效规范页码，再回退别名 %s", (search, expected, corrected) => {
  const probes: number[] = [];
  const mounted = mountReadPage(`/bidlog?${search}&page_size=20`, () => {
    const query = usePublicQueryState().state;
    probes.push(query.peek().page);
    usePublicPageBounds(query, "BidLog", (query) => [["page", query.pageSize]]);
  });
  try {
    expect(probes).toEqual([Number(expected)]);
    expect(mounted.location.searchParams.has("bidlog_page")).toBe(!corrected);
    expect(mounted.writes.map((write) => write.url.searchParams.get("page"))).toEqual(
      corrected ? [expected] : [],
    );
  } finally {
    mounted.dispose();
  }
});

test("流水和账户分别选最后一个有效页码，再按各自页长纠偏", () => {
  const pool = mountReadPage(
    "/compensation-pool?movement_page=9223372036854775807&movement_page=bad&movement_page_size=50",
    () => {
      const query = usePublicQueryState().state;
      usePublicPageBounds(query, "CompensationPool", (query) => [
        ["movementPage", query.movementPageSize],
      ]);
    },
  );
  try {
    expect(pool.location.searchParams.getAll("movement_page")).toEqual(["201"]);
  } finally {
    pool.dispose();
  }
  const account = mountReadPage(
    "/me?bid_page=2&bid_page=9223372036854775807&proxy_page=9223372036854775807&proxy_page=3",
    () => {
      const query = usePublicQueryState().state;
      usePublicPageBounds(query, "Me", () => [
        ["bidPage", 10],
        ["proxyPage", 10],
      ]);
    },
  );
  try {
    expect(account.location.searchParams.getAll("bid_page")).toEqual(["1001"]);
    expect(account.location.searchParams.getAll("proxy_page")).toEqual([
      "9223372036854775807",
      "3",
    ]);
  } finally {
    account.dispose();
  }
});

test("出价记录地址规范化被拒时仍识别分页别名，手动翻页清除别名", () => {
  const probes: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const mounted = mountReadPage(
    "/bidlog/?bidlog_page=2",
    () => {
      probes.state = usePublicQueryState().state;
    },
    undefined,
    true,
  );
  try {
    const state = probes.state;
    if (state === undefined) throw new Error("未挂载出价记录查询");
    expect(mounted.location.pathname).toBe("/bidlog/");
    expect(state.peek()).toMatchObject({ page: 2, bidLogPageAlias: true });
    mounted.rejectWrites(false);
    expect(updatePublicPage(state, 1, "push")).toBe(true);
    expect(mounted.location.searchParams.has("bidlog_page")).toBe(false);
    expect(state.peek()).toMatchObject({ page: 1, bidLogPageAlias: false });
  } finally {
    mounted.dispose();
  }
});

test("账户地址规范化被拒时，单表翻页仍保留另一张表的原始参数", () => {
  const probes: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const mounted = mountReadPage(
    "/me/?bid_page=2&proxy_page=%30%33&keep=%2b",
    () => {
      probes.state = usePublicQueryState().state;
    },
    undefined,
    true,
  );
  try {
    const state = probes.state;
    if (state === undefined) throw new Error("未挂载账户查询");
    mounted.rejectWrites(false);
    expect(
      updatePublicQueryState(
        state,
        (query) => {
          query.bidPage = 4;
        },
        "push",
      ),
    ).toBe(true);
    expect(mounted.location.pathname).toBe("/me/");
    expect(mounted.location.search).toBe("?proxy_page=%30%33&keep=%2b&bid_page=4");
  } finally {
    mounted.dispose();
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
    mounted.router.replace("/fulfillment?page=9007199254740993&page=bad&page_size=200");
    mounted.rejectWrites(true);
    flush();
    expect(probes.bounds?.failed("page")).toBe(true);
    expect(mounted.location.searchParams.getAll("page")).toEqual(["9007199254740993", "bad"]);

    mounted.rejectWrites(false);
    probes.bounds?.retry();
    flush();
    expect(probes.bounds?.blocked("page")).toBe(false);
    expect(mounted.location.searchParams.get("page")).toBe("101");
  } finally {
    mounted.dispose();
  }
});

test("看板无法纠正重复超大页码时，启动预取与正文均不发 GET", async () => {
  const href = "/?page=9007199254740993&page=bad&page_size=20";
  const page = await mountCompetition("main", false, {
    href,
    rejectHistoryWrites: true,
    storage: new Map([["hv.public.current-auction.main", AUCTION_ID]]),
  });
  try {
    await waitFor(
      () => page.find((node) => renderedText(node) === t("pagination-clamp-error")) !== null,
    );
    await settle();
    expect(page.location.search).toBe(href.slice(1));
    expect(
      page.reads.filter((path) => path.startsWith(`/api/v1/auctions/${AUCTION_ID}/lots?`)),
    ).toEqual([]);
  } finally {
    page.dispose();
  }
});

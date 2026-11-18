// 往期列表刷新订阅、页内重试与查询快照隔离。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { t } from "@auction/web-shared/ui/i18n";
import { createRoot, flush, useContext } from "solid-js";

import { resetStoreForTest } from "../../api/swr-cache";
import { PublicRefreshGenerationContext, refreshPublicReads } from "../board/refresh";
import { PublicPager, updatePublicPage, updatePublicPageSize } from "../pagination";
import { mountReadPage } from "../pagination/browser-fixture";
import { usePublicQueryState } from "../query";
import { usePageRouteGuard } from "../query/route-page";
import { AuctionHistoryPage, AuctionList, useAuctionHistory } from "./history";

const SETTLED_AUCTION = {
  id: "00000000-0000-4000-8000-000000000101",
  world_id: "main",
  sequence: 101,
  status: "settled",
  scheduled_open_at: "2026-01-01T00:00:00Z",
  settled_at: "2026-01-02T00:00:00Z",
  lot_count: 1,
  sold_count: 1,
  unsold_count: 0,
  sold_amount: 1000,
  state_version: 1,
};

function auctionPage(
  auctions: readonly (typeof SETTLED_AUCTION)[],
  etag: string,
  page = 1,
  total = auctions.length,
): Response {
  return Response.json(
    {
      code: "ok",
      message: "ok",
      data: { auctions, page, page_size: 10, total },
    },
    { headers: { etag } },
  );
}

function mountHistory(initialResponse: Response) {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockRejectedValue(new Error("缺少测试响应"));
  fetcher.mockResolvedValueOnce(initialResponse);
  const refresh = createCell(0);
  const query = createCell<readonly [string, string, number, number]>(["main", "settled", 1, 10]);
  const previousRefresh = PublicRefreshGenerationContext.defaultValue;
  PublicRefreshGenerationContext.defaultValue = refresh;
  const root = createRoot((dispose) => {
    const history = useAuctionHistory(createValueMemo(query));
    flush();
    return { ...history, dispose };
  });
  return {
    ...root,
    fetcher,
    query,
    refresh,
    dispose() {
      root.dispose();
      flush();
      PublicRefreshGenerationContext.defaultValue = previousRefresh;
      fetcher.mockRestore();
      resetStoreForTest();
    },
  };
}

async function settleRead(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

test("外壳刷新对同一查询条件重读，列表能收到新结算场次", async () => {
  const mounted = mountHistory(auctionPage([], '"empty"'));
  try {
    await settleRead();
    expect(mounted.currentResult()).toMatchObject({ ok: true, value: { auctions: [] } });
    expect(mounted.fetcher).toHaveBeenCalledTimes(1);

    mounted.query.set(["main", "settled", 1, 10]);
    flush();
    expect(mounted.fetcher).toHaveBeenCalledTimes(1);

    mounted.fetcher.mockResolvedValueOnce(auctionPage([SETTLED_AUCTION], '"settled"'));
    refreshPublicReads(mounted.refresh);
    flush();
    expect(mounted.currentResult()).toMatchObject({ ok: true, value: { auctions: [] } });
    await settleRead();
    expect(mounted.fetcher).toHaveBeenCalledTimes(2);
    const [initial, refreshed] = mounted.fetcher.mock.calls;
    expect(refreshed?.[0]).toBe(initial?.[0]);
    expect(new Headers(refreshed?.[1]?.headers).get("If-None-Match")).toBe('"empty"');
    expect(mounted.currentResult()).toMatchObject({
      ok: true,
      value: { auctions: [SETTLED_AUCTION] },
    });
  } finally {
    mounted.dispose();
  }
});

test("错误重试只刷新本页，切换世界时旧查询的列表暂不展示", async () => {
  const mounted = mountHistory(
    Response.json({ code: "service_unavailable", message: "Unavailable" }, { status: 503 }),
  );
  try {
    await settleRead();
    expect(mounted.currentResult()).toMatchObject({ ok: false });

    mounted.fetcher.mockResolvedValueOnce(auctionPage([SETTLED_AUCTION], '"settled"'));
    mounted.retry();
    await settleRead();
    expect(mounted.fetcher).toHaveBeenCalledTimes(2);
    expect(mounted.refresh.peek()).toBe(0);
    expect(mounted.currentResult()).toMatchObject({
      ok: true,
      value: { auctions: [SETTLED_AUCTION] },
    });

    mounted.fetcher.mockResolvedValueOnce(auctionPage([], '"isekai"'));
    mounted.query.set(["isekai", "settled", 1, 10]);
    flush();
    expect(mounted.currentResult()).toBeNull();
    await settleRead();
    expect(mounted.fetcher).toHaveBeenCalledTimes(3);
    expect(mounted.currentResult()).toMatchObject({ ok: true, value: { auctions: [] } });
  } finally {
    mounted.dispose();
  }
});

test.each([1001, 1002])("往期拍卖第 %i 页在 GET 前收敛到后端偏移范围", async (page) => {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockResolvedValue(
    Response.json({
      code: "ok",
      message: "ok",
      data: { auctions: [], page: 1001, page_size: 10, total: 10_010 },
    }),
  );
  const mounted = mountReadPage(`/auctions?page=${page}&page_size=10`, AuctionList);
  try {
    await settleRead();
    const reads = fetcher.mock.calls
      .map(([input]) => new URL(String(input), mounted.location))
      .filter((url) => url.pathname === "/api/v2/auctions");
    expect(reads.map((url) => url.searchParams.get("page"))).toEqual(["1001"]);
    expect(mounted.location.searchParams.get("page")).toBe("1001");
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("往期列表只清除旧状态筛选，照常读取已结算场次", async () => {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockResolvedValueOnce(
    auctionPage([SETTLED_AUCTION], '"history"'),
  );
  const raw = "world=%6dain&page_size=10&keep=%2b%20x";
  const mounted = mountReadPage(
    `/auctions?${raw}&%73tatus=%6cive&status=unknown#rounds`,
    AuctionList,
  );
  try {
    await settleRead();
    expect(mounted.location.search).toBe(`?${raw}`);
    expect(mounted.location.hash).toBe("#rounds");
    expect(fetcher.mock.calls.map(([input]) => String(input))).toEqual([
      "/api/v2/auctions?page=1&page_size=10&status=settled&world=main",
    ]);
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("往期列表刷新缩减总数后，纠偏被拒才显示可点击的重试", async () => {
  resetStoreForTest();
  const replies = [
    auctionPage([SETTLED_AUCTION], '"last-page"', 3, 21),
    auctionPage([], '"shrunken"', 3, 1),
    auctionPage([SETTLED_AUCTION], '"first-page"'),
  ];
  const requests: string[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: RequestInfo | URL) => {
        const url = new URL(String(input), "https://auction.test");
        if (url.pathname !== "/api/v2/auctions") return new Response("{}", { status: 503 });
        requests.push(url.searchParams.get("page") ?? "");
        return replies.shift() ?? new Response("{}", { status: 503 });
      },
      { preconnect() {} },
    ),
  );
  const probe: {
    state?: ReturnType<typeof usePublicQueryState>["state"];
    read?: ReturnType<typeof useAuctionHistory>;
    refresh?: typeof PublicRefreshGenerationContext.defaultValue;
    onPageClamp?: (page: number) => boolean;
  } = {};
  const currentPage = () => {
    const result = probe.read?.currentResult();
    return result?.ok === true ? result.value : null;
  };
  let pager: unknown;
  const mounted = mountReadPage(
    "/auctions?world=main&page=3&page_size=10",
    () => {
      const state = usePublicQueryState().state;
      const routeStillOnPage = usePageRouteGuard("AuctionList");
      probe.state = state;
      probe.read = useAuctionHistory(
        createValueMemo(
          () => [state().world ?? "all", "settled", state().page, state().pageSize] as const,
        ),
      );
      probe.refresh = useContext(PublicRefreshGenerationContext);
      probe.onPageClamp = (page) => routeStillOnPage() && updatePublicPage(state, page, "replace");
    },
    ({ tag, props }) => {
      if (typeof tag !== "function") return;
      if (tag === PublicPager) {
        Reflect.apply(tag, undefined, [props]);
      } else if (tag === Pager) {
        // JSX 替身会取一次属性值。为实际分页器保留列表快照和页码的响应式读取。
        pager = Reflect.apply(tag, undefined, [
          {
            ...props,
            get total() {
              return currentPage()?.total ?? null;
            },
            get page() {
              return probe.state?.().page ?? 1;
            },
          },
        ]);
      }
    },
  );
  try {
    await settleRead();
    const page = currentPage();
    const { state, refresh, onPageClamp } = probe;
    if (
      page === null ||
      state === undefined ||
      refresh === undefined ||
      onPageClamp === undefined
    ) {
      throw new Error("往期列表未完成首次读取");
    }
    expect(page.total).toBe(21);
    mounted.run(() =>
      AuctionHistoryPage({
        page,
        pageNumber: state().page,
        world: state().world,
        onPageClamp,
        onPageChange: (next) => updatePublicPage(state, next, "push"),
        onPageSizeChange: (size) => updatePublicPageSize(state, size),
      }),
    );
    flush();
    const feedback = () =>
      mounted
        .visible(pager)
        .filter(
          ({ children }) =>
            children.includes(t("pagination-clamp-error")) || children.includes(t("data-retry")),
        );
    expect(feedback()).toHaveLength(0);
    expect(requests).toEqual(["3"]);

    mounted.rejectWrites(true);
    refreshPublicReads(refresh);
    await settleRead();
    expect(currentPage()?.total).toBe(1);
    expect(mounted.location.searchParams.get("page")).toBe("3");
    expect(requests).toEqual(["3", "3"]);
    const visible = feedback();
    expect(
      visible.some(
        ({ tag, children }) => tag === "span" && children.includes(t("pagination-clamp-error")),
      ),
    ).toBe(true);
    const retry = visible.find(({ tag }) => tag === "button")?.props.onClick;
    if (typeof retry !== "function") throw new Error("未显示可重试的页码提示");

    mounted.rejectWrites(false);
    retry();
    await settleRead();
    expect(mounted.writes.at(-1)?.mode).toBe("replace");
    expect(mounted.location.searchParams.has("page")).toBe(false);
    expect(requests).toEqual(["3", "3", "1"]);
    expect(currentPage()?.page).toBe(1);
    expect(feedback()).toHaveLength(0);
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

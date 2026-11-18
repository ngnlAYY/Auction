// 往期列表刷新订阅、页内重试与查询快照隔离。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createRoot, flush } from "solid-js";

import { resetStoreForTest } from "../../api/swr-cache";
import { PublicRefreshGenerationContext, refreshPublicReads } from "../board/refresh";
import { useAuctionHistory } from "./history";

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

function auctionPage(auctions: readonly (typeof SETTLED_AUCTION)[], etag: string): Response {
  return Response.json(
    {
      code: "ok",
      message: "ok",
      data: { auctions, page: 1, page_size: 10, total: auctions.length },
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

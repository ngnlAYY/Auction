// 出价、履约页面的目录门禁、按需翻页与地址写入失败恢复。

import { expect, jest, onTestFinished, spyOn, test } from "bun:test";
import { flush } from "solid-js";

import { resetStoreForTest } from "../../../api/swr-cache";
import { mountReadPage } from "../../pagination/browser-fixture";
import { Fulfillment } from "../fulfillment";
import { BidLog } from ".";

function settledAuction(sequence: number) {
  return {
    id: `00000000-0000-4000-8000-${String(sequence).padStart(12, "0")}`,
    world_id: "main",
    sequence,
    status: "settled",
    scheduled_open_at: "2026-01-01T00:00:00Z",
    settled_at: "2026-01-02T00:00:00Z",
    lot_count: 0,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: 1,
  };
}

const LATEST = settledAuction(201);
const OLDER = settledAuction(1);
const FIRST_PAGE = Array.from({ length: 200 }, (_, index) => settledAuction(201 - index));

function mockReadRequests(
  first: readonly ReturnType<typeof settledAuction>[],
  second: () => Promise<Response> = async () => directoryPage([OLDER], 2, first.length + 1),
  total = first.length + 1,
) {
  resetStoreForTest();
  const urls: URL[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: URL | RequestInfo) => {
        const url = new URL(String(input), "https://auction.test");
        urls.push(url);
        if (url.pathname !== "/api/v1/auctions") {
          return new Response("{}", { status: 503 });
        }
        if (url.searchParams.get("status") !== "settled") {
          return directoryPage([], 1, 0);
        }
        return url.searchParams.get("page") === "1" ? directoryPage(first, 1, total) : second();
      },
      { preconnect() {} },
    ),
  );
  return {
    urls,
    dispose() {
      fetcher.mockRestore();
      resetStoreForTest();
    },
  };
}

function directoryPage(
  auctions: readonly ReturnType<typeof settledAuction>[],
  page: number,
  total: number,
) {
  return Response.json({
    code: "ok",
    message: "ok",
    data: { auctions, page, page_size: 200, total },
  });
}

async function settleRead() {
  for (let turn = 0; turn < 3; turn += 1) {
    await Bun.sleep(0);
    flush();
  }
}

const pages = [
  ["/bidlog", "bid-log", BidLog],
  ["/fulfillment", "fulfillment", Fulfillment],
] as const;

test.each(pages.flatMap((entry) => [3, 1002].map((page) => [...entry, page] as const)))(
  "%s 默认选场与页码重置只写一次，正文从第一页开始",
  async (path, api, render, page) => {
    const reads = mockReadRequests([LATEST], undefined, 1);
    const search = `?world=main&page=${page}&page_size=10`;
    const mounted = mountReadPage(`${path}${search}`, render);
    try {
      await settleRead();
      const body = reads.urls.filter(
        (url) => url.pathname === `/api/v1/auctions/${LATEST.id}/${api}`,
      );
      expect(body.map((url) => url.searchParams.get("page"))).toEqual(["1"]);
      expect(body[0]?.searchParams.get("page_size")).toBe("10");
      const selected = mounted.writes.filter(({ url }) => url.search !== search);
      expect(selected.map(({ mode, url }) => [mode, url.search])).toEqual([
        ["replace", `?world=main&page_size=10&auction=${LATEST.id}`],
      ]);
      expect(mounted.location.searchParams.get("page")).toBeNull();
    } finally {
      mounted.dispose();
      reads.dispose();
    }
  },
);

test.each(pages)(
  "%s 显示搜索框隐藏的换行，修正并写址后只读取一次正文",
  async (path, api, render) => {
    const reads = mockReadRequests([LATEST], undefined, 1);
    const mounted = mountReadPage(`${path}?auction=${LATEST.id}&lot=One%0A12`, render);
    try {
      await settleRead();
      const input = mounted.elements.find(
        ({ tag, props }) => tag === "input" && props.name === "lot",
      );
      const error = mounted.elements.find(
        ({ props }) => props.id === input?.props["aria-describedby"],
      );
      expect(error?.props.role).toBe("status");
      expect(
        mounted.elements.some(
          ({ tag, children }) => tag === "bdi" && children.includes('"One\\n12"'),
        ),
      ).toBe(true);
      const body = () => reads.urls.filter((url) => url.pathname.endsWith(`/${api}`));
      expect(body()).toHaveLength(0);
      const onInput = input?.props.onInput;
      if (typeof onInput !== "function") throw new Error("未挂载 lot 输入");
      jest.useFakeTimers();
      onTestFinished(() => jest.useRealTimers());
      const inputCorrection = () => onInput({ currentTarget: { value: "One12" } });
      const debounce = async (milliseconds: number) => {
        jest.advanceTimersByTime(milliseconds);
        for (let turn = 0; turn < 10; turn += 1) {
          await Promise.resolve();
          flush();
        }
      };
      mounted.rejectWrites(true);
      inputCorrection();
      await debounce(300);
      expect(mounted.location.searchParams.get("lot")).toBe("One\n12");
      expect(body()).toHaveLength(0);

      mounted.rejectWrites(false);
      inputCorrection();
      await debounce(299);
      expect(mounted.location.searchParams.get("lot")).toBe("One\n12");
      expect(body()).toHaveLength(0);
      await debounce(1);
      expect(mounted.location.searchParams.get("lot")).toBe("One12");
      expect(body().map((url) => [url.pathname, url.searchParams.get("lot")])).toEqual([
        [`/api/v1/auctions/${LATEST.id}/${api}`, "One12"],
      ]);
    } finally {
      mounted.dispose();
      reads.dispose();
      jest.useRealTimers();
    }
  },
);

test.each(pages)(
  "%s 默认场次确认后正文先读，场次下拉按需读取后续目录",
  async (path, api, render) => {
    const continuation = Promise.withResolvers<Response>();
    const reads = mockReadRequests(FIRST_PAGE, () => continuation.promise);
    const mounted = mountReadPage(`${path}?world=main`, render);
    const body = `/api/v1/auctions/${LATEST.id}/${api}`;
    try {
      await settleRead();
      expect(mounted.location.searchParams.get("auction")).toBe(LATEST.id);
      expect(reads.urls.map((url) => url.pathname)).toContain(body);
      expect(
        reads.urls.filter(
          (url) => url.pathname === "/api/v1/auctions" && url.searchParams.get("page") === "2",
        ),
      ).toHaveLength(0);

      const focus = mounted.elements.find(
        ({ tag, props }) => tag === "select" && props.name === "auction",
      )?.props.onFocus;
      if (typeof focus !== "function") throw new Error("未挂载场次下拉框");
      focus();
      await settleRead();
      expect(
        reads.urls.filter(
          (url) => url.pathname === "/api/v1/auctions" && url.searchParams.get("page") === "2",
        ),
      ).toHaveLength(1);
      continuation.resolve(directoryPage([OLDER], 2, 201));
      await settleRead();
      expect(reads.urls.filter((url) => url.pathname === body)).toHaveLength(1);
    } finally {
      continuation.resolve(directoryPage([OLDER], 2, 201));
      mounted.dispose();
      reads.dispose();
    }
  },
);

test("全部世界中的异世界首页不能提前确认为默认场次", async () => {
  const continuation = Promise.withResolvers<Response>();
  const isekai = FIRST_PAGE.map((auction) => ({ ...auction, world_id: "isekai" }));
  const reads = mockReadRequests(isekai, () => continuation.promise);
  const mounted = mountReadPage("/fulfillment", Fulfillment);
  try {
    await settleRead();
    expect(reads.urls.some((url) => url.searchParams.get("page") === "2")).toBe(true);
    expect(reads.urls.some((url) => url.pathname.endsWith("/fulfillment"))).toBe(false);

    continuation.resolve(directoryPage([OLDER], 2, 201));
    await settleRead();
    expect(mounted.location.searchParams.get("auction")).toBe(OLDER.id);
    const body = reads.urls.find((url) => url.pathname.endsWith("/fulfillment"));
    expect(body?.pathname).toBe(`/api/v1/auctions/${OLDER.id}/fulfillment`);
    expect(body?.searchParams.get("world")).toBe("main");
  } finally {
    continuation.resolve(directoryPage([OLDER], 2, 201));
    mounted.dispose();
    reads.dispose();
  }
});

test.each(pages)(
  "%s 深链场次要等可见目录确认，并按实际页长替换越界页码",
  async (path, api, render) => {
    const continuation = Promise.withResolvers<Response>();
    const reads = mockReadRequests(FIRST_PAGE, () => continuation.promise);
    const mounted = mountReadPage(
      `${path}?world=main&auction=${OLDER.id}&page_size=200&page=102`,
      render,
    );
    try {
      await settleRead();
      expect(reads.urls.some((url) => url.pathname.endsWith(`/${api}`))).toBe(false);
      expect(reads.urls.some((url) => url.searchParams.get("page") === "2")).toBe(true);
      const expectedPage = path === "/fulfillment" ? "101" : "51";
      expect(mounted.location.searchParams.get("page")).toBe(expectedPage);
      continuation.resolve(directoryPage([OLDER], 2, 201));
      await settleRead();
      const body = reads.urls.find((url) => url.pathname.endsWith(`/${api}`));
      expect(body?.pathname).toBe(`/api/v1/auctions/${OLDER.id}/${api}`);
      expect(body?.searchParams.get("page")).toBe(expectedPage);
      expect(body?.searchParams.get("page_size")).toBe(path === "/fulfillment" ? "100" : "200");
      expect(body?.searchParams.get("world")).toBe("main");
    } finally {
      continuation.resolve(directoryPage([OLDER], 2, 201));
      mounted.dispose();
      reads.dispose();
    }
  },
);

test.each(pages)("%s 单场自动替换被拒后结束加载，手动选择可恢复", async (path, api, render) => {
  const reads = mockReadRequests([LATEST], undefined, 1);
  const mounted = mountReadPage(path, render);
  mounted.rejectWrites(true);
  try {
    await settleRead();
    expect(mounted.location.searchParams.has("auction")).toBe(false);
    expect(mounted.loading.peek().size).toBe(0);
    expect(reads.urls.some((url) => url.pathname.endsWith(`/${api}`))).toBe(false);

    const change = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "auction",
    )?.props.onChange;
    if (typeof change !== "function") throw new Error("未挂载场次下拉框");
    const select = { value: LATEST.id };
    change({ currentTarget: select });
    expect(select.value).toBe("");

    mounted.rejectWrites(false);
    select.value = LATEST.id;
    change({ currentTarget: select });
    await settleRead();
    expect(mounted.location.searchParams.get("auction")).toBe(LATEST.id);
    expect(reads.urls.some((url) => url.pathname === `/api/v1/auctions/${LATEST.id}/${api}`)).toBe(
      true,
    );
  } finally {
    mounted.dispose();
    reads.dispose();
  }
});

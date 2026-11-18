// 目录读取单次重读与唯一 URL 构造点的回归断言。

import { expect, jest, spyOn, test } from "bun:test";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";

import type { Result } from "@auction/web-shared/result";
import { err, ok } from "@auction/web-shared/result";

import {
  type CacheRead,
  invalidateAfterCommand,
  invalidateAll,
  resetStoreForTest,
} from "../swr-cache";

const { catalogLevelFilterDefault } = await import("../model");
const { currentLotsUrl, fetchCurrentLots, retryOnceUnlessAuthoritative } = await import("./lots");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

function catalogResponse(): Response {
  return Response.json({
    code: "ok",
    message: "",
    data: {
      auction_id: AUCTION_ID,
      auction_status: "live",
      projection_kind: "live",
      presentation_mode: "live",
      bidding_open: true,
      opens_at: "2026-11-04T12:00:00Z",
      preview_revision: 0,
      lots: [],
      page: 1,
      page_size: 20,
      total: 0,
      category_counts: {
        all: 0,
        ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
      },
    },
  });
}

async function settleRead(): Promise<void> {
  for (let step = 0; step < 64; step += 1) await Promise.resolve();
}

test.each([
  [false, "4", null, 4_000, false, false, 2, false],
  [
    true,
    "Wed, 04 Nov 2026 12:00:04 GMT",
    "Wed, 04 Nov 2026 12:00:00 GMT",
    4_000,
    false,
    false,
    3,
    false,
  ],
  [true, "4", null, 4_000, true, false, 2, false],
  [false, "4", null, 4_000, false, true, 1, false],
  [false, "100000", null, 60_000, false, false, 1, false],
  [false, "invalid", null, 1_000, false, false, 2, false],
  [true, "4", null, 4_000, true, false, 2, true],
] as const)(
  "目录在真实 GET 边界遵守重试头、可见和取消状态（有缓存 %s，头 %s）",
  async (stale, header, date, delay, hidden, aborted, total, invalidateWhileWaiting) => {
    const documentBefore = Object.getOwnPropertyDescriptor(globalThis, "document");
    const fakeDocument = { hidden: false };
    Object.defineProperty(globalThis, "document", { configurable: true, value: fakeDocument });
    jest.useFakeTimers();
    resetStoreForTest();
    const paths: string[] = [];
    const failedRead = stale ? 2 : 1;
    const headers = new Headers({ "Retry-After": header });
    if (date !== null) headers.set("Date", date);
    const fetch = spyOn(globalThis, "fetch").mockImplementation(
      Object.assign(
        async (path: URL | RequestInfo) => {
          paths.push(String(path));
          return paths.length === failedRead
            ? Response.json(
                { code: "rate_limited", message: "", data: null },
                { status: 429, headers },
              )
            : catalogResponse();
        },
        { preconnect() {} },
      ),
    );
    const controller = new AbortController();
    const read = () =>
      fetchCurrentLots(
        AUCTION_ID,
        null,
        null,
        catalogLevelFilterDefault(),
        1,
        20,
        controller.signal,
      );
    try {
      if (stale) {
        expect((await read()).ok).toBe(true);
        invalidateAll();
      }
      const result = read();
      await settleRead();
      expect(paths).toHaveLength(failedRead);
      jest.advanceTimersByTime(delay - 1);
      await settleRead();
      expect(paths).toHaveLength(failedRead);
      if (invalidateWhileWaiting) invalidateAfterCommand();
      fakeDocument.hidden = hidden;
      if (aborted) controller.abort();
      jest.advanceTimersByTime(1);
      await settleRead();
      expect(paths).toEqual(
        Array(total).fill(`/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`),
      );
      if (aborted) expect(await result).toEqual(err("network_error"));
      else if (header === "100000" || invalidateWhileWaiting)
        expect(await result).toEqual(err("rate_limited"));
      else expect((await result).ok).toBe(true);
    } finally {
      controller.abort();
      fetch.mockRestore();
      jest.useRealTimers();
      resetStoreForTest();
      if (documentBefore === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", documentBefore);
    }
  },
);

type CatalogRead = Result<CacheRead<string>, string>;

function authoritative(value: string): CatalogRead {
  return ok({ value, staleFallback: null });
}

function stale(value: string): CatalogRead {
  return ok({ value, staleFallback: 1_000, deliverFallback: () => ok(value) });
}

/// 依次给出每次读取的结果，返回最终结果、读取次数与等待次数；读取与等待都立即就绪。
async function runCatalogRetry(reads: readonly CatalogRead[]) {
  let index = 0;
  let readCount = 0;
  let waitCount = 0;
  const controller = new AbortController();
  const read = async () => {
    readCount += 1;
    const next = reads[index];
    index += 1;
    return next ?? err("unexpected_read");
  };
  const result = await retryOnceUnlessAuthoritative(
    read,
    () => {
      waitCount += 1;
      return read();
    },
    "network_error",
    controller.signal,
  );
  return { result, readCount, waitCount };
}

test("catalog_read_retries_once_after_a_stale_fallback：目录读取在旧快照回退后只重读一次", async () => {
  // 首次读到权威值：直接返回，不等待、不重读。
  expect(await runCatalogRetry([authoritative("v2")])).toEqual({
    result: ok("v2"),
    readCount: 1,
    waitCount: 0,
  });
  // 首次只得到暂时性失败回退的旧快照：等待后重读，追上权威值。
  expect(await runCatalogRetry([stale("v1"), authoritative("v2")])).toEqual({
    result: ok("v2"),
    readCount: 2,
    waitCount: 1,
  });
  // 第二次仍是回退：接受旧快照，不再重试。
  expect(await runCatalogRetry([stale("v1"), stale("v1")])).toEqual({
    result: ok("v1"),
    readCount: 2,
    waitCount: 1,
  });
  // 第二次失败关闭：返回错误，不拿首次的旧快照兜底。
  expect(await runCatalogRetry([stale("v1"), err("not_found")])).toEqual({
    result: err("not_found"),
    readCount: 2,
    waitCount: 1,
  });
  // 首次失败（没有可回退的快照）：照旧再读一次。
  expect(await runCatalogRetry([err("read_busy"), authoritative("v1")])).toEqual({
    result: ok("v1"),
    readCount: 2,
    waitCount: 1,
  });
});

test("lot_list_url_always_requests_the_summary_projection：lot 列表 URL 总是请求摘要投影", () => {
  // 读取、预取、标记页与历史场次共用这一构造点：同一组参数得到逐字相同的 URL。
  expect(currentLotsUrl(AUCTION_ID, null, null, catalogLevelFilterDefault(), 1, 20)).toBe(
    `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`,
  );
  const levels = { maxLevel: 450, includeUnleveled: false };
  expect(currentLotsUrl(AUCTION_ID, "clo", "rare", levels, 3, 50)).toBe(
    `/api/v1/auctions/${AUCTION_ID}/lots?page=3&page_size=50&category=clo&equipment_group=rare&max_level=450&include_unleveled=false&fields=summary`,
  );
  // 逗号列表作为单个参数发出；URLSearchParams 编码逗号，服务端解码后按列表解析。
  expect(currentLotsUrl(AUCTION_ID, "clo,one", null, catalogLevelFilterDefault(), 1, 20)).toBe(
    `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&category=clo%2Cone&fields=summary`,
  );
});

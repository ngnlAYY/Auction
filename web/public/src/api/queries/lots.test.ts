// 目录读取单次重读与唯一 URL 构造点的回归断言。

import { expect, test } from "bun:test";

import type { Result } from "@auction/web-shared/result";
import { err, ok } from "@auction/web-shared/result";

import type { CacheRead } from "../swr-cache";

const { catalogLevelFilterDefault } = await import("../model");
const { currentLotsUrl, retryOnceUnlessAuthoritative } = await import("./lots");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

type CatalogRead = Result<CacheRead<string>, string>;

function authoritative(value: string): CatalogRead {
  return ok({ value, staleFallback: null });
}

function stale(value: string): CatalogRead {
  return ok({ value, staleFallback: 1_000 });
}

/// 依次给出每次读取的结果，返回最终结果、读取次数与等待次数；读取与等待都立即就绪。
async function runCatalogRetry(reads: readonly CatalogRead[]) {
  let index = 0;
  let readCount = 0;
  let waitCount = 0;
  const controller = new AbortController();
  const result = await retryOnceUnlessAuthoritative(
    async () => {
      readCount += 1;
      const next = reads[index];
      index += 1;
      return next ?? err("unexpected_read");
    },
    async () => {
      waitCount += 1;
      return true;
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
});

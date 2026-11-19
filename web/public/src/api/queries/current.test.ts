// 当前场次 URL、身份绑定与历史场次深链请求的回归断言。

import { expect, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { err, ok } from "@auction/web-shared/result";
import { resetStoreForTest } from "../swr-cache";

const { currentAuctionUrl, fetchHistoricalAuction, isTargetCurrent } = await import("./current");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const SETTLED_FIRST_PAGE = "/api/v1/auctions?page=1&page_size=200&status=settled";
const SUMMARY_PATH = `/api/v1/auctions/${AUCTION_ID}/summary`;
const LOTS_PATH = `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`;
const SUMMARY_FIXTURE = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/public/auction-summary.json",
  import.meta.url,
);

test("current_auction_url_always_requests_the_proxy_default_mode：当前场次 URL 总是请求代理默认模式", () => {
  // 看板、外壳、标记页与按场次查找共用这一构造点；解码器要求的键必须由请求显式选择，
  // 不带参数的缺省响应没有该键。
  expect(currentAuctionUrl("main")).toBe(
    "/api/v1/auctions/current?world=main&include=proxy_default_mode",
  );
  expect(currentAuctionUrl("isekai")).toBe(
    "/api/v1/auctions/current?world=isekai&include=proxy_default_mode",
  );
});

function currentWith(status: string): CurrentAuctionItemResponse {
  return {
    id: AUCTION_ID,
    world: "main",
    number: 12,
    status,
    opens_at: "2026-11-01T00:00:00Z",
    scheduled_ends_at: "2026-11-08T00:00:00Z",
    presentation_mode: "live",
    bidding_open: true,
    preview_revision: 1,
    proxy_default_mode: "fast",
  };
}

test("target_current_tolerates_only_the_live_final_switch：目标场次只容忍 live/final 切换", () => {
  for (const [current, expected] of [
    ["live", "live"],
    ["final", "final"],
    ["live", "final"],
    ["final", "live"],
  ] as const) {
    expect(
      isTargetCurrent(currentWith(current), AUCTION_ID, expected),
      `${current} vs ${expected}`,
    ).toBe(true);
  }
  for (const [current, expected] of [
    ["preview", "live"],
    ["live", "preview"],
    ["final", "settling"],
  ] as const) {
    expect(
      isTargetCurrent(currentWith(current), AUCTION_ID, expected),
      `${current} vs ${expected}`,
    ).toBe(false);
  }
  expect(isTargetCurrent(currentWith("preview"), AUCTION_ID, "preview")).toBe(true);
  const other = "0190f2b4-1c2d-4e5f-8a6b-000000000000";
  expect(isTargetCurrent(currentWith("live"), other, "live")).toBe(false);
});

test.each([true, false])(
  "活动场次深链的当前场次命中可以先于无关读取交付（命中 %s）",
  async (matched) => {
    resetStoreForTest();
    const target = currentWith("live");
    const settled = Promise.withResolvers<Response>();
    const otherWorld = Promise.withResolvers<Response>();
    const otherStarted = Promise.withResolvers<void>();
    const reads: string[] = [];
    const success = (data: unknown) => Response.json({ code: "ok", message: "", data });
    const refusal = () =>
      Response.json({ code: "internal_error", message: "", data: null }, { status: 500 });
    const fake = spyOn(globalThis, "fetch").mockImplementation(
      Object.assign(
        async (input: URL | RequestInfo) => {
          const path = String(input);
          reads.push(path);
          if (path.startsWith(`/api/v1/auctions/${AUCTION_ID}/lots?`))
            return success({
              auction_id: AUCTION_ID,
              auction_status: "live",
              projection_kind: "live",
              presentation_mode: "live",
              bidding_open: true,
              opens_at: target.opens_at,
              preview_revision: 1,
              lots: [],
              page: 1,
              page_size: 20,
              total: 0,
              category_counts: {
                all: 0,
                ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
              },
            });
          if (path === "/api/v1/worlds")
            return success({ worlds: { main: { enabled: true }, isekai: { enabled: true } } });
          if (path === currentAuctionUrl("main"))
            return success({ auction: matched ? target : null });
          if (path === currentAuctionUrl("isekai")) {
            otherStarted.resolve();
            return otherWorld.promise;
          }
          if (path === SETTLED_FIRST_PAGE) return settled.promise;
          throw new Error(`意外的场次深链请求 ${path}`);
        },
        { preconnect() {} },
      ),
    );
    const controller = new AbortController();
    const result = fetchHistoricalAuction(AUCTION_ID, controller.signal);
    try {
      const unresolved = Symbol("等待无关查询");
      const within = <T>(promise: Promise<T>) =>
        Promise.race([promise, Bun.sleep(150).then(() => unresolved)]);
      expect(await within(otherStarted.promise)).not.toBe(unresolved);
      expect(reads.filter((path) => path.endsWith("status=settled"))).toHaveLength(1);
      if (matched) {
        expect(await within(result)).toEqual(ok(target));
      } else {
        expect(await within(result)).toBe(unresolved);
        otherWorld.resolve(refusal());
        expect(await within(result)).toEqual(err("internal_error"));
      }
      expect(reads.filter((path) => path === SUMMARY_PATH)).toHaveLength(0);
    } finally {
      controller.abort();
      otherWorld.resolve(refusal());
      settled.resolve(refusal());
      await result;
      fake.mockRestore();
      resetStoreForTest();
    }
  },
);

function success(data: unknown): Response {
  return Response.json({ code: "ok", message: "", data });
}

function failure(status: number, code: string): Response {
  return Response.json({ code, message: "", data: null }, { status });
}

function settledLots(): Response {
  return success({
    auction_id: AUCTION_ID,
    auction_status: "settled",
    projection_kind: "live",
    presentation_mode: "live",
    bidding_open: false,
    opens_at: currentWith("settled").opens_at,
    preview_revision: 1,
    lots: [],
    page: 1,
    page_size: 20,
    total: 0,
    category_counts: {
      all: 0,
      ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
    },
  });
}

function directoryRow(id: string, sequence: number) {
  return {
    id,
    world_id: "isekai",
    sequence,
    status: "settled",
    scheduled_open_at: "2026-11-01T00:00:00Z",
    effective_end_at: "2026-11-08T00:00:00Z",
    lot_count: 0,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: 1,
  };
}

function settledPage(auctions: ReturnType<typeof directoryRow>[], total = auctions.length) {
  return success({ auctions, page: 1, page_size: 200, total });
}

function summaryFixture(): Response {
  return new Response(readFileSync(SUMMARY_FIXTURE, "utf8"));
}

function settledContext(world: string, number: number, endsAt: string) {
  return ok({
    ...currentWith("settled"),
    world,
    number,
    scheduled_ends_at: endsAt,
    bidding_open: false,
    proxy_default_mode: null,
  });
}

async function withHistoricalReads(
  serve: (path: string) => Response | Promise<Response>,
  verify: (read: () => ReturnType<typeof fetchHistoricalAuction>, paths: string[]) => Promise<void>,
): Promise<void> {
  resetStoreForTest();
  const paths: string[] = [];
  const fake = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: URL | RequestInfo) => {
        const path = String(input);
        paths.push(path);
        return serve(path);
      },
      { preconnect() {} },
    ),
  );
  const controller = new AbortController();
  try {
    await verify(() => fetchHistoricalAuction(AUCTION_ID, controller.signal), paths);
  } finally {
    controller.abort();
    fake.mockRestore();
    resetStoreForTest();
  }
}

test("已结算深链并行读取 lots 与目录，第一页命中时直接使用目录", async () => {
  const lots = Promise.withResolvers<Response>();
  const directory = Promise.withResolvers<Response>();
  const started = Promise.withResolvers<void>();
  await withHistoricalReads(
    (path) => {
      if (path === LOTS_PATH) {
        started.resolve();
        return lots.promise;
      }
      if (path === SETTLED_FIRST_PAGE) return directory.promise;
      throw new Error(`意外的场次深链请求 ${path}`);
    },
    async (read, paths) => {
      const result = read();
      try {
        const startedBeforeDirectory = await Promise.race([
          started.promise.then(() => true),
          Bun.sleep(500).then(() => false),
        ]);
        expect(startedBeforeDirectory, "lots 应在目录响应前发出").toBe(true);
        expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH]);
      } finally {
        lots.resolve(settledLots());
        directory.resolve(settledPage([directoryRow(AUCTION_ID, 12)]));
        await result;
      }
      expect(await result).toEqual(settledContext("isekai", 12, "2026-11-08T00:00:00Z"));
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH]);
    },
  );
});

test.each([false, true])(
  "已结算深链在目录第一页缺席时只读一次摘要（有后续页 %s）",
  async (hasMore) => {
    const rows = hasMore
      ? Array.from({ length: 200 }, (_, index) =>
          directoryRow(`0190f2b4-1c2d-4e5f-8a6b-${String(index).padStart(12, "0")}`, index + 1),
        )
      : [];
    await withHistoricalReads(
      (path) => {
        if (path === LOTS_PATH) return settledLots();
        if (path === SETTLED_FIRST_PAGE) return settledPage(rows, hasMore ? 201 : 0);
        if (path === SUMMARY_PATH) return summaryFixture();
        return failure(503, "storage_unavailable");
      },
      async (read, paths) => {
        const result = await read();
        expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH, SUMMARY_PATH]);
        expect(result).toEqual(settledContext("main", 5, "2025-11-17T09:46:40Z"));
      },
    );
  },
);

test.each([
  [404, "not_found"],
  [503, "storage_unavailable"],
] as const)("lots 读取失败时不读摘要（HTTP %s）", async (status, code) => {
  await withHistoricalReads(
    (path) => {
      if (path === LOTS_PATH) return failure(status, code);
      if (path === SETTLED_FIRST_PAGE) return settledPage([]);
      throw new Error(`意外的场次深链请求 ${path}`);
    },
    async (read, paths) => {
      expect(await read()).toEqual(err(code));
      expect(paths.filter((path) => path !== LOTS_PATH)).toEqual([SETTLED_FIRST_PAGE]);
    },
  );
});

test("目录第一页读取失败时保留错误，不把读取失败当成目录缺席", async () => {
  await withHistoricalReads(
    (path) => {
      if (path === LOTS_PATH) return settledLots();
      if (path === SETTLED_FIRST_PAGE) return failure(503, "storage_unavailable");
      throw new Error(`意外的场次深链请求 ${path}`);
    },
    async (read, paths) => {
      expect(await read()).toEqual(err("storage_unavailable"));
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH]);
    },
  );
});

test("摘要返回 404 时沿用历史场次不存在错误，下次读取可以重新请求", async () => {
  let summaryReads = 0;
  await withHistoricalReads(
    (path) => {
      if (path === LOTS_PATH) return settledLots();
      if (path === SETTLED_FIRST_PAGE) return settledPage([]);
      if (path === SUMMARY_PATH) {
        summaryReads += 1;
        return summaryReads === 1 ? failure(404, "not_found") : summaryFixture();
      }
      throw new Error(`意外的场次深链请求 ${path}`);
    },
    async (read, paths) => {
      const missing = await read();
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH, SUMMARY_PATH]);
      expect(missing).toEqual(err("拍卖目录中不存在目标拍卖"));
      expect(await read()).toEqual(settledContext("main", 5, "2025-11-17T09:46:40Z"));
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH, SUMMARY_PATH, SUMMARY_PATH]);
    },
  );
});

test.each([
  ["storage_unavailable", "storage_unavailable"],
  ["not_found", "invalid_response"],
] as const)("摘要返回 503（错误码 %s）时按读取失败处理，重试重新请求摘要", async (code, error) => {
  let summaryReads = 0;
  await withHistoricalReads(
    (path) => {
      if (path === LOTS_PATH) return settledLots();
      if (path === SETTLED_FIRST_PAGE) return settledPage([]);
      if (path === SUMMARY_PATH) {
        summaryReads += 1;
        return summaryReads === 1 ? failure(503, code) : summaryFixture();
      }
      throw new Error(`意外的场次深链请求 ${path}`);
    },
    async (read, paths) => {
      const failed = await read();
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH, SUMMARY_PATH]);
      expect(failed).toEqual(err(error));
      expect(await read()).toEqual(settledContext("main", 5, "2025-11-17T09:46:40Z"));
      expect(paths).toEqual([SETTLED_FIRST_PAGE, LOTS_PATH, SUMMARY_PATH, SUMMARY_PATH]);
    },
  );
});

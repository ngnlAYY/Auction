// 当前场次 URL 与 live/final 身份容忍规则的回归断言。

import { expect, spyOn, test } from "bun:test";

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { err, ok } from "@auction/web-shared/result";
import { resetStoreForTest } from "../swr-cache";

const { currentAuctionUrl, fetchHistoricalAuction, isTargetCurrent } = await import("./current");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

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
          if (path === "/api/v1/auctions?page=1&page_size=200&status=settled")
            return settled.promise;
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

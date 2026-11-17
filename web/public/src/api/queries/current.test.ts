// 当前场次 URL 与 live/final 身份容忍规则的回归断言。

import { expect, test } from "bun:test";

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";

const { currentAuctionUrl, isTargetCurrent } = await import("./current");

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

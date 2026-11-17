// 当前拍卖解码的回归断言：显式请求的冻结代理默认模式必须存在且在闭集内。

import { expect, test } from "bun:test";

import type { JsonObject } from "@auction/web-shared/net/wire";

const { validCurrentAuctionResponse } = await import("./auction");

function current(proxyDefaultMode: unknown | undefined): JsonObject {
  const auction: JsonObject = {
    id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000002",
    world: "main",
    number: 12,
    status: "live",
    opens_at: "2026-11-01T00:00:00Z",
    scheduled_ends_at: "2026-11-08T00:00:00Z",
    presentation_mode: "live",
    bidding_open: true,
    preview_revision: 1,
  };
  if (proxyDefaultMode !== undefined) {
    auction.proxy_default_mode = proxyDefaultMode;
  }
  return { auction };
}

test("current_auction_requires_a_frozen_proxy_default_mode：当前拍卖必须有冻结的代理默认模式", () => {
  for (const mode of ["fast", "slow"]) {
    expect(validCurrentAuctionResponse(current(mode), "main"), mode).toBe(true);
  }
  for (const invalid of [undefined, null, "", "Fast", "final_fast", 1]) {
    expect(validCurrentAuctionResponse(current(invalid), "main"), String(invalid)).toBe(false);
  }
  expect(validCurrentAuctionResponse({ auction: null }, "main")).toBe(true);
});

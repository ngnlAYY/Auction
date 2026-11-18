// 当前拍卖与目录解码的回归断言：显式请求的冻结代理默认模式必须存在且在闭集内；未限定世界
// 的 v2 目录沿用原有世界字段规则。

import { expect, test } from "bun:test";

import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonObject } from "@auction/web-shared/net/wire";
import { ok } from "@auction/web-shared/result";

const { validCurrentAuctionResponse } = await import("./auction");
const { decodeAuctionListV2 } = await import("./index");

function responseWith(payload: JsonObject): TransportResponse {
  return {
    status: 200,
    statusText: "OK",
    headers: new Headers(),
    body: null,
    text: () => "",
    json: () => ok(payload),
  };
}

function historyEnvelope(worldId: string): JsonObject {
  return {
    code: "ok",
    message: "ok",
    data: {
      auctions: [
        {
          id: "6f1c2a5e-1b2c-4d3e-8f4a-000000000003",
          world_id: worldId,
          sequence: 3,
          status: "settled",
          scheduled_open_at: "2026-11-01T00:00:00Z",
          lot_count: 1,
          sold_count: 1,
          unsold_count: 0,
          sold_amount: 100,
          state_version: 1,
        },
      ],
      page: 1,
      page_size: 20,
      total: 1,
    },
  };
}

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

test("v2_history_directory_keeps_unknown_worlds_without_a_world_filter：未限定世界的 v2 目录保持原有解码规则", () => {
  const decoded = decodeAuctionListV2(responseWith(historyEnvelope("other")), null, "settled");
  expect(decoded.ok).toBe(true);
  if (!decoded.ok) {
    throw new Error(decoded.error);
  }
  expect(decoded.value.auctions.map((entry) => entry.world_id)).toEqual(["other"]);
});

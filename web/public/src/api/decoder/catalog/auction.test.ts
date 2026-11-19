// 当前拍卖、目录与已结算摘要的闭集及字段规则回归。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonObject, parseJson } from "@auction/web-shared/net/wire";
import { err, ok } from "@auction/web-shared/result";
import { object } from "..";

const { validCurrentAuctionResponse } = await import("./auction");
const { decodeAuctionListV2, decodeSettledAuctionSummary } = await import("./index");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const SUMMARY_FIXTURE = new URL(
  "../../../../../../crates/auction-contracts/tests/fixtures/public/auction-summary.json",
  import.meta.url,
);

function responseWithText(text: string): TransportResponse {
  return {
    status: 200,
    statusText: "OK",
    headers: new Headers(),
    body: null,
    text: () => text,
    json(policy) {
      const result = parseJson(text, policy);
      return result.ok ? result : err({ kind: "decode" });
    },
  };
}

function responseWith(payload: JsonObject): TransportResponse {
  return responseWithText(JSON.stringify(payload));
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

function sharedSummary() {
  const text = readFileSync(SUMMARY_FIXTURE, "utf8");
  const parsed = parseJson(text);
  const envelope = parsed.ok ? object(parsed.value) : null;
  const data = object(envelope?.data);
  if (envelope === null || data === null) throw new Error("缺少已结算场次摘要样本");
  return { text, envelope, data };
}

test("已结算场次摘要解码接受后端序列化样本和显式 null 截止时间", () => {
  const { text, envelope, data } = sharedSummary();
  const decoded = decodeSettledAuctionSummary(responseWithText(text), AUCTION_ID);
  if (!decoded.ok) throw new Error(decoded.error);
  const actual: JsonObject = decoded.value;
  expect(actual).toEqual(data);
  const isekai = { ...decoded.value, world: "isekai", effective_end_at: null };
  expect(
    decodeSettledAuctionSummary(responseWith({ ...envelope, data: isekai }), AUCTION_ID),
  ).toEqual(ok(isekai));
});

test("已结算场次摘要解码拒绝字段缺失、多余、身份错误及非法字段", () => {
  const { envelope, data } = sharedSummary();
  for (const key of Object.keys(data)) {
    const missing = { ...data };
    delete missing[key];
    expect(
      decodeSettledAuctionSummary(responseWith({ ...envelope, data: missing }), AUCTION_ID),
    ).toEqual(err("invalid_response"));
  }
  for (const change of [
    { status: "settled" },
    { id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c" },
    { world: "main/isekai" },
    { sequence: 1.5 },
    { sequence: 0 },
    { sequence: Number.MAX_SAFE_INTEGER + 1 },
    { effective_end_at: "2025-11-17" },
    { effective_end_at: "2025-02-30T09:46:40Z" },
    { effective_end_at: "2025-11-17T09:46:40+08:00" },
  ]) {
    const response = responseWith({ ...envelope, data: { ...data, ...change } });
    expect(decodeSettledAuctionSummary(response, AUCTION_ID)).toEqual(err("invalid_response"));
  }
});

// 竞价日志场次目录回归断言：null 世界读取全部世界，合并结果按世界分组并且同期号倒序。

import { expect, spyOn, test } from "bun:test";

import type { JsonObject } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";

import { resetStoreForTest } from "../swr-cache";
import { fetchBidLogAuctionStatus, fetchBidLogAuctions } from "./bid-log";

const MAIN_LIVE_ID = "00000000-0000-4000-8000-000000000001";

function auction(
  id: string,
  worldId: string,
  sequence: number,
  status: string,
  stateVersion = 1,
): JsonObject {
  return {
    id,
    world_id: worldId,
    sequence,
    status,
    scheduled_open_at: "2026-11-01T00:00:00Z",
    lot_count: 0,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: stateVersion,
  };
}

function envelope(auctions: readonly JsonObject[]): string {
  return JSON.stringify({
    code: "ok",
    message: "ok",
    data: {
      auctions,
      page: 1,
      page_size: 200,
      total: auctions.length,
    },
  });
}

function requestedStatus(url: string): string | null {
  return new URL(url, "http://localhost").searchParams.get("status");
}

test("all_world_bid_log_directory_omits_world_and_groups_by_world：全部世界目录请求不带 world，合并后按世界分组", async () => {
  const byStatus: Readonly<Record<string, readonly JsonObject[]>> = {
    live: [
      auction(MAIN_LIVE_ID, "main", 5, "live", 2),
      auction("00000000-0000-4000-8000-000000000002", "isekai", 9, "live"),
      auction("00000000-0000-4000-8000-000000000003", "main", 3, "live"),
    ],
    final: [
      auction("00000000-0000-4000-8000-000000000004", "main", 4, "final"),
      auction("00000000-0000-4000-8000-000000000005", "isekai", 8, "final"),
      auction(MAIN_LIVE_ID, "main", 5, "final", 3),
    ],
    // 目录逐页读取期间该场次已结算，以同一 ID 再次出现；合并时必须保留一条。
    settled: [
      auction(MAIN_LIVE_ID, "main", 5, "settled", 5),
      auction("00000000-0000-4000-8000-000000000006", "isekai", 7, "settled"),
    ],
  };
  const requestedUrls: string[] = [];
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    fetchSpy.mockImplementation(
      Object.assign(
        async (input: URL | RequestInfo) => {
          const url =
            typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
          requestedUrls.push(url);
          const status = requestedStatus(url);
          const auctions = status === null ? undefined : byStatus[status];
          if (auctions === undefined) {
            return new Response("{}", { status: 500 });
          }
          return new Response(envelope(auctions), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        },
        { preconnect: () => {} },
      ),
    );

    const result = await fetchBidLogAuctions(null, new AbortController().signal);
    if (!result.ok) {
      throw new Error(result.error);
    }

    expect(requestedUrls.map((url) => requestedStatus(url)).sort()).toEqual([
      "final",
      "live",
      "settled",
    ]);
    for (const url of requestedUrls) {
      expect(new URL(url, "http://localhost").searchParams.has("world"), url).toBe(false);
    }
    expect(result.value.map((entry) => [entry.world_id, entry.sequence])).toEqual([
      ["main", 5],
      ["main", 4],
      ["main", 3],
      ["isekai", 9],
      ["isekai", 8],
      ["isekai", 7],
    ]);
    expect(
      result.value
        .filter((entry) => entry.id === MAIN_LIVE_ID)
        .map((entry) => [entry.status, entry.state_version]),
    ).toEqual([["settled", 5]]);
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
});

test.each([
  [2, 3],
  [3, 2],
])(
  "duplicate_bid_log_auctions_keep_the_highest_version：输入版本 %i、%i",
  async (first, second) => {
    const fetchSpy = spyOn(globalThis, "fetch");
    resetStoreForTest();
    try {
      fetchSpy.mockImplementation(
        Object.assign(
          async (input: URL | RequestInfo) => {
            const url =
              typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
            const auctions =
              requestedStatus(url) === "live"
                ? [
                    auction(MAIN_LIVE_ID, "main", 5, "live", first),
                    auction(MAIN_LIVE_ID, "main", 5, "live", second),
                  ]
                : [];
            return new Response(envelope(auctions));
          },
          { preconnect: () => {} },
        ),
      );
      const result = await fetchBidLogAuctions(null, new AbortController().signal);
      expect(result.ok && result.value.map((entry) => entry.state_version)).toEqual([3]);
    } finally {
      fetchSpy.mockRestore();
      resetStoreForTest();
    }
  },
);

test("all_world_bid_log_directory_rejects_unknown_world：不限世界目录出现未知世界时整体失败", async () => {
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    fetchSpy.mockImplementation(
      Object.assign(
        async (input: URL | RequestInfo) => {
          const url =
            typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
          const status = requestedStatus(url);
          const auctions = status === null ? [] : [auction(MAIN_LIVE_ID, "other", 5, status)];
          return new Response(envelope(auctions), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        },
        { preconnect: () => {} },
      ),
    );

    const result = await fetchBidLogAuctions(null, new AbortController().signal);
    expect(result).toEqual(err("invalid_response"));
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
});

test("specified_world_bid_log_directory_rejects_another_world：指定世界目录出现其他世界时整体失败", async () => {
  const fetchSpy = spyOn(globalThis, "fetch");
  resetStoreForTest();
  try {
    fetchSpy.mockImplementation(
      Object.assign(
        async () =>
          new Response(envelope([auction(MAIN_LIVE_ID, "isekai", 5, "live")]), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        { preconnect: () => {} },
      ),
    );

    const result = await fetchBidLogAuctionStatus("main", "live", new AbortController().signal);
    expect(result).toEqual(err("invalid_response"));
  } finally {
    fetchSpy.mockRestore();
    resetStoreForTest();
  }
});

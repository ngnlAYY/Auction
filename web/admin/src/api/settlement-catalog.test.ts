// 从结算目录 GET 验证跨世界微秒排序、同刻次键和后端样本。

import { expect, spyOn, test } from "bun:test";

import { jsonObjectAt, parseJsonObject } from "./api-request-fixture";
import { fetchAdminSettlementCatalog } from "./settlement-catalog";
import { adminContract, readAdminFixture, requireOk } from "./settlement-fixture.test";

function catalogWithinSameMillisecond() {
  return {
    auctions: [
      {
        auction_id: "0f8fad5b-d9cb-469f-a165-70867728950e",
        world_id: "main",
        sequence: 9,
        settled_at: "2026-11-06T10:00:00.123999Z",
      },
      {
        auction_id: "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60",
        world_id: "isekai",
        sequence: 9,
        settled_at: "2026-11-06T10:00:00.123001Z",
      },
    ],
    pagination: { page: 1, page_size: 10, total: 2 },
  };
}

function body(data: ReturnType<typeof catalogWithinSameMillisecond>): string {
  return JSON.stringify({ code: "ok", message: "ok", data });
}

function readCatalog(json: string, world: string, size: number) {
  const worldQuery = world === "all" ? "" : "&world_id=isekai";
  return readAdminFixture(
    json,
    `/api/v1/admin/settlement-catalog?page=1&page_size=${size}${worldQuery}`,
    (host, signal) => fetchAdminSettlementCatalog(host, world, null, 1, size, signal),
  );
}

test("cross_world_catalog_keeps_submillisecond_order", async () => {
  const data = catalogWithinSameMillisecond();
  expect((await readCatalog(body(data), "all", 10)).ok).toBe(true);
  data.auctions.reverse();

  // 公开 GET 会把目录校验原因归一为 invalid_response；同时观察既有的诊断入口。
  const error = spyOn(await import("./failure"), "invalidResponseError");
  try {
    expect(await readCatalog(body(data), "all", 10)).toEqual({
      ok: false,
      error: "invalid_response",
    });
    expect(error.mock.calls).toEqual([["settlement_catalog_order"]]);
  } finally {
    error.mockRestore();
  }
});

test("cross_world_catalog_breaks_exact_time_ties_by_world", async () => {
  const data = catalogWithinSameMillisecond();
  const [main, isekai] = data.auctions;
  if (main === undefined || isekai === undefined) {
    throw new Error("同刻样本必须包含两个世界");
  }
  isekai.settled_at = main.settled_at;
  const error = spyOn(await import("./failure"), "invalidResponseError");
  try {
    expect(await readCatalog(body(data), "all", 10)).toEqual({
      ok: false,
      error: "invalid_response",
    });
    expect(error.mock.calls).toEqual([["settlement_catalog_order"]]);
    data.auctions.reverse();
    expect((await readCatalog(body(data), "all", 10)).ok).toBe(true);
    expect(error.mock.calls).toEqual([["settlement_catalog_order"]]);
  } finally {
    error.mockRestore();
  }
});

test("catalog_decodes_backend_contract_fixtures_at_microsecond_precision", async () => {
  const all = adminContract("settlement-catalog");
  const catalog = requireOk(await readCatalog(all, "all", 50));
  expect(catalog.auctions[0]?.worldId).toBe("main");

  const reversed = parseJsonObject(all, "跨世界目录");
  const data = jsonObjectAt(reversed, "/data");
  if (!Array.isArray(data.auctions) || data.auctions.length !== 2) {
    throw new Error("排序样本必须有两条场次");
  }
  data.auctions.reverse();
  expect(await readCatalog(JSON.stringify(reversed), "all", 50)).toEqual({
    ok: false,
    error: "invalid_response",
  });

  const one = requireOk(await readCatalog(adminContract("settlement-catalog-world"), "isekai", 50));
  expect(one.auctions[0]?.worldId).toBe("isekai");
});

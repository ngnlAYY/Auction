// 旧严格客户端回放：v1 过滤后的响应仍可解码，未过滤的代管态与 season_closed 行必须被拒绝。
// 服务端过滤本身由 auction-storage 的 postgres_v1_owned_list_hides_inexpressible_rows 覆盖。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { OwnedListingPageSchema, OwnedListingSchema } from ".";
import { readResponse } from "./consignment-fixture";

const FIXTURES = new URL(
  "../../../../../../crates/auction-contracts/tests/fixtures/public/",
  import.meta.url,
);

/** 读取原始夹具 JSON，不经任何 schema。 */
async function readRaw(name: string): Promise<unknown> {
  return Bun.file(new URL(`consignment-v1-${name}.json`, FIXTURES)).json();
}

test("v1 过滤后的本人列表与详情经严格 schema 解码，保留待寄与运营下架", async () => {
  const { data: page } = await readResponse(
    "public/consignment-v1-owned-listings-filtered.json",
    OwnedListingPageSchema,
  );
  expect(page.total).toBe(2);
  expect(
    page.items.map(({ stored_state, effective_reason }) => [stored_state, effective_reason]),
  ).toEqual([
    ["awaiting_deposit", null],
    ["closed", "admin_delisted"],
  ]);
  const { data: detail } = await readResponse(
    "public/consignment-v1-owned-detail-closed.json",
    OwnedListingSchema,
  );
  expect(detail).toMatchObject({ effective_state: "closed", effective_reason: "admin_delisted" });
});

test("严格 schema 拒绝未过滤的代管态与 closed/season_closed 行", async () => {
  const response = (await readRaw("owned-listings-filtered")) as {
    data: { items: unknown[]; total: number };
  };
  // 基线可解码，加入不可表达行（total 同步加一）后才失败，说明过滤是必要的。
  expect(decode(OwnedListingPageSchema, response.data).ok).toBe(true);
  for (const row of ["owned-row-escrow", "owned-row-season-closed"]) {
    const polluted = {
      ...response.data,
      items: [...response.data.items, await readRaw(row)],
      total: response.data.total + 1,
    };
    expect(decode(OwnedListingPageSchema, polluted).ok).toBe(false);
  }
});

// 用 Rust 契约的共享 v2 夹具核对本人列表与详情的解码、请求目标与查询边界。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { ALICE, OWNED_PATH } from "../surface/consignment/private-fixture.test";
import { fakeFetch } from "../surface/consignment/read-fixture.test";
import {
  fetchOwnedConsignmentListingsV2,
  fetchOwnedConsignmentListingV2,
} from "./consignment-owned-v2";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/public/",
  import.meta.url,
);
const SIGNAL = () => new AbortController().signal;
const QUERY = { world: null, mode: null, state: null, page: 1, pageSize: 20 } as const;

function fixture(name: string): string {
  return readFileSync(new URL(name, FIXTURES), "utf8");
}

function json(body: string): Response {
  return new Response(body, { headers: { "content-type": "application/json" } });
}

test("共享夹具的 v2 本人列表解码，allowed_actions 原样保留", async () => {
  const body = fixture("consignment-owned-listings-v2.json");
  const { data } = JSON.parse(body) as { data: { page: number; page_size: number } };
  const transport = fakeFetch(() => json(body));
  try {
    const result = await fetchOwnedConsignmentListingsV2(
      ALICE.user_id,
      { ...QUERY, page: data.page, pageSize: data.page_size },
      SIGNAL(),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(
      result.value.items.some(({ allowed_actions: actions }) => actions.includes("withdraw")),
    ).toBe(true);
    expect(transport.requests[0]?.url.pathname).toBe(OWNED_PATH);
  } finally {
    transport.restore();
  }
});

test("列表回显的页码或页长与请求不符判响应无效；v2 闭集之外的状态不发请求", async () => {
  const body = fixture("consignment-owned-listings-v2.json");
  const { data } = JSON.parse(body) as { data: { page: number; page_size: number } };
  const transport = fakeFetch(() => json(body));
  try {
    const mismatch = await fetchOwnedConsignmentListingsV2(
      ALICE.user_id,
      { ...QUERY, page: data.page, pageSize: data.page_size + 1 },
      SIGNAL(),
    );
    expect(mismatch).toEqual({ ok: false, error: { status: 200, code: "invalid_response" } });
    const unknown = await fetchOwnedConsignmentListingsV2(
      ALICE.user_id,
      { ...QUERY, state: "not_a_state" as never },
      SIGNAL(),
    );
    expect(unknown).toEqual({ ok: false, error: { status: 400, code: "invalid_query" } });
    expect(transport.requests).toHaveLength(1);
  } finally {
    transport.restore();
  }
});

test("共享夹具的 v2 本人详情解码；返回其他挂单判响应无效", async () => {
  const raw = JSON.parse(fixture("consignment-listing-v2-owned.json")) as {
    data?: { listing_id: string };
    listing_id?: string;
  };
  const listingId = raw.data?.listing_id ?? raw.listing_id ?? "";
  const body = fixture("consignment-listing-v2-owned.json");
  const transport = fakeFetch(() => json(body));
  try {
    const ok = await fetchOwnedConsignmentListingV2(ALICE.user_id, listingId, SIGNAL());
    expect(ok.ok).toBe(true);
    const other = await fetchOwnedConsignmentListingV2(
      ALICE.user_id,
      "00000000-0000-4000-8000-0000000000ee",
      SIGNAL(),
    );
    expect(other).toEqual({ ok: false, error: { status: 200, code: "invalid_response" } });
    expect(transport.requests[0]?.url.pathname).toBe(`${OWNED_PATH}/${listingId}`);
  } finally {
    transport.restore();
  }
});

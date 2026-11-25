// 用 Rust 契约的共享 v2 夹具核对 Admin 列表与详情的解码。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { createAdminRequestHarness } from "../api-request-fixture";
import { fetchAdminListingsV2, fetchAdminListingV2, LISTINGS_V2_URL } from "./read-v2";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const SIGNAL = () => new AbortController().signal;
const NO_FILTERS = { world: "", mode: "", state: "", sellerUserId: "" };

function fixture(name: string): string {
  return readFileSync(new URL(name, FIXTURES), "utf8");
}

test("共享夹具的 v2 列表解码，止售与动作清单原样保留", async () => {
  const body = fixture("consignment-listings-v2.json");
  const { data } = JSON.parse(body) as { data: { page: number; page_size: number; total: number } };
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const requests = harness.installFetch([{ body }]);
    const result = await fetchAdminListingsV2(
      host,
      NO_FILTERS,
      BigInt(data.page),
      data.page_size,
      SIGNAL(),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.value.total).toBe(data.total);
    expect(result.value.items.some(({ hold }) => hold !== null)).toBe(true);
    expect(requests[0]?.url.startsWith(`${LISTINGS_V2_URL}?`)).toBe(true);
  } finally {
    harness.cleanup();
  }
});

test("共享夹具的 v2 详情解码；挂单身份不符判响应无效", async () => {
  const body = fixture("consignment-listing-v2-detail.json");
  const { data } = JSON.parse(body) as { data: { record: { listing: { listing_id: string } } } };
  const listingId = data.record.listing.listing_id;
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const requests = harness.installFetch([{ body }, { body }]);
    const ok = await fetchAdminListingV2(host, listingId, SIGNAL());
    expect(ok.ok).toBe(true);
    const other = await fetchAdminListingV2(host, "00000000-0000-4000-8000-0000000000ee", SIGNAL());
    expect(other).toEqual({ ok: false, error: "invalid_response" });
    expect(requests[0]?.url).toBe(`${LISTINGS_V2_URL}/${listingId}`);
  } finally {
    harness.cleanup();
  }
});

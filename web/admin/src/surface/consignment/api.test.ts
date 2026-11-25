// 验证 Admin 寄售列表与详情的请求、严格解码及读错误投影。

import { expect, test } from "bun:test";

import { createAdminRequestHarness, TEST_SESSION } from "../../api/api-request-fixture";
import { fetchAdminListingsV2, fetchAdminListingV2 } from "../../api/consignment/read-v2";
import { registerAdminSessionHost } from "../../api/session";
import {
  adminDetail,
  adminListing,
  COMMAND_ID,
  DETAIL,
  failure,
  LISTING_ID,
  LISTINGS,
  SELLER_ID,
  SNAPSHOT,
  success,
} from "./fixtures";

const FILTERS = { world: "", mode: "", state: "", sellerUserId: "" };
const SIGNAL = () => new AbortController().signal;

test("列表校验有效状态与精确卖家；不会按已记录状态误筛", async () => {
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const record = adminListing(
      {
        stored_state: "resolving",
        effective_state: "closed",
        effective_reason: "service_disabled",
        view: { kind: "closed", reason: "service_disabled", equipment: null },
      },
      { admin_actions: [] },
    );
    const page = { items: [record], page: 1, page_size: 37, total: 1 };
    const requests = harness.installFetch([
      success(page),
      success({ ...page, items: [{ ...record, seller_user_id: COMMAND_ID }] }),
      success(page),
      success({ ...page, page_size: 20 }),
    ]);
    const selected = { world: "main", mode: "direct", state: "closed", sellerUserId: SELLER_ID };
    const list = (filters: typeof selected) =>
      fetchAdminListingsV2(host, filters, 1n, 37, SIGNAL());
    expect(await list(selected)).toEqual({ ok: true, value: page });
    expect(await list(selected)).toEqual({ ok: false, error: "invalid_response" });
    expect(await list({ ...selected, state: "resolving" })).toEqual({
      ok: false,
      error: "invalid_response",
    });
    expect(await list(selected)).toEqual({ ok: false, error: "invalid_response" });
    expect(requests[0]?.url).toBe(
      `${LISTINGS}?page=1&page_size=37&world=main&mode=direct&state=closed&seller_user_id=${SELLER_ID}`,
    );
  } finally {
    harness.cleanup();
  }
});

test("列表拒绝非法筛选、201 页长和超预算偏移；接受边界与合法空页", async () => {
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const page = { items: [], page: 51, page_size: 200, total: 12 };
    const requests = harness.installFetch([
      success(page),
      success({ ...page, page: 1, page_size: 1 }),
    ]);
    const list = (page: bigint, size: number, filters = FILTERS) =>
      fetchAdminListingsV2(host, filters, page, size, SIGNAL());
    for (const filter of [
      { world: "unsupported" },
      { mode: "wrong" },
      { state: "listed" },
      { sellerUserId: "partial" },
    ]) {
      expect(await list(1n, 37, { ...FILTERS, ...filter })).toEqual({
        ok: false,
        error: "invalid_query",
      });
    }
    expect(await list(1n, 201)).toEqual({ ok: false, error: "admin_pagination_bounds" });
    expect(await list(52n, 200)).toEqual({ ok: false, error: "admin_pagination_bounds" });
    expect(await list(51n, 200)).toEqual({ ok: true, value: page });
    expect((await list(1n, 1)).ok).toBe(true);
    expect(requests.map((request) => request.url)).toEqual([
      `${LISTINGS}?page=51&page_size=200`,
      `${LISTINGS}?page=1&page_size=1`,
    ]);
  } finally {
    harness.cleanup();
  }
});

test("详情拒绝错挂单、非整数版本、重复字段及错误截断标志；小数属性可用", async () => {
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const ready = adminDetail(
      adminListing({
        stored_state: "displayed",
        effective_state: "displayed",
        view: { kind: "displayed", equipment: SNAPSHOT },
      }),
    );
    const response = success(ready);
    const requests = harness.installFetch([
      response,
      success(adminDetail(adminListing({ listing_id: COMMAND_ID }))),
      { body: response.body.replace('"version":3', '"version":3.0') },
      { body: response.body.replace('"version":3', '"version":3,"version":3') },
      success({ ...ready, conflicts_truncated: true }),
    ]);
    const fetch = () => fetchAdminListingV2(host, LISTING_ID, SIGNAL());
    expect(await fetchAdminListingV2(host, "invalid", SIGNAL())).toEqual({
      ok: false,
      error: "invalid_listing_id",
    });
    expect(await fetch()).toEqual({ ok: true, value: ready });
    for (let index = 0; index < 4; index += 1) {
      expect(await fetch()).toEqual({ ok: false, error: "invalid_response" });
    }
    expect(requests.every(({ url }) => url === DETAIL)).toBe(true);
    expect(requests).toHaveLength(5);
  } finally {
    harness.cleanup();
  }
});

test("读错误区分挂单不存在、参数错误和 503；仅可信 401 通知原认证外壳", async () => {
  const harness = createAdminRequestHarness();
  let invalidations = 0;
  const registration = registerAdminSessionHost({
    check() {},
    writeReview: {
      isCurrent: () => true,
      observe() {},
      cookieGeneration: () => TEST_SESSION.csrfToken,
      invalidate() {
        invalidations += 1;
      },
    },
    writesAllowed: () => true,
    confirmed: () => TEST_SESSION,
  });
  try {
    const replies = [
      failure(404, "listing_not_found"),
      failure(400, "invalid_query"),
      failure(503, "consignment_unavailable"),
      { status: 503, body: "" },
      failure(409, "listing_not_found"),
      {
        status: 404,
        body: failure(404, "listing_not_found").body.replace('"data":null', '"data":{}'),
      },
      failure(401, "unauthenticated"),
    ];
    harness.installFetch(replies);
    const codes = [
      "listing_not_found",
      "invalid_query",
      "consignment_unavailable",
      "http_503",
      "invalid_response",
      "invalid_response",
      "unauthenticated",
    ];
    for (const code of codes) {
      const result =
        code === "invalid_query"
          ? await fetchAdminListingsV2(registration.host, FILTERS, 1n, 20, SIGNAL())
          : await fetchAdminListingV2(registration.host, LISTING_ID, SIGNAL());
      expect(result).toEqual({ ok: false, error: code });
      expect(invalidations).toBe(code === "unauthenticated" ? 1 : 0);
    }
  } finally {
    registration.release();
    harness.cleanup();
  }
});

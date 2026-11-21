// 验证 Admin 寄售 API 请求、严格解码及真实 Admin 写守卫的失败证据。

import { expect, test } from "bun:test";

import {
  createAdminRequestHarness,
  installFixtureFetch,
  recordRequest,
  TEST_SESSION,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { prepareAdminDelist } from "../../api/consignment/delist";
import { fetchAdminListing } from "../../api/consignment/detail";
import { fetchAdminListings } from "../../api/consignment/list";
import { adminWriteFailureDisplayCode, observedResponseStatus } from "../../api/failure";
import { registerAdminSessionHost } from "../../api/session";
import {
  adminDetail,
  adminListing,
  COMMAND_ID,
  DELIST,
  DETAIL,
  delistReceipt,
  failure,
  LISTING_ID,
  LISTINGS,
  SELLER_ID,
  SNAPSHOT,
  success,
} from "./fixtures";

const FILTERS = { world: "", mode: "", state: "", sellerUserId: "" };
const VALID_REASON = "Reviewed by operations";
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
      { can_delist: false },
    );
    const page = { items: [record], page: 1, page_size: 37, total: 1 };
    const requests = harness.installFetch([
      success(page),
      success({ ...page, items: [{ ...record, seller_user_id: COMMAND_ID }] }),
      success(page),
      success({ ...page, page_size: 20 }),
    ]);
    const selected = { world: "main", mode: "direct", state: "closed", sellerUserId: SELLER_ID };
    const list = (filters: typeof selected) => fetchAdminListings(host, filters, 1n, 37, SIGNAL());
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
      fetchAdminListings(host, filters, page, size, SIGNAL());
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
    const fetch = () => fetchAdminListing(host, LISTING_ID, SIGNAL());
    expect(await fetchAdminListing(host, "invalid", SIGNAL())).toEqual({
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
          ? await fetchAdminListings(registration.host, FILTERS, 1n, 20, SIGNAL())
          : await fetchAdminListing(registration.host, LISTING_ID, SIGNAL());
      expect(result).toEqual({ ok: false, error: code });
      expect(invalidations).toBe(code === "unauthenticated" ? 1 : 0);
    }
  } finally {
    registration.release();
    harness.cleanup();
  }
});

test("下架经真实 CSRF 守卫发送并拒绝错命令、错版本与伪成功", async () => {
  const harness = createAdminRequestHarness();
  const calls: string[] = [];
  const posts: { method: string; csrfPresent: boolean; body: string }[] = [];
  const replies = [
    success(delistReceipt(COMMAND_ID)),
    success(delistReceipt(SELLER_ID)),
    success(delistReceipt(COMMAND_ID, 5)),
    { ...success(delistReceipt(COMMAND_ID)), status: 202 },
    success({ ...delistReceipt(COMMAND_ID), listing_id: SELLER_ID }),
    success({ ...delistReceipt(COMMAND_ID), operation: "withdraw" }),
  ];
  const restore = installFixtureFetch(async (input, init) => {
    const request = recordRequest(input, init);
    calls.push(new URL(request.url, "https://auction.test").pathname);
    if (calls.at(-1) === "/api/v1/admin/session") return Response.json(TEST_SESSION_BODY);
    if (calls.at(-1) !== DELIST) throw new Error("未预期的内存请求");
    posts.push({
      method: request.method,
      csrfPresent: new Headers(init?.headers).get("x-csrf-token") === TEST_SESSION.csrfToken,
      body: request.body,
    });
    const reply = replies.shift();
    if (reply === undefined) throw new Error("内存回执用尽");
    return new Response(reply.body, { status: reply.status ?? 200 });
  });
  try {
    const { host } = harness.registerHost();
    harness.installBrowserRequest();
    const submit = prepareAdminDelist(host, LISTING_ID, 3, VALID_REASON);
    expect(await submit(COMMAND_ID, SIGNAL())).toEqual({
      ok: true,
      value: delistReceipt(COMMAND_ID),
    });
    expect(await submit(SELLER_ID, SIGNAL())).toEqual({
      ok: false,
      error: { kind: "not-sent", code: "invalid_request" },
    });
    for (const status of [200, 200, 202, 200, 200]) {
      expect(await submit(COMMAND_ID, SIGNAL())).toEqual({
        ok: false,
        error: { kind: "outcome-unknown", code: "invalid_response", responseStatus: status },
      });
    }
    expect(posts).toEqual(
      Array.from({ length: 6 }, () => ({
        method: "POST",
        csrfPresent: true,
        body: JSON.stringify({ command_id: COMMAND_ID, expected_version: 3, reason: VALID_REASON }),
      })),
    );
    expect(calls).toEqual(
      Array.from({ length: 6 }, () => ["/api/v1/admin/session", DELIST]).flat(),
    );
  } finally {
    restore();
    harness.cleanup();
  }
});

test("下架保留各 HTTP 错误码与状态，503 outcome_unknown 进入未知分支", async () => {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const outcomes = [
      [404, "listing_not_found"],
      [409, "stale_version"],
      [409, "listing_not_active"],
      [409, "command_id_conflict"],
      [400, "invalid_reason"],
      [403, "csrf_invalid"],
      [503, "consignment_unavailable"],
      [503, "outcome_unknown"],
    ] as const;
    const requests = harness.installFetch(
      outcomes.flatMap(([status, code]) => [
        { body: JSON.stringify(TEST_SESSION_BODY) },
        failure(status, code),
      ]),
    );
    for (const [status, code] of outcomes) {
      const result = await prepareAdminDelist(
        host,
        LISTING_ID,
        3,
        VALID_REASON,
      )(COMMAND_ID, SIGNAL());
      if (result.ok) throw new Error("错误应答被当作写成功");
      expect(observedResponseStatus(result.error)).toBe(status);
      expect(adminWriteFailureDisplayCode(result.error)).toBe(code);
      expect(result.error.kind).toBe(
        code === "outcome_unknown" ? "outcome-unknown" : "verified-http",
      );
    }
    expect(requests).toHaveLength(2 * outcomes.length);
  } finally {
    harness.cleanup();
  }
});

test("下架接口前置校验保护私有说明；256 字节的规范输入可发送", async () => {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const requests = harness.installFetch([
      { body: JSON.stringify(TEST_SESSION_BODY) },
      success(delistReceipt(COMMAND_ID)),
    ]);
    const run = (reason: string) =>
      prepareAdminDelist(host, LISTING_ID, 3, reason)(COMMAND_ID, SIGNAL());
    expect(await run("fixture key=not-a-secret")).toEqual({
      ok: false,
      error: { kind: "not-sent", code: "invalid_reason" },
    });
    expect(requests).toEqual([]);
    const valid = `${"审".repeat(85)}!`;
    expect((await run(valid)).ok).toBe(true);
    expect(requests[1]?.body).toContain(valid);
  } finally {
    harness.cleanup();
  }
});

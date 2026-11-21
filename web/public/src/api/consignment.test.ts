// 寄售三个公开 GET 的 HTTP 契约、严格解码和取消测试。

import { expect, test } from "bun:test";
import { installFakeWindow } from "@auction/web-shared/router/test-window";

import {
  detail,
  failure,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  LISTINGS_PATH,
  pageResponse,
  response,
  STATUS_PATH,
  status,
  summary,
} from "../surface/consignment/read-fixture.test";
import { fetchConsignmentCatalog } from "./consignment-catalog";
import { fetchConsignmentDetail } from "./consignment-detail";
import { fetchConsignmentStatus } from "./consignment-status";

const QUERY = {
  world: "main",
  mode: "direct",
  category: "Staff",
  q: "a%_+ 剑",
  page: 3,
  pageSize: 37,
} as const;

test("三个公开读取按契约编码，匿名、禁缓存且成功严格解码", async () => {
  const terminal = new AbortController().signal;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(true));
    if (url.pathname === LISTINGS_PATH) return pageResponse(url, [summary()], 121);
    return response(detail());
  });
  try {
    expect(await fetchConsignmentStatus(terminal)).toEqual({ ok: true, value: status(true) });
    expect(await fetchConsignmentCatalog(QUERY, terminal)).toEqual({
      ok: true,
      value: { items: [summary()], page: 3, total: 121, page_size: 37 },
    });
    expect(await fetchConsignmentDetail(LISTING_A, terminal)).toEqual({
      ok: true,
      value: detail(),
    });
    expect(transport.requests.map(({ url }) => url.pathname + url.search)).toEqual([
      STATUS_PATH,
      `${LISTINGS_PATH}?world=main&mode=direct&category=Staff&q=a%25_%2B+%E5%89%91&page=3&page_size=37`,
      `${LISTINGS_PATH}/${LISTING_A}`,
    ]);
    for (const { init } of transport.requests) {
      expect(init).toMatchObject({
        method: "GET",
        cache: "no-store",
        credentials: "omit",
        redirect: "error",
      });
      expect(new Headers(init?.headers).get("Accept")).toBe("application/json");
    }
  } finally {
    transport.restore();
  }
});

test.each([
  [404, "consignment_disabled", "disabled"],
  [404, "listing_not_found", "not-found"],
  [400, "invalid_query", "invalid-query"],
  [503, "consignment_unavailable", "unavailable"],
  [404, "invalid_query", "invalid-response"],
  [400, "consignment_disabled", "invalid-response"],
  [500, "listing_not_found", "invalid-response"],
] as const)("HTTP %i 与 %s 组合分类为 %s", async (http, code, kind) => {
  const transport = fakeFetch(() => failure(http, code));
  try {
    expect(await fetchConsignmentDetail(LISTING_A, new AbortController().signal)).toEqual({
      ok: false,
      error: { kind },
    });
  } finally {
    transport.restore();
  }
});

test("拒绝错误信封、嵌套重复键、其他挂单和夹带私有字段的详情", async () => {
  const valid = detail();
  const invalidReplies = [
    Response.json({ code: "accepted", message: "ok", data: valid }),
    response(detail(LISTING_B)),
    response({ ...valid, equipment: { ...valid.equipment, owner: "Fake Detail Owner" } }),
    response({ ...valid, equipment: { ...valid.equipment, alt_name: undefined } }),
    new Response(
      JSON.stringify({ code: "ok", message: "ok", data: valid }).replace(
        '"price":1234',
        '"price":17,"price":1234',
      ),
    ),
  ];
  const transport = fakeFetch(
    () => invalidReplies.shift() ?? failure(503, "consignment_unavailable"),
  );
  try {
    for (let index = 0; index < 5; index += 1) {
      expect(await fetchConsignmentDetail(LISTING_A, new AbortController().signal)).toEqual({
        ok: false,
        error: { kind: "invalid-response" },
      });
    }
    expect(await fetchConsignmentDetail("invalid-listing", new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "not-found" },
    });
    expect(transport.requests).toHaveLength(5);
  } finally {
    transport.restore();
  }
});

test("能力矛盾、错页及错筛选的成功外壳均被拒绝", async () => {
  const invalidReplies = [
    response({ ...status(false), registration_ready: true }),
    response({ items: [], total: 0, page: 1, page_size: 37 }),
    response({ items: [summary(LISTING_B)], total: 120, page: 3, page_size: 37 }),
  ];
  const transport = fakeFetch(
    () => invalidReplies.shift() ?? failure(503, "consignment_unavailable"),
  );
  const terminal = new AbortController().signal;
  try {
    expect(await fetchConsignmentStatus(terminal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
    for (let index = 0; index < 2; index += 1) {
      expect(await fetchConsignmentCatalog(QUERY, terminal)).toEqual({
        ok: false,
        error: { kind: "invalid-response" },
      });
    }
  } finally {
    transport.restore();
  }
});

test("取消传到 fetch，迟到的成功结果仍作为传输失败退出", async () => {
  const controller = new AbortController();
  const pending = Promise.withResolvers<Response>();
  const transport = fakeFetch(() => pending.promise);
  try {
    const read = fetchConsignmentStatus(controller.signal);
    expect(transport.requests[0]?.init?.signal?.aborted).toBe(false);
    controller.abort();
    expect(transport.requests[0]?.init?.signal?.aborted).toBe(true);
    pending.resolve(response(status(true)));
    expect(await read).toEqual({
      ok: false,
      error: { kind: "transport", error: { kind: "network", status: null } },
    });
  } finally {
    transport.restore();
  }
});

test("已收到 503 响应头时，正文传输失败仍归为服务不可用", async () => {
  const body = new ReadableStream({
    start(controller) {
      controller.error(new Error("fake body interruption"));
    },
  });
  const transport = fakeFetch(() => new Response(body, { status: 503 }));
  try {
    expect(await fetchConsignmentStatus(new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "unavailable" },
    });
  } finally {
    transport.restore();
  }
});

test("无效成功投影只上报通用解码分类", async () => {
  const browser = installFakeWindow("/consignment");
  const transport = fakeFetch(() => response({ ...status(true), private_note: "fake detail" }));
  try {
    expect(await fetchConsignmentStatus(new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
    expect(transport.requests).toHaveLength(2);
    const notice = transport.requests[1];
    expect(notice?.init?.method).toBe("POST");
    expect(notice?.init?.body).toBeUndefined();
    expect(Object.fromEntries(notice?.url.searchParams ?? [])).toEqual({
      class: "dto_decode",
      build: "test-build",
    });
  } finally {
    transport.restore();
    browser.restore();
  }
});

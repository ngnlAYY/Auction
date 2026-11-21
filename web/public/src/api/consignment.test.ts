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

test("能力矛盾和错页的成功外壳均被拒绝", async () => {
  const invalidReplies = [
    response({ ...status(false), registration_ready: true }),
    response({ items: [], total: 0, page: 1, page_size: 37 }),
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
    expect(await fetchConsignmentCatalog(QUERY, terminal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
  } finally {
    transport.restore();
  }
});

test("HTTP 200 的状态缺少 enabled 时返回 invalid-response", async () => {
  const transport = fakeFetch(() => response({ ...status(true), enabled: undefined }));
  try {
    expect(await fetchConsignmentStatus(new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
  } finally {
    transport.restore();
  }
});

const MATCHED_QUERY = { ...QUERY, q: "Oak" };

test.each([
  ["页长", MATCHED_QUERY, summary(), 20],
  ["世界", MATCHED_QUERY, { ...summary(), world_id: "isekai" }, 37],
  ["模式", { ...MATCHED_QUERY, mode: "consigned" }, summary(), 37],
  ["分类", MATCHED_QUERY, { ...summary(), category: "Shield" }, 37],
] as const)("目录响应与请求仅有%s不匹配时拒绝", async (_, query, listing, pageSize) => {
  const transport = fakeFetch(() =>
    response({ items: [listing], total: 121, page: 3, page_size: pageSize }),
  );
  try {
    expect(await fetchConsignmentCatalog(query, new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
  } finally {
    transport.restore();
  }
});

function rawSuccess(data: unknown): string {
  return JSON.stringify({ code: "ok", message: "ok", data });
}

const rawStatus = rawSuccess(status(true));
const rawCatalog = rawSuccess({ items: [summary()], total: 121, page: 3, page_size: 20 });
const rawDetail = rawSuccess(detail());
const readCatalog = (signal: AbortSignal) =>
  fetchConsignmentCatalog({ ...QUERY, pageSize: 20 }, signal);
const readDetail = (signal: AbortSignal) => fetchConsignmentDetail(LISTING_A, signal);

test.each([
  ["状态费率", fetchConsignmentStatus, rawStatus, '"fee_bps":500', '"fee_bps":5e2'],
  ["状态最低价", fetchConsignmentStatus, rawStatus, '"min_price":1', '"min_price":1.0'],
  ["状态最高价", fetchConsignmentStatus, rawStatus, '"max_price":1000000000', '"max_price":1e9'],
  ["目录总数", readCatalog, rawCatalog, '"total":121', '"total":121.0'],
  ["目录页码", readCatalog, rawCatalog, '"page":3', '"page":3.0'],
  ["目录页长", readCatalog, rawCatalog, '"page_size":20', '"page_size":2e1'],
  ["目录标价", readCatalog, rawCatalog, '"price":1234', '"price":1234.0'],
  ["目录版本", readCatalog, rawCatalog, '"version":2', '"version":2.0000000000000001'],
  ["目录等级", readCatalog, rawCatalog, '"value":42', '"value":42.0'],
  ["详情标价", readDetail, rawDetail, '"price":1234', '"price":1234.0'],
  ["详情版本", readDetail, rawDetail, '"version":2', '"version":2.0000000000000001'],
  ["详情摘要等级", readDetail, rawDetail, '"value":42', '"value":42.0'],
  ["详情装备等级", readDetail, rawDetail, '"value":42},"tradeable"', '"value":42.0},"tradeable"'],
] as const)("%s只接受 JSON 整数词法", async (_, read, body, token, replacement) => {
  const altered = body.replace(token, replacement);
  expect(altered).not.toBe(body);
  const transport = fakeFetch(() => new Response(altered));
  try {
    expect(await read(new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
  } finally {
    transport.restore();
  }
});

test("详情装备属性接受小数和指数写法", async () => {
  const body = rawDetail.replace('"base":0', '"base":1e-3');
  const transport = fakeFetch(() => new Response(body));
  try {
    const result = await readDetail(new AbortController().signal);
    expect(result.ok && result.value.equipment.stat_groups[0]?.stats[1]).toEqual({
      name: "Damage",
      value: -2.25,
      base: 0.001,
    });
  } finally {
    transport.restore();
  }
});

test.each([
  [200, rawDetail.replaceAll("Fake Oak Staff", "Fake ~ Staff")],
  [404, '{"code":"listing_not_found","message":"Fake ~ message","data":null,"request_id":null}'],
] as const)("HTTP %i 的非法 UTF-8 使整份响应无效", async (http, body) => {
  const bytes = new TextEncoder().encode(body).map((byte) => (byte === 0x7e ? 0xff : byte));
  const transport = fakeFetch(() => new Response(bytes, { status: http }));
  try {
    expect(await readDetail(new AbortController().signal)).toEqual({
      ok: false,
      error: { kind: "invalid-response" },
    });
  } finally {
    transport.restore();
  }
});

test.each(["code", "message", "data", "request_id"] as const)(
  "404 错误外壳缺少 %s 时拒绝确定结果",
  async (missing) => {
    const envelope = {
      code: "listing_not_found",
      message: "fake server message",
      data: null,
      request_id: "fake-request-id",
    };
    const transport = fakeFetch(() =>
      Response.json({ ...envelope, [missing]: undefined }, { status: 404 }),
    );
    try {
      expect(await readDetail(new AbortController().signal)).toEqual({
        ok: false,
        error: { kind: "invalid-response" },
      });
    } finally {
      transport.restore();
    }
  },
);

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

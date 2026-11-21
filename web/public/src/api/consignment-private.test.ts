// 六条本人 API 的同源传输、闭集解码和错误分类。

import { expect, test } from "bun:test";

import {
  ALICE,
  acceptedReceipt,
  COMMAND_ID,
  COMMAND_PATH,
  EQUIPMENT_URL,
  expectedUser,
  listing,
  OWNED_PATH,
  pageData,
  receipt,
} from "../surface/consignment/private-fixture.test";
import {
  failure,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  LISTINGS_PATH,
  response,
} from "../surface/consignment/read-fixture.test";
import {
  fetchConsignmentCommand,
  fetchOwnedConsignmentListing,
  fetchOwnedConsignmentListings,
} from "./consignment-owned";
import {
  registerConsignmentListing,
  repriceConsignmentListing,
  withdrawConsignmentListing,
} from "./consignment-write";

const USER = ALICE.user_id;
const SIGNAL = () => new AbortController().signal;
const QUERY = { world: null, mode: null, state: null, page: 1, pageSize: 37 } as const;
const REGISTER = {
  command_id: COMMAND_ID,
  mode: "consigned",
  equipment_url: EQUIPMENT_URL,
  price: 101,
  expected_fee_bps: 500,
} as const;
const REPRICE = { command_id: COMMAND_ID, expected_version: 3, price: 101 };
const WITHDRAW = { command_id: COMMAND_ID, expected_version: 3 };

test("六条请求绑定用户、方法、无缓存及正确回执", async () => {
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === OWNED_PATH) return response(pageData(url, [listing("resolving")]));
    if (url.pathname === `${OWNED_PATH}/${LISTING_A}`) return response(listing("displayed"));
    if (url.pathname.startsWith(COMMAND_PATH))
      return response({ http_status: 202, code: "accepted", receipt: receipt() });
    if (url.pathname.endsWith("/price"))
      return response(
        receipt(COMMAND_ID, { operation: "reprice", listing_version: 4, disposition: "repriced" }),
      );
    if (url.pathname.endsWith("/withdraw"))
      return response(
        receipt(COMMAND_ID, {
          operation: "withdraw",
          listing_version: 4,
          state: "closed",
          disposition: "closed",
          price: null,
        }),
      );
    return acceptedReceipt();
  });
  try {
    expect((await fetchOwnedConsignmentListings(USER, QUERY, SIGNAL())).ok).toBe(true);
    expect((await fetchOwnedConsignmentListing(USER, LISTING_A, SIGNAL())).ok).toBe(true);
    expect((await fetchConsignmentCommand(USER, COMMAND_ID, SIGNAL())).ok).toBe(true);
    expect((await registerConsignmentListing(USER, REGISTER, SIGNAL())).ok).toBe(true);
    expect((await repriceConsignmentListing(USER, LISTING_A, REPRICE, SIGNAL())).ok).toBe(true);
    expect((await withdrawConsignmentListing(USER, LISTING_A, WITHDRAW, SIGNAL())).ok).toBe(true);
    expect(transport.requests.map(({ init, url }) => [init?.method, url.pathname])).toEqual([
      ["GET", OWNED_PATH],
      ["GET", `${OWNED_PATH}/${LISTING_A}`],
      ["GET", `${COMMAND_PATH}/${COMMAND_ID}`],
      ["POST", LISTINGS_PATH],
      ["PATCH", `${LISTINGS_PATH}/${LISTING_A}/price`],
      ["POST", `${LISTINGS_PATH}/${LISTING_A}/withdraw`],
    ]);
    for (const request of transport.requests) {
      expect(expectedUser(request)).toBe(USER);
      expect(request.init?.cache).toBe("no-store");
      expect(request.init?.credentials).toBe("same-origin");
      expect(request.url.href.includes(FIXTURE_FRAGMENT)).toBe(false);
    }
  } finally {
    transport.restore();
  }
});
const FIXTURE_FRAGMENT = EQUIPMENT_URL.split("/").at(-1) ?? "";

test("查询边界、非法身份、UUID、金额和版本在发包前失败关闭", async () => {
  const transport = fakeFetch(({ url }) => response(pageData(url, [], 0)));
  try {
    for (const [page, pageSize] of [
      [10_001, 1],
      [51, 200],
      [2, 37],
    ] as const) {
      expect(
        (await fetchOwnedConsignmentListings(USER, { ...QUERY, page, pageSize }, SIGNAL())).ok,
      ).toBe(true);
    }
    for (const [page, pageSize] of [
      [10_002, 1],
      [1, 201],
      [1, 0],
      [0, 37],
      [1.5, 20],
    ] as const) {
      expect(
        await fetchOwnedConsignmentListings(USER, { ...QUERY, page, pageSize }, SIGNAL()),
      ).toEqual({ ok: false, error: { status: 400, code: "invalid_query" } });
    }
    expect((await fetchOwnedConsignmentListing(USER, `${LISTING_A}/extra`, SIGNAL())).ok).toBe(
      false,
    );
    expect(
      (await fetchConsignmentCommand(USER, "00000000-0000-0000-0000-000000000000", SIGNAL())).ok,
    ).toBe(false);
    expect((await fetchOwnedConsignmentListings("invalid", QUERY, SIGNAL())).ok).toBe(false);
    expect((await registerConsignmentListing(USER, { ...REGISTER, price: 0 }, SIGNAL())).ok).toBe(
      false,
    );
    expect(
      (
        await repriceConsignmentListing(
          USER,
          LISTING_A,
          { ...REPRICE, expected_version: 0 },
          SIGNAL(),
        )
      ).ok,
    ).toBe(false);
    expect(
      (
        await withdrawConsignmentListing(
          USER,
          LISTING_A,
          { ...WITHDRAW, command_id: "bad" },
          SIGNAL(),
        )
      ).ok,
    ).toBe(false);
    expect(transport.requests).toHaveLength(3);
  } finally {
    transport.restore();
  }
});

test("严格解码拒绝含额外字段、错误筛选、错误目标、命令或成功外壳的响应", async () => {
  let next = () => response(listing("displayed", { listing_id: LISTING_B }));
  const transport = fakeFetch(() => next());
  try {
    expect((await fetchOwnedConsignmentListing(USER, LISTING_A, SIGNAL())).ok).toBe(false);
    next = () => response({ ...listing("displayed"), equipment_url: EQUIPMENT_URL });
    expect((await fetchOwnedConsignmentListing(USER, LISTING_A, SIGNAL())).ok).toBe(false);
    next = () => response({ items: [listing("displayed")], total: 1, page: 1, page_size: 37 });
    expect(
      (await fetchOwnedConsignmentListings(USER, { ...QUERY, state: "closed" }, SIGNAL())).ok,
    ).toBe(false);
    next = () => response({ http_status: 202, code: "accepted", receipt: receipt(LISTING_B) });
    expect((await fetchConsignmentCommand(USER, COMMAND_ID, SIGNAL())).ok).toBe(false);
    next = () => response(receipt());
    expect((await registerConsignmentListing(USER, REGISTER, SIGNAL())).ok).toBe(false);
    next = () => acceptedReceipt(receipt(LISTING_B));
    expect((await registerConsignmentListing(USER, REGISTER, SIGNAL())).ok).toBe(false);
    next = () =>
      new Response(
        '{"code":"ok","message":"","data":{"items":[],"total":0,"page":1,"page":1,"page_size":37}}',
        { headers: { "content-type": "application/json" } },
      );
    expect((await fetchOwnedConsignmentListings(USER, QUERY, SIGNAL())).ok).toBe(false);
    next = () => acceptedReceipt(receipt(COMMAND_ID, { price: 102 }));
    expect((await registerConsignmentListing(USER, REGISTER, SIGNAL())).ok).toBe(false);
    next = () =>
      response(
        receipt(COMMAND_ID, { operation: "reprice", listing_version: 99, disposition: "repriced" }),
      );
    expect((await repriceConsignmentListing(USER, LISTING_A, REPRICE, SIGNAL())).ok).toBe(false);
    next = () =>
      response(
        receipt(COMMAND_ID, {
          operation: "withdraw",
          listing_id: LISTING_B,
          listing_version: 4,
          state: "closed",
          disposition: "closed",
        }),
      );
    expect((await withdrawConsignmentListing(USER, LISTING_A, WITHDRAW, SIGNAL())).ok).toBe(false);
  } finally {
    transport.restore();
  }
});

const ERRORS = [
  [400, "invalid_equipment_url"],
  [400, "invalid_price"],
  [400, "invalid_command_id"],
  [400, "invalid_listing_request"],
  [400, "invalid_expected_user_id"],
  [401, "unauthenticated"],
  [403, "participant_ineligible"],
  [403, "origin_forbidden"],
  [404, "listing_not_found"],
  [404, "consignment_disabled"],
  [409, "fee_changed"],
  [409, "already_open"],
  [409, "equipment_busy"],
  [409, "history_unproven"],
  [409, "history_owner_mismatch"],
  [409, "history_alias_conflict"],
  [409, "world_unavailable"],
  [409, "stale_version"],
  [409, "listing_not_active"],
  [409, "command_id_conflict"],
  [409, "session_changed"],
  [413, "body_too_large"],
  [415, "unsupported_media_type"],
  [429, "listing_limit_reached"],
  [429, "registration_rate_limited"],
  [429, "rate_limited"],
  [503, "consignment_unavailable"],
  [503, "outcome_unknown"],
  [503, "storage_unavailable"],
] as const;

test.each(ERRORS)("HTTP %i %s 单独保留安全分类", async (status, code) => {
  const transport = fakeFetch(() => failure(status, code));
  try {
    expect(await registerConsignmentListing(USER, REGISTER, SIGNAL())).toEqual({
      ok: false,
      error: { status, code },
    });
  } finally {
    transport.restore();
  }
});

test("未知错误字符串与来源正文无回显", async () => {
  const transport = fakeFetch(() =>
    Response.json({ code: FIXTURE_FRAGMENT, message: EQUIPMENT_URL, data: null }, { status: 409 }),
  );
  try {
    expect(await registerConsignmentListing(USER, REGISTER, SIGNAL())).toEqual({
      ok: false,
      error: { status: 409, code: "unknown_error" },
    });
  } finally {
    transport.restore();
  }
});

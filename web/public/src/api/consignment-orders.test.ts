// 用 Rust 契约的共享夹具核对购买、订单读取的请求目标、回执绑定与拒绝分类。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { ALICE } from "../surface/consignment/private-fixture.test";
import { fakeFetch } from "../surface/consignment/read-fixture.test";
import {
  fetchOwnedOrder,
  fetchOwnedOrders,
  type OwnedOrderQuery,
  purchaseBusinessRejected,
  purchaseConsignmentListing,
  purchaseWriteRejected,
} from "./consignment-orders";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/public/",
  import.meta.url,
);
const SIGNAL = () => new AbortController().signal;
const QUERY: OwnedOrderQuery = { role: null, world: null, state: null, page: 1, pageSize: 20 };
const PURCHASE = "/api/v2/consignment/listings/00000000-0000-4000-8000-000000000101/purchase";
const LISTING = "00000000-0000-4000-8000-000000000101";

function fixture(name: string): string {
  return readFileSync(new URL(name, FIXTURES), "utf8");
}

function json(body: string, status = 200): Response {
  return new Response(body, { status, headers: { "content-type": "application/json" } });
}

/** 共享夹具的信封码是通用的 ok；处理器真实返回 201 created，此处按真实线协议改写。 */
function createdReceipt(): string {
  const wire = JSON.parse(fixture("consignment-purchase-receipt-v2.json"));
  return JSON.stringify({ ...wire, code: "created" });
}

function request(): { command_id: string; expected_version: number; expected_price: number } {
  return JSON.parse(fixture("consignment-purchase-request-v2.json"));
}

test("购买回执与原请求逐字相符才算成功，请求体即冻结的三个字段", async () => {
  const command = request();
  const transport = fakeFetch(() => json(createdReceipt(), 201));
  try {
    const result = await purchaseConsignmentListing(ALICE.user_id, LISTING, command, SIGNAL());
    expect(result.ok).toBe(true);
    expect(transport.requests[0]?.url.pathname).toBe(PURCHASE);
    expect(JSON.parse(String(transport.requests[0]?.init?.body))).toEqual(command);
  } finally {
    transport.restore();
  }
});

test("回执答非所问（价格不同或版本未前进）按结果未知，保留原命令重放", async () => {
  const command = request();
  const transport = fakeFetch(() => json(createdReceipt(), 201));
  try {
    const priced = await purchaseConsignmentListing(
      ALICE.user_id,
      LISTING,
      { ...command, expected_price: command.expected_price + 1 },
      SIGNAL(),
    );
    const stale = await purchaseConsignmentListing(
      ALICE.user_id,
      LISTING,
      { ...command, expected_version: 6 },
      SIGNAL(),
    );
    for (const result of [priced, stale]) {
      expect(result).toEqual({ ok: false, error: { status: 201, code: "invalid_response" } });
      if (!result.ok) expect(purchaseWriteRejected(result.error)).toBe(false);
    }
  } finally {
    transport.restore();
  }
});

test("只有契约配对的确定拒绝可结案；outcome_unknown、504、断线与解码失败都不可", () => {
  const decided = [
    { status: 409, code: "stale_version" },
    { status: 409, code: "price_changed" },
    { status: 403, code: "self_purchase" },
    { status: 503, code: "funding_unavailable" },
  ];
  // 处理器在调用用例前的 400 拒绝（价格、请求、路径）没有写账本，可结案。
  for (const code of ["invalid_price", "invalid_listing_request", "invalid_listing_id"]) {
    expect(purchaseWriteRejected({ status: 400, code })).toBe(true);
  }
  const undecided = [
    { status: 503, code: "outcome_unknown" },
    { status: 504, code: "timeout" },
    { status: 0, code: "network_error" },
    { status: 201, code: "invalid_response" },
    { status: 500, code: "unknown_error" },
    // 码与状态不配对不能证明首次写入已被拒绝。
    { status: 503, code: "stale_version" },
  ];
  for (const error of decided) expect(purchaseBusinessRejected(error)).toBe(true);
  for (const error of undecided) expect(purchaseWriteRejected(error)).toBe(false);
});

test("本人订单列表与详情解码；筛选或合同身份与请求不符判响应无效", async () => {
  const body = fixture("consignment-orders-v2.json");
  const transport = fakeFetch(() => json(body));
  try {
    const page = await fetchOwnedOrders(ALICE.user_id, QUERY, SIGNAL());
    expect(page.ok).toBe(true);
    // 夹具含买卖两种角色；只请求 buyer 时回显 seller 订单必须被拒绝。
    const filtered = await fetchOwnedOrders(ALICE.user_id, { ...QUERY, role: "buyer" }, SIGNAL());
    expect(filtered).toEqual({ ok: false, error: { status: 200, code: "invalid_response" } });
  } finally {
    transport.restore();
  }
  const single = fakeFetch(() => json(fixture("consignment-order-v2-buyer.json")));
  try {
    const other = await fetchOwnedOrder(
      ALICE.user_id,
      "00000000-0000-4000-8000-0000000000ee",
      SIGNAL(),
    );
    expect(other).toEqual({ ok: false, error: { status: 200, code: "invalid_response" } });
  } finally {
    single.restore();
  }
});

test("非本人订单的 404 listing_not_found 原样传出，不当作空订单", async () => {
  const transport = fakeFetch(() =>
    json(
      JSON.stringify({ code: "listing_not_found", message: "", data: null, request_id: null }),
      404,
    ),
  );
  try {
    const result = await fetchOwnedOrder(
      ALICE.user_id,
      "00000000-0000-4000-8000-000000000501",
      SIGNAL(),
    );
    expect(result).toEqual({ ok: false, error: { status: 404, code: "listing_not_found" } });
  } finally {
    transport.restore();
  }
});

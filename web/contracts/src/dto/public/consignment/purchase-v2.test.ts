// 验证购买请求、回执与拒绝码：命令 ID 与版本绑定、重放与确定拒绝。

import { expect, test } from "bun:test";

import * as v from "valibot";

import { decode, struct } from "../../../wire";
import { readFixture, readResponse } from "./consignment-fixture";
import {
  isTrustedPurchaseRejection,
  PurchaseReceiptV2Schema,
  PurchaseRejectionCodeV2Schema,
  PurchaseRequestV2Schema,
  purchaseReceiptAnswers,
  purchaseRejectionStatus,
} from "./purchase-v2";

const REQUEST = "public/consignment-purchase-request-v2.json";
const RECEIPT = "public/consignment-purchase-receipt-v2.json";
const ENUMS = "public/consignment-order-enums-v2.json";

type Wire = Record<string, unknown>;

async function receiptWire(): Promise<Wire> {
  const { data } = await readResponse(RECEIPT, v.record(v.string(), v.unknown()));
  return structuredClone(data);
}

test("购买回执必须回答请求：命令 ID、价格相同，挂单版本向前推进", async () => {
  const request = await readFixture(REQUEST, PurchaseRequestV2Schema);
  const { data: receipt } = await readResponse(RECEIPT, PurchaseReceiptV2Schema);
  expect(purchaseReceiptAnswers(request, receipt)).toBe(true);
  expect(
    purchaseReceiptAnswers(request, {
      ...receipt,
      command_id: "00000000-0000-4000-8000-00000000ffff",
    }),
  ).toBe(false);
  expect(purchaseReceiptAnswers(request, { ...receipt, price: 102 })).toBe(false);
  for (const listing_version of [5, 4, 1]) {
    expect(purchaseReceiptAnswers(request, { ...receipt, listing_version })).toBe(false);
  }
  // 重放返回原回执，版本比预期版本多走几步仍是同一回执。
  expect(purchaseReceiptAnswers(request, { ...receipt, listing_version: 9 })).toBe(true);
});

test("购买回执只接受 purchased、sale_pending 与 awaiting_payment 的组合", async () => {
  const wire = await receiptWire();
  expect(decode(PurchaseReceiptV2Schema, wire).ok).toBe(true);
  for (const changes of [
    { schema_version: 1 },
    { state: "available" },
    { order_state: "awaiting_handoff" },
    { disposition: "closed" },
    { operation: "withdraw" },
    { contract_version: 0 },
    { price: 0 },
    { seller_name: "FixtureSeller" },
  ]) {
    expect(decode(PurchaseReceiptV2Schema, { ...wire, ...changes }).ok).toBe(false);
  }
  const missing = { ...wire };
  delete missing.schema_version;
  expect(decode(PurchaseReceiptV2Schema, missing).ok).toBe(false);
});

test("购买请求强制 UUIDv4、正版本与 1 到十亿的预期价", async () => {
  const request = await readFixture(REQUEST, PurchaseRequestV2Schema);
  expect(request).toEqual({
    command_id: "00000000-0000-4000-8000-000000000510",
    expected_version: 5,
    expected_price: 101,
  });
  for (const changes of [
    { command_id: "00000000-0000-5000-8000-000000000510" },
    { expected_version: 0 },
    { expected_price: 0 },
    { expected_price: 1_000_000_001 },
    { buyer_user_id: "00000000-0000-4000-8000-000000000521" },
  ]) {
    expect(decode(PurchaseRequestV2Schema, { ...request, ...changes }).ok).toBe(false);
  }
});

test("购买拒绝码与状态和共享夹具一致，只有 funding_unavailable 是可信 503", async () => {
  const enums = await readFixture(
    ENUMS,
    struct({ purchase_rejections: v.array(struct({ code: v.string(), status: v.number() })) }),
  );
  const options: readonly string[] = PurchaseRejectionCodeV2Schema.options;
  expect([...options].sort()).toEqual(enums.purchase_rejections.map(({ code }) => code).sort());
  for (const { code, status } of enums.purchase_rejections) {
    expect(purchaseRejectionStatus(v.parse(PurchaseRejectionCodeV2Schema, code))).toBe(status);
  }
  expect(isTrustedPurchaseRejection("funding_unavailable")).toBe(true);
  expect(isTrustedPurchaseRejection("outcome_unknown")).toBe(false);
});

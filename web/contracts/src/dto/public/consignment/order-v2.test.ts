// 验证订单读模型：金额守恒、按角色省略、阶段矩阵与公开字段白名单。

import { expect, test } from "bun:test";

import * as v from "valibot";

import { decode, struct } from "../../../wire";
import { ListingSummaryV2PageSchema } from "./catalog-v2";
import { readFixture, readResponse } from "./consignment-fixture";
import {
  DeliveryStageV2Schema,
  LegStageV2Schema,
  OrderActionV2Schema,
  OrderKindV2Schema,
  OrderPageV2Schema,
  OrderRoleV2Schema,
  OrderStateV2Schema,
  OrderV2Schema,
  PaymentStageV2Schema,
} from "./order-v2";

const BUYER = "public/consignment-order-v2-buyer.json";
const SELLER = "public/consignment-order-v2-seller.json";
const WITHDRAWAL = "public/consignment-order-v2-withdrawal.json";
const PAGE = "public/consignment-orders-v2.json";
const CATALOG = "public/consignment-listings-v2.json";
const MATRIX = "public/consignment-order-v2-stage-matrix.json";
const ENUMS = "public/consignment-order-enums-v2.json";

type Wire = Record<string, unknown>;

function record(value: unknown): Wire {
  return v.parse(v.record(v.string(), v.unknown()), value);
}

async function wire(file: string): Promise<Wire> {
  const { data } = await readResponse(file, v.unknown());
  return record(structuredClone(data));
}

const ok = (input: Wire) => decode(OrderV2Schema, input).ok;

test("订单夹具解码，卖方视图带 F 与 S，买方与撤回订单不带", async () => {
  const buyer = await readResponse(BUYER, OrderV2Schema);
  const seller = await readResponse(SELLER, OrderV2Schema);
  const withdrawal = await readResponse(WITHDRAWAL, OrderV2Schema);
  expect(seller.data).toMatchObject({ price: 101, fee: 5, seller_amount: 96 });
  expect("fee" in buyer.data).toBe(false);
  expect("seller_amount" in withdrawal.data).toBe(false);
  expect(withdrawal.data).toMatchObject({ price: null, deadline_at: null });
  const page = await readResponse(PAGE, OrderPageV2Schema);
  expect(page.data.items).toHaveLength(2);
});

test("卖方金额守恒且在范围内，F=0 合法", async () => {
  const seller = await wire(SELLER);
  expect(ok({ ...seller, price: 1, fee: 0, seller_amount: 1 })).toBe(true);
  for (const changes of [
    { fee: 6 },
    { seller_amount: 95 },
    { fee: 101, seller_amount: 1 },
    { fee: 101, seller_amount: 0 },
    { fee: -1, seller_amount: 102 },
    { price: 0, fee: 0, seller_amount: 0 },
    { price: 1_000_000_001, fee: 5, seller_amount: 999_999_996 },
    { fee: null },
    { price: null },
    { deadline_at: null },
  ]) {
    expect(ok({ ...seller, ...changes })).toBe(false);
  }
  for (const key of ["fee", "seller_amount"]) {
    const missing = { ...seller };
    delete missing[key];
    expect(ok(missing)).toBe(false);
  }
});

test("买方视图与撤回订单不得带私有金额或价格", async () => {
  const buyer = await wire(BUYER);
  const withdrawal = await wire(WITHDRAWAL);
  expect(ok({ ...buyer, fee: 5, seller_amount: 96 })).toBe(false);
  expect(ok({ ...buyer, fee: 5 })).toBe(false);
  for (const changes of [
    { price: 101 },
    { deadline_at: "2026-11-28T18:10:00Z" },
    { fee: 0 },
    { role: "buyer" },
  ]) {
    expect(ok({ ...withdrawal, ...changes })).toBe(false);
  }
});

test("阶段矩阵与领域投影共用夹具：合法组合通过，矛盾组合失败", async () => {
  const matrix = await readFixture(
    MATRIX,
    struct({
      cases: v.array(
        struct({ name: v.string(), facts: struct({ kind: v.string() }), stages: v.unknown() }),
      ),
      invalid: v.array(struct({ name: v.string(), kind: v.string(), stages: v.unknown() })),
    }),
  );
  const seller = await wire(SELLER);
  const withdrawal = await wire(WITHDRAWAL);
  const stamped = (kind: string, stages: unknown): Wire => ({
    ...(kind === "withdrawal" ? withdrawal : seller),
    ...record(stages),
  });
  expect(matrix.cases.length).toBeGreaterThanOrEqual(20);
  for (const item of matrix.cases) {
    expect(ok(stamped(item.facts.kind, item.stages)), item.name).toBe(true);
  }
  for (const item of matrix.invalid) {
    expect(ok(stamped(item.kind, item.stages)), item.name).toBe(false);
  }
  const buyer = await wire(BUYER);
  expect(ok({ ...buyer, allowed_actions: ["claim"] })).toBe(false);
  expect(ok({ ...buyer, order_state: "paid" })).toBe(false);
});

test("公开 DTO 只含白名单字段，夹带私密字段一律失败", async () => {
  const orderKeys = [
    "allowed_actions",
    "contract_id",
    "deadline_at",
    "delivery_stage",
    "equipment_id",
    "kind",
    "listing_id",
    "order_state",
    "payment_stage",
    "payout_stage",
    "price",
    "refund_stage",
    "role",
    "version",
    "world",
  ];
  const buyer = await wire(BUYER);
  expect(Object.keys(buyer).sort()).toEqual(orderKeys);
  expect(Object.keys(await wire(SELLER)).sort()).toEqual(
    [...orderKeys, "fee", "seller_amount"].sort(),
  );
  const catalog = await readResponse(CATALOG, ListingSummaryV2PageSchema);
  const item = record(structuredClone(catalog.data.items[0]));
  for (const key of [
    "buyer_user_id",
    "seller_user_id",
    "buyer_name",
    "reservation_code",
    "equipment_key",
    "external_mid",
    "mail_body",
  ]) {
    for (const name of [BUYER, SELLER, WITHDRAWAL]) {
      expect(ok({ ...(await wire(name)), [key]: "x" }), `${name} ${key}`).toBe(false);
    }
    const page = decode(ListingSummaryV2PageSchema, {
      items: [{ ...item, [key]: "x" }],
      total: 1,
      page: 1,
      page_size: 20,
    });
    expect(page.ok, key).toBe(false);
  }
});

test("订单枚举与共享夹具逐字一致", async () => {
  const strings = v.array(v.string());
  const enums = await readFixture(
    ENUMS,
    struct({
      order_kinds: strings,
      order_roles: strings,
      order_states: strings,
      delivery_stages: strings,
      payment_stages: strings,
      leg_stages: strings,
      order_actions: strings,
    }),
  );
  const same = (options: readonly string[], expected: string[]) =>
    expect([...options].sort()).toEqual([...expected].sort());
  same(OrderKindV2Schema.options, enums.order_kinds);
  same(OrderRoleV2Schema.options, enums.order_roles);
  same(OrderStateV2Schema.options, enums.order_states);
  same(DeliveryStageV2Schema.options, enums.delivery_stages);
  same(PaymentStageV2Schema.options, enums.payment_stages);
  same(LegStageV2Schema.options, enums.leg_stages);
  same(OrderActionV2Schema.options, enums.order_actions);
});

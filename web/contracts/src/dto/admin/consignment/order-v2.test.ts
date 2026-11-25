// 验证 Admin 订单与证据分页：金额守恒、买卖双方不同账号、证据条目自洽。

import { expect, test } from "bun:test";

import * as v from "valibot";

import { decode } from "../../../wire";
import { readResponse } from "../../public/consignment/consignment-fixture";
import {
  AdminOrderV2Schema,
  OrderEvidenceItemV2Schema,
  OrderEvidencePageV2Schema,
} from "./order-v2";

const ORDER = "admin/consignment-order-v2.json";
const EVIDENCE = "admin/consignment-order-v2-evidence.json";

type Wire = Record<string, unknown>;

async function wire(file: string): Promise<Wire> {
  const { data } = await readResponse(file, v.record(v.string(), v.unknown()));
  return structuredClone(data);
}

test("Admin 订单夹具解码，金额守恒且买卖双方不同账号", async () => {
  await readResponse(ORDER, AdminOrderV2Schema);
  const order = await wire(ORDER);
  for (const changes of [
    { fee: 6 },
    { fee: 101, seller_amount: 1 },
    { buyer_user_id: "00000000-0000-4000-8000-000000000522" },
    { buyer_user_id: null },
  ]) {
    expect(decode(AdminOrderV2Schema, { ...order, ...changes }).ok).toBe(false);
  }
});

test("证据分页解码，条目不自洽或夹带原信正文时失败", async () => {
  const { data: page } = await readResponse(EVIDENCE, OrderEvidencePageV2Schema);
  expect(page.items.map(({ kind }) => kind)).toEqual([
    "round",
    "attempt",
    "return_take",
    "observation",
    "cash_use",
    "blocker",
  ]);
  const items = (await wire(EVIDENCE)).items as Wire[];
  const bad = (index: number, changes: Wire) =>
    decode(OrderEvidenceItemV2Schema, { ...items[index], ...changes }).ok;
  expect(bad(1, { write_started: false })).toBe(false);
  expect(bad(2, { fence: 1 })).toBe(false);
  expect(bad(2, { state: "write_started" })).toBe(false);
  expect(bad(5, { mail_body: "x" })).toBe(false);
  expect(bad(3, { digest: "A".repeat(64) })).toBe(false);
});

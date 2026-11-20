// 验证玩家写请求、写成功回执和首次命令结果的线协议。

import { expect, expectTypeOf, test } from "bun:test";

import type { InferOutput } from "valibot";

import { decode } from "../../../wire";
import {
  CommandReceiptSchema,
  type RegisterListing,
  RegisterListingSchema,
  type RepriceListing,
  RepriceListingSchema,
  StoredCommandResultSchema,
  type WithdrawListing,
  WithdrawListingSchema,
} from ".";
import { StoredCommandReceiptSchema } from "./commands";
import { readFixture, readResponse } from "./consignment-fixture";

const REGISTER = "public/consignment-register-request.json";
const REPRICE = "public/consignment-reprice-request.json";
const WITHDRAW = "public/consignment-withdraw-request.json";
const REGISTERED = "public/consignment-register-receipt.json";
const REPRICED = "public/consignment-reprice-receipt.json";
const WITHDRAWN = "public/consignment-withdraw-receipt.json";
const ACCEPTED = "public/consignment-command-result.json";
const REJECTED = "public/consignment-command-rejected.json";

test("登记、改价与取消请求夹具和 interface 对齐", async () => {
  expectTypeOf<InferOutput<typeof RegisterListingSchema>>().toEqualTypeOf<RegisterListing>();
  expectTypeOf<InferOutput<typeof RepriceListingSchema>>().toEqualTypeOf<RepriceListing>();
  expectTypeOf<InferOutput<typeof WithdrawListingSchema>>().toEqualTypeOf<WithdrawListing>();
  const registration = await readFixture(REGISTER, RegisterListingSchema);
  const { mode, price, expected_fee_bps } = registration;
  expect({ mode, price, expected_fee_bps }).toEqual({
    mode: "consigned",
    price: 101,
    expected_fee_bps: 500,
  });
  expect(await readFixture(REPRICE, RepriceListingSchema)).toMatchObject({
    expected_version: 1,
    price: 102,
  });
  expect(await readFixture(WITHDRAW, WithdrawListingSchema)).toMatchObject({ expected_version: 2 });
});

test("写请求拒绝错误命令身份、非安全整数、错误类型和额外键", async () => {
  const registration = await readFixture(REGISTER, RegisterListingSchema);
  const reprice = await readFixture(REPRICE, RepriceListingSchema);
  const withdraw = await readFixture(WITHDRAW, WithdrawListingSchema);
  const { equipment_url: _equipmentUrl, ...missingEquipmentUrl } = registration;
  expect(decode(RegisterListingSchema, missingEquipmentUrl).ok).toBe(false);
  expect(decode(RegisterListingSchema, { ...registration, equipment_url: null }).ok).toBe(false);
  expect(decode(RegisterListingSchema, { ...registration, mode: { consigned: null } }).ok).toBe(
    false,
  );
  expect(
    decode(RegisterListingSchema, { ...registration, seller_user_id: "fixture-only" }).ok,
  ).toBe(false);
  expect(decode(RegisterListingSchema, { ...registration, expected_fee_bps: 500.5 }).ok).toBe(
    false,
  );
  for (const price of [0, -1, "101", 101.5, 1_000_000_001, Number.MAX_SAFE_INTEGER + 1]) {
    expect(decode(RegisterListingSchema, { ...registration, price }).ok).toBe(false);
    expect(decode(RepriceListingSchema, { ...reprice, price }).ok).toBe(false);
  }
  for (const price of [1, 1_000_000_000]) {
    expect(decode(RegisterListingSchema, { ...registration, price }).ok).toBe(true);
    expect(decode(RepriceListingSchema, { ...reprice, price }).ok).toBe(true);
  }
  expect(decode(WithdrawListingSchema, { ...withdraw, command_id: "invalid" }).ok).toBe(false);
  expect(decode(WithdrawListingSchema, { ...withdraw, listing_id: "fixture-only" }).ok).toBe(false);
  expect(
    decode(WithdrawListingSchema, { ...withdraw, expected_version: Number.MAX_SAFE_INTEGER + 1 })
      .ok,
  ).toBe(false);
  expect(decode(WithdrawListingSchema, { ...withdraw, expected_version: 0 }).ok).toBe(false);
  expect(
    decode(WithdrawListingSchema, { ...withdraw, expected_version: Number.MAX_SAFE_INTEGER }).ok,
  ).toBe(true);
});

test("登记请求必须显式提供预期费率", async () => {
  const registration = await readFixture(REGISTER, RegisterListingSchema);
  const { expected_fee_bps: _fee, ...request } = registration;
  expect(decode(RegisterListingSchema, request).ok).toBe(false);
  expect(decode(RegisterListingSchema, { ...request, expected_fee_bps: 0 }).ok).toBe(true);
});

test("写回执夹具区分已登记、已改价、未变化和已关闭", async () => {
  const registered = await readResponse(REGISTERED, CommandReceiptSchema);
  const repriced = await readResponse(REPRICED, CommandReceiptSchema);
  const unchanged = await readResponse(
    "public/consignment-unchanged-receipt.json",
    CommandReceiptSchema,
  );
  const withdrawn = await readResponse(WITHDRAWN, CommandReceiptSchema);
  expect(registered).toMatchObject({
    code: "accepted",
    data: {
      operation: "register",
      disposition: "registered",
      state: "resolving",
      listing_version: 1,
      price: 101,
    },
  });
  expect(repriced.data).toMatchObject({
    operation: "reprice",
    disposition: "repriced",
    price: 102,
  });
  expect(unchanged.data).toMatchObject({
    operation: "reprice",
    disposition: "unchanged",
    price: 102,
  });
  expect(withdrawn.data).toMatchObject({
    operation: "withdraw",
    disposition: "closed",
    state: "closed",
    price: 102,
  });
});

test("写回执拒绝未知或矛盾处置、缺失可空键和不合法目标", async () => {
  const { data: registered } = await readResponse(REGISTERED, CommandReceiptSchema);
  const { data: repriced } = await readResponse(REPRICED, CommandReceiptSchema);
  const { data: withdrawn } = await readResponse(WITHDRAWN, CommandReceiptSchema);
  const { price: _price, ...missingPrice } = withdrawn;
  expect(decode(CommandReceiptSchema, missingPrice).ok).toBe(false);
  expect(decode(CommandReceiptSchema, { ...withdrawn, price: null }).ok).toBe(true);
  for (const invalid of [
    { ...registered, listing_id: null },
    { ...registered, listing_version: 2 },
    { ...registered, state: "displayed" },
    { ...registered, price: null },
    { ...registered, operation: "apply_lookup" },
    { ...registered, disposition: "pending" },
    { ...registered, disposition: "rejected" },
    { ...registered, equipment_url: "fixture-only" },
    { ...repriced, state: "closed" },
    { ...repriced, price: null },
    { ...withdrawn, listing_version: null },
    { ...withdrawn, operation: "reprice" },
  ]) {
    expect(decode(CommandReceiptSchema, invalid).ok).toBe(false);
  }
});

test("已登记回执拒绝其他有效操作，已关闭回执拒绝活动态", async () => {
  const { data: registered } = await readResponse(REGISTERED, CommandReceiptSchema);
  const { data: withdrawn } = await readResponse(WITHDRAWN, CommandReceiptSchema);
  expect(decode(CommandReceiptSchema, { ...registered, operation: "reprice" }).ok).toBe(false);
  expect(decode(CommandReceiptSchema, { ...withdrawn, state: "displayed" }).ok).toBe(false);
});

test("查命令夹具区分外层读取成功与首次 202 或 409", async () => {
  const accepted = await readResponse(ACCEPTED, StoredCommandResultSchema);
  const rejected = await readResponse(REJECTED, StoredCommandResultSchema);
  expect(accepted).toMatchObject({
    code: "ok",
    data: { http_status: 202, code: "accepted", receipt: { disposition: "registered" } },
  });
  expect(rejected).toMatchObject({
    code: "ok",
    data: {
      http_status: 409,
      code: "fee_changed",
      receipt: {
        disposition: "rejected",
        listing_id: null,
        listing_version: null,
        state: null,
        price: null,
      },
    },
  });
});

test("查命令拒绝未知结果码、覆盖原状态或误把拒绝解成写成功", async () => {
  const { data: accepted } = await readResponse(ACCEPTED, StoredCommandResultSchema);
  const { data: rejected } = await readResponse(REJECTED, StoredCommandResultSchema);
  const { listing_id: _listingId, ...missingListingId } = rejected.receipt;
  expect(decode(StoredCommandReceiptSchema, missingListingId).ok).toBe(false);
  expect(decode(StoredCommandReceiptSchema, { ...rejected.receipt, listing_version: "1" }).ok).toBe(
    false,
  );
  expect(decode(CommandReceiptSchema, rejected.receipt).ok).toBe(false);
  for (const invalid of [
    { ...rejected, http_status: 200 },
    { ...rejected, http_status: "409" },
    { ...rejected, code: "command_id_conflict" },
    { ...rejected, code: "constructor" },
    { ...rejected, receipt: accepted.receipt },
    { ...accepted, receipt: rejected.receipt },
    { ...accepted, http_status: 200, code: "ok" },
  ]) {
    expect(decode(StoredCommandResultSchema, invalid).ok).toBe(false);
  }
});

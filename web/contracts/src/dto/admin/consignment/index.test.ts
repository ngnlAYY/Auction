// 验证运营寄售列表、详情、碰撞记录与下架的线协议。

import { expect, expectTypeOf, test } from "bun:test";

import * as v from "valibot";

import { decode } from "../../../wire";
import { CommandReceiptSchema } from "../../public/consignment";
import { readFixture, readResponse } from "../../public/consignment/consignment-fixture";
import {
  type AdminDelistListing,
  AdminDelistListingSchema,
  AdminListingDetailSchema,
  AdminListingPageSchema,
  AdminListingSchema,
  ListingAuctionConflictSchema,
} from ".";

const PAGE = "admin/consignment-listings.json";
const DETAIL = "admin/consignment-listing-detail.json";
const DELIST = "admin/consignment-delist-request.json";

// 明显的模拟链接，仅用于验证写入和响应都拒绝运营说明中的装备路径。
const EQUIPMENT_LINK = "https://example.invalid/EQUIP/42/0000000000";

test("运营列表和详情夹具保留安全碰撞与下架能力", async () => {
  const { data: page } = await readResponse(PAGE, AdminListingPageSchema);
  const { data: detail } = await readResponse(DETAIL, AdminListingDetailSchema);
  expect(
    page.items.map(({ can_delist, listing }) => [can_delist, listing.allowed_actions]),
  ).toEqual([
    [false, []],
    [true, []],
  ]);
  expect(detail.record).toMatchObject({
    can_delist: false,
    listing: { effective_state: "closed" },
  });
  expect(detail.conflicts[0]?.lot_id).toBeDefined();
  expect(typeof detail.conflicts_truncated).toBe("boolean");
});

test("运营记录拒绝卖家动作、终态下架、缺失说明键和说明中的装备链接", async () => {
  const { data: page } = await readResponse(PAGE, AdminListingPageSchema);
  const [closed, active] = page.items;
  if (!closed || !active) throw new Error("运营夹具须有关闭项及活动项");
  const { admin_delist_reason: _delistReason, ...missingDelistReason } = active;
  expect(decode(AdminListingSchema, { ...active, admin_delist_reason: null }).ok).toBe(true);
  for (const invalid of [
    missingDelistReason,
    { ...active, listing: { ...active.listing, allowed_actions: ["reprice"] } },
    { ...closed, can_delist: true },
    { ...closed, admin_delist_reason: EQUIPMENT_LINK },
    { ...closed, seller_user_id: 42 },
  ]) {
    expect(decode(AdminListingSchema, invalid).ok).toBe(false);
  }
  expect(
    decode(AdminListingPageSchema, { ...page, items: [{ ...closed, can_delist: true }] }).ok,
  ).toBe(false);
});

test("运营详情拒绝缺少截断标记、超 50 条碰撞及多余碰撞字段", async () => {
  const { data: detail } = await readResponse(DETAIL, AdminListingDetailSchema);
  const [conflict] = detail.conflicts;
  if (!conflict) throw new Error("运营详情样本须有关联拍卖");
  expect(decode(ListingAuctionConflictSchema, conflict).ok).toBe(true);
  expect(
    decode(ListingAuctionConflictSchema, { ...conflict, equipment_url: "fixture-only" }).ok,
  ).toBe(false);
  expect(decode(ListingAuctionConflictSchema, { ...conflict, detected_at: null }).ok).toBe(false);
  const { conflicts_truncated: _truncated, ...missingTruncated } = detail;
  expect(decode(AdminListingDetailSchema, missingTruncated).ok).toBe(false);
  expect(
    decode(AdminListingDetailSchema, {
      ...detail,
      conflicts: Array.from({ length: 51 }, () => conflict),
    }).ok,
  ).toBe(false);
});

test("运营详情仅在恰好返回 50 条碰撞时可标记截断", async () => {
  const { data: detail } = await readResponse(DETAIL, AdminListingDetailSchema);
  const [conflict] = detail.conflicts;
  if (!conflict) throw new Error("运营详情样本须有关联拍卖");
  const short = { ...detail, conflicts: [conflict] };
  expect(decode(AdminListingDetailSchema, short).ok).toBe(true);
  expect(decode(AdminListingDetailSchema, { ...short, conflicts_truncated: true }).ok).toBe(false);
  const conflicts = Array.from({ length: 50 }, () => conflict);
  for (const conflicts_truncated of [false, true]) {
    expect(decode(AdminListingDetailSchema, { ...detail, conflicts, conflicts_truncated }).ok).toBe(
      true,
    );
  }
});

test("运营下架请求和已关闭回执夹具对齐", async () => {
  expectTypeOf<
    v.InferOutput<typeof AdminDelistListingSchema>
  >().toEqualTypeOf<AdminDelistListing>();
  const request = await readFixture(DELIST, AdminDelistListingSchema);
  const { data: receipt } = await readResponse(
    "admin/consignment-delist-receipt.json",
    CommandReceiptSchema,
  );
  expect(receipt).toMatchObject({
    command_id: request.command_id,
    operation: "admin_delist",
    disposition: "closed",
    state: "closed",
    price: 101,
  });
});

function readReasonCases() {
  return readFixture(
    "admin/consignment-delist-reason-cases.json",
    v.array(v.tuple([v.string(), v.string(), v.boolean()])),
  );
}

test("下架请求筛查已知凭据形态及 Unicode 行分隔符", async () => {
  const cases = await readReasonCases();
  const request = await readFixture(DELIST, AdminDelistListingSchema);
  for (const [label, reason, accepted] of cases) {
    expect(decode(AdminDelistListingSchema, { ...request, reason }).ok, label).toBe(accepted);
  }
});

test("历史说明筛查已知凭据形态及 Unicode 行分隔符", async () => {
  const cases = await readReasonCases();
  const { data: page } = await readResponse(PAGE, AdminListingPageSchema);
  const [closed, ...otherRecords] = page.items;
  if (!closed) throw new Error("运营列表夹具须包含已下架的历史");
  for (const [label, reason, accepted] of cases) {
    const record = { ...closed, admin_delist_reason: reason };
    const history = { ...page, items: [record, ...otherRecords] };
    expect(decode(AdminListingPageSchema, history).ok, label).toBe(accepted);
  }
});

test("运营下架请求拒绝错误版本、非规范说明、装备链接和额外键", async () => {
  const request = await readFixture(DELIST, AdminDelistListingSchema);
  expect(decode(AdminDelistListingSchema, { ...request, reason: `${"装".repeat(85)}a` }).ok).toBe(
    true,
  );
  for (const reason of [
    null,
    "",
    " Fixture reason",
    "Fixture\nreason",
    "装".repeat(86),
    EQUIPMENT_LINK,
  ]) {
    expect(decode(AdminDelistListingSchema, { ...request, reason }).ok).toBe(false);
  }
  expect(decode(AdminDelistListingSchema, { ...request, expected_version: 1.5 }).ok).toBe(false);
  expect(decode(AdminDelistListingSchema, { ...request, seller_user_id: "fixture-only" }).ok).toBe(
    false,
  );
});

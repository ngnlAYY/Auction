// 验证寄售公开目录、分页及装备白名单的线协议。

import { expect, expectTypeOf, test } from "bun:test";

import { decode } from "../../../wire";
import {
  type ListingDetail,
  ListingDetailSchema,
  type ListingSummary,
  ListingSummaryPageSchema,
  ListingSummarySchema,
} from ".";
import {
  EquipmentLevelSchema,
  type EquipmentStat,
  type EquipmentStatGroup,
  EquipmentStatGroupSchema,
  EquipmentStatSchema,
  type EquipmentView,
  EquipmentViewSchema,
} from "./catalog";
import { readResponse } from "./consignment-fixture";

const DETAIL = "public/consignment-listing-detail.json";

async function readDetail() {
  return (await readResponse(DETAIL, ListingDetailSchema)).data;
}

test("公开目录和两种详情夹具保留模式、装备编号及属性", async () => {
  const { data: page } = await readResponse(
    "public/consignment-listings.json",
    ListingSummaryPageSchema,
  );
  const direct = await readDetail();
  const { data: consigned } = await readResponse(
    "public/consignment-listing-detail-awaiting-deposit.json",
    ListingDetailSchema,
  );
  expect(page.items.map(({ mode, state }) => [mode, state])).toEqual([
    ["consigned", "awaiting_deposit"],
    ["direct", "displayed"],
  ]);
  expect(direct.summary).toMatchObject({ state: "displayed", price: 101 });
  expect(direct.summary.external_equipment_id).toHaveLength(32);
  expect(direct.equipment.stat_groups[0]?.stats.map(({ value, base }) => [value, base])).toEqual([
    [12.5, 10.25],
    [-2.5, null],
  ]);
  expect(consigned.summary).toMatchObject({ state: "awaiting_deposit", purchasable: false });
  expect(consigned.equipment.level).toEqual({ kind: "unassigned" });
});

test("分页允许空页及偏移上界，拒绝越界和非安全整数", () => {
  const page = { items: [], total: Number.MAX_SAFE_INTEGER, page: 10_001, page_size: 1 };
  expect(decode(ListingSummaryPageSchema, page)).toEqual({ ok: true, value: page });
  expect(decode(ListingSummaryPageSchema, { ...page, page: 1, page_size: 37 }).ok).toBe(true);
  for (const patch of [
    { total: -1 },
    { total: Number.MAX_SAFE_INTEGER + 1 },
    { page: 0 },
    { page: 10_002 },
    { page: 1.5 },
    { page_size: 0 },
    { page: 1, page_size: 201 },
    { items: {} },
  ]) {
    expect(decode(ListingSummaryPageSchema, { ...page, ...patch }).ok).toBe(false);
  }
});

test("分页条数及总数必须与本页位置一致", async () => {
  const { data: page } = await readResponse(
    "public/consignment-listings.json",
    ListingSummaryPageSchema,
  );
  expect(decode(ListingSummaryPageSchema, { ...page, page_size: 1 }).ok).toBe(false);
  expect(decode(ListingSummaryPageSchema, { ...page, total: 0 }).ok).toBe(false);
  expect(decode(ListingSummaryPageSchema, { ...page, page: 2 }).ok).toBe(false);
  expect(decode(ListingSummaryPageSchema, { ...page, page: 2, total: 39 }).ok).toBe(true);
  expect(decode(ListingSummaryPageSchema, { ...page, page: 2, items: [] }).ok).toBe(true);
  expect(decode(ListingSummaryPageSchema, { ...page, total: 0, items: [] }).ok).toBe(true);
});

test("公开摘要拒绝未知模式和状态、缺失字段及不合规标量", async () => {
  const { summary } = await readDetail();
  const { seller_name: _sellerName, ...missingSeller } = summary;
  expect(decode(ListingSummarySchema, missingSeller).ok).toBe(false);
  expect(decode(ListingSummarySchema, Object.values(summary)).ok).toBe(false);
  expect(decode(ListingSummarySchema, { ...summary, version: Number.MAX_SAFE_INTEGER }).ok).toBe(
    true,
  );
  for (const patch of [
    { mode: "future_mode" },
    { mode: null },
    { mode: "consigned" },
    { state: "resolving" },
    { world_id: "future_world" },
    { level: { kind: "soulbound" } },
    { price: "101" },
    { price: 101.5 },
    { price: 1_000_000_001 },
    { version: Number.MAX_SAFE_INTEGER + 1 },
    { external_equipment_id: "042" },
    { external_equipment_id: "42\n" },
    { external_equipment_id: 42 },
    { listing_id: "00000000-0000-0000-0000-000000000000" },
    { listing_id: "00000000-0000-4000-8000-00000000000A" },
    { updated_at: "2026-11-20T10:10:00-08:00" },
    { updated_at: "2026-11-20 18:10:00Z" },
    { updated_at: "2026-02-30T18:10:00Z" },
    { updated_at: "2026-11-20T18:10:00Z\n" },
    { purchasable: true },
  ]) {
    expect(decode(ListingSummarySchema, { ...summary, ...patch }).ok).toBe(false);
  }
  for (const updated_at of ["2024-02-29t18:10:00z", "2026-11-20T18:10:00.123456789+00:00"]) {
    expect(decode(ListingSummarySchema, { ...summary, updated_at }).ok).toBe(true);
  }
});

test("装备编号拒绝零和 33 位十进制串", async () => {
  const { summary } = await readDetail();
  expect(decode(ListingSummarySchema, { ...summary, external_equipment_id: "0" }).ok).toBe(false);
  expect(
    decode(ListingSummarySchema, { ...summary, external_equipment_id: "1".repeat(33) }).ok,
  ).toBe(false);
});

test("装备等级按形态封闭，属性验类型及字节和数值边界", async () => {
  const { equipment } = await readDetail();
  const { alt_name: _altName, ...missingAltName } = equipment;
  const stat = { name: "Fixture stat", value: 12.5, base: null };
  for (const level of [
    { kind: "assigned", value: 0 },
    { kind: "assigned", value: 100_001 },
    { kind: "assigned", value: "42" },
    { kind: "assigned", value: 42.5 },
    { kind: "assigned" },
    { kind: "unassigned", value: 42 },
    { kind: "soulbound", value: null },
    { kind: "future_level" },
  ]) {
    expect(decode(EquipmentLevelSchema, level).ok).toBe(false);
  }
  expect(decode(EquipmentStatSchema, stat)).toEqual({ ok: true, value: stat });
  expect(decode(EquipmentStatSchema, { name: stat.name, value: stat.value }).ok).toBe(false);
  for (const value of [1_000_000_001, Number.POSITIVE_INFINITY, "12.5", null]) {
    expect(decode(EquipmentStatSchema, { ...stat, value }).ok).toBe(false);
  }
  expect(decode(EquipmentStatGroupSchema, { name: "Fixture group", stats: stat }).ok).toBe(false);
  expect(decode(EquipmentViewSchema, missingAltName).ok).toBe(false);
  expect(decode(EquipmentViewSchema, { ...equipment, alt_name: null }).ok).toBe(true);
  expect(decode(EquipmentViewSchema, { ...equipment, name: `${"装".repeat(170)}ab` }).ok).toBe(
    true,
  );
  for (const patch of [
    { name: "装".repeat(171) },
    { name: " Fixture equipment" },
    { category: "Cloth\nFixture" },
    { source: "upstream-raw" },
    { level: { kind: "soulbound" }, tradeable: true },
  ]) {
    expect(decode(EquipmentViewSchema, { ...equipment, ...patch }).ok).toBe(false);
  }
});

test("公开 DTO 类型精确限定公开字段", () => {
  expectTypeOf<keyof ListingDetail>().toEqualTypeOf<"summary" | "equipment">();
  expectTypeOf<keyof ListingSummary>().toEqualTypeOf<
    | "listing_id"
    | "version"
    | "mode"
    | "state"
    | "world_id"
    | "external_equipment_id"
    | "name"
    | "category"
    | "level"
    | "price"
    | "seller_name"
    | "updated_at"
    | "expires_at"
    | "purchasable"
  >();
  expectTypeOf<keyof EquipmentView>().toEqualTypeOf<
    | "name"
    | "alt_name"
    | "category"
    | "level"
    | "tradeable"
    | "stat_groups"
    | "source"
    | "observed_at"
  >();
  expectTypeOf<keyof EquipmentStat>().toEqualTypeOf<"name" | "value" | "base">();
  expectTypeOf<keyof EquipmentStatGroup>().toEqualTypeOf<"name" | "stats">();
});

test("装备属性及属性分组拒绝额外凭据字段", async () => {
  const detail = await readDetail();
  const [group] = detail.equipment.stat_groups;
  const stat = group?.stats[0];
  if (!group || !stat) throw new Error("公开详情夹具须包含装备属性");
  expect(decode(EquipmentStatSchema, { ...stat, key: "fixture-only" }).ok).toBe(false);
  expect(decode(EquipmentStatGroupSchema, { ...group, equipment_url: "fixture-only" }).ok).toBe(
    false,
  );
});

test("公开详情的装备名称、分类和等级须与摘要一致", async () => {
  const detail = await readDetail();
  const summary = { ...detail.summary, level: { kind: "unassigned" } };
  expect(decode(ListingDetailSchema, { ...detail, summary }).ok).toBe(false);
  for (const [field, value] of [
    ["name", "Fixture other name"],
    ["category", "Fixture other category"],
    ["level", { kind: "assigned", value: 43 }],
    ["level", { kind: "unassigned" }],
  ] as const) {
    expect(
      decode(ListingDetailSchema, { ...detail, equipment: { ...detail.equipment, [field]: value } })
        .ok,
      field,
    ).toBe(false);
  }
});

test("公开详情拒绝含秘密或额外字段的各层对象，也拒绝不可交易快照", async () => {
  const detail = await readDetail();
  const { summary, equipment } = detail;
  for (const key of [
    "equipment_url",
    "game_equipment_url",
    "key",
    "seller_user_id",
    "owner",
    "__proto__",
  ]) {
    const extra = { [key]: "fixture-only" };
    expect(decode(ListingDetailSchema, { ...detail, ...extra }).ok).toBe(false);
    expect(decode(ListingDetailSchema, { equipment, summary: { ...summary, ...extra } }).ok).toBe(
      false,
    );
    expect(decode(ListingDetailSchema, { summary, equipment: { ...equipment, ...extra } }).ok).toBe(
      false,
    );
  }
  expect(
    decode(ListingDetailSchema, { summary, equipment: { ...equipment, tradeable: false } }).ok,
  ).toBe(false);
  expect(decode(ListingDetailSchema, { summary }).ok).toBe(false);
});

// 验证 v2 公开/本人挂单投影、全部状态、原因闭集与撤回意愿回执。

import { expect, expectTypeOf, test } from "bun:test";

import * as v from "valibot";

import { decode, struct } from "../../../wire";
import { readFixture, readResponse } from "./consignment-fixture";
import {
  ListingActionV2Schema,
  ListingDetailV2Schema,
  ListingReasonV2Schema,
  ListingStateV2Schema,
  ListingSummaryV2Schema,
  ListingViewV2Schema,
  OwnedListingV2Schema,
  PublicListingStateV2Schema,
  type WithdrawListingV2,
  WithdrawListingV2Schema,
  WithdrawReceiptV2Schema,
} from "./listing-v2";
import {
  ConsignmentDispositionV2Schema,
  RequestErrorCodeV2Schema,
  WriteRejectionCodeV2Schema,
} from "./receipt";
import { SeasonStateV2Schema } from "./status-v2";

const PUBLIC = "public/consignment-listing-v2-public.json";
const OWNED = "public/consignment-listing-v2-owned.json";
const HELD = "public/consignment-listing-v2-held.json";
const VIEWS = "public/consignment-listing-views-v2.json";
const WITHDRAWAL = "public/consignment-withdrawal-requested-receipt.json";
const WITHDRAW_CLOSED = "public/consignment-withdraw-closed-receipt.json";
const WITHDRAW_REQUEST = "public/consignment-withdrawal-request-v2.json";
const ENUMS = "public/consignment-enums-v2.json";

const EQUIPMENT = {
  name: "Fixture Oak Staff",
  alt_name: null,
  category: "Staff",
  level: { kind: "assigned", value: 42 },
  tradeable: true,
  stat_groups: [],
  source: "hvdata-equip-v1",
  observed_at: "2026-11-20T18:10:00Z",
} as const;

function record(value: unknown): Record<string, unknown> {
  return value as Record<string, unknown>;
}

test("公开详情只表达 public 状态与购买动作", async () => {
  const { data: detail } = await readResponse(PUBLIC, ListingDetailV2Schema);
  expect(detail.summary).toMatchObject({
    state: "available",
    generation: 1,
    effective_reason: null,
    allowed_actions: [],
    purchasable: false,
  });
  const summary = record(structuredClone(detail.summary));
  for (const state of [
    "deposit_accepted",
    "sale_pending",
    "held_off_market",
    "withdrawal_pending",
  ]) {
    expect(decode(ListingSummaryV2Schema, { ...summary, state }).ok).toBe(false);
  }
  for (const actions of [["admin_delist"], ["release_hold"], ["reprice"]]) {
    expect(decode(ListingSummaryV2Schema, { ...summary, allowed_actions: actions }).ok).toBe(false);
  }
  expect(
    decode(ListingSummaryV2Schema, {
      ...summary,
      allowed_actions: ["purchase", "purchase"],
    }).ok,
  ).toBe(false);
  expect(decode(ListingSummaryV2Schema, { ...summary, generation: null }).ok).toBe(false);
  expect(
    decode(ListingSummaryV2Schema, {
      ...summary,
      level: { kind: "soulbound" },
    }).ok,
  ).toBe(false);
  expect(
    decode(ListingSummaryV2Schema, {
      ...summary,
      listing_id: "00000000-0000-0000-0000-000000000000",
    }).ok,
  ).toBe(false);
});

test("公开购买能力保持布尔，并只在 available 上允许 purchase", async () => {
  const { data: detail } = await readResponse(PUBLIC, ListingDetailV2Schema);
  const summary = record(structuredClone(detail.summary));
  const purchasable = { ...summary, purchasable: true, allowed_actions: ["purchase"] };
  expect(decode(ListingSummaryV2Schema, purchasable).ok).toBe(true);
  expect(
    decode(ListingSummaryV2Schema, {
      ...purchasable,
      state: "awaiting_deposit",
      generation: null,
    }).ok,
  ).toBe(false);
});

test("公开详情要求摘要与安全快照一致", async () => {
  const { data: detail } = await readResponse(PUBLIC, ListingDetailV2Schema);
  expect(
    decode(ListingDetailV2Schema, {
      ...detail,
      equipment: { ...detail.equipment, name: "Fixture Other Staff" },
    }).ok,
  ).toBe(false);
});

test("本人挂单接受 available 与 held_off_market 形状", async () => {
  const { data: available } = await readResponse(OWNED, OwnedListingV2Schema);
  const { data: held } = await readResponse(HELD, OwnedListingV2Schema);
  expect(available).toMatchObject({
    stored_state: "available",
    effective_state: "available",
    allowed_actions: ["reprice", "withdraw"],
    generation: 1,
  });
  expect(held).toMatchObject({
    effective_state: "held_off_market",
    effective_reason: "operations_hold",
    allowed_actions: ["withdraw"],
  });
  const view = record(structuredClone(held.view));
  const heldWire = record(structuredClone(held));
  expect(
    decode(OwnedListingV2Schema, {
      ...heldWire,
      view: { ...view, reason: "quota_blocked", hold_id: null },
      effective_reason: "quota_blocked",
      allowed_actions: ["withdraw", "relist"],
    }).ok,
  ).toBe(true);
  expect(
    decode(OwnedListingV2Schema, {
      ...heldWire,
      allowed_actions: ["withdraw", "relist"],
    }).ok,
  ).toBe(false);
  expect(
    decode(OwnedListingV2Schema, {
      ...heldWire,
      view: { ...view, hold_id: null },
    }).ok,
  ).toBe(false);
});

test("seller_ineligible 与 service_disabled 的 held 和 closed 视图都合法", async () => {
  const { data: held } = await readResponse(HELD, OwnedListingV2Schema);
  if (held.view.kind !== "held_off_market") throw new Error("夹具须为 held_off_market");
  const heldWire = record(structuredClone(held));
  for (const reason of ["seller_ineligible", "service_disabled"] as const) {
    expect(
      decode(OwnedListingV2Schema, {
        ...heldWire,
        effective_reason: reason,
        view: {
          kind: "held_off_market",
          equipment: held.view.equipment,
          reason,
          hold_id: null,
        },
      }).ok,
    ).toBe(true);
    expect(
      decode(OwnedListingV2Schema, {
        ...heldWire,
        stored_state: "closed",
        effective_state: "closed",
        effective_reason: reason,
        generation: null,
        allowed_actions: [],
        view: {
          kind: "closed",
          reason,
          equipment: held.view.equipment,
        },
      }).ok,
    ).toBe(true);
  }
});

test("本人动作与状态、处分代和有效投影严格一致", async () => {
  const { data: available } = await readResponse(OWNED, OwnedListingV2Schema);
  const wire = record(structuredClone(available));
  for (const allowed_actions of [["purchase"], ["relist"], ["reprice", "reprice"]]) {
    expect(decode(OwnedListingV2Schema, { ...wire, allowed_actions }).ok).toBe(false);
  }
  expect(decode(OwnedListingV2Schema, { ...wire, generation: null }).ok).toBe(false);
  expect(decode(OwnedListingV2Schema, { ...wire, effective_state: "held_off_market" }).ok).toBe(
    false,
  );
  expect(decode(OwnedListingV2Schema, { ...wire, effective_reason: "review" }).ok).toBe(false);
});

test("全部 12 种本人视图按共享夹具解码", async () => {
  const views = await readFixture(VIEWS, v.array(ListingViewV2Schema));
  expect(views.map(({ kind }) => kind)).toEqual([
    "resolving",
    "lookup_failed",
    "ineligible",
    "displayed",
    "awaiting_deposit",
    "deposit_accepted",
    "available",
    "sale_pending",
    "held_off_market",
    "withdrawal_pending",
    "closed",
    "expired",
  ]);
});

test("视图原因必须属于对应闭集", async () => {
  const views = await readFixture(VIEWS, v.array(ListingViewV2Schema));
  const byKind = new Map(views.map((view) => [view.kind, view]));
  for (const [kind, reason] of [
    ["closed", "quota_blocked"],
    ["expired", "sold"],
    ["held_off_market", "sold"],
    ["lookup_failed", "not_tradeable"],
    ["ineligible", "owner_missing"],
  ] as const) {
    const view = record(structuredClone(byKind.get(kind)));
    expect(decode(ListingViewV2Schema, { ...view, reason }).ok).toBe(false);
  }
});

test("held_off_market 七个原因都能解码，运营止售必须有 hold_id", async () => {
  const { data: held } = await readResponse(HELD, OwnedListingV2Schema);
  const view = record(structuredClone(held.view));
  for (const reason of [
    "unpaid_recovered",
    "paid_returned",
    "seller_ineligible",
    "service_disabled",
    "quota_blocked",
    "operations_hold",
    "review",
  ]) {
    expect(
      decode(ListingViewV2Schema, {
        ...view,
        reason,
        hold_id: reason === "operations_hold" ? "00000000-0000-4000-8000-000000000416" : null,
      }).ok,
    ).toBe(true);
  }
  expect(decode(ListingViewV2Schema, { ...view, hold_id: null }).ok).toBe(false);
});

test("撤回意愿回执携带 L 与 intent ID", async () => {
  const receipt = await readResponse(WITHDRAWAL, WithdrawReceiptV2Schema);
  expect(receipt.data).toMatchObject({
    operation: "withdraw",
    disposition: "withdrawal_requested",
    listing_version: 4,
    state: "deposit_accepted",
    intent_id: "00000000-0000-4000-8000-000000000417",
  });
  for (const state of ["deposit_accepted", "withdrawal_pending", "held_off_market"]) {
    expect(decode(WithdrawReceiptV2Schema, { ...receipt.data, state }).ok).toBe(true);
  }
  for (const state of ["available", "resolving", "closed"]) {
    expect(decode(WithdrawReceiptV2Schema, { ...receipt.data, state }).ok).toBe(false);
  }
  const closed = await readResponse(WITHDRAW_CLOSED, WithdrawReceiptV2Schema);
  expect(closed.data).toMatchObject({ disposition: "closed", state: "closed" });
  for (const state of ["available", "awaiting_deposit", "withdrawal_pending"]) {
    expect(decode(WithdrawReceiptV2Schema, { ...closed.data, state }).ok).toBe(false);
  }
  expect(
    decode(WithdrawReceiptV2Schema, {
      ...closed.data,
      intent_id: "00000000-0000-4000-8000-000000000417",
    }).ok,
  ).toBe(false);
});

test("撤回回执必须携带固定 schema 2", async () => {
  for (const file of [WITHDRAWAL, WITHDRAW_CLOSED]) {
    const { data } = await readResponse(file, WithdrawReceiptV2Schema);
    for (const schemaVersion of [1, 3]) {
      expect(decode(WithdrawReceiptV2Schema, { ...data, schema_version: schemaVersion }).ok).toBe(
        false,
      );
    }
    const missing = record(structuredClone(data));
    delete missing.schema_version;
    expect(decode(WithdrawReceiptV2Schema, missing).ok).toBe(false);
  }
});

test("v2 撤回请求强制 UUIDv4 与正版本", async () => {
  expectTypeOf<v.InferOutput<typeof WithdrawListingV2Schema>>().toEqualTypeOf<WithdrawListingV2>();
  const request = await readFixture(WITHDRAW_REQUEST, WithdrawListingV2Schema);
  expect(request.expected_version).toBe(4);
  for (const command_id of [
    "00000000-0000-5000-8000-000000000001",
    "00000000-0000-0000-8000-000000000001",
    "00000000-0000-4000-8000-00000000000A",
  ]) {
    expect(decode(WithdrawListingV2Schema, { ...request, command_id }).ok).toBe(false);
  }
  for (const expected_version of [0, -1, 1.5, "4"]) {
    expect(decode(WithdrawListingV2Schema, { ...request, expected_version }).ok).toBe(false);
  }
});

test("挂单相关枚举与共享夹具逐字一致", async () => {
  const enums = await readFixture(
    ENUMS,
    struct({
      season_states: v.array(v.string()),
      listing_states: v.array(v.string()),
      public_listing_states: v.array(v.string()),
      listing_reasons: v.array(v.string()),
      hold_reasons: v.array(v.string()),
      listing_actions: v.array(v.string()),
      dispositions: v.array(v.string()),
      rejection_codes: v.array(v.string()),
      request_error_codes: v.array(v.string()),
    }),
  );
  for (const value of enums.season_states) {
    expect(v.is(SeasonStateV2Schema, value)).toBe(true);
  }
  for (const value of enums.listing_states) {
    expect(v.is(ListingStateV2Schema, value)).toBe(true);
  }
  for (const value of enums.public_listing_states) {
    expect(v.is(PublicListingStateV2Schema, value)).toBe(true);
  }
  for (const value of enums.listing_reasons) {
    expect(v.is(ListingReasonV2Schema, value)).toBe(true);
  }
  for (const value of enums.hold_reasons) {
    expect(v.is(ListingReasonV2Schema, value)).toBe(true);
  }
  for (const value of enums.listing_actions) {
    expect(v.is(ListingActionV2Schema, value)).toBe(true);
  }
  for (const value of enums.dispositions) {
    expect(v.is(ConsignmentDispositionV2Schema, value)).toBe(true);
  }
  expect(enums.dispositions).toContain("closed");
  for (const value of enums.rejection_codes) {
    expect(v.is(WriteRejectionCodeV2Schema, value)).toBe(true);
  }
  expect(enums.rejection_codes).toContain("fence_mismatch");
  for (const value of enums.request_error_codes) {
    expect(v.is(RequestErrorCodeV2Schema, value)).toBe(true);
  }
  expect(enums.request_error_codes).toHaveLength(9);
});

test("公开快照字段白名单与安全 DTO 一致", () => {
  expect(Object.keys(record(EQUIPMENT)).sort()).toEqual(
    [
      "alt_name",
      "category",
      "level",
      "name",
      "observed_at",
      "source",
      "stat_groups",
      "tradeable",
    ].sort(),
  );
});

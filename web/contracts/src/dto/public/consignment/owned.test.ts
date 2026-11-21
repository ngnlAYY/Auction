// 验证本人挂单的七种视图、解析进度、有效态与冻结报价。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { OwnedListingPageSchema, OwnedListingSchema } from ".";
import { readResponse } from "./consignment-fixture";
import { type ListingView, ListingViewSchema, LookupProgressSchema } from "./owned";
import { ListingFeeBpsSchema } from "./scalar";

const OWNED = "public/consignment-owned-listing.json";
const PAGE = "public/consignment-owned-listings.json";

async function readOwnedPage() {
  return (await readResponse(PAGE, OwnedListingPageSchema)).data;
}

test("登记后首次本人读取只有解析进度，且保留显式 null", async () => {
  const { data: listing } = await readResponse(OWNED, OwnedListingSchema);
  expect(listing).toMatchObject({
    version: 1,
    stored_state: "resolving",
    effective_state: "resolving",
    effective_reason: null,
    view: { kind: "resolving", progress: { attempts: 0, max_attempts: 6, last_reason: null } },
  });
  expect(Object.keys(listing.view).sort()).toEqual(["kind", "progress"]);
});

test("本人分页覆盖七种视图，保留快照拒挂及先于落库的关闭", async () => {
  const { items } = await readOwnedPage();
  expect(items.map(({ view }) => view.kind).sort()).toEqual([
    "awaiting_deposit",
    "closed",
    "displayed",
    "expired",
    "ineligible",
    "lookup_failed",
    "resolving",
  ]);
  expect(items.find(({ view }) => view.kind === "closed")).toMatchObject({
    stored_state: "displayed",
    effective_state: "closed",
    effective_reason: "auction_lot_created",
    allowed_actions: [],
    view: { kind: "closed", equipment: { tradeable: true } },
  });
  expect(items.find(({ view }) => view.kind === "ineligible")?.view).toMatchObject({
    kind: "ineligible",
    reason: "not_tradeable",
    equipment: { level: { kind: "soulbound" }, tradeable: false },
  });
  const { data: mismatch } = await readResponse(
    "public/consignment-owned-listing-owner-mismatch.json",
    OwnedListingSchema,
  );
  expect(mismatch.view).toMatchObject({
    kind: "ineligible",
    reason: "owner_mismatch",
    equipment: { tradeable: true },
  });
  expect(decode(OwnedListingPageSchema, { items, page: 1, page_size: 20 }).ok).toBe(false);
});

test("解析进度拒绝越界、未知原因、缺失的可空键与租约字段", async () => {
  const { data: listing } = await readResponse(OWNED, OwnedListingSchema);
  if (listing.view.kind !== "resolving") throw new Error("首次读取须为解析中");
  const { progress } = listing.view;
  expect(
    decode(LookupProgressSchema, { ...progress, next_attempt_at: null, last_reason: null }).ok,
  ).toBe(true);
  const { next_attempt_at: _nextAttempt, ...missingNextAttempt } = progress;
  const { last_reason: _lastReason, ...missingLastReason } = progress;
  for (const invalid of [
    missingNextAttempt,
    missingLastReason,
    { ...progress, attempts: -1 },
    { ...progress, attempts: 7 },
    { ...progress, attempts: 0.5 },
    { ...progress, max_attempts: 7 },
    { ...progress, last_reason: "future_reason" },
    { ...progress, last_reason: "owner_mismatch" },
    { ...progress, paused: "false" },
    { ...progress, token: "fixture-only" },
  ]) {
    expect(decode(LookupProgressSchema, invalid).ok).toBe(false);
  }
});

test("视图拒绝未知判别值和七种形态各自的错误字段", async () => {
  const { items } = await readOwnedPage();
  const invalidFields = {
    resolving: { equipment: null },
    lookup_failed: { reason: "owner_mismatch" },
    ineligible: { reason: "owner_missing" },
    displayed: { equipment: null },
    awaiting_deposit: { accepts_mail: true },
    closed: { reason: "display_expired" },
    expired: { reason: "seller_cancelled" },
  } satisfies Record<ListingView["kind"], object>;
  expect(decode(ListingViewSchema, { kind: "registered_only" }).ok).toBe(false);
  for (const { view } of items) {
    expect(decode(ListingViewSchema, { ...view, ...invalidFields[view.kind] }).ok).toBe(false);
  }
  for (const { view } of items) {
    if (view.kind === "lookup_failed") {
      const { last_lookup_reason: _lastLookupReason, ...missingReason } = view;
      expect(decode(ListingViewSchema, missingReason).ok).toBe(false);
      expect(decode(ListingViewSchema, { ...view, last_lookup_reason: null }).ok).toBe(true);
    }
    if (view.kind === "closed" || view.kind === "expired") {
      const { equipment: _equipment, ...missingEquipment } = view;
      expect(decode(ListingViewSchema, missingEquipment).ok).toBe(false);
    }
  }
});

test("本人投影拒绝越权动作、相互矛盾的状态或模式，以及终结时错误的快照", async () => {
  const { items } = await readOwnedPage();
  const closed = items.find(({ view }) => view.kind === "closed");
  const expired = items.find(({ view }) => view.kind === "expired");
  const displayed = items.find(({ view }) => view.kind === "displayed");
  if (closed?.view.kind !== "closed" || expired?.view.kind !== "expired" || !displayed) {
    throw new Error("样本缺少发布、关闭或到期视图");
  }
  const { effective_reason: _effectiveReason, ...missingReason } = displayed;
  expect(decode(OwnedListingSchema, missingReason).ok).toBe(false);
  for (const invalid of [
    { ...closed, allowed_actions: ["withdraw"] },
    { ...closed, stored_state: "closed", effective_state: "displayed" },
    { ...closed, effective_reason: "seller_cancelled" },
    {
      ...closed,
      effective_reason: "seller_cancelled",
      view: { ...closed.view, reason: "seller_cancelled" },
    },
    { ...closed, mode: "consigned" },
    { ...closed, view: { ...closed.view, equipment: null } },
    { ...closed, stored_state: "resolving" },
    {
      ...expired,
      effective_reason: "deposit_expired",
      view: { ...expired.view, reason: "deposit_expired" },
    },
    { ...displayed, stored_state: "resolving" },
    { ...displayed, allowed_actions: ["relist"] },
  ]) {
    expect(decode(OwnedListingSchema, invalid).ok).toBe(false);
  }
  expect(decode(OwnedListingSchema, { ...expired, stored_state: "resolving" }).ok).toBe(true);
  expect(decode(OwnedListingSchema, { ...closed, stored_state: "closed" }).ok).toBe(true);
});

test("两种发布态都要求可交易装备，本人详情只从快照读取装备属性", async () => {
  const { items } = await readOwnedPage();
  for (const kind of ["displayed", "awaiting_deposit"] as const) {
    const listing = items.find(({ view }) => view.kind === kind);
    if (listing?.view.kind !== kind) throw new Error("样本缺少已发布视图");
    const { view } = listing;
    const equipment = { ...view.equipment, tradeable: false };
    expect(decode(OwnedListingSchema, { ...listing, view: { ...view, equipment } }).ok, kind).toBe(
      false,
    );
    for (const field of ["name", "category", "level"] as const) {
      expect(decode(OwnedListingSchema, { ...listing, [field]: view.equipment[field] }).ok).toBe(
        false,
      );
    }
  }
});

test("已解析拒挂的原因须与快照交易性配对", async () => {
  const { data: listing } = await readResponse(
    "public/consignment-owned-listing-owner-mismatch.json",
    OwnedListingSchema,
  );
  const { view } = listing;
  if (view.kind !== "ineligible") throw new Error("样本缺少已解析拒挂视图");
  expect(decode(ListingViewSchema, { ...view, reason: "not_tradeable" }).ok).toBe(false);
  const equipment = { ...view.equipment, tradeable: false };
  expect(decode(ListingViewSchema, { ...view, equipment }).ok).toBe(false);
  expect(decode(ListingViewSchema, { ...view, equipment, reason: "not_tradeable" }).ok).toBe(true);
});

test("已发布挂单关闭或到期须保留可交易快照", async () => {
  const { items } = await readOwnedPage();
  for (const [source, closedReason, expiredReason] of [
    ["displayed", "auction_lot_created", "display_expired"],
    ["awaiting_deposit", "service_disabled", "deposit_expired"],
  ] as const) {
    const listing = items.find(({ view }) => view.kind === source);
    if (listing?.view.kind !== source) throw new Error("样本缺少已发布视图");
    const { equipment } = listing.view;
    const soulbound = { ...equipment, level: { kind: "soulbound" }, tradeable: false };
    for (const [kind, reason] of [
      ["closed", closedReason],
      ["expired", expiredReason],
    ] as const) {
      const terminal = {
        ...listing,
        effective_state: kind,
        effective_reason: reason,
        allowed_actions: [],
        view: { kind, reason, equipment },
      };
      expect(decode(OwnedListingSchema, terminal).ok, reason).toBe(true);
      for (const [snapshot, scenario] of [
        [null, "无快照"],
        [soulbound, "灵魂绑定快照"],
        [{ ...equipment, tradeable: false }, "不可交易快照"],
      ] as const) {
        const invalid = { ...terminal, view: { ...terminal.view, equipment: snapshot } };
        expect(decode(OwnedListingSchema, invalid).ok, `${reason}，${scenario}`).toBe(false);
        expect(
          decode(OwnedListingSchema, { ...invalid, stored_state: kind }).ok,
          `已落库：${reason}，${scenario}`,
        ).toBe(kind === "closed" && snapshot === null);
      }
      expect(decode(OwnedListingSchema, { ...terminal, stored_state: kind }).ok, reason).toBe(true);
    }
  }
});

test("解析到期无论是否已落库都不带快照", async () => {
  const { items } = await readOwnedPage();
  const expired = items.find(({ view }) => view.kind === "expired");
  const displayed = items.find(({ view }) => view.kind === "displayed");
  if (expired?.view.kind !== "expired" || displayed?.view.kind !== "displayed") {
    throw new Error("样本缺少解析到期或已展示视图");
  }
  for (const stored_state of ["resolving", "expired"]) {
    const terminal = { ...expired, stored_state };
    expect(decode(OwnedListingSchema, terminal).ok).toBe(true);
    const view = { ...expired.view, equipment: displayed.view.equipment };
    expect(decode(OwnedListingSchema, { ...terminal, view }).ok).toBe(false);
  }
});

test("本人报价接受 101 扣 5 到手 96，拒绝不一致或不安全金额", async () => {
  const { items } = await readOwnedPage();
  const consigned = items.find(({ view }) => view.kind === "awaiting_deposit");
  const direct = items.find(({ view }) => view.kind === "displayed");
  if (!consigned || !direct) throw new Error("样本须包含两种已发布模式");
  expect(consigned).toMatchObject({
    price: 101,
    fee_bps: 500,
    estimated_fee: 5,
    estimated_seller_amount: 96,
  });
  for (const patch of [
    { pricing_version: 2 },
    { fee_bps: 10_000 },
    { estimated_fee: 6, estimated_seller_amount: 95 },
    { estimated_fee: 5, estimated_seller_amount: 95 },
    { estimated_fee: "5" },
    { estimated_fee: 5.5 },
    { estimated_seller_amount: Number.MAX_SAFE_INTEGER + 1 },
  ]) {
    expect(decode(OwnedListingSchema, { ...consigned, ...patch }).ok).toBe(false);
  }
  expect(
    decode(OwnedListingSchema, {
      ...direct,
      fee_bps: 500,
      estimated_fee: 5,
      estimated_seller_amount: 96,
    }).ok,
  ).toBe(false);
});

test("本人报价 101 按 5000 bps 向下取整为手续费 50、到手 51", async () => {
  const { items } = await readOwnedPage();
  const listing = items.find(({ view }) => view.kind === "awaiting_deposit");
  if (!listing) throw new Error("样本缺少寄售挂单");
  const quote = {
    ...listing,
    price: 101,
    fee_bps: 5_000,
    estimated_fee: 50,
    estimated_seller_amount: 51,
  };
  expect(decode(OwnedListingSchema, quote)).toMatchObject({
    ok: true,
    value: { estimated_fee: 50, estimated_seller_amount: 51 },
  });
});

test("寄售费率接受 9999 并拒绝 -1 和 10000", () => {
  expect(decode(ListingFeeBpsSchema, -1).ok).toBe(false);
  expect(decode(ListingFeeBpsSchema, 9_999)).toEqual({ ok: true, value: 9_999 });
  expect(decode(ListingFeeBpsSchema, 10_000).ok).toBe(false);
});

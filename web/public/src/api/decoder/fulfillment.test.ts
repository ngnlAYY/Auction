// 履约进度解码的回归断言：状态闭集、分页边界、字段成对关系与严格 UTC 期限格式。

import { expect, test } from "bun:test";

import type { JsonObject } from "@auction/web-shared/net/wire";
import { compareText } from "@auction/web-shared/text";

const { FULFILLMENT_STATUSES, utcSecondTimestamp, validFulfillmentPage, validLot } = await import(
  "./fulfillment"
);

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

function lot(status: string): JsonObject {
  return {
    lot_id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c",
    lot_number: 3,
    equipment_name: "Legendary Ethereal Staff",
    equipment_category: "Staff",
    seller_name: "seller",
    status,
  };
}

function sold(status: string): JsonObject {
  return { ...lot(status), buyer_name: "buyer", final_price: 1_500_000 };
}

function page(lots: readonly unknown[]): JsonObject {
  return {
    auction: { id: AUCTION_ID, world: "main", number: 12 },
    lots,
    page: 1,
    page_size: 50,
    total: 3,
  };
}

function accepted(value: unknown): boolean {
  return validFulfillmentPage(value, AUCTION_ID, "main", 1, 50);
}

test("statuses_are_a_closed_set_of_distinct_values：履约状态是互不重复的闭集", () => {
  const statuses = [...FULFILLMENT_STATUSES].sort(compareText);
  const distinct = [...new Set(statuses)];
  expect(distinct.length).toBe(31);
  expect(validLot(lot("withdrawn")), "闭集外的状态整页拒绝").toBe(false);
  expect(validLot(lot("UNSOLD_RETURNED"))).toBe(false);
});

test("typical_page_is_accepted：典型履约页通过校验", () => {
  const awaiting = { ...sold("awaiting_payment"), payment_deadline_at: "2026-11-16T09:46:40Z" };
  const resent = {
    ...sold("cod_resent_awaiting_payment"),
    payment_deadline_at: "2026-11-16T09:46:40Z",
    cod_return_count: 1,
    cod_return_limit: 3,
    lot_id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2d",
  };
  const unsold = {
    ...lot("unsold_return_pending"),
    lot_id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2e",
  };
  expect(accepted(page([awaiting, resent, unsold]))).toBe(true);
  // 历史成交缺 recovery 时等待付款可以没有期限；未知状态可带或不带成交事实。
  expect(validLot(sold("awaiting_payment"))).toBe(true);
  expect(validLot(lot("unknown"))).toBe(true);
  expect(validLot(sold("unknown"))).toBe(true);
});

test("page_is_bound_to_the_request_identity：页码绑定请求的场次、世界与分页", () => {
  const value = page([lot("unsold_returned")]);
  expect(accepted(value)).toBe(true);
  expect(validFulfillmentPage(value, AUCTION_ID, "isekai", 1, 50), "世界不符").toBe(false);
  expect(validFulfillmentPage(value, AUCTION_ID, "main", 2, 50), "页码不符").toBe(false);
  expect(validFulfillmentPage(value, AUCTION_ID, "main", 1, 20), "页大小不符").toBe(false);
  const other = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2f";
  expect(validFulfillmentPage(value, other, "main", 1, 50), "场次不符").toBe(false);
  const duplicated = page([lot("unsold_returned"), lot("unsold_return_sent")]);
  expect(accepted(duplicated), "同页 lot 重复").toBe(false);
  const shortTotal = { ...page([lot("unsold_returned")]), total: 0 };
  expect(accepted(shortTotal), "总数小于本页").toBe(false);
});

test("non_empty_pages_end_within_the_total：非空页末行不超过总数", () => {
  const secondPage = (lots: readonly unknown[], total: number) =>
    validFulfillmentPage({ ...page(lots), page: 2, total }, AUCTION_ID, "main", 2, 50);
  // 第 2 页从第 51 件开始：总数只有 1 件时不可能还有一行。
  expect(secondPage([lot("unsold_returned")], 1), "offset 加本页行数超过总数").toBe(false);
  expect(secondPage([lot("unsold_returned")], 50)).toBe(false);
  expect(secondPage([lot("unsold_returned")], 51)).toBe(true);
  // 越过末页的空页仍然有效，由分页器纠偏页码。
  expect(secondPage([], 1)).toBe(true);
  expect(secondPage([], 0)).toBe(true);
});

test("extra_keys_are_rejected_at_every_level：每一层的多余键都被拒绝", () => {
  const extraPage = { ...page([]), cursor: "x" };
  expect(accepted(extraPage)).toBe(false);
  const extraAuction = {
    ...page([]),
    auction: { id: AUCTION_ID, world: "main", number: 12, status: "settled" },
  };
  expect(accepted(extraAuction)).toBe(false);
  for (const key of ["delivery_id", "blocking_code", "proxy_mode", "obligation_id"]) {
    const extra = { ...sold("completed"), [key]: "x" };
    expect(validLot(extra), key).toBe(false);
  }
});

test("sale_fields_are_paired_and_follow_the_status：成交字段成对且随状态出现", () => {
  expect(validLot(lot("completed")), "成交状态缺成交事实").toBe(false);
  expect(validLot(sold("unsold_returned")), "流拍状态不得带成交事实").toBe(false);
  const buyerOnly = { ...lot("completed"), buyer_name: "buyer" };
  expect(validLot(buyerOnly), "买家与成交价成对").toBe(false);
  const free = { ...sold("completed"), final_price: 0 };
  expect(validLot(free), "成交价为正").toBe(false);
});

test("payment_deadline_only_accompanies_payment_waits：付款期限只随付款等待状态出现", () => {
  for (const status of ["awaiting_payment", "payment_overdue", "cod_resent_awaiting_payment"]) {
    const waiting: JsonObject = { ...sold(status), payment_deadline_at: "2026-11-16T09:46:40Z" };
    if (status === "cod_resent_awaiting_payment") {
      waiting.cod_return_count = 1;
      waiting.cod_return_limit = 3;
    }
    expect(validLot(waiting), status).toBe(true);
  }
  expect(validLot(sold("payment_overdue")), "逾期必有期限").toBe(false);
  const completed = { ...sold("completed"), payment_deadline_at: "2026-11-16T09:46:40Z" };
  expect(validLot(completed), "其他状态不带期限").toBe(false);
  const malformed = { ...sold("awaiting_payment"), payment_deadline_at: "tomorrow" };
  expect(validLot(malformed)).toBe(false);
  const offset = { ...sold("awaiting_payment"), payment_deadline_at: "2026-11-16T17:46:40+08:00" };
  expect(validLot(offset), "期限只接受 UTC 秒精度").toBe(false);
});

test("payment_deadline_format_is_strict_utc_seconds：付款期限是严格 UTC 秒格式", () => {
  for (const acceptedValue of [
    "2026-11-16T09:46:40Z",
    "2028-02-29T23:59:59Z",
    "2026-01-01T00:00:00Z",
  ]) {
    expect(utcSecondTimestamp(acceptedValue), acceptedValue).toBe(true);
  }
  for (const rejected of [
    "2026-11-16T09:46:40.123Z",
    "2026-11-16T09:46:40+00:00",
    "2026-11-16T09:46:40",
    "2026-11-16 09:46:40Z",
    "2026-11-16t09:46:40z",
    " 2026-11-16T09:46:40Z",
    "+2026-11-16T09:46:40Z",
    "2026-1-16T09:46:40Z",
    "2026-02-29T00:00:00Z",
    "2026-13-01T00:00:00Z",
    "2026-11-00T00:00:00Z",
    "2026-11-16T24:00:00Z",
    "2026-11-16T09:60:00Z",
    "2026-11-16T09:46:60Z",
    "２０２６-11-16T09:46:40Z",
    "",
  ]) {
    expect(utcSecondTimestamp(rejected), rejected).toBe(false);
  }
});

test("cod_returns_are_paired_and_bounded：COD 退回次数成对且受上限约束", () => {
  const withReturns = (status: string, count: unknown, limit: unknown): JsonObject => ({
    ...sold(status),
    cod_return_count: count,
    cod_return_limit: limit,
  });
  expect(validLot(withReturns("returned_by_buyer", 1, 3))).toBe(true);
  expect(validLot(withReturns("cod_return_limit_reached", 3, 3))).toBe(true);
  expect(validLot(withReturns("cod_return_limit_reached", 2, 3))).toBe(false);
  expect(validLot(withReturns("returned_by_buyer", 0, 3))).toBe(false);
  expect(validLot(withReturns("returned_by_buyer", 4, 3))).toBe(false);
  expect(validLot(withReturns("returned_by_buyer", 1, 11))).toBe(false);
  expect(validLot(withReturns("returned_by_buyer", "1", 3))).toBe(false);
  expect(validLot(sold("cod_resend_pending")), "重寄状态必有退回次数").toBe(false);
  const countOnly = { ...sold("returned_by_buyer"), cod_return_count: 1 };
  expect(validLot(countOnly), "次数与上限成对").toBe(false);
  const unsold = {
    ...lot("unsold_return_pending"),
    cod_return_count: 1,
    cod_return_limit: 3,
  };
  expect(validLot(unsold), "流拍没有退回次数").toBe(false);
  // 发生过退回后不会再处于首次付款的两种状态：带退回次数即矛盾组合。
  const awaiting = {
    ...withReturns("awaiting_payment", 1, 3),
    payment_deadline_at: "2026-11-16T09:46:40Z",
  };
  expect(validLot(awaiting), "等待确认收款不带退回次数").toBe(false);
  expect(validLot(withReturns("payment_mail_pending", 1, 3))).toBe(false);
  // 历史成交缺 recovery 时，等待确认收款可以既无期限也无退回次数。
  expect(validLot(sold("awaiting_payment"))).toBe(true);
  expect(validLot(sold("payment_mail_pending"))).toBe(true);
  // 发生过退回后仍可逾期或付款处理中，期间成对公开退回次数。
  const overdue = {
    ...withReturns("payment_overdue", 1, 3),
    payment_deadline_at: "2026-11-16T09:46:40Z",
  };
  expect(validLot(overdue)).toBe(true);
  expect(validLot(withReturns("payment_on_hold", 2, 3))).toBe(true);
});

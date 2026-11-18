// 成交分组在本页刷新时的标识与顺序。

import { expect, test } from "bun:test";
import type { PublicPriceHistoryEntryResponse } from "@auction/contracts/dto/public/catalog/history";

import { historyGroups } from "./presentation";

const SALE: PublicPriceHistoryEntryResponse = {
  lot_id: "lot-1",
  auction_id: "auction-1",
  world_id: "main",
  auction_sequence: 1,
  lot_number: 1,
  equipment_name: "Phase Robe",
  equipment_category: "cloth",
  equipment_level: null,
  buyer_name: "Buyer",
  seller_name: "Seller",
  price: 1000,
  closed_at: "2026-01-01T00:00:00Z",
};

test.each(["equipment", "buyer", "seller"])("组首变化时原组保留标识（%s）", (grouping) => {
  const newer = { ...SALE, lot_id: "lot-2", lot_number: 2, closed_at: "2026-01-02T00:00:00Z" };
  const [before] = historyGroups([SALE], grouping);
  const [after] = historyGroups([newer, SALE], grouping);
  expect(after?.key).toBe(before?.key);
  expect(after?.rows).toEqual([newer, SALE]);
});

test("实际分组字段或分组方式不同就使用不同标识", () => {
  const sharedLabel = { ...SALE, buyer_name: SALE.seller_name };
  const [byBuyer] = historyGroups([sharedLabel], "buyer");
  const [bySeller] = historyGroups([sharedLabel], "seller");
  const [otherBuyer] = historyGroups([SALE], "buyer");
  expect(byBuyer?.label).toBe(bySeller?.label);
  expect(byBuyer?.key).not.toBe(bySeller?.key);
  expect(byBuyer?.key).not.toBe(otherBuyer?.key);
});

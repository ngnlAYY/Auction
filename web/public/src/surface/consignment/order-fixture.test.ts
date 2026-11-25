// 订单与购买页的离线夹具：v2 公开目录响应与订单响应，复用 read-fixture 的传输与挂载替身。

import type {
  ListingDetailV2,
  ListingSummaryV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import type { OrderV2 } from "@auction/contracts/dto/public/consignment/order-v2";

import { detail as detailV1, LISTING_A, response, summary as summaryV1 } from "./read-fixture.test";

export const LISTINGS_PATH = "/api/v2/consignment/listings";
export const ORDERS_PATH = "/api/v2/me/consignment/orders";
export const CONTRACT_A = "00000000-0000-4000-8000-000000000501";
export const COMMAND_A = "00000000-0000-4000-8000-000000000510";

/** v2 公开摘要；`available` 且可购买时带 purchase 动作。 */
export function summary(listingId = LISTING_A, purchasable = false): ListingSummaryV2 {
  const base = summaryV1(listingId);
  return {
    ...base,
    // 只有寄售件才有代管态；可购买的一定是在售的寄售件。
    mode: purchasable ? "consigned" : base.mode,
    state: purchasable ? "available" : base.state,
    generation: purchasable ? 1 : null,
    effective_reason: null,
    allowed_actions: purchasable ? ["purchase"] : [],
    purchasable,
  };
}

/** v2 公开详情，装备快照沿用 v1 夹具。 */
export function detail(listingId = LISTING_A, purchasable = false): ListingDetailV2 {
  return { summary: summary(listingId, purchasable), equipment: detailV1(listingId).equipment };
}

/** 回显请求页码与页长的 v2 目录页响应。 */
export function pageResponse(url: URL, items: ListingSummaryV2[], total = items.length): Response {
  return response({
    items,
    total,
    page: Number(url.searchParams.get("page")),
    page_size: Number(url.searchParams.get("page_size")),
  });
}

/** 买家视角的成交订单：不带 fee 与 seller_amount。 */
export function buyerOrder(world: "main" | "isekai" = "main"): OrderV2 {
  return {
    contract_id: CONTRACT_A,
    kind: "sale",
    role: "buyer",
    listing_id: LISTING_A,
    equipment_id: "00000000-0000-4000-8000-000000000502",
    world,
    order_state: "awaiting_payment",
    delivery_stage: "sent",
    payment_stage: "unpaid",
    payout_stage: "not_applicable",
    refund_stage: "not_applicable",
    allowed_actions: [],
    version: 2,
    price: 101,
    deadline_at: "2026-11-28T18:10:00Z",
  };
}

/** 卖方视角的同一订单：P = F + S，另带 fee 与 seller_amount。 */
export function sellerOrder(world: "main" | "isekai" = "main"): OrderV2 {
  return {
    ...buyerOrder(world),
    role: "seller",
    payout_stage: "blocked",
    fee: 5,
    seller_amount: 96,
  };
}

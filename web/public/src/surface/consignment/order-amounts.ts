// 购买确认与订单页的金额口径：买家应付毛额与按角色可见的订单金额。

import type { ListingWorld } from "@auction/contracts/dto/public/consignment";
import type { OrderV2 } from "@auction/contracts/dto/public/consignment/order-v2";

/** 买家付款的构成：标价 P、载体本金与 COD 毛额。 */
export interface PurchaseQuote {
  readonly price: number;
  /** 载体本金（Credits）。main 直接付原件 COD，没有载体，为 0；isekai 载体含 1 Credit。 */
  readonly carrierCredit: 0 | 1;
  /** 买家在游戏内实付的 COD 毛额：main 为 P，isekai 为 P + 1。 */
  readonly codGross: number;
}

/**
 * 买家付款口径，与服务端 `expected_gross` 同口径（D9 §4.2）：
 * main 原件 COD 为 P，isekai 载体 COD 为 P + 1 而载体含 1 Credit。仅用于展示，
 * 金额、资格与幂等始终由服务端决定（不变量 9）。
 */
export function purchaseQuote(world: ListingWorld, price: number): PurchaseQuote {
  const carrierCredit = world === "isekai" ? 1 : 0;
  return { price, carrierCredit, codGross: price + carrierCredit };
}

/** 订单页按角色展示的金额行；买方只看到标价与应付，卖方另见手续费与到手。 */
export interface OrderAmountRows {
  readonly price: number;
  /** 买方才需要付 COD；卖方视图为 null。 */
  readonly codGross: number | null;
  readonly fee: number | null;
  readonly sellerAmount: number | null;
}

/** 撤回订单没有金额，返回 null；字段省略即不展示，不在浏览器重算手续费。 */
export function orderAmountRows(order: OrderV2): OrderAmountRows | null {
  if (order.kind === "withdrawal" || order.price === null) return null;
  const quote = purchaseQuote(order.world, order.price);
  return {
    price: order.price,
    codGross: order.role === "buyer" ? quote.codGross : null,
    fee: order.role === "seller" ? (order.fee ?? null) : null,
    sellerAmount: order.role === "seller" ? (order.seller_amount ?? null) : null,
  };
}

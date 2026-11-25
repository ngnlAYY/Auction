// 购买确认与订单页的金额口径：main 付 P，isekai 载体付 P + 1，卖方才见手续费与到手。

import { expect, test } from "bun:test";
import { orderAmountRows, purchaseQuote } from "./order-amounts";
import { buyerOrder, sellerOrder } from "./order-fixture.test";

test("main 付 P 且无载体本金，isekai 载体 COD 为 P + 1 并含 1 Credit", () => {
  expect(purchaseQuote("main", 101)).toEqual({ price: 101, carrierCredit: 0, codGross: 101 });
  expect(purchaseQuote("isekai", 101)).toEqual({ price: 101, carrierCredit: 1, codGross: 102 });
});

test("订单金额按角色显示：买方无 F/S，卖方无 COD；撤回订单没有金额", () => {
  expect(orderAmountRows(buyerOrder("isekai"))).toEqual({
    price: 101,
    codGross: 102,
    fee: null,
    sellerAmount: null,
  });
  expect(orderAmountRows(sellerOrder("main"))).toEqual({
    price: 101,
    codGross: null,
    fee: 5,
    sellerAmount: 96,
  });
  expect(
    orderAmountRows({ ...sellerOrder(), kind: "withdrawal", price: null, deadline_at: null }),
  ).toBeNull();
});

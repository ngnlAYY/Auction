// 页面路由归属判断：自动纠偏只在当前匹配仍属于本页时写回地址栏。

import { expect, test } from "bun:test";
import type { RouteMatch } from "@auction/web-shared/router";

import { routeBelongsToPage } from "./route-page";

const BID_LOG_ROUTE: RouteMatch = {
  kind: "matched",
  name: "BidLog",
  params: {},
  query: {},
};

test("route_belongs_to_page_only_accepts_the_same_matched_page：只有同名命中路由属于本页", () => {
  expect(routeBelongsToPage(BID_LOG_ROUTE, "BidLog")).toBe(true);
  expect(routeBelongsToPage(BID_LOG_ROUTE, "Fulfillment")).toBe(false);
  expect(routeBelongsToPage({ kind: "not-found", segments: [] }, "BidLog")).toBe(false);
});

test("route_belongs_to_page_matches_auction_list_by_name：往期拍卖同样按路由名判断", () => {
  const auctionListRoute: RouteMatch = {
    kind: "matched",
    name: "AuctionList",
    params: {},
    query: {},
  };
  expect(routeBelongsToPage(auctionListRoute, "AuctionList")).toBe(true);
  expect(routeBelongsToPage(auctionListRoute, "BidLog")).toBe(false);
});

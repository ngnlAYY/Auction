// 运营挂单列表的 URL 筛选和页码，非法筛选交给 API 边界拒绝。

import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { ListingFilters } from "../../api/consignment/list";
import { pairValue, parsePage, pushPage, pushText } from "../query/primitives";

const FILTER_KEYS = ["world", "mode", "state", "seller_user_id", "page"] as const;

/** 挂单列表请求所用的筛选与独立分页。 */
export interface ListingQuery extends ListingFilters {
  readonly page: bigint;
  readonly duplicate: boolean;
}

/** 挂载列表的筛选历史；页长由 AdminPager 单独管理。 */
export function useListingQuery(): QueryState<ListingQuery> {
  return useQueryState(
    FILTER_KEYS,
    (pairs) => ({
      world: pairValue(pairs, "world") ?? "",
      mode: pairValue(pairs, "mode") ?? "",
      state: pairValue(pairs, "state") ?? "",
      sellerUserId: pairValue(pairs, "seller_user_id") ?? "",
      page: parsePage(pairs, "page"),
      duplicate: FILTER_KEYS.some((key) => pairs.filter(([name]) => name === key).length > 1),
    }),
    (query) => {
      const pairs: QueryPair[] = [];
      pushText(pairs, "world", query.world);
      pushText(pairs, "mode", query.mode);
      pushText(pairs, "state", query.state);
      pushText(pairs, "seller_user_id", query.sellerUserId);
      pushPage(pairs, "page", query.page);
      return pairs;
    },
    { sameAddress: "write" },
  );
}

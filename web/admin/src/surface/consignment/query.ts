// 运营挂单列表的 URL 筛选和页码；畸形值按未设置处理，不转发给 API。

import { ListingStateV2Schema } from "@auction/contracts/dto/public/consignment/listing-v2";
import {
  ListingModeSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
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
      world: parseChoice(pairs, "world", ListingWorldSchema.options),
      mode: parseChoice(pairs, "mode", ListingModeSchema.options),
      state: parseChoice(pairs, "state", ListingStateV2Schema.options),
      sellerUserId: parseUuidText(pairs, "seller_user_id"),
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

// 履约进度页的筛选常量、世界范围与资源快照类型。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { Result } from "@auction/web-shared/result";

import type { FulfillmentRead } from "../../../api/fulfillment";
import type { ScopeSelection } from "../bid-log/scope-selection";

/** 写回输入筛选前的短暂防抖；地址栏只在用户停下后更新。 */
export const LOT_FILTER_WRITE_DEBOUNCE_MILLIS = 300;

/** 筛选错误提示的稳定 id；失效时输入框通过 aria-describedby 关联它。 */
export const LOT_FILTER_ERROR_ID = "public-fulfillment-filter-error";

/** 整理全部世界的已结算目录：稳定地先主世界、后异世界，世界内保持接口返回顺序；分页读取
 * 期间新结算的场次可能重复出现，按场次 id 保留首次出现的一项。 */
export function orderFulfillmentAuctionDirectory(
  auctions: readonly AuctionSummaryResponse[],
): AuctionSummaryResponse[] {
  const ordered: AuctionSummaryResponse[] = [];
  const seen = new Set<string>();
  for (const world of ["main", "isekai"] as const) {
    for (const auction of auctions) {
      if (auction.world_id !== world || seen.has(auction.id)) {
        continue;
      }
      seen.add(auction.id);
      ordered.push(auction);
    }
  }
  return ordered;
}

/** 正文的互斥状态：请求失败、目录为空、地址栏筛选非法、已确认的表格快照或加载中。
 * 非法筛选不进入加载态；表格状态只承载页码与分页大小已匹配的读取快照。 */
export type FulfillmentBodyState =
  | { readonly kind: "error" }
  | { readonly kind: "empty-auctions" }
  | { readonly kind: "invalid-filter" }
  | { readonly kind: "table"; readonly read: FulfillmentRead }
  | { readonly kind: "loading" }
  | { readonly kind: "unselected" };

/** 一次进度读取的查询身份。`selection` 防止跨范围或跨代际复用旧快照，`world` 是所选场次
 * 自己的世界：既用于发往服务端的请求，也用于展示时核对场次世界。完成快照只有与当前场次、
 * 范围选择、世界、筛选、页码与分页大小完全一致时才能展示，避免迟到结果覆盖新查询。 */
export type FulfillmentRequest = readonly [
  auctionId: string,
  selection: ScopeSelection,
  world: string,
  lot: string | null,
  page: number,
  pageSize: number,
];

/** 进度读取的完成快照及其查询身份；目录确认场次已不可见后，迟到结果不能恢复旧行和分页。 */
export interface FulfillmentProgressSnapshot {
  readonly request: FulfillmentRequest;
  readonly result: Result<FulfillmentRead | null, string>;
}

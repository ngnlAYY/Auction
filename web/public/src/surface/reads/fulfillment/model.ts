// 履约进度页的筛选常量与资源快照类型。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { Result } from "@auction/web-shared/result";

import type { FulfillmentRead } from "../../../api/fulfillment";

/** 写回输入筛选前的短暂防抖；地址栏只在用户停下后更新。 */
export const LOT_FILTER_WRITE_DEBOUNCE_MILLIS = 300;

/** 筛选错误提示的稳定 id；失效时输入框通过 aria-describedby 关联它。 */
export const LOT_FILTER_ERROR_ID = "public-fulfillment-filter-error";

/** 正文的互斥状态：请求失败、目录为空、地址栏筛选非法、已确认的表格快照或加载中。
 * 非法筛选不进入加载态；表格状态只承载页码与分页大小已匹配的读取快照。 */
export type FulfillmentBodyState =
  | { readonly kind: "error" }
  | { readonly kind: "empty-auctions" }
  | { readonly kind: "invalid-filter" }
  | { readonly kind: "table"; readonly read: FulfillmentRead }
  | { readonly kind: "loading" };

/** 一次进度读取的查询身份。完成快照只有与当前场次、世界、筛选、页码与分页大小完全一致时
 * 才能展示，避免迟到结果覆盖新查询。 */
export type FulfillmentRequest = readonly [
  auctionId: string,
  world: string,
  lot: string | null,
  page: number,
  pageSize: number,
];

/** 已结算场次目录的读取快照；世界与刷新代号共同绑定。后台刷新保留的旧目录只能用于展示，
 * 不能纠偏新世界或新代际的场次选择。 */
export interface FulfillmentAuctionsSnapshot {
  readonly world: string;
  readonly generation: number;
  readonly result: Result<AuctionSummaryResponse[], string>;
}

/** 进度读取的完成快照及其查询身份；目录确认场次已不可见后，迟到结果不能恢复旧行和分页。 */
export interface FulfillmentProgressSnapshot {
  readonly request: FulfillmentRequest;
  readonly result: Result<FulfillmentRead | null, string>;
}

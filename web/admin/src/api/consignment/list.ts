// 运营挂单列表的筛选草稿与偏移预算；读取走 v2（read-v2.ts）。

/** 含空串的筛选草稿；非空值须符合各字段的契约。 */
export interface ListingFilters {
  readonly world: string;
  readonly mode: string;
  readonly state: string;
  readonly sellerUserId: string;
}

/** 与寄售读取端的偏移预算一致。 */
export const LISTING_MAX_OFFSET = 10_000;

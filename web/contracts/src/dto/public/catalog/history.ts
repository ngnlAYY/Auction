// Public 竞价日志和价格历史 DTO。

import * as v from "valibot";
import { integer, optionalNullable, struct } from "../../../wire";

/** 竞价日志中落败代理的公开投影：只有出价人与落败时的上限，不含模式。 */
export const PublicDefeatedProxyResponseSchema = struct({
  bidder_name: v.string(),
  maximum_amount: integer,
});

export type PublicDefeatedProxyResponse = v.InferOutput<typeof PublicDefeatedProxyResponseSchema>;

/** Public 竞价日志条目的稳定响应。 */
export const PublicBidLogEntryResponseSchema = struct({
  id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  lot_number: integer,
  amount: integer,
  equipment_name: v.string(),
  equipment_category: v.string(),
  bidder_name: v.string(),
  source: optionalNullable(v.string()),
  source_post_number: optionalNullable(integer),
  /** 仅在论坛主题、帖子与楼层均可证明时返回的真实帖子链接。 */
  source_post_url: optionalNullable(v.string()),
  valid: v.boolean(),
  accepted_state_version: integer,
  fact_index: integer,
  mechanism: v.string(),
  created_at: v.string(),
  /** 本条代理出价在代理对代理竞争中击败的授权；只在 `include=defeated_proxy` 时输出，
   * 页面在本条之前按一次代理出价展示。 */
  defeated_proxy: optionalNullable(PublicDefeatedProxyResponseSchema),
});

export type PublicBidLogEntryResponse = v.InferOutput<typeof PublicBidLogEntryResponseSchema>;

/** Public 竞价日志列表响应；编号页与 cursor 页使用已发布的互斥字段形状。 */
export const PublicBidLogListResponseSchema = struct({
  bids: v.array(PublicBidLogEntryResponseSchema),
  page: optionalNullable(integer),
  page_size: integer,
  total: optionalNullable(integer),
  cursor: optionalNullable(v.string()),
  next_cursor: optionalNullable(v.string()),
  has_more: optionalNullable(v.boolean()),
});

export type PublicBidLogListResponse = v.InferOutput<typeof PublicBidLogListResponseSchema>;

/** Public 价格历史条目的稳定响应。 */
export const PublicPriceHistoryEntryResponseSchema = struct({
  lot_id: v.string(),
  auction_id: v.string(),
  world_id: v.string(),
  auction_sequence: integer,
  lot_number: integer,
  equipment_name: v.string(),
  equipment_category: v.string(),
  equipment_level: optionalNullable(integer),
  buyer_name: v.string(),
  seller_name: v.string(),
  price: integer,
  closed_at: v.string(),
});

export type PublicPriceHistoryEntryResponse = v.InferOutput<
  typeof PublicPriceHistoryEntryResponseSchema
>;

/** Public 价格历史编号页的完整过滤集统计。 */
export const PublicPriceHistoryStatsResponseSchema = struct({
  count: integer,
  min: integer,
  max: integer,
  median: integer,
  /** 精确中位数；规范十进制整数或 .5，缺失时沿用旧整数 median。 */
  median_exact: optionalNullable(v.string()),
});

export type PublicPriceHistoryStatsResponse = v.InferOutput<
  typeof PublicPriceHistoryStatsResponseSchema
>;

/** Public 价格历史响应；cursor 页不携带 stats/total。 */
export const PublicPriceHistoryListResponseSchema = struct({
  rows: v.array(PublicPriceHistoryEntryResponseSchema),
  stats: optionalNullable(PublicPriceHistoryStatsResponseSchema),
  page: optionalNullable(integer),
  page_size: integer,
  total: optionalNullable(integer),
  cursor: optionalNullable(v.string()),
  next_cursor: optionalNullable(v.string()),
  has_more: optionalNullable(v.boolean()),
});

export type PublicPriceHistoryListResponse = v.InferOutput<
  typeof PublicPriceHistoryListResponseSchema
>;

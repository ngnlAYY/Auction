// Public 拍卖历史列表和当前拍卖 DTO。

import * as v from "valibot";
import { integer, optionalNullable, struct } from "../../../wire";

/** Public 拍卖历史列表中的稳定摘要。 */
export const AuctionSummaryResponseEntries = {
  id: v.string(),
  world_id: v.string(),
  sequence: integer,
  status: v.string(),
  scheduled_open_at: v.string(),
  opened_at: optionalNullable(v.string()),
  final_phase_at: optionalNullable(v.string()),
  effective_end_at: optionalNullable(v.string()),
  settled_at: optionalNullable(v.string()),
  lot_count: integer,
  sold_count: integer,
  unsold_count: integer,
  sold_amount: integer,
  state_version: integer,
};

export const AuctionSummaryResponseSchema = struct(AuctionSummaryResponseEntries);

export type AuctionSummaryResponse = v.InferOutput<typeof AuctionSummaryResponseSchema>;

/** v2 拍卖摘要：沿用 v1 的平铺字段，论坛地址只在新版本中发布。 */
export const AuctionSummaryV2ResponseSchema = struct({
  ...AuctionSummaryResponseEntries,
  /** 已发布且有内容证明的论坛主题地址；没有主题或尚未对账时省略。 */
  forum_thread_url: optionalNullable(v.string()),
});

export type AuctionSummaryV2Response = v.InferOutput<typeof AuctionSummaryV2ResponseSchema>;

/** Public 历史拍卖分页信封；默认条目维持 v1 契约，v2 显式选择扩展摘要。 */
export function auctionListResponseSchema<const TSchema extends v.GenericSchema>(
  auctionSchema: TSchema,
) {
  return struct({
    auctions: v.array(auctionSchema),
    page: optionalNullable(integer),
    page_size: integer,
    total: optionalNullable(integer),
    cursor: optionalNullable(v.string()),
    next_cursor: optionalNullable(v.string()),
    has_more: optionalNullable(v.boolean()),
  });
}

export type AuctionListResponse<T = AuctionSummaryResponse> = {
  auctions: T[];
  page: number | null;
  page_size: number;
  total: number | null;
  cursor: string | null;
  next_cursor: string | null;
  has_more: boolean | null;
};

/** 当前拍卖条目的稳定响应。 */
export const CurrentAuctionItemResponseSchema = struct({
  id: v.string(),
  world: v.string(),
  number: integer,
  status: v.string(),
  opens_at: v.string(),
  scheduled_ends_at: optionalNullable(v.string()),
  presentation_mode: v.string(),
  bidding_open: v.boolean(),
  preview_revision: integer,
  /** 该场冻结策略的代理默认模式（`fast`|`slow`），Public 代理新建表单以它为初始选项；
   * 服务端只在请求带 `include=proxy_default_mode` 时输出，缺省响应省略该键，保持旧页面按闭集
   * 解码的原字段集合；浏览器按往期目录拼装的已结算场次没有该值。 */
  proxy_default_mode: optionalNullable(v.string()),
});

export type CurrentAuctionItemResponse = v.InferOutput<typeof CurrentAuctionItemResponseSchema>;

/** 当前拍卖中的公开上下文。 */
export const CurrentAuctionResponseSchema = struct({
  auction: optionalNullable(CurrentAuctionItemResponseSchema),
});

export type CurrentAuctionResponse = v.InferOutput<typeof CurrentAuctionResponseSchema>;

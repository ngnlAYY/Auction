// Public 履约进度 DTO：已结算场次逐件的结算后流程状态。
// 字段闭集首发锁定；新增字段只能经可选查询参数或新版本路由。不含邮件、投递、履约或命令标识、
// 诊断码、Bot 身份与代理模式。

import * as v from "valibot";
import { int32, integer, optionalNullable, struct } from "../../../wire";

/** 履约进度所属的已结算场次。 */
export const PublicFulfillmentAuctionResponseSchema = struct({
  id: v.string(),
  world: v.string(),
  /** 场次期号（`auctions.sequence`）。 */
  number: integer,
});

export type PublicFulfillmentAuctionResponse = v.InferOutput<
  typeof PublicFulfillmentAuctionResponseSchema
>;

/** 单件 lot 的公开履约状态。 */
export const PublicFulfillmentLotResponseSchema = struct({
  lot_id: v.string(),
  /** 类别内编号，与竞价日志、补偿池明细同一口径。 */
  lot_number: int32,
  equipment_name: v.string(),
  equipment_category: v.string(),
  /** 投稿卖家名；没有投稿记录的历史 lot 为 `—`。 */
  seller_name: v.string(),
  /** 成交 lot 的买家当前用户名，与 `final_price` 成对出现；流拍不输出。 */
  buyer_name: optionalNullable(v.string()),
  final_price: optionalNullable(integer),
  /** 闭集状态，取值与含义见路由契约。 */
  status: v.string(),
  /** 当前付款期限（RFC 3339 UTC，秒精度）；只在等待付款、已逾期与重寄后等待付款时输出。 */
  payment_deadline_at: optionalNullable(v.string()),
  /** 发生过 COD 退回时与 `cod_return_limit` 成对输出。 */
  cod_return_count: optionalNullable(int32),
  cod_return_limit: optionalNullable(int32),
});

export type PublicFulfillmentLotResponse = v.InferOutput<typeof PublicFulfillmentLotResponseSchema>;

/** 履约进度编号页。 */
export const PublicFulfillmentPageResponseSchema = struct({
  auction: PublicFulfillmentAuctionResponseSchema,
  lots: v.array(PublicFulfillmentLotResponseSchema),
  page: integer,
  page_size: integer,
  total: integer,
});

export type PublicFulfillmentPageResponse = v.InferOutput<
  typeof PublicFulfillmentPageResponseSchema
>;

// Public 补偿池及账本 DTO。

import * as v from "valibot";
import { int32, integer, optionalNullable, struct } from "../../../wire";

/** Public 补偿池批次的稳定响应。 */
export const PublicCompensationRoundResponseSchema = struct({
  auction_id: v.string(),
  world_id: v.string(),
  auction_sequence: integer,
  /** 该场冻结的结算算法：`legacy_zero_v0`、`hold_curve_v1` 或 `hold_curve_v2`。 */
  policy_mode: v.string(),
  opening_balance: integer,
  tax_income: integer,
  discount_demand: integer,
  compensation_out: integer,
  closing_balance: integer,
  movement_count: integer,
  /** 本场成交撤销后写入的冲正笔数。 */
  reversal_count: integer,
  /** 本场冲正对池余额的带符号净变化。 */
  reversal_amount: integer,
  settled_at: v.string(),
});

export type PublicCompensationRoundResponse = v.InferOutput<
  typeof PublicCompensationRoundResponseSchema
>;

/** Public 补偿池账本 movement 的稳定响应；完整装备 URL 只允许进入 wire，调试输出必须脱敏。 */
export const PublicCompensationMovementResponseSchema = struct({
  sequence: int32,
  lot_id: optionalNullable(v.string()),
  lot_number: optionalNullable(int32),
  equipment_name: optionalNullable(v.string()),
  equipment_category: optionalNullable(v.string()),
  game_equipment_url: optionalNullable(v.string()),
  hold_ppm: optionalNullable(int32),
  adjustment_ppm: optionalNullable(int32),
  leader_held_nanos: optionalNullable(integer),
  kind: v.string(),
  amount: integer,
  balance_after: integer,
  created_at: v.string(),
});

export type PublicCompensationMovementResponse = v.InferOutput<
  typeof PublicCompensationMovementResponseSchema
>;

/** Public 补偿池批次列表响应；编号页与 cursor 页字段互斥。 */
export const PublicCompensationResponseSchema = struct({
  /** 两个世界共用的补偿池（main Credits）当前余额。 */
  balance: integer,
  /** 补偿池最近一次已提交更新时间。 */
  updated_at: v.string(),
  rounds: v.array(PublicCompensationRoundResponseSchema),
  page: optionalNullable(integer),
  page_size: integer,
  total: optionalNullable(integer),
  cursor: optionalNullable(v.string()),
  next_cursor: optionalNullable(v.string()),
  has_more: optionalNullable(v.boolean()),
});

export type PublicCompensationResponse = v.InferOutput<typeof PublicCompensationResponseSchema>;

export const PublicCompensationMovementPageResponseSchema = struct({
  round: PublicCompensationRoundResponseSchema,
  movements: v.array(PublicCompensationMovementResponseSchema),
  total: integer,
  page: integer,
  page_size: integer,
});

export type PublicCompensationMovementPageResponse = v.InferOutput<
  typeof PublicCompensationMovementPageResponseSchema
>;

/** 成交撤销后对补偿池的冲正。 */
export const PublicCompensationLotReversalResponseSchema = struct({
  /** 对池余额的带符号净变化：退回补偿减去冲回附加费。 */
  amount: integer,
  /** 池余额不足、未能冲回的附加费。 */
  tax_shortfall: integer,
  balance_after: integer,
  reversed_at: v.string(),
});

export type PublicCompensationLotReversalResponse = v.InferOutput<
  typeof PublicCompensationLotReversalResponseSchema
>;

/** 单个已售 lot 的公开结算明细；完整装备 URL 只允许进入 wire，调试输出必须脱敏。 */
export const PublicCompensationLotResponseSchema = struct({
  lot_id: v.string(),
  lot_number: int32,
  equipment_name: v.string(),
  equipment_category: v.string(),
  game_equipment_url: v.string(),
  end_price: integer,
  hold_ppm: int32,
  adjustment_ppm: int32,
  leader_held_nanos: integer,
  tax_amount: integer,
  /** 名义优惠需求：按调整比例计算、尚未按池余额缩放的优惠。 */
  discount_amount: integer,
  compensation_amount: integer,
  buyer_amount: integer,
  seller_amount: integer,
  /** 买家玩家名，与成交历史同一口径。 */
  buyer_name: v.string(),
  /** 卖家玩家名；没有投稿记录的历史 lot 为 `—`。 */
  seller_name: v.string(),
  /** 成交是否已被撤销；升级前撤销的成交可能没有冲正。 */
  voided: v.boolean(),
  reversal: optionalNullable(PublicCompensationLotReversalResponseSchema),
});

export type PublicCompensationLotResponse = v.InferOutput<
  typeof PublicCompensationLotResponseSchema
>;

export const PublicCompensationLotPageResponseSchema = struct({
  round: PublicCompensationRoundResponseSchema,
  lots: v.array(PublicCompensationLotResponseSchema),
  total: integer,
  page: integer,
  page_size: integer,
});

export type PublicCompensationLotPageResponse = v.InferOutput<
  typeof PublicCompensationLotPageResponseSchema
>;

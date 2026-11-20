// Admin 结算报告、拍卖生命周期投影与列表响应 DTO。

import * as v from "valibot";
import {
  displayDurationNanos,
  int32,
  integer,
  optionalField,
  optionalNullable,
  requiredNullable,
  strictStruct,
  struct,
} from "../../wire";
import type { AdminStrictPaginationResponse } from "./pagination";
import { AdminStrictPaginationResponseSchema } from "./pagination";

export const AdminSettlementAuctionResponseSchema = struct({
  id: v.string(),
  world_id: v.string(),
  sequence: integer,
  status: v.string(),
  preview_scheduled_at: v.string(),
  scheduled_open_at: v.string(),
  preview_opened_at: optionalNullable(v.string()),
  preview_manual_open: v.boolean(),
  preview_revision: integer,
  opened_at: optionalNullable(v.string()),
  final_phase_at: optionalNullable(v.string()),
  settled_at: optionalNullable(v.string()),
  cancelled_at: optionalNullable(v.string()),
  cancelled_reason: optionalNullable(v.string()),
  cancelled_by: optionalNullable(v.string()),
  state_version: integer,
  created_at: v.string(),
  updated_at: v.string(),
});

export type AdminSettlementAuctionResponse = v.InferOutput<
  typeof AdminSettlementAuctionResponseSchema
>;

/** Admin 拍卖命令能力；仅表示对应路由存在，不能据此扩大业务授权。 */
export const AdminAuctionCapabilitiesResponseSchema = strictStruct({
  cancel_auction: v.boolean(),
  manual_early_open: v.boolean(),
  manual_early_settlement: v.boolean(),
  manual_intake: v.boolean(),
  manual_preview_open: v.boolean(),
  modify_schedule: v.boolean(),
});

export type AdminAuctionCapabilitiesResponse = v.InferOutput<
  typeof AdminAuctionCapabilitiesResponseSchema
>;

/** Admin 拍卖列表中的生命周期安全投影。
 * 反序列化按旧浏览器 decoder 的闭集语义收紧：nullable 生命周期字段必须显式出现（可为 null），
 * `lot_count` 允许省略但出现时不得为 null；未知字段一律拒绝。 */
export const AdminAuctionResponseSchema = strictStruct({
  id: v.string(),
  world_id: v.string(),
  sequence: integer,
  status: v.string(),
  preview_scheduled_at: v.string(),
  scheduled_open_at: v.string(),
  preview_opened_at: requiredNullable(v.string()),
  preview_manual_open: v.boolean(),
  preview_revision: integer,
  opened_at: requiredNullable(v.string()),
  final_phase_at: requiredNullable(v.string()),
  settled_at: requiredNullable(v.string()),
  cancelled_at: requiredNullable(v.string()),
  cancelled_reason: requiredNullable(v.string()),
  cancelled_by: requiredNullable(v.string()),
  state_version: integer,
  /** 键缺失与 null 都读作 null。 */
  scheduled_ends_at: optionalNullable(v.string()),
  /** 旧 wire 的兼容扩展字段：允许缺失，但一旦出现就不接受 JSON null。 */
  lot_count: optionalField(integer),
  created_at: v.string(),
  updated_at: v.string(),
});

export type AdminAuctionResponse = v.InferOutput<typeof AdminAuctionResponseSchema>;

/** Admin 拍卖列表数字分页。 */
export type AdminAuctionPaginationResponse = AdminStrictPaginationResponse;

/** Admin 拍卖列表响应；列表和 lot 数量来自同一 PostgreSQL 快照。 */
export const AdminAuctionListResponseSchema = strictStruct({
  auctions: v.pipe(v.array(AdminAuctionResponseSchema), v.readonly()),
  capabilities: AdminAuctionCapabilitiesResponseSchema,
  pagination: AdminStrictPaginationResponseSchema,
});

export type AdminAuctionListResponse = v.InferOutput<typeof AdminAuctionListResponseSchema>;

export const AdminSettlementSummaryResponseSchema = struct({
  opening_balance: integer,
  tax_income: integer,
  discount_demand: integer,
  compensation_out: integer,
  closing_balance: integer,
  movement_count: integer,
  settled_at: v.string(),
});

export type AdminSettlementSummaryResponse = v.InferOutput<
  typeof AdminSettlementSummaryResponseSchema
>;

/** Admin 结算 lot 的 adjustment metadata。 */
export const AdminSettlementAdjustmentResponseSchema = struct({
  policy: optionalNullable(v.string()),
});

export type AdminSettlementAdjustmentResponse = v.InferOutput<
  typeof AdminSettlementAdjustmentResponseSchema
>;

/** Admin 结算报告中的单个 lot。 */
export const AdminSettlementLotResponseSchema = struct({
  id: v.string(),
  lot_number: int32,
  status: v.string(),
  equipment_name: v.string(),
  equipment_category: v.string(),
  /** 游戏装备详情链接；由 Admin 后端按世界和外部装备标识生成。 */
  game_equipment_url: v.string(),
  seller: optionalNullable(v.string()),
  winner: optionalNullable(v.string()),
  final_price: integer,
  listed_price: integer,
  buyer_amount: integer,
  seller_amount: integer,
  tax_amount: integer,
  /** 奖池实分支出；hold_curve_v2 仅为按全场预算同比缩放后的基础优惠。 */
  compensation_amount: integer,
  hold_ppm: int32,
  adjustment_ppm: int32,
  /** 赢家累计领先纳秒数；流拍为 0。 */
  leader_held_nanos: displayDurationNanos,
  adjustment: AdminSettlementAdjustmentResponseSchema,
});

export type AdminSettlementLotResponse = v.InferOutput<typeof AdminSettlementLotResponseSchema>;

/** Admin 结算报告响应；未结算拍卖的 `settlement` 为 null，且不伪造 lot 金额。 */
export const AdminSettlementReportResponseSchema = struct({
  auction: AdminSettlementAuctionResponseSchema,
  settlement: optionalNullable(AdminSettlementSummaryResponseSchema),
  lots: v.pipe(v.array(AdminSettlementLotResponseSchema), v.readonly()),
});

export type AdminSettlementReportResponse = v.InferOutput<
  typeof AdminSettlementReportResponseSchema
>;

/** 旧结算读取 decoder 的严格镜像：`settlement`、`seller`、`winner` 的键必须显式出现（可为
 * `null`），内嵌 `auction` 使用 Admin 列表的闭集形状，`adjustment.policy` 可以缺省但显式
 * `null` 会被拒绝。结算读取只用于展示，不是 G1 的命令回执，因此不附字段策略；金额、状态与
 * 归属等路由业务校验仍由结算 API 车道完成。 */
const AdminSettlementAdjustmentStrictSchema = struct({
  policy: optionalField(v.string()),
});

const AdminSettlementLotStrictSchema = struct({
  id: v.string(),
  lot_number: int32,
  status: v.string(),
  equipment_name: v.string(),
  equipment_category: v.string(),
  game_equipment_url: v.string(),
  seller: requiredNullable(v.string()),
  winner: requiredNullable(v.string()),
  final_price: integer,
  listed_price: integer,
  buyer_amount: integer,
  seller_amount: integer,
  tax_amount: integer,
  compensation_amount: integer,
  hold_ppm: int32,
  adjustment_ppm: int32,
  leader_held_nanos: displayDurationNanos,
  adjustment: AdminSettlementAdjustmentStrictSchema,
});

export const AdminStrictSettlementReportResponseSchema = struct({
  auction: AdminAuctionResponseSchema,
  settlement: requiredNullable(AdminSettlementSummaryResponseSchema),
  lots: v.pipe(v.array(AdminSettlementLotStrictSchema), v.readonly()),
});

export type AdminStrictSettlementReportResponse = v.InferOutput<
  typeof AdminStrictSettlementReportResponseSchema
>;

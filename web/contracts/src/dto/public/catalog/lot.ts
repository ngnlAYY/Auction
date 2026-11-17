// Public 正式/预览 lot、装备和响应外壳 DTO。

import * as v from "valibot";
import {
  float,
  integer,
  jsonValue,
  optionalField,
  optionalNullable,
  stringMap,
  struct,
  withDefault,
} from "../../../wire";

type EquipmentCategoryCode = readonly [string, string, string];

/** 公开分类键、规范装备大类名与 lot 代码前缀的对照表，供两端浏览器共用。
 *
 * 分类键是 lot 列表筛选与 `category_counts` 的 JSON 键，大类名是装备 `category` 字段值，
 * 前缀组成玩家可见的 lot 代码；顺序与分类统计一致。与领域映射的逐项一致性由 `auction-http`
 * 测试核对，修改任何一项都会改变论坛可竞价代码。 */
export const EQUIPMENT_CATEGORY_CODES: readonly [
  EquipmentCategoryCode,
  EquipmentCategoryCode,
  EquipmentCategoryCode,
  EquipmentCategoryCode,
  EquipmentCategoryCode,
  EquipmentCategoryCode,
  EquipmentCategoryCode,
] = [
  ["one", "One-Handed", "One"],
  ["two", "Two-Handed", "Two"],
  ["sta", "Staff", "Sta"],
  ["shd", "Shield", "Shd"],
  ["clo", "Cloth", "Clo"],
  ["lig", "Light", "Lig"],
  ["hea", "Heavy", "Hea"],
];

/** 终阶段代理限制的公开契约。 */
export const PublicFinalRestrictionResponseSchema = struct({
  new_slow_proxy_allowed: v.boolean(),
  existing_slow_proxy_converted: v.boolean(),
});

export type PublicFinalRestrictionResponse = v.InferOutput<
  typeof PublicFinalRestrictionResponseSchema
>;

/** 公开装备 stat 的稳定 JSON 形状。 */
export const PublicEquipmentStatResponseSchema = struct({
  value: v.string(),
  base: optionalNullable(float),
  roll: optionalNullable(float),
});

export type PublicEquipmentStatResponse = v.InferOutput<typeof PublicEquipmentStatResponseSchema>;

const NONEMPTY_STRING = v.pipe(v.string(), v.nonEmpty());

/** 公开装备详情；attributes 仅由 lot 详情显式传入。
 * 完整游戏装备 URL 含 key，允许 wire 展示但不能进入调试输出。 */
export const PublicEquipmentResponseSchema = struct({
  name: v.string(),
  category: v.string(),
  /** 与论坛标色规则同源的装备分组：`rare` 或 `fusion`。 */
  equipment_group: v.string(),
  level: optionalNullable(integer),
  primary_attribute_initials: optionalField(NONEMPTY_STRING),
  info_summary: withDefault(v.string(), ""),
  stats: withDefault(stringMap(PublicEquipmentStatResponseSchema), {}),
  avg_roll: optionalNullable(float),
  /** lot 列表与详情返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。 */
  game_equipment_url: v.string(),
  attributes: optionalNullable(jsonValue),
});

export type PublicEquipmentResponse = v.InferOutput<typeof PublicEquipmentResponseSchema>;

/** 正式竞拍 lot 的公开契约。 */
export const PublicLiveLotResponseSchema = struct({
  id: v.string(),
  auction_id: v.string(),
  lot_number: integer,
  status: v.string(),
  start_price: integer,
  current_price: integer,
  next_bid: integer,
  has_leader: v.boolean(),
  leader_name: optionalNullable(v.string()),
  seller_name: v.string(),
  effective_end_at: optionalNullable(v.string()),
  state_version: integer,
  equipment: PublicEquipmentResponseSchema,
});

export type PublicLiveLotResponse = v.InferOutput<typeof PublicLiveLotResponseSchema>;

/** 预览 lot 的公开契约；调试输出保留公开装备属性，但隐藏完整游戏装备 URL 中的 key。 */
export const PublicPreviewLotResponseEntries = {
  id: v.string(),
  auction_id: v.string(),
  code: v.string(),
  status: v.string(),
  seller_name: v.string(),
  start_price: integer,
  equipment_name: v.string(),
  /** 与正式目录共用的装备分组：`rare` 或 `fusion`。 */
  equipment_group: v.string(),
  equipment_category: optionalField(NONEMPTY_STRING),
  equipment_level: optionalNullable(integer),
  primary_attribute_initials: optionalField(NONEMPTY_STRING),
  avg_roll: optionalNullable(float),
  /** lot 列表与详情返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。 */
  game_equipment_url: v.string(),
};

export const PublicPreviewLotResponseSchema = struct(PublicPreviewLotResponseEntries);

export type PublicPreviewLotResponse = v.InferOutput<typeof PublicPreviewLotResponseSchema>;

/** 预览详情装备的公开契约；分类在旧版 wire 中允许缺失。
 * 与正式详情相同，完整游戏装备 URL 只进入 wire，不进入调试输出。 */
export const PublicPreviewEquipmentResponseSchema = struct({
  name: v.string(),
  /** 必须与预览摘要的装备分组相同。 */
  equipment_group: v.string(),
  category: optionalField(NONEMPTY_STRING),
  level: optionalNullable(integer),
  primary_attribute_initials: optionalField(NONEMPTY_STRING),
  info_summary: withDefault(v.string(), ""),
  stats: withDefault(stringMap(PublicEquipmentStatResponseSchema), {}),
  avg_roll: optionalNullable(float),
  /** lot 详情返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。 */
  game_equipment_url: v.string(),
});

export type PublicPreviewEquipmentResponse = v.InferOutput<
  typeof PublicPreviewEquipmentResponseSchema
>;

/** 预览 lot 详情沿用摘要字段，并附加完整公开装备投影。 */
export const PublicPreviewLotDetailResponseSchema = struct({
  ...PublicPreviewLotResponseEntries,
  equipment: PublicPreviewEquipmentResponseSchema,
});

export type PublicPreviewLotDetailResponse = v.InferOutput<
  typeof PublicPreviewLotDetailResponseSchema
>;

/** lot 列表的无标签数组，保持旧 JSON 契约的 preview/live 形状。 */
export const PublicLotListItemsResponseSchema = v.union([
  v.array(PublicPreviewLotResponseSchema),
  v.array(PublicLiveLotResponseSchema),
]);

export type PublicLotListItemsResponse = v.InferOutput<typeof PublicLotListItemsResponseSchema>;

/** lot 详情的无标签对象，保持旧 JSON 契约的 preview/live 形状。 */
export const PublicLotDetailItemResponseSchema = v.union([
  PublicPreviewLotDetailResponseSchema,
  PublicLiveLotResponseSchema,
]);

export type PublicLotDetailItemResponse = v.InferOutput<typeof PublicLotDetailItemResponseSchema>;

/** Public lot 列表与详情共用的响应外壳。
 * 旧 v1 客户端对顶层字段使用严格闭集校验；世界、场次号和计划结束时间只属于
 * `/api/v1/auctions/current`，不能作为扩展字段混入 lot 响应。 */
export const PublicLotEnvelopeResponseSchema = struct({
  auction_id: v.string(),
  auction_status: v.string(),
  projection_kind: v.string(),
  presentation_mode: v.string(),
  bidding_open: v.boolean(),
  opens_at: v.string(),
  preview_revision: integer,
  final_restriction: optionalNullable(PublicFinalRestrictionResponseSchema),
  lots: optionalNullable(PublicLotListItemsResponseSchema),
  lot: optionalNullable(PublicLotDetailItemResponseSchema),
  total: optionalNullable(integer),
  page: optionalNullable(integer),
  page_size: optionalNullable(integer),
  category_counts: optionalNullable(stringMap(integer)),
});

export type PublicLotEnvelopeResponse = v.InferOutput<typeof PublicLotEnvelopeResponseSchema>;

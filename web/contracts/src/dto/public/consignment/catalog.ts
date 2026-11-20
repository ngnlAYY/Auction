// 寄售能力、列表分页、公开目录与可安全显示的装备快照。

import * as v from "valibot";

import { float, requiredNullable, strictStruct, unsignedInteger } from "../../../wire";
import { MAX_PUBLIC_PAGE_SIZE } from "../pagination";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ExternalEquipmentIdSchema,
  LISTING_PRICE_MAX,
  ListingFeeBpsSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "./scalar";
import {
  ListingModeSchema,
  ListingWorldSchema,
  listingStateMatchesMode,
  PublicListingStateSchema,
} from "./states";

/** 匿名可读的寄售能力，前三期收件和购买固定为 false。 */
export const ConsignmentStatusSchema = v.pipe(
  strictStruct({
    enabled: v.boolean(),
    registration_ready: v.boolean(),
    registration_modes: v.array(ListingModeSchema),
    registration_worlds: v.array(ListingWorldSchema),
    accepts_mail: v.literal(false),
    purchasable: v.literal(false),
    fee_bps: ListingFeeBpsSchema,
    min_price: v.literal(1),
    max_price: v.literal(LISTING_PRICE_MAX),
  }),
  v.check((status) =>
    status.registration_ready
      ? status.enabled
      : status.registration_modes.length === 0 && status.registration_worlds.length === 0,
  ),
);
/** Public 页面可用的服务能力快照。 */
export type ConsignmentStatus = v.InferOutput<typeof ConsignmentStatusSchema>;

/** 公开目录、本人和运营列表共用分页契约，空页仍有 total。 */
export function listingPageSchema<const TItem extends v.GenericSchema>(itemSchema: TItem) {
  return v.pipe(
    strictStruct({
      items: v.array(itemSchema),
      total: unsignedInteger,
      page: v.pipe(unsignedInteger, v.minValue(1)),
      page_size: v.pipe(unsignedInteger, v.minValue(1), v.maxValue(MAX_PUBLIC_PAGE_SIZE)),
    }),
    v.check(({ page, page_size }) => page - 1 <= Math.floor(10_000 / page_size)),
  );
}

/** 展示等级使用本站判别联合，永不透传来源等级值。 */
export const EquipmentLevelSchema = v.union([
  strictStruct({
    kind: v.literal("assigned"),
    value: v.pipe(unsignedInteger, v.minValue(1), v.maxValue(100_000)),
  }),
  strictStruct({ kind: v.literal("unassigned") }),
  strictStruct({ kind: v.literal("soulbound") }),
]);
/** 白名单快照的展示等级。 */
export type EquipmentLevel = v.InferOutput<typeof EquipmentLevelSchema>;

const EQUIPMENT_NAME = consignmentTextSchema(512);
const CATEGORY_OR_STAT_NAME = consignmentTextSchema(128);
const STAT_NUMBER = v.pipe(float, v.minValue(-1e9), v.maxValue(1e9));

/** 允许展示的单条装备属性，base 键必须出现。 */
export const EquipmentStatSchema = strictStruct({
  name: CATEGORY_OR_STAT_NAME,
  value: STAT_NUMBER,
  base: requiredNullable(STAT_NUMBER),
});
/** 展示用属性数值。 */
export type EquipmentStat = v.InferOutput<typeof EquipmentStatSchema>;

/** 稳定命名的装备属性分组。 */
export const EquipmentStatGroupSchema = strictStruct({
  name: CATEGORY_OR_STAT_NAME,
  stats: v.array(EquipmentStatSchema),
});
/** 展示用属性分组。 */
export type EquipmentStatGroup = v.InferOutput<typeof EquipmentStatGroupSchema>;

/** 安全装备快照，公开、本人和运营视图共同使用。 */
export const EquipmentViewSchema = v.pipe(
  strictStruct({
    name: EQUIPMENT_NAME,
    alt_name: requiredNullable(EQUIPMENT_NAME),
    category: CATEGORY_OR_STAT_NAME,
    level: EquipmentLevelSchema,
    tradeable: v.boolean(),
    stat_groups: v.array(EquipmentStatGroupSchema),
    source: v.literal("hvdata-equip-v1"),
    observed_at: ConsignmentTimestampSchema,
  }),
  v.check((equipment) => equipment.level.kind !== "soulbound" || !equipment.tradeable),
);
/** 供浏览器展示的装备白名单。 */
export type EquipmentView = v.InferOutput<typeof EquipmentViewSchema>;

/** 公开条目白名单。装备编号是字符串，卖家用游戏显示名标识。 */
export const ListingSummarySchema = v.pipe(
  strictStruct({
    listing_id: ConsignmentUuidSchema,
    version: ListingVersionSchema,
    mode: ListingModeSchema,
    state: PublicListingStateSchema,
    world_id: ListingWorldSchema,
    external_equipment_id: ExternalEquipmentIdSchema,
    name: EQUIPMENT_NAME,
    category: CATEGORY_OR_STAT_NAME,
    level: EquipmentLevelSchema,
    price: ListingPriceSchema,
    seller_name: v.string(),
    updated_at: ConsignmentTimestampSchema,
    expires_at: ConsignmentTimestampSchema,
    purchasable: v.literal(false),
  }),
  v.check(
    (summary) =>
      listingStateMatchesMode(summary.state, summary.mode) && summary.level.kind !== "soulbound",
  ),
);
/** 公开目录及详情共用的摘要。 */
export type ListingSummary = v.InferOutput<typeof ListingSummarySchema>;

/** 独立公开详情，只接收可交易装备的快照。 */
export const ListingDetailSchema = v.pipe(
  strictStruct({
    summary: ListingSummarySchema,
    equipment: EquipmentViewSchema,
  }),
  v.check(({ equipment }) => equipment.tradeable),
);
/** 公开摘要及安全详情。 */
export type ListingDetail = v.InferOutput<typeof ListingDetailSchema>;

/** 公开目录的分页响应数据。 */
export const ListingSummaryPageSchema = listingPageSchema(ListingSummarySchema);
/** 带准确总数的公开目录页。 */
export type ListingSummaryPage = v.InferOutput<typeof ListingSummaryPageSchema>;

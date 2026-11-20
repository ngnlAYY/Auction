// 本人挂单的持久态、有效态、解析进度和带判别键的视图。

import * as v from "valibot";

import { requiredNullable, strictStruct, unsignedInteger } from "../../../wire";
import { EquipmentViewSchema, listingPageSchema } from "./catalog";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  ExternalEquipmentIdSchema,
  ListingFeeBpsSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "./scalar";
import {
  ActiveListingStateSchema,
  ClosedReasonSchema,
  EXPIRY_SOURCE_STATE,
  ExpiredReasonSchema,
  ForcedClosureReasonSchema,
  IneligibleReasonSchema,
  ListingActionSchema,
  ListingModeSchema,
  ListingReasonSchema,
  type ListingState,
  ListingStateSchema,
  ListingWorldSchema,
  LookupFailedReasonSchema,
  LookupReasonSchema,
  listingStateMatchesMode,
} from "./states";

const MAX_LOOKUP_ATTEMPTS = 6;

/** 解析进度只携带安全分类，排除工作租约、令牌与实例标识。 */
export const LookupProgressSchema = strictStruct({
  attempts: v.pipe(unsignedInteger, v.maxValue(MAX_LOOKUP_ATTEMPTS)),
  max_attempts: v.literal(MAX_LOOKUP_ATTEMPTS),
  next_attempt_at: requiredNullable(ConsignmentTimestampSchema),
  last_reason: requiredNullable(LookupReasonSchema),
  paused: v.boolean(),
  deadline_at: ConsignmentTimestampSchema,
});
/** 本人可见的解析进度。 */
export type LookupProgress = v.InferOutput<typeof LookupProgressSchema>;

const TRADEABLE_EQUIPMENT = v.pipe(
  EquipmentViewSchema,
  v.check(({ tradeable }) => tradeable),
);

/** 每种有效态只接受该变体的字段与原因，安全快照可以显式为空。 */
export const ListingViewSchema = v.union([
  strictStruct({ kind: v.literal("resolving"), progress: LookupProgressSchema }),
  strictStruct({
    kind: v.literal("lookup_failed"),
    reason: LookupFailedReasonSchema,
    last_lookup_reason: requiredNullable(LookupReasonSchema),
  }),
  v.pipe(
    strictStruct({
      kind: v.literal("ineligible"),
      equipment: EquipmentViewSchema,
      reason: IneligibleReasonSchema,
    }),
    v.check(({ equipment, reason }) => equipment.tradeable === (reason !== "not_tradeable")),
  ),
  strictStruct({ kind: v.literal("displayed"), equipment: TRADEABLE_EQUIPMENT }),
  strictStruct({
    kind: v.literal("awaiting_deposit"),
    equipment: TRADEABLE_EQUIPMENT,
    accepts_mail: v.literal(false),
  }),
  strictStruct({
    kind: v.literal("closed"),
    reason: ClosedReasonSchema,
    equipment: requiredNullable(EquipmentViewSchema),
  }),
  strictStruct({
    kind: v.literal("expired"),
    reason: ExpiredReasonSchema,
    equipment: requiredNullable(EquipmentViewSchema),
  }),
]);
/** 本人或运营页根据 kind 读取对应视图。 */
export type ListingView = v.InferOutput<typeof ListingViewSchema>;

const VIEW_STATE = {
  resolving: "resolving",
  lookup_failed: "rejected",
  ineligible: "rejected",
  displayed: "displayed",
  awaiting_deposit: "awaiting_deposit",
  closed: "closed",
  expired: "expired",
} satisfies Record<ListingView["kind"], ListingState>;

const OWNED_LISTING_FIELDS = strictStruct({
  listing_id: ConsignmentUuidSchema,
  version: ListingVersionSchema,
  world_id: ListingWorldSchema,
  external_equipment_id: ExternalEquipmentIdSchema,
  mode: ListingModeSchema,
  price: ListingPriceSchema,
  pricing_version: v.literal(1),
  fee_bps: ListingFeeBpsSchema,
  estimated_fee: unsignedInteger,
  estimated_seller_amount: unsignedInteger,
  created_at: ConsignmentTimestampSchema,
  updated_at: ConsignmentTimestampSchema,
  expires_at: ConsignmentTimestampSchema,
  stored_state: ListingStateSchema,
  effective_state: ListingStateSchema,
  effective_reason: requiredNullable(ListingReasonSchema),
  allowed_actions: v.array(ListingActionSchema),
  view: ListingViewSchema,
});

function isConsistentProjection(listing: v.InferOutput<typeof OWNED_LISTING_FIELDS>): boolean {
  const { mode, stored_state, effective_state, effective_reason, allowed_actions, view } = listing;
  const reason = "reason" in view ? view.reason : null;
  const expirySource = view.kind === "expired" ? EXPIRY_SOURCE_STATE[view.reason] : null;
  if (
    effective_state !== VIEW_STATE[view.kind] ||
    effective_reason !== reason ||
    !listingStateMatchesMode(stored_state, mode) ||
    !listingStateMatchesMode(effective_state, mode) ||
    (expirySource !== null && !listingStateMatchesMode(expirySource, mode))
  ) {
    return false;
  }
  // 不变量 6：仅强制关闭和本态到期可以先于落库生效。
  if (
    stored_state !== effective_state &&
    !(
      v.is(ActiveListingStateSchema, stored_state) &&
      (v.is(ForcedClosureReasonSchema, reason) || stored_state === expirySource)
    )
  ) {
    return false;
  }
  if (!v.is(ActiveListingStateSchema, effective_state) && allowed_actions.length > 0) {
    return false;
  }
  if (view.kind === "closed" || view.kind === "expired") {
    // 已落库的关闭可能源于解析期或发布后，关闭原因无法区分两者。
    const previous = expirySource ?? stored_state;
    if (previous === "displayed" || previous === "awaiting_deposit") {
      return view.equipment !== null;
    }
    if (previous === "resolving") return view.equipment === null;
  }
  return true;
}

/** 本人列表和详情共用的安全投影，版本始终来自已提交事实。 */
export const OwnedListingSchema = v.pipe(
  OWNED_LISTING_FIELDS,
  v.check(isConsistentProjection),
  v.check(({ mode, price, fee_bps, estimated_fee, estimated_seller_amount }) => {
    // 不变量 6、10：乘积最多约 10^13，只核对线上冻结报价的取整和金额守恒。
    return (
      (mode !== "direct" || fee_bps === 0) &&
      estimated_fee === Math.floor((price * fee_bps) / 10_000) &&
      estimated_seller_amount === price - estimated_fee
    );
  }),
);
/** 单条本人挂单及服务端允许的动作。 */
export type OwnedListing = v.InferOutput<typeof OwnedListingSchema>;

/** 本人活动挂单与历史记录的分页响应。 */
export const OwnedListingPageSchema = listingPageSchema(OwnedListingSchema);
/** 带准确总数的本人挂单页。 */
export type OwnedListingPage = v.InferOutput<typeof OwnedListingPageSchema>;

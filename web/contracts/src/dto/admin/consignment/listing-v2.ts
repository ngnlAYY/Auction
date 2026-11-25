// Admin v2 挂单读模型：代管态、有效止售、position 摘要与 Admin 允许动作的线协议。

import * as v from "valibot";

import { requiredNullable, strictStruct } from "../../../wire";
import { listingPageSchema } from "../../public/consignment/catalog";
import { ListingGenerationSchema, OwnedListingV2Schema } from "../../public/consignment/listing-v2";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  ListingVersionSchema,
} from "../../public/consignment/scalar";
import { DELIST_REASON, ListingAuctionConflictSchema } from ".";

const MAX_AUCTION_CONFLICTS = 50;

/** 代管 position 的独立状态闭集。 */
export const PositionStateV2Schema = v.picklist([
  "awaiting_take",
  "in_custody",
  "committed",
  "in_transit",
  "reconcile",
  "handed_over",
  "season_ended",
  "lost",
]);
/** position 状态。 */
export type PositionStateV2 = v.InferOutput<typeof PositionStateV2Schema>;

/** 服务器允许 Admin 对挂单执行的动作。 */
export const AdminListingActionV2Schema = v.picklist(["delist", "release_hold"]);
/** Admin 挂单动作。 */
export type AdminListingActionV2 = v.InferOutput<typeof AdminListingActionV2Schema>;

/** 尚未解除的运营止售。 */
export const AdminHoldV2Schema = strictStruct({
  hold_id: ConsignmentUuidSchema,
  set_at: ConsignmentTimestampSchema,
});
/** 有效止售。 */
export type AdminHoldV2 = v.InferOutput<typeof AdminHoldV2Schema>;

/** 挂单的代管 position 摘要，不含原信键、装备 key 或证明。 */
export const AdminPositionV2Schema = strictStruct({
  position_id: ConsignmentUuidSchema,
  state: PositionStateV2Schema,
  version: ListingVersionSchema,
  generation: requiredNullable(ListingGenerationSchema),
});
/** position 摘要。 */
export type AdminPositionV2 = v.InferOutput<typeof AdminPositionV2Schema>;

const DELISTABLE_STATES = new Set([
  "resolving",
  "displayed",
  "awaiting_deposit",
  "deposit_accepted",
  "available",
  "held_off_market",
]);

const ADMIN_LISTING_V2_FIELDS = strictStruct({
  listing: OwnedListingV2Schema,
  seller_user_id: ConsignmentUuidSchema,
  seller_name: v.string(),
  admin_delist_reason: requiredNullable(DELIST_REASON),
  hold: requiredNullable(AdminHoldV2Schema),
  position: requiredNullable(AdminPositionV2Schema),
  admin_actions: v.array(AdminListingActionV2Schema),
});

function isConsistentAdminListing(record: v.InferOutput<typeof ADMIN_LISTING_V2_FIELDS>): boolean {
  const { listing, hold, position, admin_actions: actions } = record;
  // Admin 视图不带卖家动作；hold_id 只在 `hold` 展示（本人视图不带），止售只在在库下架态出现。
  if (listing.allowed_actions.length > 0) return false;
  if (hold !== null && listing.effective_state !== "held_off_market") return false;
  // 只有 position 处于 awaiting_take 的在库下架件可以没有处分代。
  if (
    listing.effective_state === "held_off_market" &&
    listing.generation === null &&
    position?.state !== "awaiting_take"
  ) {
    return false;
  }
  if (position !== null && position.generation !== listing.generation) return false;
  const delist = actions.includes("delist");
  const release = actions.includes("release_hold");
  if (actions.length !== Number(delist) + Number(release)) return false;
  if (delist && release) return false;
  if (delist && (hold !== null || !DELISTABLE_STATES.has(listing.effective_state))) return false;
  if (release && (hold === null || listing.effective_state !== "held_off_market")) return false;
  return true;
}

/** 运营可见的 v2 挂单投影；运营动作只在 `admin_actions`，卖家动作恒空。 */
export const AdminListingV2Schema = v.pipe(
  ADMIN_LISTING_V2_FIELDS,
  v.check(isConsistentAdminListing),
);
/** 运营 v2 列表和详情共用的单条记录。 */
export type AdminListingV2 = v.InferOutput<typeof AdminListingV2Schema>;

/** 运营 v2 查询分页响应。 */
export const AdminListingPageV2Schema = listingPageSchema(AdminListingV2Schema);
/** 带准确总数的运营 v2 挂单页。 */
export type AdminListingPageV2 = v.InferOutput<typeof AdminListingPageV2Schema>;

/** 详情接受至多 50 条最近碰撞。 */
export const AdminListingDetailV2Schema = v.pipe(
  strictStruct({
    record: AdminListingV2Schema,
    conflicts: v.pipe(v.array(ListingAuctionConflictSchema), v.maxLength(MAX_AUCTION_CONFLICTS)),
    conflicts_truncated: v.boolean(),
  }),
  v.check(
    ({ conflicts, conflicts_truncated }) =>
      !conflicts_truncated || conflicts.length === MAX_AUCTION_CONFLICTS,
  ),
);
/** Admin v2 挂单详情及安全碰撞投影。 */
export type AdminListingDetailV2 = v.InferOutput<typeof AdminListingDetailV2Schema>;

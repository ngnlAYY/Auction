// 寄售状态、原因与卖家动作的线协议闭集。

import * as v from "valibot";

import type { ListingMode } from "./listing-options";

export {
  type ListingMode,
  ListingModeSchema,
  type ListingWorld,
  ListingWorldSchema,
} from "./listing-options";

/** 公开目录只承载已经核验的挂单。 */
export const PublicListingStateSchema = v.picklist(["displayed", "awaiting_deposit"]);
/** 公开目录可见状态。 */
export type PublicListingState = v.InferOutput<typeof PublicListingStateSchema>;

/** 可由卖家继续管理的已知状态集合。 */
export const ActiveListingStateSchema = v.picklist([
  "resolving",
  ...PublicListingStateSchema.options,
]);

/** 本人与运营读取的完整状态。 */
export const ListingStateSchema = v.picklist([
  ...ActiveListingStateSchema.options,
  "rejected",
  "expired",
  "closed",
]);
/** 本人与运营读取的持久态或有效态。 */
export type ListingState = v.InferOutput<typeof ListingStateSchema>;

/** 读侧可在落库前观察到的强制关闭原因。 */
export const ForcedClosureReasonSchema = v.picklist([
  "auction_lot_created",
  "seller_ineligible",
  "service_disabled",
]);

/** 安全关闭原因。 */
export const ClosedReasonSchema = v.picklist([
  ...ForcedClosureReasonSchema.options,
  "seller_cancelled",
  "admin_delisted",
]);

/** 到期原因保留发生到期的活动状态。 */
export const EXPIRY_SOURCE_STATE = {
  lookup_deadline: "resolving",
  display_expired: "displayed",
  deposit_expired: "awaiting_deposit",
} satisfies Record<string, ListingState>;

/** 安全到期原因。 */
export const ExpiredReasonSchema = v.picklist([
  "lookup_deadline",
  "display_expired",
  "deposit_expired",
]);

/** 已取得安全快照后产生的拒挂原因。 */
export const IneligibleReasonSchema = v.picklist([
  "not_tradeable",
  "owner_mismatch",
  "history_owner_mismatch",
  "history_alias_conflict",
  "history_unproven",
  "equipment_busy",
]);

/** 未取得可信快照的终止原因。 */
export const LookupFailedReasonSchema = v.picklist([
  "owner_missing",
  "equipment_not_found",
  "source_request_rejected",
  "source_http_invalid",
  "source_schema_invalid",
  "source_shape_unconfirmed",
  "source_identity_mismatch",
  "source_too_large",
  "lookup_exhausted",
  "credential_unavailable",
]);

/** 挂单终态的安全原因集合。 */
export const ListingReasonSchema = v.picklist([
  ...ClosedReasonSchema.options,
  ...ExpiredReasonSchema.options,
  ...IneligibleReasonSchema.options,
  ...LookupFailedReasonSchema.options,
]);
/** 可用显式译文映射展示的终态原因。 */
export type ListingReason = v.InferOutput<typeof ListingReasonSchema>;

/** 解析暂时失败的安全分类。 */
export const LookupReasonSchema = v.picklist([
  "source_unavailable",
  "source_timeout",
  "source_rate_limited",
]);
/** 读取进度与耗尽结果的暂时失败分类。 */
export type LookupReason = v.InferOutput<typeof LookupReasonSchema>;

/** 服务器允许本人执行的挂单动作。 */
export const ListingActionSchema = v.picklist(["reprice", "withdraw"]);
/** 本人挂单的允许动作。 */
export type ListingAction = v.InferOutput<typeof ListingActionSchema>;

/** 解码时检查已发布状态与不可变模式是否相容。 */
export function listingStateMatchesMode(state: ListingState, mode: ListingMode): boolean {
  if (state === "displayed") return mode === "direct";
  if (state === "awaiting_deposit") return mode === "consigned";
  return true;
}

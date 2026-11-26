// v2 公开与本人挂单读模型，覆盖第 4 期新增状态、处分代、原因和允许动作。

import * as v from "valibot";

import { integer, JSON_MAX_SAFE_INTEGER, requiredNullable, strictStruct } from "../../../wire";
import { EquipmentLevelSchema, EquipmentViewSchema, listingPageSchema } from "./catalog";
import { CommandUuidV4Schema } from "./command";
import { LookupProgressSchema } from "./owned";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ExternalEquipmentIdSchema,
  ListingFeeBpsSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "./scalar";
import { ListingModeSchema, ListingWorldSchema, LookupReasonSchema } from "./states";

/** 挂单持久态与有效态使用的完整 v2 闭集。 */
export const ListingStateV2Schema = v.picklist([
  "resolving",
  "displayed",
  "awaiting_deposit",
  "deposit_accepted",
  "available",
  "sale_pending",
  "held_off_market",
  "withdrawal_pending",
  "rejected",
  "expired",
  "closed",
]);
/** v2 挂单状态。 */
export type ListingStateV2 = v.InferOutput<typeof ListingStateV2Schema>;

/** 公开目录可表达的 v2 状态；私有状态在此类型中不可表达。 */
export const PublicListingStateV2Schema = v.picklist([
  "displayed",
  "awaiting_deposit",
  "available",
]);
/** 公开挂单状态。 */
export type PublicListingStateV2 = v.InferOutput<typeof PublicListingStateV2Schema>;

const LISTING_REASON_VALUES = [
  "auction_lot_created",
  "seller_ineligible",
  "service_disabled",
  "seller_cancelled",
  "admin_delisted",
  "lookup_deadline",
  "display_expired",
  "deposit_expired",
  "not_tradeable",
  "owner_missing",
  "owner_mismatch",
  "history_owner_mismatch",
  "history_alias_conflict",
  "history_unproven",
  "equipment_busy",
  "equipment_not_found",
  "source_request_rejected",
  "source_http_invalid",
  "source_schema_invalid",
  "source_shape_unconfirmed",
  "source_identity_mismatch",
  "source_too_large",
  "lookup_exhausted",
  "credential_unavailable",
  "unpaid_recovered",
  "paid_returned",
  "quota_blocked",
  "operations_hold",
  "review",
  "sold",
  "withdrawn_delivered",
  "season_closed",
  "loss_confirmed",
] as const;

/** v2 挂单终态、在库下架与既有解析/拒挂原因。 */
export const ListingReasonV2Schema = v.picklist(LISTING_REASON_VALUES);
/** v2 挂单原因。 */
export type ListingReasonV2 = v.InferOutput<typeof ListingReasonV2Schema>;

const HOLD_REASONS = new Set<string>([
  "unpaid_recovered",
  "paid_returned",
  "seller_ineligible",
  "service_disabled",
  "quota_blocked",
  "operations_hold",
  "review",
]);
const CLOSED_REASONS = new Set<string>([
  "auction_lot_created",
  "seller_ineligible",
  "service_disabled",
  "seller_cancelled",
  "admin_delisted",
  "sold",
  "withdrawn_delivered",
  "season_closed",
  "loss_confirmed",
]);
const EXPIRY_REASONS = new Set<string>(["lookup_deadline", "display_expired", "deposit_expired"]);
const LOOKUP_FAILURE_REASONS = new Set<string>([
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
const INELIGIBLE_REASONS = new Set<string>([
  "not_tradeable",
  "owner_mismatch",
  "history_owner_mismatch",
  "history_alias_conflict",
  "history_unproven",
  "equipment_busy",
]);

/** 服务器允许本人对挂单执行的 v2 动作。 */
export const ListingActionV2Schema = v.picklist([
  "reprice",
  "withdraw",
  "relist",
  "issue_reservation",
  "purchase",
  "admin_delist",
  "release_hold",
]);
/** v2 挂单动作。 */
export type ListingActionV2 = v.InferOutput<typeof ListingActionV2Schema>;

/** 处分代与解析代共用的正安全整数区间。 */
export const ListingGenerationSchema = v.pipe(
  integer,
  v.minValue(1),
  v.maxValue(JSON_MAX_SAFE_INTEGER),
);
/** 挂单处分代。 */
export type ListingGeneration = v.InferOutput<typeof ListingGenerationSchema>;

/** 回执中的逐件 L：挂单 ID、版本与状态。 */
export const ListingStampV2Schema = strictStruct({
  listing_id: ConsignmentUuidSchema,
  listing_version: ListingVersionSchema,
  state: ListingStateV2Schema,
});
/** 逐件 L。 */
export type ListingStampV2 = v.InferOutput<typeof ListingStampV2Schema>;

const TRADEABLE_EQUIPMENT = v.pipe(
  EquipmentViewSchema,
  v.check(({ tradeable }) => tradeable),
);

/** 本人或 Admin 可见的 v2 有效视图。 */
export const ListingViewV2Schema = v.union([
  strictStruct({ kind: v.literal("resolving"), progress: LookupProgressSchema }),
  v.pipe(
    strictStruct({
      kind: v.literal("lookup_failed"),
      reason: ListingReasonV2Schema,
      last_lookup_reason: requiredNullable(LookupReasonSchema),
    }),
    v.check(({ reason }) => LOOKUP_FAILURE_REASONS.has(reason)),
  ),
  v.pipe(
    strictStruct({
      kind: v.literal("ineligible"),
      equipment: EquipmentViewSchema,
      reason: ListingReasonV2Schema,
    }),
    v.check(
      ({ equipment, reason }) =>
        INELIGIBLE_REASONS.has(reason) && equipment.tradeable === (reason !== "not_tradeable"),
    ),
  ),
  strictStruct({ kind: v.literal("displayed"), equipment: TRADEABLE_EQUIPMENT }),
  strictStruct({ kind: v.literal("awaiting_deposit"), equipment: TRADEABLE_EQUIPMENT }),
  strictStruct({ kind: v.literal("deposit_accepted"), equipment: TRADEABLE_EQUIPMENT }),
  strictStruct({ kind: v.literal("available"), equipment: TRADEABLE_EQUIPMENT }),
  strictStruct({ kind: v.literal("sale_pending"), equipment: TRADEABLE_EQUIPMENT }),
  v.pipe(
    strictStruct({
      kind: v.literal("held_off_market"),
      equipment: TRADEABLE_EQUIPMENT,
      reason: ListingReasonV2Schema,
      hold_id: requiredNullable(ConsignmentUuidSchema),
    }),
    v.check(({ reason }) => HOLD_REASONS.has(reason)),
  ),
  strictStruct({ kind: v.literal("withdrawal_pending"), equipment: TRADEABLE_EQUIPMENT }),
  v.pipe(
    strictStruct({
      kind: v.literal("closed"),
      reason: ListingReasonV2Schema,
      equipment: requiredNullable(EquipmentViewSchema),
    }),
    v.check(
      ({ reason, equipment }) =>
        CLOSED_REASONS.has(reason) && (equipment === null || equipment.tradeable),
    ),
  ),
  v.pipe(
    strictStruct({
      kind: v.literal("expired"),
      reason: ListingReasonV2Schema,
      equipment: requiredNullable(EquipmentViewSchema),
    }),
    v.check(
      ({ reason, equipment }) =>
        EXPIRY_REASONS.has(reason) && (equipment === null || equipment.tradeable),
    ),
  ),
]);
/** 本人或运营页根据 kind 读取对应视图。 */
export type ListingViewV2 = v.InferOutput<typeof ListingViewV2Schema>;

const VIEW_STATE_V2 = {
  resolving: "resolving",
  lookup_failed: "rejected",
  ineligible: "rejected",
  displayed: "displayed",
  awaiting_deposit: "awaiting_deposit",
  deposit_accepted: "deposit_accepted",
  available: "available",
  sale_pending: "sale_pending",
  held_off_market: "held_off_market",
  withdrawal_pending: "withdrawal_pending",
  closed: "closed",
  expired: "expired",
} satisfies Record<ListingViewV2["kind"], ListingStateV2>;

function viewReason(view: ListingViewV2): ListingReasonV2 | null {
  return "reason" in view ? view.reason : null;
}

function expirySourceState(reason: ListingReasonV2 | null): ListingStateV2 | null {
  if (reason === "lookup_deadline") return "resolving";
  if (reason === "display_expired") return "displayed";
  if (reason === "deposit_expired") return "awaiting_deposit";
  return null;
}

function stateMatchesMode(state: ListingStateV2, mode: v.InferOutput<typeof ListingModeSchema>) {
  if (state === "displayed") return mode === "direct";
  if (
    state === "awaiting_deposit" ||
    state === "deposit_accepted" ||
    state === "available" ||
    state === "sale_pending" ||
    state === "held_off_market" ||
    state === "withdrawal_pending"
  ) {
    return mode === "consigned";
  }
  return true;
}

function isLiveState(state: ListingStateV2): boolean {
  return state !== "rejected" && state !== "expired" && state !== "closed";
}

function isForcedClosure(reason: ListingReasonV2): boolean {
  return (
    reason === "auction_lot_created" ||
    reason === "seller_ineligible" ||
    reason === "service_disabled"
  );
}

function actionAllowedIn(
  action: ListingActionV2,
  state: ListingStateV2,
  reason: ListingReasonV2 | null,
): boolean {
  if (action === "purchase" || action === "admin_delist" || action === "release_hold") {
    return false;
  }
  // 决定 24：运营止售的件只能撤回，不能手动或自动重新上架。
  if (action === "relist") {
    return state === "held_off_market" && reason !== null && reason !== "operations_hold";
  }
  if (action === "issue_reservation") return state === "awaiting_deposit";
  if (action === "reprice") {
    return (
      state === "resolving" ||
      state === "displayed" ||
      state === "awaiting_deposit" ||
      state === "available"
    );
  }
  return (
    state === "resolving" ||
    state === "displayed" ||
    state === "awaiting_deposit" ||
    state === "deposit_accepted" ||
    state === "available" ||
    state === "held_off_market"
  );
}

function generationMatches(
  state: ListingStateV2,
  generation: ListingGeneration | null,
  reason: ListingReasonV2 | null,
): boolean {
  if (state === "resolving" || state === "displayed" || state === "awaiting_deposit") {
    return generation === null;
  }
  if (state === "available" || state === "sale_pending" || state === "withdrawal_pending") {
    return generation !== null;
  }
  // 本人视图不带 position 状态：本期只有对已接受、尚未取得的件设运营止售才会在没有处分代时
  // 进入 held_off_market，其余原因必须有代；Admin 视图再按 position 核对。
  if (state === "held_off_market") return generation !== null || reason === "operations_hold";
  return true;
}

function hasUniqueItems<T>(items: readonly T[]): boolean {
  return new Set(items).size === items.length;
}

const LISTING_SUMMARY_V2_FIELDS = strictStruct({
  listing_id: ConsignmentUuidSchema,
  version: ListingVersionSchema,
  mode: ListingModeSchema,
  state: PublicListingStateV2Schema,
  world_id: ListingWorldSchema,
  external_equipment_id: ExternalEquipmentIdSchema,
  name: consignmentTextSchema(512),
  category: consignmentTextSchema(128),
  level: EquipmentLevelSchema,
  price: ListingPriceSchema,
  seller_name: v.string(),
  seller_verified: v.boolean(),
  updated_at: ConsignmentTimestampSchema,
  expires_at: ConsignmentTimestampSchema,
  generation: requiredNullable(ListingGenerationSchema),
  effective_reason: requiredNullable(ListingReasonV2Schema),
  allowed_actions: v.array(ListingActionV2Schema),
  purchasable: v.boolean(),
});

/** v2 公开目录与详情共用的安全摘要。 */
export const ListingSummaryV2Schema = v.pipe(
  LISTING_SUMMARY_V2_FIELDS,
  v.check((summary) => {
    if (!stateMatchesMode(summary.state, summary.mode)) return false;
    if (summary.effective_reason !== null) return false;
    if (summary.level.kind === "soulbound") return false;
    if (
      (summary.state === "displayed" || summary.state === "awaiting_deposit") &&
      summary.generation !== null
    ) {
      return false;
    }
    if (summary.state === "available" && summary.generation === null) return false;
    if (!hasUniqueItems(summary.allowed_actions)) return false;
    if (summary.allowed_actions.some((action) => action !== "purchase")) return false;
    const canPurchase = summary.allowed_actions.length === 1;
    if (summary.purchasable !== canPurchase) return false;
    return !summary.purchasable || summary.state === "available";
  }),
);
/** 公开摘要。 */
export type ListingSummaryV2 = v.InferOutput<typeof ListingSummaryV2Schema>;

/** v2 公开详情：摘要与同一行的安全装备快照。 */
export const ListingDetailV2Schema = v.pipe(
  strictStruct({
    summary: ListingSummaryV2Schema,
    equipment: EquipmentViewSchema,
  }),
  v.check(({ summary, equipment }) => {
    const { level } = summary;
    const snapshotLevel = equipment.level;
    return (
      equipment.tradeable &&
      summary.name === equipment.name &&
      summary.category === equipment.category &&
      level.kind === snapshotLevel.kind &&
      (level.kind !== "assigned" ||
        (snapshotLevel.kind === "assigned" && level.value === snapshotLevel.value))
    );
  }),
);
/** 公开详情。 */
export type ListingDetailV2 = v.InferOutput<typeof ListingDetailV2Schema>;

const OWNED_LISTING_V2_FIELDS = strictStruct({
  listing_id: ConsignmentUuidSchema,
  version: ListingVersionSchema,
  world_id: ListingWorldSchema,
  external_equipment_id: ExternalEquipmentIdSchema,
  mode: ListingModeSchema,
  price: ListingPriceSchema,
  pricing_version: v.literal(1),
  fee_bps: ListingFeeBpsSchema,
  estimated_fee: integer,
  estimated_seller_amount: integer,
  created_at: ConsignmentTimestampSchema,
  updated_at: ConsignmentTimestampSchema,
  expires_at: ConsignmentTimestampSchema,
  generation: requiredNullable(ListingGenerationSchema),
  position_id: requiredNullable(ConsignmentUuidSchema),
  loss_case_id: requiredNullable(ConsignmentUuidSchema),
  stored_state: ListingStateV2Schema,
  effective_state: ListingStateV2Schema,
  effective_reason: requiredNullable(ListingReasonV2Schema),
  allowed_actions: v.array(ListingActionV2Schema),
  view: ListingViewV2Schema,
});

function snapshotMatches(
  view: ListingViewV2,
  storedState: ListingStateV2,
  state: ListingStateV2,
  expirySource: ListingStateV2 | null,
): boolean {
  if (view.kind !== "closed" && view.kind !== "expired") return true;
  if (view.equipment !== null && !view.equipment.tradeable) return false;
  const previous = view.kind === "expired" ? (expirySource ?? storedState) : storedState;
  if (
    (previous === "displayed" ||
      previous === "awaiting_deposit" ||
      previous === "deposit_accepted" ||
      previous === "available" ||
      previous === "sale_pending" ||
      previous === "held_off_market" ||
      previous === "withdrawal_pending") &&
    view.equipment === null
  ) {
    return false;
  }
  if (previous === "resolving" && view.equipment !== null) return false;
  const terminal = CLOSED_REASONS.has(view.reason)
    ? "closed"
    : EXPIRY_REASONS.has(view.reason)
      ? "expired"
      : INELIGIBLE_REASONS.has(view.reason) || LOOKUP_FAILURE_REASONS.has(view.reason)
        ? "rejected"
        : "held_off_market";
  return terminal === state;
}

function isConsistentOwnedListing(listing: v.InferOutput<typeof OWNED_LISTING_V2_FIELDS>): boolean {
  const state = VIEW_STATE_V2[listing.view.kind];
  const reason = viewReason(listing.view);
  const expirySource = expirySourceState(reason);
  if (listing.effective_state !== state || listing.effective_reason !== reason) return false;
  if (
    !stateMatchesMode(listing.stored_state, listing.mode) ||
    !stateMatchesMode(state, listing.mode) ||
    (expirySource !== null && !stateMatchesMode(expirySource, listing.mode))
  ) {
    return false;
  }
  // 不变量 6：有效态可以先看到强制终结或本态到期，但不能复活或跳过已提交的发布。
  if (listing.stored_state !== state) {
    const allowed =
      isLiveState(listing.stored_state) &&
      ((reason !== null && isForcedClosure(reason)) ||
        (state === "expired" && expirySource === listing.stored_state));
    if (!allowed) return false;
  }
  if (!isLiveState(state) && listing.allowed_actions.length > 0) return false;
  if (!hasUniqueItems(listing.allowed_actions)) return false;
  if (listing.allowed_actions.some((action) => !actionAllowedIn(action, state, reason))) {
    return false;
  }
  if (!generationMatches(state, listing.generation, reason)) return false;
  return snapshotMatches(listing.view, listing.stored_state, state, expirySource);
}

/** v2 本人列表与详情共用的完整投影。 */
export const OwnedListingV2Schema = v.pipe(
  OWNED_LISTING_V2_FIELDS,
  v.check(isConsistentOwnedListing),
  // 损失案件只按 position 建立：有案件必有 position。
  v.check(({ position_id, loss_case_id }) => loss_case_id === null || position_id !== null),
  v.check(({ mode, price, fee_bps, estimated_fee, estimated_seller_amount }) => {
    // 不变量 6、10：领域 quote 负责计算。这里只核对线上冻结报价的取整和金额守恒。
    return (
      (mode !== "direct" || (fee_bps === 0 && estimated_fee === 0)) &&
      estimated_fee === Math.floor((price * fee_bps) / 10_000) &&
      estimated_seller_amount === price - estimated_fee
    );
  }),
);
/** 本人挂单投影。 */
export type OwnedListingV2 = v.InferOutput<typeof OwnedListingV2Schema>;

/** 本人 v2 列表分页体。 */
export const OwnedListingPageV2Schema = listingPageSchema(OwnedListingV2Schema);
/** 带准确总数的本人 v2 挂单页。 */
export type OwnedListingPageV2 = v.InferOutput<typeof OwnedListingPageV2Schema>;

/** 本人 v2 撤回的三种合法回执：代管件登记意愿、已在库件直接建撤回代、未代管件按前三期关闭。 */
export const WithdrawReceiptV2Schema = v.union([
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("withdraw"),
    disposition: v.literal("withdrawal_requested"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    // 代管件登记撤回意愿：available 转入 withdrawal_pending，其余两态保留原态。
    state: v.picklist(["deposit_accepted", "withdrawal_pending", "held_off_market"]),
    intent_id: ConsignmentUuidSchema,
  }),
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("withdraw"),
    disposition: v.literal("withdrawal_accepted"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    // 已在库件直接绑定撤回合同（7C-2）：合同与装备已确定。
    state: v.literal("withdrawal_pending"),
    contract_id: ConsignmentUuidSchema,
    equipment_id: ConsignmentUuidSchema,
  }),
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("withdraw"),
    disposition: v.literal("closed"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    state: v.literal("closed"),
  }),
]);
/** 撤回回执。 */
export type WithdrawReceiptV2 = v.InferOutput<typeof WithdrawReceiptV2Schema>;

/** 本人 v2 撤回或取消请求。不变量 1、6：稳定 UUIDv4 与预期版本。 */
export interface WithdrawListingV2 {
  command_id: string;
  expected_version: number;
}

/** 撤回或取消请求的闭集线协议。 */
export const WithdrawListingV2Schema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
});

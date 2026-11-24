// 将寄售契约的安全取值映射到本站译文键。

import type {
  ListingDisposition,
  ListingMode,
  ListingReason,
  ListingState,
  ListingView,
  LookupReason,
  StoredCommandCode,
} from "@auction/contracts/dto/public/consignment";

const MODE_KEYS = {
  direct: "consignment-direct",
  consigned: "consignment-consigned",
} satisfies Record<ListingMode, string>;

const STATE_KEYS = {
  resolving: "consignment-resolving",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  // 持久或有效状态的总称复用通用「已拒绝」，具体分类由 view.kind 给出。
  rejected: "admin-audit-result-rejected",
  expired: "consignment-expired",
  closed: "consignment-closed",
} satisfies Record<ListingState, string>;

const VIEW_KEYS = {
  resolving: "consignment-resolving",
  lookup_failed: "consignment-lookup-failed",
  ineligible: "consignment-ineligible",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  expired: "consignment-expired",
  closed: "consignment-closed",
} satisfies Record<ListingView["kind"], string>;

const LISTING_REASON_KEYS = {
  not_tradeable: "consignment-not-tradeable",
  owner_missing: "consignment-owner-missing",
  owner_mismatch: "consignment-owner-mismatch",
  history_owner_mismatch: "consignment-history-owner-mismatch",
  history_alias_conflict: "consignment-history-alias-conflict",
  history_unproven: "consignment-history-unproven",
  equipment_busy: "consignment-equipment-busy",
  equipment_not_found: "consignment-equipment-not-found",
  source_request_rejected: "consignment-source-request-rejected",
  source_http_invalid: "consignment-source-http-invalid",
  source_schema_invalid: "consignment-source-schema-invalid",
  source_shape_unconfirmed: "consignment-source-shape-unconfirmed",
  source_identity_mismatch: "consignment-source-identity-mismatch",
  source_too_large: "consignment-source-too-large",
  lookup_exhausted: "consignment-lookup-exhausted",
  credential_unavailable: "consignment-credential-unavailable",
  auction_lot_created: "consignment-auction-lot-created",
  seller_ineligible: "consignment-seller-ineligible",
  service_disabled: "consignment-service-disabled",
  seller_cancelled: "consignment-seller-cancelled",
  admin_delisted: "consignment-admin-delisted",
  lookup_deadline: "consignment-lookup-deadline",
  display_expired: "consignment-display-expired",
  deposit_expired: "consignment-deposit-expired",
} satisfies Record<ListingReason, string>;

const LOOKUP_REASON_KEYS = {
  source_unavailable: "consignment-source-unavailable",
  source_timeout: "consignment-source-timeout",
  source_rate_limited: "consignment-source-rate-limited",
} satisfies Record<LookupReason, string>;

// Admin 本期只处理下架。其他成功处置没有对应文案。
const DISPOSITION_KEYS = {
  registered: null,
  repriced: null,
  unchanged: null,
  closed: "consignment-closed",
  rejected: "admin-audit-result-rejected",
} satisfies Record<ListingDisposition, string | null>;

// 契约闭集只含已落账结果。下架的即时冲突来自合同 §7.2；其余拒绝走 Admin 通用提示。
// 新增 HTTP 错误码时要同步这张表。
const WRITE_ERROR_KEYS = {
  ok: null,
  accepted: null,
  participant_ineligible: null,
  listing_not_found: null,
  already_open: null,
  equipment_busy: null,
  history_unproven: null,
  history_owner_mismatch: null,
  history_alias_conflict: null,
  fee_changed: null,
  stale_version: "consignment-stale-version",
  listing_not_active: "consignment-listing-not-active",
  world_unavailable: null,
  registration_rate_limited: null,
  listing_limit_reached: null,
  command_id_conflict: "consignment-command-conflict",
} satisfies Record<StoredCommandCode, string | null> & Record<string, string | null>;

function isMappedWriteCode(code: string): code is keyof typeof WRITE_ERROR_KEYS {
  return Object.hasOwn(WRITE_ERROR_KEYS, code);
}

/** 挂单模式的译文键。 */
export function listingModeKey(mode: ListingMode): string {
  return MODE_KEYS[mode];
}

/** 持久态或有效态的译文键。 */
export function listingStateKey(state: ListingState): string {
  return STATE_KEYS[state];
}

/** 挂单视图的译文键。 */
export function listingViewKey(kind: ListingView["kind"]): string {
  return VIEW_KEYS[kind];
}

/** 挂单的资格、解析终止、关闭与到期原因的译文键。 */
export function listingReasonKey(reason: ListingReason): string {
  return LISTING_REASON_KEYS[reason];
}

/** 解析进度或解析失败视图中最近一次暂时失败的译文键。 */
export function listingLookupReasonKey(reason: LookupReason): string {
  return LOOKUP_REASON_KEYS[reason];
}

/** 回执处置有独立文案时返回译文键。 */
export function listingDispositionKey(disposition: ListingDisposition): string | null {
  return DISPOSITION_KEYS[disposition];
}

/** 已知写入错误的专用译文键；返回 null 时沿用调用方的通用提示。 */
export function listingWriteErrorKey(code: string): string | null {
  return isMappedWriteCode(code) ? WRITE_ERROR_KEYS[code] : null;
}

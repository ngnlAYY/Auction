// 将寄售契约的安全取值映射到本站译文键。

import type {
  ListingDisposition,
  ListingMode,
  LookupReason,
  StoredCommandCode,
} from "@auction/contracts/dto/public/consignment";
import type {
  ListingReasonV2,
  ListingStateV2,
  ListingViewV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";

const MODE_KEYS = {
  direct: "consignment-direct",
  consigned: "consignment-consigned",
} satisfies Record<ListingMode, string>;

const STATE_KEYS = {
  resolving: "consignment-resolving",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  deposit_accepted: "consignment-deposit-accepted",
  available: "consignment-available",
  sale_pending: "consignment-sale-pending",
  held_off_market: "consignment-held-off-market",
  withdrawal_pending: "consignment-withdrawal-pending",
  // 合同只为两种拒绝视图提供文案，聚合的 rejected 态由调用方按 view.kind 显示。
  rejected: null,
  expired: "consignment-expired",
  closed: "consignment-closed",
} satisfies Record<ListingStateV2, string | null>;

const VIEW_KEYS = {
  resolving: "consignment-resolving",
  lookup_failed: "consignment-lookup-failed",
  ineligible: "consignment-ineligible",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  deposit_accepted: "consignment-deposit-accepted",
  available: "consignment-available",
  sale_pending: "consignment-sale-pending",
  held_off_market: "consignment-held-off-market",
  withdrawal_pending: "consignment-withdrawal-pending",
  expired: "consignment-expired",
  closed: "consignment-closed",
} satisfies Record<ListingViewV2["kind"], string>;

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
  unpaid_recovered: "consignment-unpaid-recovered",
  paid_returned: "consignment-paid-returned",
  quota_blocked: "consignment-quota-blocked",
  operations_hold: "consignment-operations-hold",
  review: "consignment-review",
  sold: "consignment-sold",
  withdrawn_delivered: "consignment-withdrawn-delivered",
  season_closed: "consignment-season-closed",
  loss_confirmed: "consignment-loss-confirmed",
} satisfies Record<ListingReasonV2, string>;

const LOOKUP_REASON_KEYS = {
  source_unavailable: "consignment-source-unavailable",
  source_timeout: "consignment-source-timeout",
  source_rate_limited: "consignment-source-rate-limited",
} satisfies Record<LookupReason, string>;

// 合同未定义登记、改价、未变化的独立回执文案，写成功后重读。拒绝按错误码提示。
const DISPOSITION_KEYS = {
  registered: null,
  repriced: null,
  unchanged: null,
  closed: "consignment-closed",
  rejected: null,
} satisfies Record<ListingDisposition, string | null>;

// 契约闭集只含已落账结果。即时 HTTP 拒绝按合同 §7.2 与 §8.2 逐项列出。
// 新增 HTTP 错误码时要同步这张表。
const WRITE_ERROR_KEYS = {
  ok: null,
  accepted: null,
  participant_ineligible: "consignment-seller-ineligible",
  listing_not_found: "consignment-not-found",
  already_open: "consignment-already-open",
  equipment_busy: "consignment-equipment-busy",
  history_unproven: "consignment-history-unproven",
  history_owner_mismatch: "consignment-history-owner-mismatch",
  history_alias_conflict: "consignment-history-alias-conflict",
  fee_changed: "consignment-fee-changed",
  stale_version: "consignment-stale-version",
  listing_not_active: "consignment-listing-not-active",
  world_unavailable: "consignment-world-unavailable",
  registration_rate_limited: "consignment-registration-rate-limited",
  listing_limit_reached: "consignment-listing-limit-reached",
  command_id_conflict: "consignment-command-conflict",
  invalid_price: "consignment-invalid-price",
  invalid_equipment_url: "consignment-invalid-equipment-url",
  consignment_disabled: "consignment-disabled",
  consignment_unavailable: "consignment-unavailable",
} satisfies Record<StoredCommandCode, string | null> & Record<string, string | null>;

function isMappedWriteCode(code: string): code is keyof typeof WRITE_ERROR_KEYS {
  return Object.hasOwn(WRITE_ERROR_KEYS, code);
}

/** 挂单模式的译文键。 */
export function listingModeKey(mode: ListingMode): string {
  return MODE_KEYS[mode];
}

/** 状态的译文键；rejected 需使用服务端的视图种类取文案。 */
export function listingStateKey(state: ListingStateV2): string | null {
  return STATE_KEYS[state];
}

/** 挂单视图的译文键。 */
export function listingViewKey(kind: ListingViewV2["kind"]): string {
  return VIEW_KEYS[kind];
}

/** 挂单的资格、解析终止、关闭与到期原因的译文键。 */
export function listingReasonKey(reason: ListingReasonV2): string {
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

/** 本人重新上架页的站内地址；入口只在服务端 allowed_actions 含 relist 时出现。 */
export function relistConsignmentHref(listingId: string): string {
  return `/consignment/me/${listingId}/relist`;
}

/** 本人损失回执页（只读）；只在挂单带损失案件时由本人详情链接。 */
export function lossReceiptHref(positionId: string): string {
  return `/consignment/loss/${positionId}`;
}

// D9 §6.1 重列的拒绝码；未列出的码沿用通用写入提示。
const RELIST_ERROR_KEYS: Readonly<Record<string, string>> = {
  action_not_allowed: "consignment-relist-not-allowed",
  operations_hold: "consignment-relist-hold",
  equipment_committed: "consignment-relist-custody",
  custody_unproven: "consignment-relist-custody",
  season_closed: "consignment-deposit-season-closed",
};

/** 重列专用错误码的译文键；返回 null 时沿用 listingWriteErrorKey 与通用提示。 */
export function relistErrorKey(code: string): string | null {
  return Object.hasOwn(RELIST_ERROR_KEYS, code)
    ? (RELIST_ERROR_KEYS[code] ?? null)
    : listingWriteErrorKey(code);
}

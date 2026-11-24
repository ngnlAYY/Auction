// 验证寄售闭集的译文映射、五语资源和 HTTP 错误回退。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import type {
  ListingDisposition,
  ListingMode,
  ListingReason,
  ListingState,
  ListingView,
  LookupReason,
  StoredCommandCode,
} from "@auction/contracts/dto/public/consignment";
import {
  ListingModeSchema,
  ListingReasonSchema,
  ListingStateSchema,
  LookupReasonSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { type LocaleCode, SUPPORTED_LOCALES } from "@auction/web-shared/ui/i18n/catalog";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";

import {
  listingDispositionKey,
  listingLookupReasonKey,
  listingModeKey,
  listingReasonKey,
  listingStateKey,
  listingViewKey,
  listingWriteErrorKey,
} from "./labels";

const I18N_ROOT = new URL("../../../../../crates/auction-web-admin/i18n/", import.meta.url);

// 键名逐项取自合同 §8.2，写拒绝与回执另外核对 §7 与 §9。
const MODE_KEYS = {
  direct: "consignment-direct",
  consigned: "consignment-consigned",
} satisfies Record<ListingMode, string>;

const STATE_KEYS = {
  resolving: "consignment-resolving",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  rejected: "admin-audit-result-rejected",
  expired: "consignment-expired",
  closed: "consignment-closed",
} satisfies Record<ListingState, string>;

const REASON_KEYS = {
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

const RECEIPT_KEYS = {
  registered: null,
  repriced: null,
  unchanged: null,
  closed: "consignment-closed",
  rejected: "admin-audit-result-rejected",
} satisfies Record<ListingDisposition, string | null>;

const VIEW_KEYS = {
  resolving: "consignment-resolving",
  lookup_failed: "consignment-lookup-failed",
  ineligible: "consignment-ineligible",
  displayed: "consignment-displayed",
  awaiting_deposit: "consignment-awaiting-deposit",
  closed: "consignment-closed",
  expired: "consignment-expired",
} satisfies Record<ListingView["kind"], string>;

const VIEW_KINDS = keysOf(VIEW_KEYS);
const WRITE_CODES = Object.keys(WRITE_ERROR_KEYS);
const DISPOSITIONS = keysOf(RECEIPT_KEYS);

function keysOf<Key extends string>(record: Record<Key, unknown>): Key[] {
  return Object.keys(record).filter((key): key is Key => Object.hasOwn(record, key));
}

test("两种模式与七种视图分别显示其对应标签", () => {
  for (const mode of ListingModeSchema.options) {
    expect(listingModeKey(mode), mode).toBe(MODE_KEYS[mode]);
  }
  for (const kind of VIEW_KINDS) {
    expect(listingViewKey(kind), kind).toBe(VIEW_KEYS[kind]);
  }
});

test("状态与拒绝视图的文案分别处理", () => {
  for (const state of ListingStateSchema.options) {
    expect(listingStateKey(state), state).toBe(STATE_KEYS[state]);
  }
});

test("资格、终止、到期与最近一次解析失败各有区分文案", () => {
  for (const reason of ListingReasonSchema.options) {
    expect(listingReasonKey(reason), reason).toBe(REASON_KEYS[reason]);
  }
  for (const reason of LookupReasonSchema.options) {
    expect(listingLookupReasonKey(reason), reason).toBe(LOOKUP_REASON_KEYS[reason]);
  }
});

test("写回执只使用符合处置语义的文案", () => {
  for (const disposition of DISPOSITIONS) {
    expect(listingDispositionKey(disposition), disposition).toBe(RECEIPT_KEYS[disposition]);
  }
});

test("已知写入拒绝显示专用译文或退回通用提示", () => {
  for (const [code, expectedKey] of Object.entries(WRITE_ERROR_KEYS)) {
    expect(listingWriteErrorKey(code), code).toBe(expectedKey);
  }
  expect(listingWriteErrorKey("invalid_reason")).toBeNull();
  expect(listingWriteErrorKey("outcome_unknown")).toBeNull();
});

test("未知 HTTP 错误与继承属性退回通用提示", () => {
  for (const code of [
    "future_error",
    "stale-version",
    "",
    "__proto__",
    "constructor",
    "toString",
  ]) {
    expect(listingWriteErrorKey(code), code).toBeNull();
  }
});

function loadBundle(code: LocaleCode, root = I18N_ROOT) {
  const source = readFileSync(new URL(`${code}.ftl`, root), "utf8");
  // 当前文件独立加载，避免默认语言回退掩盖缺键。
  const bundle = buildBundle(code, source, code, null);
  if (!bundle.ok) throw new Error(`${code} 的 FTL 无法加载`);
  return bundle.value;
}

test("映射输出在本站五种 FTL 中各自存在", () => {
  const keys = new Set([
    ...ListingModeSchema.options.map(listingModeKey),
    ...ListingStateSchema.options.map(listingStateKey),
    ...VIEW_KINDS.map(listingViewKey),
    ...ListingReasonSchema.options.map(listingReasonKey),
    ...LookupReasonSchema.options.map(listingLookupReasonKey),
    ...DISPOSITIONS.map(listingDispositionKey),
    ...WRITE_CODES.map(listingWriteErrorKey),
  ]);
  for (const { code } of SUPPORTED_LOCALES) {
    const bundle = loadBundle(code);
    for (const key of keys) {
      if (key !== null) {
        expect(bundle.getMessage(key)?.value, `${code} 缺少 ${key}`).toBeDefined();
      }
    }
  }
});

// 合同 §8.2 与 §9 要求两站共用的键。按后缀列出，保留页面接线前的未引用键诊断。
const SHARED_KEY_SUFFIXES = [
  "mode",
  "direct",
  "consigned",
  "world",
  "eid",
  "price",
  "seller",
  "expires",
  "attempts",
  "next-attempt",
  "paused",
  "resolving",
  "lookup-failed",
  "ineligible",
  "displayed",
  "awaiting-deposit",
  "expired",
  "closed",
  "not-tradeable",
  "owner-missing",
  "owner-mismatch",
  "history-owner-mismatch",
  "history-alias-conflict",
  "history-unproven",
  "equipment-busy",
  "equipment-not-found",
  "source-request-rejected",
  "source-http-invalid",
  "source-schema-invalid",
  "source-shape-unconfirmed",
  "source-identity-mismatch",
  "source-too-large",
  "source-unavailable",
  "source-timeout",
  "source-rate-limited",
  "lookup-exhausted",
  "credential-unavailable",
  "auction-lot-created",
  "seller-ineligible",
  "service-disabled",
  "seller-cancelled",
  "admin-delisted",
  "lookup-deadline",
  "display-expired",
  "deposit-expired",
  "stale-version",
  "listing-not-active",
  "command-conflict",
];

test("两站共有的挂单键使用相同的简体中文文案", () => {
  const adminBundle = loadBundle("zh-CN");
  const publicBundle = loadBundle(
    "zh-CN",
    new URL("../../../../../crates/auction-web-public/i18n/", import.meta.url),
  );
  const messages = SHARED_KEY_SUFFIXES.map((suffix) => {
    const key = `consignment-${suffix}`;
    const adminValue = adminBundle.getMessage(key)?.value;
    const publicValue = publicBundle.getMessage(key)?.value;
    expect(adminValue, `Admin 缺少 ${key}`).toBeDefined();
    expect(publicValue, `Public 缺少 ${key}`).toBeDefined();
    return { key, adminValue, publicValue };
  });
  for (const { key, adminValue, publicValue } of messages) {
    expect(adminValue, key).toEqual(publicValue);
  }
});

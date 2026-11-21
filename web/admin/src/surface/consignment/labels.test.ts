// 验证寄售闭集的译文映射、五语资源和 HTTP 错误回退。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import type {
  ListingDisposition,
  ListingView,
  StoredCommandCode,
} from "@auction/contracts/dto/public/consignment";
import {
  ListingModeSchema,
  ListingReasonSchema,
  ListingStateSchema,
  LookupReasonSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { SUPPORTED_LOCALES } from "@auction/web-shared/ui/i18n/catalog";
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

const STORED_ERROR_HAS_LABEL = {
  ok: false,
  accepted: false,
  participant_ineligible: false,
  listing_not_found: false,
  already_open: false,
  equipment_busy: false,
  history_unproven: false,
  history_owner_mismatch: false,
  history_alias_conflict: false,
  fee_changed: false,
  stale_version: true,
  listing_not_active: true,
  world_unavailable: false,
  registration_rate_limited: false,
  listing_limit_reached: false,
} satisfies Record<StoredCommandCode, boolean>;

const LIVE_ERROR_KEYS = {
  command_id_conflict: "consignment-command-conflict",
};

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
const WRITE_CODES = [...Object.keys(STORED_ERROR_HAS_LABEL), ...Object.keys(LIVE_ERROR_KEYS)];
const DISPOSITIONS = keysOf(RECEIPT_KEYS);

function keysOf<Key extends string>(record: Record<Key, unknown>): Key[] {
  return Object.keys(record).filter((key): key is Key => Object.hasOwn(record, key));
}

test("两种模式与七种视图分别显示其对应标签", () => {
  expect(ListingModeSchema.options.map(listingModeKey)).toEqual([
    "consignment-direct",
    "consignment-consigned",
  ]);
  for (const kind of VIEW_KINDS) {
    expect(listingViewKey(kind), kind).toBe(VIEW_KEYS[kind]);
  }
});

test("状态与拒绝视图的文案分别处理", () => {
  expect(ListingStateSchema.options.map(listingStateKey)).toEqual([
    "consignment-resolving",
    "consignment-displayed",
    "consignment-awaiting-deposit",
    "admin-audit-result-rejected",
    "consignment-expired",
    "consignment-closed",
  ]);
});

test("资格、终止、到期与最近一次解析失败各有区分文案", () => {
  const reasons = [
    ...ListingReasonSchema.options.map(listingReasonKey),
    ...LookupReasonSchema.options.map(listingLookupReasonKey),
  ];
  expect(new Set(reasons).size).toBe(reasons.length);
  expect(listingReasonKey("owner_missing")).toBe("consignment-owner-missing");
  expect(listingReasonKey("history_owner_mismatch")).toBe("consignment-history-owner-mismatch");
  expect(listingReasonKey("source_shape_unconfirmed")).toBe("consignment-source-shape-unconfirmed");
  expect(listingReasonKey("auction_lot_created")).toBe("consignment-auction-lot-created");
  expect(listingReasonKey("lookup_deadline")).toBe("consignment-lookup-deadline");
  expect(LookupReasonSchema.options.map(listingLookupReasonKey)).toEqual([
    "consignment-source-unavailable",
    "consignment-source-timeout",
    "consignment-source-rate-limited",
  ]);
});

test("写回执只使用符合处置语义的文案", () => {
  for (const disposition of DISPOSITIONS) {
    expect(listingDispositionKey(disposition), disposition).toBe(RECEIPT_KEYS[disposition]);
  }
});

test("已知写入拒绝显示专用译文或退回通用提示", () => {
  for (const [code, hasLabel] of Object.entries(STORED_ERROR_HAS_LABEL)) {
    expect(listingWriteErrorKey(code) !== null, code).toBe(hasLabel);
  }
  for (const [code, expectedKey] of Object.entries(LIVE_ERROR_KEYS)) {
    expect(listingWriteErrorKey(code), code).toBe(expectedKey);
  }
  expect(listingWriteErrorKey("stale_version")).toBe("consignment-stale-version");
  expect(listingWriteErrorKey("listing_not_active")).toBe("consignment-listing-not-active");
  expect(listingWriteErrorKey("listing_not_found")).toBeNull();
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
    const source = readFileSync(new URL(`${code}.ftl`, I18N_ROOT), "utf8");
    // 当前文件独立加载，避免默认语言回退掩盖缺键。
    const bundle = buildBundle(code, source, code, null);
    if (!bundle.ok) throw new Error(`${code} 的 FTL 无法加载`);
    for (const key of keys) {
      if (key !== null) {
        expect(bundle.value.getMessage(key)?.value, `${code} 缺少 ${key}`).toBeDefined();
      }
    }
  }
});

function consignmentMessages(file: URL): Map<string, string> {
  const messages = new Map<string, string>();
  for (const line of readFileSync(file, "utf8").split(/\r?\n/u)) {
    const [, key, text] = /^(consignment-[a-z-]+) = (.+)$/u.exec(line) ?? [];
    if (key !== undefined && text !== undefined) messages.set(key, text);
  }
  return messages;
}

test("两站共有的挂单键使用相同的简体中文文案", () => {
  const admin = consignmentMessages(new URL("zh-CN.ftl", I18N_ROOT));
  // 直接比对两份 FTL，测试无需导入 Public 应用。
  const publicMessages = consignmentMessages(
    new URL("../../../../../crates/auction-web-public/i18n/zh-CN.ftl", import.meta.url),
  );
  const sharedKeys = [...admin.keys()].filter((key) => publicMessages.has(key));
  expect(sharedKeys).toContain("consignment-direct");
  for (const key of sharedKeys) {
    expect(admin.get(key), key).toBe(publicMessages.get(key));
  }
});

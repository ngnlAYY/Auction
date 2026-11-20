// 退件译文闭集、代管展示、事件摘要和投递来源链接。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  ADMIN_SETTLEMENT_RETURN_ACTIONS,
  ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
  ADMIN_SETTLEMENT_RETURN_BLOCKERS,
  ADMIN_SETTLEMENT_RETURN_EVENT_KINDS,
  ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
  ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
  ADMIN_SETTLEMENT_RETURN_PLANS,
  ADMIN_SETTLEMENT_RETURN_REASONS,
  ADMIN_SETTLEMENT_RETURN_STATUSES,
  ADMIN_SETTLEMENT_RETURN_TAKE_STATES,
  ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS,
} from "@auction/contracts/dto/admin/returns";

import {
  actionEffectKey,
  actionKey,
  blockerKey,
  eventDetailsText,
  eventKey,
  kindKey,
  locationKey,
  matchKey,
  planKey,
  reasonKey,
  settlementReturnCase,
  showsAssetLocation,
  statusKey,
  takeStateKey,
  takeUnknownGuidanceKey,
  takeUnknownReasonKey,
} from "./labels";

type KeyFn = (value: string) => string | null;
const I18N_ROOT = new URL("../../../../../crates/auction-web-admin/i18n/", import.meta.url);

function ftlKeys(source: string): Set<string> {
  const keys = new Set<string>();
  for (const line of source.split(/\r?\n/u)) {
    const key = line.match(/^([A-Za-z][A-Za-z0-9-]*)\s*=/u)?.[1];
    if (key !== undefined) {
      keys.add(key);
    }
  }
  return keys;
}

test("resent_case_does_not_present_historical_custody_as_current", () => {
  expect(showsAssetLocation("resent")).toBe(false);
  expect(showsAssetLocation("held")).toBe(true);
  expect(showsAssetLocation("take_unknown")).toBe(true);
});

test("every_closed_value_has_a_translated_label_in_all_locales", () => {
  const sets: readonly (readonly [readonly string[], KeyFn])[] = [
    [ADMIN_SETTLEMENT_RETURN_STATUSES, statusKey],
    [ADMIN_SETTLEMENT_RETURN_MATCH_STATES, matchKey],
    [ADMIN_SETTLEMENT_RETURN_REASONS, reasonKey],
    [ADMIN_SETTLEMENT_RETURN_ACTIONS, actionKey],
    [ADMIN_SETTLEMENT_RETURN_ACTIONS, actionEffectKey],
    [ADMIN_SETTLEMENT_RETURN_BLOCKERS, blockerKey],
    [ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS, locationKey],
    [ADMIN_SETTLEMENT_RETURN_PLANS, planKey],
    [ADMIN_SETTLEMENT_RETURN_EVENT_KINDS, eventKey],
    [ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, kindKey],
    [ADMIN_SETTLEMENT_RETURN_TAKE_STATES, takeStateKey],
    [ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS, takeUnknownReasonKey],
    [ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS, takeUnknownGuidanceKey],
  ];
  const locales = new Map<string, Set<string>>();
  for (const locale of ["en", "zh-CN", "zh-TW", "ja", "ko"]) {
    locales.set(locale, ftlKeys(readFileSync(new URL(`${locale}.ftl`, I18N_ROOT), "utf8")));
  }
  const seen = new Set<string>();
  for (const [values, keyFor] of sets) {
    for (const value of values) {
      const key = keyFor(value);
      if (key === null) {
        throw new Error(`${value} 缺少译文键`);
      }
      expect(seen.has(key), key).toBe(false);
      seen.add(key);
      for (const [locale, keys] of locales) {
        expect(keys.has(key), `${locale} 缺少 ${key}`).toBe(true);
      }
    }
    expect(keyFor("not_a_value")).toBeNull();
  }
});

test("event_details_render_sorted_scalars_only", () => {
  expect(
    eventDetailsText({
      reason_code: "returned_assets_match",
      dispatch_no: 2,
      backfilled: true,
      delivery_id: null,
      nested: { secret: "x" },
    }),
  ).toBe(
    "backfilled=true · delivery_id=- · dispatch_no=2 · nested=… · reason_code=returned_assets_match",
  );
  expect(eventDetailsText("text")).toBe("");
  expect(eventDetailsText({})).toBe("");
});

test("delivery_origin_links_only_settlement_return_cases", () => {
  const id = "0f8fad5b-d9cb-469f-a165-70867728950e";
  expect(settlementReturnCase("settlement_return_action", `settlement_return:${id}`)).toBe(id);
  expect(settlementReturnCase("settlement", `settlement_return:${id}`)).toBeNull();
  expect(settlementReturnCase("settlement_return_action", id)).toBeNull();
  expect(
    settlementReturnCase("settlement_return_action", "settlement_return:not-a-uuid"),
  ).toBeNull();
});

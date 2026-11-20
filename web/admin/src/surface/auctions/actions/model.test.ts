// 对照 Rust 生命周期动作、时钟与原因校验。

import { expect, test } from "bun:test";
import { utf8ByteLength } from "@auction/web-shared/text";
import type { AdminAuctionCapabilities } from "../../../api/auctions";
import {
  lifecycleActions,
  pendingOpenMillis,
  staleAuctionFailure,
  validateLifecycleReason,
} from "./model";

const OPEN_AT = "2026-11-14T12:00:00Z";
const OPEN_MILLIS = 1_794_657_600_000;

function capabilities(manualEarlyOpen: boolean): AdminAuctionCapabilities {
  return {
    cancel_auction: true,
    manual_early_open: manualEarlyOpen,
    manual_early_settlement: true,
    manual_intake: false,
    manual_preview_open: true,
    modify_schedule: false,
  };
}

test("preview_open_action_appears_when_scheduled_time_arrives", () => {
  expect(lifecycleActions("preview", capabilities(false), OPEN_AT, OPEN_MILLIS - 1)).toEqual([
    "cancel",
  ]);
  expect(pendingOpenMillis("preview", false, OPEN_AT, OPEN_MILLIS - 1)).toBe(OPEN_MILLIS);
  expect(lifecycleActions("preview", capabilities(false), OPEN_AT, OPEN_MILLIS)).toEqual([
    "open",
    "cancel",
  ]);
  expect(pendingOpenMillis("preview", false, OPEN_AT, OPEN_MILLIS)).toBeNull();
});

test("manual_early_open_does_not_wait_for_the_clock", () => {
  const early = OPEN_MILLIS - 3_600_000;
  expect(lifecycleActions("preview", capabilities(true), OPEN_AT, early)).toEqual([
    "open",
    "cancel",
  ]);
  expect(pendingOpenMillis("preview", true, OPEN_AT, early)).toBeNull();
});

test("other_statuses_and_unparseable_times_never_need_the_clock", () => {
  for (const [status, expected] of [
    ["draft", ["preview", "cancel"]],
    ["live", ["final-phase", "early-settlement"]],
    ["final", ["early-settlement"]],
    ["settling", []],
    ["settled", []],
    ["cancelled", []],
  ] as const) {
    expect(lifecycleActions(status, capabilities(false), OPEN_AT, OPEN_MILLIS - 1)).toEqual([
      ...expected,
    ]);
    expect(lifecycleActions(status, capabilities(false), OPEN_AT, OPEN_MILLIS + 1)).toEqual([
      ...expected,
    ]);
    expect(pendingOpenMillis(status, false, OPEN_AT, OPEN_MILLIS - 1)).toBeNull();
  }
  expect(pendingOpenMillis("preview", false, "not a time", OPEN_MILLIS)).toBeNull();
  expect(lifecycleActions("preview", capabilities(false), "not a time", OPEN_MILLIS)).toEqual([
    "cancel",
  ]);
});

test("lifecycle_reason_obeys_utf8_and_control_character_rules", () => {
  const exactLimit = `${"拍".repeat(341)}a`;
  expect(utf8ByteLength(exactLimit)).toBe(1024);
  expect(utf8ByteLength(`${exactLimit}b`)).toBe(1025);
  expect(validateLifecycleReason(exactLimit)).toEqual({ ok: true, value: exactLimit });
  expect(validateLifecycleReason("  要核对  ")).toEqual({ ok: true, value: "要核对" });
  expect(validateLifecycleReason(`${exactLimit}b`)).toEqual({ ok: false, error: "too_long" });
  expect(validateLifecycleReason(" ")).toEqual({ ok: false, error: "required" });
  for (const value of ["原因\n第二行", "原因\u0085"]) {
    expect(validateLifecycleReason(value)).toEqual({ ok: false, error: "control_character" });
  }
});

test("only_version_rejections_enter_the_auction_stale_path", () => {
  expect(staleAuctionFailure("stale_auction_state")).toBe(true);
  for (const code of [
    "stale_state_version",
    "not_sent:stale_auction_state",
    "command_conflict",
    "outcome_unknown",
  ]) {
    expect(staleAuctionFailure(code)).toBe(false);
  }
});

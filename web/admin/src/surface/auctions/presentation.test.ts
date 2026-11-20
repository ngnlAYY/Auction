// 对照 Rust 排期顺序与本地分钟文本的校验。

import { expect, test } from "bun:test";
import { ok } from "@auction/web-shared/result";
import { withUtc } from "./auction-time-fixture.test";
import { parseLocalScheduleTime, validateSchedule } from "./presentation";

test("creation_requires_open_strictly_after_preview", () => {
  const now = Date.parse("2099-04-01T11:00:00Z");
  const preview = ok("2099-04-01T12:00:00Z");
  const open = ok("2099-04-01T13:00:00Z");
  const end = ok("2099-04-01T14:00:00Z");
  expect(validateSchedule(preview, open, end, "create", now).ok).toBe(true);
  expect(validateSchedule(preview, preview, end, "create", now)).toEqual({
    ok: false,
    error: ["open", "open_before_preview"],
  });
});

test("schedule_still_requires_local_minute_input", () =>
  withUtc(() => {
    expect(parseLocalScheduleTime("")).toEqual({ ok: false, error: "required" });
    expect(parseLocalScheduleTime("2027-03-14T03:30")).toEqual({
      ok: true,
      value: "2027-03-14T03:30:00Z",
    });
    for (const input of ["2027-03-14T03:30:00", "2027-03-14 03:30", "2027-02-29T12:00"]) {
      expect(parseLocalScheduleTime(input)).toEqual({ ok: false, error: "invalid_time" });
    }
  }));

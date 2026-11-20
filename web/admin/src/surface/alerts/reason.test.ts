// 告警备注校验与页面控件的播报、纠错及焦点迁移。

import { expect, spyOn, test } from "bun:test";
import { fireT11Event, installT11Clock, settleT11View } from "../dashboard/t11-solid-fixture.test";
import { expectReasonFeedback, withAlertsPage } from "./alerts-page-fixture.test";
import { validAlertReasonLength } from "./reason";

test("alert_reason_limit_counts_utf8_bytes_of_the_trimmed_payload", () => {
  const exactly = `${"告".repeat(682)}ab`;
  expect(new TextEncoder().encode(exactly).byteLength).toBe(2_048);
  expect(validAlertReasonLength(exactly)).toBe(true);
  expect(validAlertReasonLength(`${exactly}c`)).toBe(false);
  expect(validAlertReasonLength(`  ${exactly}  `)).toBe(true);
  expect(validAlertReasonLength("😀".repeat(512))).toBe(true);
  expect(validAlertReasonLength("😀".repeat(513))).toBe(false);
  expect(validAlertReasonLength("")).toBe(true);
});

test("reason_error_is_cleared_only_when_the_draft_is_corrected", async () => {
  const clock = await installT11Clock();
  try {
    await withAlertsPage(async (page) => {
      const input = page.reason();
      for (const announced of [false, true]) {
        fireT11Event(input, "onInput", "😀".repeat(513));
        fireT11Event(input, "onBlur");
        await settleT11View();
        if (announced) {
          await clock.advance(500);
        }
        expectReasonFeedback(page.tree, true, announced);
        fireT11Event(input, "onInput", "😀".repeat(513));
        await settleT11View();
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expectReasonFeedback(page.tree, true, announced);
        fireT11Event(input, "onInput", "😀".repeat(512));
        await settleT11View();
        expect(input.getAttribute("aria-invalid")).toBe("false");
        expectReasonFeedback(page.tree, false, false);
        expect(clock.pending(500)).toBe(0);
      }
    });
  } finally {
    clock.restore();
  }
});

test("blocked_submit_moves_focus_or_uses_the_announcement", async () => {
  for (const [alreadyFocused, fails, moved, callsExpected] of [
    [false, false, true, 1],
    [true, false, false, 0],
    [false, true, false, 1],
  ] as const) {
    const clock = await installT11Clock();
    try {
      await withAlertsPage(async (page) => {
        const input = page.reason();
        if (alreadyFocused) {
          input.focus();
        }
        const focus = spyOn(input, "focus");
        try {
          if (fails) {
            focus.mockImplementation(() => {
              throw new Error("焦点迁移失败");
            });
          }
          fireT11Event(input, "onInput", "告".repeat(683));
          fireT11Event(page.rowActions(false)[0], "onClick");
          await settleT11View();
          expect(page.writes).toEqual([]);
          expect(input.getAttribute("aria-invalid")).toBe("true");
          expect(input.getAttribute("aria-describedby")).toBe(
            "alert-console-reason-limit alert-console-reason-error",
          );
          expect(focus).toHaveBeenCalledTimes(callsExpected);
          expect(page.document.activeElement === input).toBe(alreadyFocused || moved);
          expectReasonFeedback(page.tree, true, false);
          if (!moved) {
            await clock.advance(500);
            expectReasonFeedback(page.tree, true, true);
          } else {
            expect(clock.pending(500)).toBe(0);
          }
        } finally {
          focus.mockRestore();
        }
      });
    } finally {
      clock.restore();
    }
  }
});

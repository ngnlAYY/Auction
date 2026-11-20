// 未决告警命令冻结、静默时长边界与确定拒绝后的列表重读。

import { expect, spyOn, test } from "bun:test";
import { parseJsonObject } from "../../api/api-request-fixture";
import { t11Reply } from "../dashboard/t11-page-fixture.test";
import { fireT11Event, oneT11Element, settleT11View } from "../dashboard/t11-solid-fixture.test";
import { findAll } from "../solid-dom-fixture";
import { withAlertsPage } from "./alerts-page-fixture.test";
import { alertCommandAcknowledge, alertCommandEntity, alertCommandSilence } from "./commands";

test("frozen_commands_keep_row_generation_trim_reason_and_convert_duration", () => {
  expect(alertCommandAcknowledge("ab", 1, "  disk replaced \n")).toEqual({
    kind: "acknowledge",
    alertId: "ab",
    incidentGeneration: 1,
    reason: "disk replaced",
  });
  expect(alertCommandSilence("ab", 2, 8, " maintenance ")).toEqual({
    kind: "silence",
    alertId: "ab",
    incidentGeneration: 2,
    reason: "maintenance",
    silenceSeconds: 28_800,
  });
  expect(alertCommandEntity(alertCommandSilence("ab", 2, 1, "").alertId)).toBe("alert:ab");
});

test("every_row_duration_stays_within_server_silence_bounds", async () => {
  await withAlertsPage(
    async (page) => {
      const control = oneT11Element(
        findAll(
          page.tree,
          (element) =>
            element.tag === "select" && element.getAttribute("aria-label") === "静默时长",
        ),
        "静默时长",
      );
      expect(
        findAll(control, (element) => element.tag === "option").map((option) => option.value),
      ).toEqual(["1", "8", "24", "72"]);
      for (const [index, [hours, seconds]] of (
        [
          [1, 3_600],
          [8, 28_800],
          [24, 86_400],
          [72, 259_200],
        ] as const
      ).entries()) {
        const command = alertCommandSilence("ab", 2, hours, "");
        if (command.kind !== "silence") throw new Error("预期为静默命令");
        expect(command.silenceSeconds).toBe(seconds);
        expect(command.silenceSeconds).toBeGreaterThanOrEqual(60);
        expect(command.silenceSeconds).toBeLessThanOrEqual(7 * 24 * 3_600);
        await page.setSilenceHours(String(hours));
        fireT11Event(page.rowActions(false)[1], "onClick");
        await settleT11View();
        expect(page.writes).toHaveLength(index + 1);
        const write = page.writes[index];
        if (write === undefined) throw new Error("没有发送静默请求");
        expect(write.method).toBe("POST");
        expect(write.url).toBe(
          "/api/v1/admin/alerts/abababababababababababababababababababababababababababababababab/silence",
        );
        const body = parseJsonObject(write.body, "静默请求");
        expect(body).toMatchObject({
          incident_generation: 2,
          reason: "",
          silence_seconds: seconds,
        });
        expect(page.reads).toEqual(Array.from({ length: index + 2 }, () => "active"));
      }
    },
    () =>
      t11Reply({
        alert: {
          id: "abababababababababababababababababababababababababababababababab",
          action: "silence",
          replayed: false,
          silenced_until: "2026-11-19T12:00:00Z",
        },
      }),
  );
});

test("only_definite_stale_rejections_reload_the_alert_list", async () => {
  // 两版宿主都会在命令了结后通知来源页。这里隔离这次通知，核对 Rust 测到的告警页自身失效条件。
  const commandSources = await import("../pending/sources");
  const hostReloads = spyOn(commandSources, "reloadsFor").mockReturnValue([]);
  try {
    for (const [code, status, reloaded] of [
      ["invalid_alert_command", 400, true],
      ["alert_stale", 409, true],
      ["outcome_unknown", 503, false],
      ["command_conflict", 409, false],
      ["network_error", null, false],
      ["http_503", 503, false],
    ] as const) {
      await withAlertsPage(
        async (page) => {
          fireT11Event(page.rowActions(false)[0], "onClick");
          await settleT11View();
          expect(
            page.writes.map(({ method, url }) => [method, url]),
            code,
          ).toEqual([
            [
              "POST",
              "/api/v1/admin/alerts/abababababababababababababababababababababababababababababababab/acknowledge",
            ],
          ]);
          expect(page.reads, code).toEqual(reloaded ? ["active", "active"] : ["active"]);
        },
        () => {
          if (status === null) {
            throw new TypeError("内存传输失败");
          }
          return {
            status,
            body:
              code === "http_503"
                ? ""
                : JSON.stringify({ code, message: "", data: null, request_id: null }),
          };
        },
      );
    }
    expect(hostReloads).toHaveBeenCalledTimes(3);
  } finally {
    hostReloads.mockRestore();
  }
});
